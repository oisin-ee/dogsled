//! The only way a message reaches an agent. Nothing types into a terminal.
//!
//! Each agent terminal has an inbox on its own host: `inbox/<terminal_id>.json`. A sender puts
//! a record there, locally or over SSH. The terminal's reader (the Pi extension, or the Claude
//! and Codex hooks) claims its pending records, shows them to the agent without touching the
//! editor, and acknowledges them. A claim is a lease: a record that is claimed but not
//! acknowledged becomes claimable again when the lease ends, with the shared backoff between
//! attempts. A terminal without a registered reader gets nothing, and the sender hears so at
//! once. The service fails the records of a terminal that is gone.
//!
//! The sender's host keeps an outbox of what it sent and asks the target host for each
//! record's state, so a later failure comes back to the sender as a record in its own inbox.

use crate::backoff::Backoff;
use crate::error::{Error, Result};
use crate::hosts::{Fleet, Host};
use crate::ledger::{Ledger, new_id};
use crate::probe::Agent;
use serde::{Deserialize, Serialize};

/// A message larger than this is refused; a reader shows it whole.
pub const MAX_TEXT: usize = 64 * 1024;
/// A record claimed this many times without an acknowledgement fails.
const MAX_ATTEMPTS: u32 = 8;
/// Delivered and failed records stay this long, so their senders can read the outcome.
const KEEP_MS: u64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Pending,
    Claimed,
    Delivered,
    Failed,
}

impl State {
    pub fn open(self) -> bool {
        matches!(self, Self::Pending | Self::Claimed)
    }
}

/// Who sent a record. `dogsled` itself sends replies, reports, and check-ins.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sender {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
    /// The sender machine's hostname.
    pub host: String,
}

impl Sender {
    pub fn dogsled() -> Self {
        Self {
            name: Some("dogsled".into()),
            host: hostname(),
            ..Self::default()
        }
    }

    fn describe(&self) -> String {
        match (&self.name, &self.pane) {
            (Some(name), Some(pane)) => format!("{name} ({pane} on {})", self.host),
            (None, Some(pane)) => format!("{pane} on {}", self.host),
            (Some(name), None) => name.clone(),
            (None, None) => format!("a shell on {}", self.host),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    /// The target terminal and its pane when the record was sent.
    pub terminal: String,
    pub pane: String,
    pub from: Sender,
    /// `message` or `task` from an agent; `reply`, `blocked`, `check-in`, or `failed` from
    /// dogsled.
    pub kind: String,
    pub text: String,
    pub created_ms: u64,
    pub state: State,
    /// Claims so far, and when the current lease ends.
    #[serde(flatten)]
    pub lease: Backoff,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_ms: Option<u64>,
}

impl Record {
    /// What the agent sees: who sent it, what kind it is, and the text.
    pub fn render(&self) -> String {
        format!(
            "dogsled {} from {}:\n{}",
            self.kind,
            self.from.describe(),
            self.text
        )
    }
}

/// A claimed record with the text its reader shows the agent, so no harness rebuilds it.
#[derive(Debug, Serialize)]
pub struct Claimed<'a> {
    #[serde(flatten)]
    pub record: &'a Record,
    /// What the agent reads: the sender, the kind, and the text.
    pub message: String,
    /// The sender as one label, for a reader's card.
    pub sender: String,
}

impl<'a> From<&'a Record> for Claimed<'a> {
    fn from(record: &'a Record) -> Self {
        Self {
            record,
            message: record.render(),
            sender: record.from.describe(),
        }
    }
}

/// The hook event a Claude Code or Codex inbox reader runs at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "PascalCase")]
pub enum HookEvent {
    Stop,
    UserPromptSubmit,
}

/// The hook output that hands a stop reminder and `records` to Claude Code or Codex, or
/// `None` when both are absent. At `Stop`, `decision: block` continues the turn with the reason as the next prompt;
/// at `UserPromptSubmit`, `additionalContext` sits beside the user's text and never replaces it.
pub fn hook_output(records: &[Record], event: HookEvent, reminder: Option<&str>) -> Option<String> {
    if records.is_empty() && reminder.is_none() {
        return None;
    }
    let text = reminder
        .map(str::to_owned)
        .into_iter()
        .chain(records.iter().map(Record::render))
        .collect::<Vec<_>>()
        .join("\n\n");
    let value = match event {
        HookEvent::Stop => serde_json::json!({ "decision": "block", "reason": text }),
        HookEvent::UserPromptSubmit => serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": text,
            }
        }),
    };
    Some(format!("{value}\n"))
}

/// Run the inbox hook for this pane: claim its records, print them in the event's format,
/// and acknowledge them. The harness has the text once it is written; a failed write leaves
/// the records claimed, and they are offered again when the lease ends.
pub fn hook(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    harness: &str,
    event: HookEvent,
    now: u64,
    board: Option<&crate::board::Board>,
) -> Result<()> {
    let pane = own_pane(None)?;
    let records = claim_for(ledger, shell, &pane, harness, now)?;
    let emit = |reminder: Option<&str>| {
        if let Some(output) = hook_output(&records, event, reminder) {
            crate::print(false, &(), || output)?;
        }
        Ok(())
    };
    if event == HookEvent::Stop {
        let notices = crate::gate::notices(ledger, shell, &pane)?;
        let empty = crate::board::Board {
            hosts: Vec::new(),
            rows: Vec::new(),
        };
        crate::gate::deliver(ledger, board.unwrap_or(&empty), &pane, &notices, emit)?;
    } else {
        emit(None)?;
    }
    if !records.is_empty() {
        let ids: Vec<String> = records.iter().map(|r| r.id.clone()).collect();
        ack_for(ledger, shell, &pane, &ids, now)?;
    }
    Ok(())
}

/// The harness that reads a terminal's inbox. Registered on the reader's first claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reader {
    pub harness: String,
    pub pane: String,
    pub registered_ms: u64,
}

pub fn hostname() -> String {
    nix::unistd::gethostname()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown".into())
}

fn store(ledger: &Ledger) -> Ledger {
    ledger.child("inbox")
}

/// Herdr terminal ids name files, so only plain ids are accepted.
pub fn validate_terminal(terminal: &str) -> Result<()> {
    if terminal.is_empty()
        || terminal.len() > 64
        || !terminal
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(Error::Invalid(format!("invalid terminal id {terminal:?}")));
    }
    Ok(())
}

fn records(ledger: &Ledger, terminal: &str) -> Result<Vec<Record>> {
    Ok(store(ledger)
        .read(&format!("{terminal}.json"))?
        .unwrap_or_default())
}

/// Change `terminal`'s records under its lock.
fn update<T>(
    ledger: &Ledger,
    terminal: &str,
    change: impl FnOnce(&mut Vec<Record>) -> Result<T>,
) -> Result<T> {
    validate_terminal(terminal)?;
    let store = store(ledger);
    store.with_lock(terminal, || {
        let before = records(ledger, terminal)?;
        let mut all = before.clone();
        let result = change(&mut all)?;
        // A reader watches this file, so an empty claim must not touch it.
        if all != before {
            store.write(&format!("{terminal}.json"), &all)?;
        }
        Ok(result)
    })
}

pub fn reader(ledger: &Ledger, terminal: &str) -> Result<Option<Reader>> {
    validate_terminal(terminal)?;
    store(ledger).read(&format!("{terminal}.reader.json"))
}

/// Put `record` in its terminal's inbox on this host. Refuses a terminal that is not in this
/// host's latest snapshot, or that has no reader: nothing would ever deliver the record.
///
/// The live agent of the pane is read once, and it must be the custodian: a pane another
/// session took over never inherits the custody, so the record waits for its own agent.
fn put_checked(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    record: Record,
    custodian: Option<&Custodian>,
) -> Result<Record> {
    // Read live state: an ended turn seals done before a message can reopen it.
    let agent = local_agent(shell, &record.pane)?;
    if let Some(custodian) = custodian
        && !holds(custodian, &record.pane, agent.agent_session.as_deref())
    {
        return Err(Error::Invalid(format!(
            "{} runs another session than the one that holds the custody",
            record.pane
        )));
    }
    let from = record.from.clone();
    crate::done::input(
        ledger,
        &agent,
        crate::done::Input::Put { from: Some(&from) },
        || put(ledger, record).map(|r| (r, true)),
    )
}

fn put(ledger: &Ledger, record: Record) -> Result<Record> {
    deliverable(ledger, &record)?;
    update(ledger, &record.terminal.clone(), |all| {
        if let Some(existing) = all.iter().find(|r| r.id == record.id) {
            return Ok(existing.clone());
        }
        all.push(record.clone());
        Ok(record)
    })
}

/// Put one legacy continuation record. The caller's durable receipt owns replay protection and
/// fixes the record ID. A retry after a crash finds the same record and changes nothing.
pub(crate) fn put_continuation(ledger: &Ledger, record: Record) -> Result<Record> {
    deliverable(ledger, &record)?;
    update(ledger, &record.terminal.clone(), |all| {
        if let Some(existing) = all.iter().find(|r| r.id == record.id) {
            if existing.pane != record.pane
                || existing.from != record.from
                || existing.kind != record.kind
                || existing.text != record.text
            {
                return Err(crate::unit_transport::conflict(
                    "a different record has this continuation ID",
                ));
            }
            return Ok(existing.clone());
        }
        all.push(record.clone());
        Ok(record)
    })
}

/// Refuse a record that nothing would deliver: too long, for a terminal that is not in this
/// host's latest snapshot, or for a terminal with no reader.
pub(crate) fn deliverable(ledger: &Ledger, record: &Record) -> Result<()> {
    validate_terminal(&record.terminal)?;
    if record.text.len() > MAX_TEXT {
        return Err(Error::Invalid(format!(
            "the message has {} bytes; the limit is {MAX_TEXT}",
            record.text.len()
        )));
    }
    let live = ledger
        .read::<crate::node::Snapshot>("host.json")?
        .is_some_and(|s| {
            s.probe
                .agent_panes
                .iter()
                .any(|a| a.terminal_id.as_deref() == Some(record.terminal.as_str()))
        });
    if !live {
        return Err(Error::Invalid(format!(
            "{} ({}) is not running on this host; nothing was sent",
            record.pane, record.terminal
        )));
    }
    if reader(ledger, &record.terminal)?.is_none() {
        return Err(Error::Refused {
            code: crate::error::Refusal::NoInboxReader,
            detail: format!(
                "{} has no dogsled inbox reader, so nothing would deliver the message; nothing was sent",
                record.pane
            ),
        });
    }
    Ok(())
}

/// Register the reader for `terminal` and claim its deliverable records: pending ones, and
/// claimed ones whose lease ended. Each claim starts a lease of the backoff delay.
pub fn claim(ledger: &Ledger, terminal: &str, reader: &Reader, now: u64) -> Result<Vec<Record>> {
    validate_terminal(terminal)?;
    let file = format!("{terminal}.reader.json");
    let store = store(ledger);
    let known = store.read::<Reader>(&file)?;
    if known.is_none_or(|k| k.harness != reader.harness || k.pane != reader.pane) {
        store.write(&file, reader)?;
    }
    let claimed = update(ledger, terminal, |all| {
        let mut claimed = Vec::new();
        for record in all.iter_mut().filter(|r| {
            r.state == State::Pending || (r.state == State::Claimed && r.lease.ready(now))
        }) {
            record.state = State::Claimed;
            record.lease.fail(now, &record.id);
            claimed.push(record.clone());
        }
        Ok(claimed)
    })?;
    Ok(claimed)
}

/// Mark `ids` delivered. Unknown or already final ids are ignored, so a repeated ack is safe.
pub fn ack(ledger: &Ledger, terminal: &str, ids: &[String], now: u64) -> Result<usize> {
    update(ledger, terminal, |all| {
        let mut count = 0;
        for record in all
            .iter_mut()
            .filter(|r| r.state.open() && ids.contains(&r.id))
        {
            record.state = State::Delivered;
            record.done_ms = Some(now);
            count += 1;
        }
        Ok(count)
    })
}

/// The states of `ids` in `terminal`'s inbox. A pruned id is absent.
pub fn states(ledger: &Ledger, terminal: &str, ids: &[String]) -> Result<Vec<Outcome>> {
    validate_terminal(terminal)?;
    Ok(records(ledger, terminal)?
        .into_iter()
        .filter(|r| ids.contains(&r.id))
        .map(|r| Outcome {
            id: r.id,
            state: r.state,
            reason: r.reason,
        })
        .collect())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub id: String,
    pub state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Every record received by `terminal`, newest first.
pub fn received(ledger: &Ledger, terminal: &str) -> Result<Vec<Record>> {
    validate_terminal(terminal)?;
    let mut all = records(ledger, terminal)?;
    all.reverse();
    Ok(all)
}

/// The service's pass over this host's inboxes: fail the records of a terminal that is gone,
/// and of a record claimed too often; drop old final records and the files of gone terminals.
/// `live` holds the terminals in this host's latest snapshot.
pub fn maintain(ledger: &Ledger, live: &[String], now: u64) -> Result<()> {
    let store = store(ledger);
    let Ok(entries) = std::fs::read_dir(&store.dir) else {
        return Ok(());
    };
    for entry in entries {
        let name = entry?.file_name();
        let Some(terminal) = name
            .to_str()
            .and_then(|n| n.strip_suffix(".json"))
            .filter(|n| !n.ends_with(".reader") && validate_terminal(n).is_ok())
        else {
            continue;
        };
        let gone = !live.iter().any(|t| t == terminal);
        let empty = update(ledger, terminal, |all| {
            for record in all.iter_mut().filter(|r| r.state.open()) {
                let reason = if gone {
                    Some("its terminal is gone".to_owned())
                } else if record.state == State::Claimed
                    && record.lease.ready(now)
                    && record.lease.attempts >= MAX_ATTEMPTS
                {
                    Some(format!(
                        "not acknowledged after {} attempts",
                        record.lease.attempts
                    ))
                } else {
                    None
                };
                if let Some(reason) = reason {
                    record.state = State::Failed;
                    record.reason = Some(reason);
                    record.done_ms = Some(now);
                }
            }
            all.retain(|r| r.done_ms.is_none_or(|t| now.saturating_sub(t) < KEEP_MS));
            Ok(all.is_empty())
        })?;
        if gone && empty {
            for file in [
                format!("{terminal}.json"),
                format!("{terminal}.reader.json"),
            ] {
                match std::fs::remove_file(store.dir.join(file)) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                    _ => (),
                }
            }
        }
    }
    Ok(())
}

/// One record this host sent, and what its sender knows of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sent {
    pub id: String,
    /// The target's host id and label, terminal, and pane.
    pub host: String,
    pub host_label: String,
    pub terminal: String,
    pub pane: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The target's harness, as Herdr reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    pub from: Sender,
    pub kind: String,
    pub created_ms: u64,
    pub state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// When to ask the target host again.
    #[serde(default)]
    pub check: Backoff,
}

const OUTBOX: &str = "outbox.json";

pub fn outbox(ledger: &Ledger) -> Result<Vec<Sent>> {
    Ok(ledger.read(OUTBOX)?.unwrap_or_default())
}

/// Where `send` delivers: an agent pane on a host, with its terminal.
#[derive(Debug, Clone)]
pub struct Target<'a> {
    pub host: &'a Host,
    pub agent: Agent,
}

/// The one agent named `target` (a pane id or an agent name) on `host`, or on any host.
pub fn resolve<'a>(
    fleet: &'a Fleet,
    target: &str,
    host: Option<&str>,
    now: u64,
) -> Result<Target<'a>> {
    let pinned = host.map(|h| fleet.find(h)).transpose()?;
    let mut found = Vec::new();
    let mut finished = None;
    for report in fleet.status(now)? {
        if pinned.is_some_and(|h| h.id != report.host.id) {
            continue;
        }
        if let Some(mark) = report
            .handoffs
            .iter()
            .find(|m| m.sealed.is_some() && (m.pane == target || m.name.as_deref() == Some(target)))
        {
            finished = Some(mark.name.clone().unwrap_or_else(|| mark.pane.clone()));
        }
        let Some(probe) = report.probe else { continue };
        for agent in probe.agent_panes {
            if agent.pane == target || agent.name.as_deref() == Some(target) {
                found.push((fleet.find(&report.host.id)?, agent));
            }
        }
    }
    match found.len() {
        0 if finished.is_some() => Err(Error::Invalid(format!(
            "{} finished; start a new child for more work",
            finished.unwrap_or_default()
        ))),
        0 => Err(Error::Invalid(format!(
            "no agent named or in pane {target}{}; see `dogsled status`",
            pinned.map_or_else(String::new, |h| format!(" on {}", h.label))
        ))),
        1 => {
            let (host, agent) = found.remove(0);
            if agent.terminal_id.is_none() {
                return Err(Error::Invalid(format!(
                    "{} on {} has no terminal id yet; its host runs an older dogsled",
                    agent.pane, host.label
                )));
            }
            Ok(Target { host, agent })
        }
        _ => Err(Error::Invalid(format!(
            "{target} matches {}; pass a pane and --host",
            found
                .iter()
                .map(|(h, a)| format!("{} on {}", a.pane, h.label))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Send `text` to `target` and record it in this host's outbox. A refusal on the target host
/// (no reader, terminal gone) is returned to the caller; nothing is recorded.
pub fn send(
    fleet: &Fleet,
    target: &Target,
    from: Sender,
    kind: &str,
    text: String,
    now: u64,
) -> Result<Sent> {
    let terminal = target
        .agent
        .terminal_id
        .clone()
        .ok_or_else(|| Error::Invalid("the target has no terminal id".into()))?;
    let record = Record {
        id: new_id(),
        terminal: terminal.clone(),
        pane: target.agent.pane.clone(),
        from: from.clone(),
        kind: kind.into(),
        text,
        created_ms: now,
        state: State::Pending,
        lease: Backoff::default(),
        reason: None,
        done_ms: None,
    };
    let stored = if target.host.is_local() {
        put_checked(fleet.ledger, fleet.shell, record, None)?
    } else {
        fleet.remote_setup(
            target.host,
            &[
                "inbox-put".into(),
                "--record".into(),
                serde_json::to_string(&record)?,
            ],
        )?
    };
    let sent = Sent {
        id: stored.id,
        host: target.host.id.clone(),
        host_label: target.host.label.clone(),
        terminal,
        pane: target.agent.pane.clone(),
        name: target.agent.name.clone(),
        harness: target.agent.agent.clone(),
        from,
        kind: kind.into(),
        created_ms: now,
        state: stored.state,
        reason: None,
        check: Backoff::default(),
    };
    record_sent(fleet.ledger, &sent, now)?;
    Ok(sent)
}

/// Keep `sent` in this host's outbox, once: a retried record keeps its first entry.
pub(crate) fn record_sent(ledger: &Ledger, sent: &Sent, now: u64) -> Result<()> {
    ledger.with_lock("outbox", || {
        let mut all = outbox(ledger)?;
        if !all.iter().any(|s| s.id == sent.id) {
            all.push(sent.clone());
        }
        all.retain(|s| s.state.open() || now.saturating_sub(s.created_ms) < KEEP_MS);
        ledger.write(OUTBOX, &all)
    })
}

/// Ask each target host for the state of this host's open sent records, with backoff per
/// record. A failure goes to the sender's own inbox when the sender has a reader.
pub fn reconcile(fleet: &Fleet, now: u64) -> Result<()> {
    let due: Vec<Sent> = outbox(fleet.ledger)?
        .into_iter()
        .filter(|s| s.state.open() && s.check.ready(now))
        .collect();
    let mut outcomes: Vec<(String, std::result::Result<Option<Outcome>, ()>)> = Vec::new();
    let mut by_target: std::collections::BTreeMap<(String, String), Vec<String>> =
        std::collections::BTreeMap::new();
    for s in &due {
        by_target
            .entry((s.host.clone(), s.terminal.clone()))
            .or_default()
            .push(s.id.clone());
    }
    for ((host, terminal), ids) in by_target {
        let result = match fleet.find(&host) {
            Ok(h) if h.is_local() => states(fleet.ledger, &terminal, &ids),
            Ok(h) => {
                let mut args = vec!["inbox-state".into(), terminal.clone()];
                args.extend(ids.iter().cloned());
                fleet.remote::<Vec<Outcome>>(h, &args)
            }
            // A host missing from this fleet view is asked again later.
            Err(e) => Err(e),
        };
        for id in ids {
            outcomes.push(match &result {
                Ok(found) => (id.clone(), Ok(found.iter().find(|o| o.id == id).cloned())),
                Err(_) => (id, Err(())),
            });
        }
    }
    if outcomes.is_empty() {
        return Ok(());
    }
    let failed = fleet.ledger.with_lock("outbox", || {
        let mut all = outbox(fleet.ledger)?;
        let mut failed = Vec::new();
        for s in &mut all {
            let Some((_, outcome)) = outcomes.iter().find(|(id, _)| *id == s.id) else {
                continue;
            };
            match outcome {
                Ok(Some(o)) if o.state.open() => {
                    s.state = o.state;
                    s.check.fail(now, &s.id);
                }
                Ok(Some(o)) => {
                    s.state = o.state;
                    s.reason.clone_from(&o.reason);
                    if o.state == State::Failed {
                        failed.push(s.clone());
                    }
                }
                Ok(None) => {
                    s.state = State::Failed;
                    s.reason = Some("the target host no longer has the record".into());
                    failed.push(s.clone());
                }
                Err(()) => s.check.fail(now, &s.id),
            }
        }
        fleet.ledger.write(OUTBOX, &all)?;
        Ok(failed)
    })?;
    for s in failed {
        report(fleet.ledger, fleet.shell, &s, now)?;
    }
    Ok(())
}

/// Tell the sender of `s` that it failed, when the sender has a reader on this host.
fn report(ledger: &Ledger, shell: &dyn crate::shell::Shell, s: &Sent, now: u64) -> Result<()> {
    let (Some(terminal), Some(pane)) = (&s.from.terminal, &s.from.pane) else {
        return Ok(());
    };
    if reader(ledger, terminal)?.is_none() {
        return Ok(());
    }
    let record = Record {
        id: format!("failed-{}", s.id),
        terminal: terminal.clone(),
        pane: pane.clone(),
        from: Sender::dogsled(),
        kind: "failed".into(),
        text: format!(
            "Your {} to {} ({} on {}) was not delivered: {}.",
            s.kind,
            s.name.as_deref().unwrap_or(&s.pane),
            s.pane,
            s.host_label,
            s.reason.as_deref().unwrap_or("unknown reason")
        ),
        created_ms: now,
        state: State::Pending,
        lease: Backoff::default(),
        reason: None,
        done_ms: None,
    };
    match put_checked(ledger, shell, record, None) {
        // The sender itself may be gone; nobody is left to tell.
        Ok(_) | Err(Error::Invalid(_) | Error::Refused { .. }) => Ok(()),
        Err(e) => Err(e),
    }
}

/// The agent in `pane` on this host, from Herdr.
pub fn local_agent(shell: &dyn crate::shell::Shell, pane: &str) -> Result<Agent> {
    crate::probe::agent_list(shell)?
        .into_iter()
        .find(|a| a.pane == pane)
        .ok_or_else(|| Error::Invalid(format!("{pane} is not an agent pane on this host")))
}

fn terminal_of(agent: &Agent) -> Result<String> {
    agent.terminal_id.clone().ok_or_else(|| {
        Error::Invalid(format!(
            "Herdr reports no terminal id for {}; upgrade Herdr",
            agent.pane
        ))
    })
}

/// Who is calling: the agent in `HERDR_PANE_ID`, else a plain shell on this host.
pub fn caller(shell: &dyn crate::shell::Shell) -> Sender {
    let host = hostname();
    let Ok(pane) = std::env::var("HERDR_PANE_ID") else {
        return Sender {
            host,
            ..Sender::default()
        };
    };
    let agent = local_agent(shell, &pane).ok();
    Sender {
        name: agent.as_ref().and_then(|a| a.name.clone()),
        terminal: agent.and_then(|a| a.terminal_id),
        pane: Some(pane),
        host,
    }
}

/// The pane a reader command acts for: `--pane`, else `HERDR_PANE_ID`.
pub fn own_pane(pane: Option<String>) -> Result<String> {
    pane.or_else(|| std::env::var("HERDR_PANE_ID").ok())
        .ok_or_else(|| Error::Invalid("HERDR_PANE_ID or --pane is required".into()))
}

/// `dogsled inbox claim`: register `harness` as the reader of `pane`'s inbox and claim its
/// deliverable records.
pub fn claim_for(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    pane: &str,
    harness: &str,
    now: u64,
) -> Result<Vec<Record>> {
    let agent = local_agent(shell, pane)?;
    let terminal = terminal_of(&agent)?;
    let reader = Reader {
        harness: harness.into(),
        pane: pane.into(),
        registered_ms: now,
    };
    crate::done::input(ledger, &agent, crate::done::Input::Claim, || {
        let records = claim(ledger, &terminal, &reader, now)?;
        let received = !records.is_empty();
        Ok((records, received))
    })
}

pub fn ack_for(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    pane: &str,
    ids: &[String],
    now: u64,
) -> Result<usize> {
    ack(ledger, &terminal_of(&local_agent(shell, pane)?)?, ids, now)
}

/// The directory and file name of `pane`'s inbox. A reader watches the directory, because
/// each write replaces the file.
pub fn path_for(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    pane: &str,
) -> Result<(String, String)> {
    let terminal = terminal_of(&local_agent(shell, pane)?)?;
    validate_terminal(&terminal)?;
    Ok((
        store(ledger).dir.display().to_string(),
        format!("{terminal}.json"),
    ))
}

/// What `pane` received and sent.
#[derive(Debug, Serialize)]
pub struct Listing {
    pub pane: String,
    pub terminal: String,
    pub received: Vec<Record>,
    pub sent: Vec<Sent>,
    /// Both lists as display lines, newest first within each direction.
    pub lines: Vec<Line>,
}

/// One record as a reader shows it.
#[derive(Debug, Serialize)]
pub struct Line {
    /// `received` or `sent`.
    pub direction: &'static str,
    pub id: String,
    pub kind: String,
    /// The sender of a received record, the target of a sent one.
    pub peer: String,
    /// `pending`, `claimed, offered again in 40s`, `delivered`, or `failed: <reason>`.
    pub state: String,
    pub open: bool,
    /// The first line of the text; sent records keep no text.
    pub summary: String,
    pub created_ms: u64,
}

fn state_text(state: State, lease: &Backoff, reason: Option<&str>, now: u64) -> String {
    match state {
        State::Claimed if !lease.ready(now) => format!(
            "claimed, offered again in {}s",
            lease.next_ms.saturating_sub(now).div_ceil(1000)
        ),
        State::Failed => format!("failed: {}", reason.unwrap_or("unknown")),
        other => format!("{other:?}").to_lowercase(),
    }
}

/// A sent record's state. Claude Code and Codex read their inbox only from a hook, so an
/// open record to one waits for the target's next prompt or stop.
fn sent_text(sent: &Sent, now: u64) -> String {
    if sent.state.open() && matches!(sent.harness.as_deref(), Some("claude" | "codex")) {
        return format!(
            "waiting for {}'s next turn",
            sent.name.as_deref().unwrap_or(&sent.pane)
        );
    }
    state_text(sent.state, &Backoff::default(), sent.reason.as_deref(), now)
}

pub fn list_for(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    pane: &str,
    now: u64,
) -> Result<Listing> {
    let terminal = terminal_of(&local_agent(shell, pane)?)?;
    let mut sent: Vec<Sent> = outbox(ledger)?
        .into_iter()
        .filter(|s| s.from.pane.as_deref() == Some(pane))
        .collect();
    sent.reverse();
    let received = received(ledger, &terminal)?;
    let lines = received
        .iter()
        .map(|r| Line {
            direction: "received",
            id: r.id.clone(),
            kind: r.kind.clone(),
            peer: r.from.describe(),
            state: state_text(r.state, &r.lease, r.reason.as_deref(), now),
            open: r.state.open(),
            summary: r.text.lines().next().unwrap_or_default().to_owned(),
            created_ms: r.created_ms,
        })
        .chain(sent.iter().map(|s| Line {
            direction: "sent",
            id: s.id.clone(),
            kind: s.kind.clone(),
            peer: format!(
                "{} on {}",
                s.name.as_deref().unwrap_or(&s.pane),
                s.host_label
            ),
            state: sent_text(s, now),
            open: s.state.open(),
            summary: String::new(),
            created_ms: s.created_ms,
        }))
        .collect();
    Ok(Listing {
        pane: pane.into(),
        terminal,
        received,
        sent,
        lines,
    })
}

impl Listing {
    pub fn render(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        for (direction, title, count) in [
            ("received", "Received", self.received.len()),
            ("sent", "Sent", self.sent.len()),
        ] {
            let _ = writeln!(out, "{title} ({count}):");
            for l in self.lines.iter().filter(|l| l.direction == direction) {
                let to = if direction == "sent" { "to" } else { "from" };
                let summary = if l.summary.is_empty() {
                    String::new()
                } else {
                    format!(": {}", l.summary)
                };
                let _ = writeln!(
                    out,
                    "  {} {} {to} {} [{}]{summary}",
                    l.id, l.kind, l.peer, l.state
                );
            }
        }
        out
    }
}

/// The service's inbox lane: maintain this host's inboxes, then follow up what it sent.
pub fn tick(fleet: &Fleet, now: u64) -> Result<()> {
    crate::unit_request::refresh(fleet, crate::unit_request::Scope::All)?;
    let live: Vec<String> = fleet
        .ledger
        .read::<crate::node::Snapshot>("host.json")?
        .map(|s| {
            s.probe
                .agent_panes
                .into_iter()
                .filter_map(|a| a.terminal_id)
                .collect()
        })
        .unwrap_or_default();
    // Without a snapshot, nothing is known to be gone.
    if !live.is_empty() {
        maintain(fleet.ledger, &live, now)?;
    }
    reconcile(fleet, now)
}

/// The text of `dogsled send`: `--message`, or `--message-file` (`-` is stdin).
pub fn message_text(message: Option<String>, file: Option<std::path::PathBuf>) -> Result<String> {
    let text = match (message, file) {
        (Some(text), _) => text,
        (None, Some(path)) if path.as_os_str() == "-" => std::io::read_to_string(std::io::stdin())?,
        (None, Some(path)) => std::fs::read_to_string(path)?,
        (None, None) => String::new(),
    };
    if text.trim().is_empty() {
        return Err(Error::Invalid("the message is empty".into()));
    }
    Ok(text)
}

/// `dogsled send`: resolve `target`, then send from the calling agent.
pub fn send_to(
    fleet: &Fleet,
    target: &str,
    host: Option<&str>,
    kind: &str,
    text: String,
    now: u64,
) -> Result<Sent> {
    let to = resolve(fleet, target, host, now)?;
    send(fleet, &to, caller(fleet.shell), kind, text, now)
}

impl Sent {
    pub fn line(&self) -> String {
        format!(
            "Sent {} to {} ({} on {}); its inbox reader shows it to the agent.\n",
            self.id,
            self.name.as_deref().unwrap_or(&self.pane),
            self.pane,
            self.host_label
        )
    }
}

/// The inbox commands a peer runs on this host over SSH.
pub fn host(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    cmd: crate::cli::HostCommand,
) -> Result<()> {
    let value = match cmd {
        crate::cli::HostCommand::InboxPut { record } => serde_json::to_value(put_checked(
            ledger,
            shell,
            serde_json::from_str(&record)?,
            None,
        )?)?,
        crate::cli::HostCommand::InboxState { terminal, ids } => {
            serde_json::to_value(states(ledger, &terminal, &ids)?)?
        }
        _ => return Err(Error::Invalid("not an inbox command".into())),
    };
    crate::print(true, &value, String::new)
}

/// The agent that answers for an item now.
///
/// The custody record names the current custodian by pane and by session. An item with no
/// custody record is still held by its origin owner, which is the custody the record
/// would have started from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Custodian {
    /// The pane that answers for the item.
    pub pane: String,
    /// The custodian's harness session, when one is recorded.
    pub session: Option<String>,
}

/// The current custodian of `item`, from the custody records this host holds.
///
/// A custody record wins: it names the custodian and the session that holds the custody.
/// Without one the origin owner holds the item, as it did before the custody existed, and
/// it holds it by pane alone: an item with no custody record has no recorded custodian
/// session to prove, so the pane decides then.
#[must_use]
pub fn custodian_of(records: &[crate::custody::Custody], item: &crate::items::Item) -> Custodian {
    match records.iter().find(|record| record.item == item.id) {
        Some(record) => Custodian {
            pane: record.custodian_pane.clone(),
            session: record.custodian_session.clone(),
        },
        None => Custodian {
            pane: item.owner_pane.clone(),
            session: None,
        },
    }
}

/// Whether the agent in `pane`, running `session`, holds `custodian`.
///
/// The pane must match. A session both sides know decides: a pane id that was reused by
/// another session never inherits the custody. A custody that names a session needs a live
/// session that is readable and equal: a session nobody can read proves nothing, so the
/// custody is not held then.
#[must_use]
pub fn holds(custodian: &Custodian, pane: &str, session: Option<&str>) -> bool {
    if custodian.pane != pane {
        return false;
    }
    match (custodian.session.as_deref(), session) {
        (Some(recorded), Some(live)) => recorded == live,
        (Some(_), None) => false,
        (None, _) => true,
    }
}

/// Whether an event id names `item`.
///
/// After typed identity validation, check that this deduplication key still names the
/// captured event. It cannot discover a subject or grant delivery authority.
/// A child event names its item in the record id: `reply-{item}-{seq}`,
/// `blocked-{item}-{seq}`, or `unit-reply-{result}`, whose result carries the item id.
/// The tail of the first two is a number, so only the item it was built for matches.
fn names(id: &str, item: &crate::items::Item) -> bool {
    if let Some(signal) = &item.signal
        && id == format!("signal-{}-{}", item.id, signal.event_id)
    {
        return true;
    }
    if let Some(result) = id.strip_prefix("unit-reply-") {
        return item
            .reply
            .as_ref()
            .and_then(|reply| reply.result_id.as_deref())
            == Some(result);
    }
    if id.starts_with("reply-") {
        return item
            .reply
            .as_ref()
            .is_some_and(|reply| id == format!("reply-{}-{}", item.id, reply.seq));
    }
    let Some(rest) = id.strip_prefix("blocked-") else {
        return false;
    };
    let Some(tail) = rest
        .strip_prefix(&item.id)
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return false;
    };
    !tail.is_empty() && tail.bytes().all(|byte| byte.is_ascii_digit())
}

/// Authority captured while the caller still holds the child identity. Event IDs are
/// only deduplication keys; they never grant pane-only delivery authority.
#[derive(Debug, Clone)]
pub struct ChildIdentity {
    item: crate::items::Item,
    custody: Option<crate::custody::Custody>,
}

impl ChildIdentity {
    pub fn capture(ledger: &Ledger, item: &crate::items::Item) -> Result<Self> {
        crate::custody::locked(ledger, &item.id, |custody| {
            Ok(Self {
                item: item.clone(),
                custody: custody.cloned(),
            })
        })
    }

    fn validate(
        &self,
        ledger: &Ledger,
        id: &str,
        custody: Option<&crate::custody::Custody>,
    ) -> Result<Custodian> {
        let items = crate::items::list(ledger)?;
        let mut matches = items.iter().filter(|item| item.id == self.item.id);
        let Some(item) = matches.next() else {
            return Err(Error::Invalid(
                "child notification undelivered: subject expired or missing".into(),
            ));
        };
        if matches.next().is_some()
            || item.host != self.item.host
            || item.pane != self.item.pane
            || item.kind != self.item.kind
            || item.created_ms != self.item.created_ms
            || item.owner_pane != self.item.owner_pane
            || item.owner_session != self.item.owner_session
            || custody != self.custody.as_ref()
            || !names(id, &self.item)
            || !names(id, item)
        {
            return Err(Error::Invalid(
                "child notification superseded: subject or custody changed".into(),
            ));
        }
        Ok(custodian_of(
            &custody.cloned().into_iter().collect::<Vec<_>>(),
            item,
        ))
    }
}

/// Revalidate child identity and custody through the inbox write. Item removal and
/// custody claims cannot turn this event into an unscoped owner-pane notification.
pub fn notify_child(
    fleet: &Fleet,
    reports: &[crate::hosts::HostReport],
    child: &ChildIdentity,
    id: &str,
    kind: &str,
    text: String,
    now: u64,
) -> Result<bool> {
    let result = crate::custody::locked(fleet.ledger, &child.item.id, |custody| {
        fleet.ledger.with_lock("items", || {
            let custodian = child.validate(fleet.ledger, id, custody)?;
            notify_custodian(fleet, reports, &custodian, id, kind, text, now)
        })
    });
    match result {
        Err(Error::Invalid(reason) | Error::Refused { detail: reason, .. }) => {
            fleet.ledger.with_lock("notification-failures", || {
                let mut failures: std::collections::BTreeMap<String, String> = fleet
                    .ledger
                    .read("notification-failures.json")?
                    .unwrap_or_default();
                failures.insert(id.into(), reason.clone());
                fleet.ledger.write("notification-failures.json", &failures)
            })?;
            eprintln!("dogsled notification {id}: {reason}");
            Ok(false)
        }
        other => other,
    }
}

/// The terminal of `owner`, an agent pane on this host, when a reader reads its inbox.
pub fn readable(
    ledger: &Ledger,
    reports: &[crate::hosts::HostReport],
    owner: &str,
) -> Result<Option<String>> {
    let Some(terminal) = reports
        .iter()
        .find(|h| h.host.is_local())
        .and_then(|h| h.probe.as_ref())
        .and_then(|p| p.agent_panes.iter().find(|a| a.pane == owner))
        .and_then(|a| a.terminal_id.clone())
    else {
        return Ok(None);
    };
    Ok(reader(ledger, &terminal)?.map(|_| terminal))
}

/// Put a note from dogsled in the inbox of the agent that answers for it. `id` makes it
/// idempotent. `Ok(false)` when that agent is not running, has no reader, or runs another
/// session than the one that holds the custody: nothing would deliver it, so the caller
/// keeps it for later.
///
/// This boundary is only for non-child events. Child callers must use `notify_child`
/// with a captured identity; event-name prefixes never grant delivery authority.
pub fn notify(
    fleet: &Fleet,
    reports: &[crate::hosts::HostReport],
    owner: &str,
    id: &str,
    kind: &str,
    text: String,
    now: u64,
) -> Result<bool> {
    match notify_custodian(
        fleet,
        reports,
        &Custodian {
            pane: owner.into(),
            session: None,
        },
        id,
        kind,
        text,
        now,
    ) {
        Err(Error::Invalid(_) | Error::Refused { .. }) => Ok(false),
        other => other,
    }
}

fn notify_custodian(
    fleet: &Fleet,
    reports: &[crate::hosts::HostReport],
    custodian: &Custodian,
    id: &str,
    kind: &str,
    text: String,
    now: u64,
) -> Result<bool> {
    let ledger = fleet.ledger;
    let shell = fleet.shell;
    let Some(terminal) = readable(ledger, reports, &custodian.pane)? else {
        return Ok(false);
    };
    let record = Record {
        id: id.into(),
        terminal,
        pane: custodian.pane.clone(),
        from: Sender::dogsled(),
        kind: kind.into(),
        text,
        created_ms: now,
        state: State::Pending,
        lease: Backoff::default(),
        reason: None,
        done_ms: None,
    };
    // The live agent of the custodian's pane is the one that must receive the record: a
    // pane another session took over never inherits the custody.
    put_checked(ledger, shell, record, Some(custodian)).map(|_| true)
}

/// Whether this host has sent records whose outcome it does not know yet.
pub fn pending(ledger: &Ledger) -> Result<bool> {
    Ok(outbox(ledger)?.iter().any(|s| s.state.open()))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rig {
        ledger: Ledger,
    }

    impl Rig {
        fn new(terminals: &[&str]) -> Self {
            let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
            let rig = Self { ledger };
            rig.live(terminals);
            rig
        }

        fn live(&self, terminals: &[&str]) {
            let agents: Vec<serde_json::Value> = terminals
                .iter()
                .enumerate()
                .map(|(i, t)| serde_json::json!({"pane":format!("w1:p{i}"),"agent":"pi","state":"idle","name":null,"transcript":null,"transcript_modified":null,"terminal_id":t}))
                .collect();
            self.ledger
                .write(
                    "host.json",
                    &serde_json::json!({"sampled_ms":1,"probe":{"host":"local","label":"local","load1":0.0,"cores":1,"memory":{"used_bytes":0,"total_bytes":1},"disk_free_percent":50.0,"agents":{"total":0,"by_status":{}},"agent_panes":agents,"reservations":[],"panes":[]}}),
                )
                .unwrap();
        }

        fn record(terminal: &str, id: &str) -> Record {
            Record {
                id: id.into(),
                terminal: terminal.into(),
                pane: "w1:p0".into(),
                from: Sender {
                    name: Some("lead".into()),
                    pane: Some("w9:p1".into()),
                    terminal: None,
                    host: "mac".into(),
                },
                kind: "message".into(),
                text: "hello".into(),
                created_ms: 1,
                state: State::Pending,
                lease: Backoff::default(),
                reason: None,
                done_ms: None,
            }
        }

        fn reader() -> Reader {
            Reader {
                harness: "pi".into(),
                pane: "w1:p0".into(),
                registered_ms: 1,
            }
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.ledger.dir);
        }
    }

    #[test]
    fn a_terminal_without_a_reader_or_not_running_gets_nothing() {
        let rig = Rig::new(&["term_a"]);
        let error = put(&rig.ledger, Rig::record("term_a", "r1")).unwrap_err();
        assert!(
            error.to_string().contains("no dogsled inbox reader"),
            "{error}"
        );
        let error = put(&rig.ledger, Rig::record("term_gone", "r1")).unwrap_err();
        assert!(error.to_string().contains("not running"), "{error}");
        assert!(put(&rig.ledger, Rig::record("../x", "r1")).is_err());
        assert_eq!(received(&rig.ledger, "term_a").unwrap().len(), 0);
    }

    #[test]
    fn an_unacknowledged_claim_is_offered_again_after_its_lease_until_acked() {
        let rig = Rig::new(&["term_a"]);
        assert_eq!(
            claim(&rig.ledger, "term_a", &Rig::reader(), 0)
                .unwrap()
                .len(),
            0
        );
        put(&rig.ledger, Rig::record("term_a", "r1")).unwrap();
        // A repeated put of the same id is one record.
        put(&rig.ledger, Rig::record("term_a", "r1")).unwrap();
        let first = claim(&rig.ledger, "term_a", &Rig::reader(), 1_000).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(
            first[0].render(),
            "dogsled message from lead (w9:p1 on mac):\nhello"
        );
        // The lease holds it while the reader works.
        assert_eq!(
            claim(&rig.ledger, "term_a", &Rig::reader(), 1_001)
                .unwrap()
                .len(),
            0
        );
        // The reader died before its ack: the lease ends and the record comes back.
        let lease = first[0].lease.next_ms;
        assert_eq!(lease, 1_000 + crate::backoff::delay(1, "r1"));
        let second = claim(&rig.ledger, "term_a", &Rig::reader(), lease).unwrap();
        assert_eq!(second[0].lease.attempts, 2);
        assert_eq!(
            ack(&rig.ledger, "term_a", &["r1".into()], lease + 1).unwrap(),
            1
        );
        assert_eq!(
            ack(&rig.ledger, "term_a", &["r1".into()], lease + 2).unwrap(),
            0
        );
        assert_eq!(
            claim(&rig.ledger, "term_a", &Rig::reader(), u64::MAX)
                .unwrap()
                .len(),
            0
        );
        let outcome = states(&rig.ledger, "term_a", &["r1".into()]).unwrap();
        assert_eq!(outcome[0].state, State::Delivered);
    }

    #[test]
    fn records_of_a_gone_terminal_fail_and_its_files_go_once_final_records_age_out() {
        let rig = Rig::new(&["term_a", "term_b"]);
        claim(&rig.ledger, "term_a", &Rig::reader(), 0).unwrap();
        claim(&rig.ledger, "term_b", &Rig::reader(), 0).unwrap();
        put(&rig.ledger, Rig::record("term_a", "r1")).unwrap();
        put(&rig.ledger, Rig::record("term_b", "r2")).unwrap();
        maintain(&rig.ledger, &["term_b".into()], 10).unwrap();
        let gone = states(&rig.ledger, "term_a", &["r1".into()]).unwrap();
        assert_eq!(gone[0].state, State::Failed);
        assert_eq!(gone[0].reason.as_deref(), Some("its terminal is gone"));
        assert_eq!(
            states(&rig.ledger, "term_b", &["r2".into()]).unwrap()[0].state,
            State::Pending
        );
        maintain(&rig.ledger, &["term_b".into()], 10 + KEEP_MS).unwrap();
        let dir = rig.ledger.dir.join("inbox");
        assert!(!dir.join("term_a.json").exists());
        assert!(!dir.join("term_a.reader.json").exists());
        assert!(dir.join("term_b.json").exists());
    }

    #[test]
    fn a_record_claimed_too_often_fails() {
        let rig = Rig::new(&["term_a"]);
        claim(&rig.ledger, "term_a", &Rig::reader(), 0).unwrap();
        put(&rig.ledger, Rig::record("term_a", "r1")).unwrap();
        let mut now = 0;
        for _ in 0..MAX_ATTEMPTS {
            let claimed = claim(&rig.ledger, "term_a", &Rig::reader(), now).unwrap();
            now = claimed[0].lease.next_ms;
        }
        maintain(&rig.ledger, &["term_a".into()], now).unwrap();
        let outcome = &states(&rig.ledger, "term_a", &["r1".into()]).unwrap()[0];
        assert_eq!(outcome.state, State::Failed);
        assert_eq!(
            outcome.reason.as_deref(),
            Some("not acknowledged after 8 attempts")
        );
    }

    #[test]
    fn r1b_item_expiry_between_tell_and_notify_never_delivers_to_reused_owner() {
        let rig = Rig::new(&["replacement_owner"]);
        let shell = crate::shell::Fake::new();
        let policy = crate::config::Policy::default();
        let fleet = Fleet {
            ledger: &rig.ledger,
            shell: &shell,
            policy: &policy,
            home: "/".into(),
            hosts: vec![crate::hosts::Host::local()],
        };
        claim(&rig.ledger, "replacement_owner", &Rig::reader(), 0).unwrap();
        for event in [
            "reply-gone-1",
            "unit-reply-gone",
            "blocked-gone-1",
            "signal-gone-1",
        ] {
            let item = crate::items::Item {
                id: "gone".into(),
                host: "child-host".into(),
                pane: "child-pane".into(),
                owner_pane: "w1:p0".into(),
                owner_session: Some("original-owner".into()),
                ..Default::default()
            };
            crate::store::put_items(&rig.ledger, &vec![item.clone()]).unwrap();
            let child = ChildIdentity::capture(&rig.ledger, &item).unwrap();
            // Sweep captured the subject; retention removes it before the inbox boundary.
            crate::store::put_items(&rig.ledger, &Vec::<crate::items::Item>::new()).unwrap();
            assert!(
                !notify_child(&fleet, &[], &child, event, "reply", "old child".into(), 1).unwrap()
            );
            assert_eq!(received(&rig.ledger, "replacement_owner").unwrap().len(), 0);
            let failures: std::collections::BTreeMap<String, String> = rig
                .ledger
                .read("notification-failures.json")
                .unwrap()
                .unwrap();
            assert!(failures[event].contains("subject expired or missing"));
        }
        assert!(
            shell.calls.lock().unwrap().is_empty(),
            "no fallback to a pane-only live lookup"
        );
    }

    #[test]
    fn r1b_nonchild_notifications_keep_existing_delivery() {
        let rig = Rig::new(&["term_owner"]);
        let shell = crate::shell::Fake::new();
        shell.on("herdr agent list", r#"{"result":{"agents":[{"pane_id":"w1:p0","terminal_id":"term_owner","agent":"pi","agent_status":"idle"}]}}"#);
        let policy = crate::config::Policy::default();
        let fleet = Fleet {
            ledger: &rig.ledger,
            shell: &shell,
            policy: &policy,
            home: "/".into(),
            hosts: vec![crate::hosts::Host::local()],
        };
        claim(&rig.ledger, "term_owner", &Rig::reader(), 0).unwrap();
        let snapshot: crate::node::Snapshot = rig.ledger.read("host.json").unwrap().unwrap();
        let reports: Vec<crate::hosts::HostReport> = serde_json::from_value(serde_json::json!([{ "id":"local", "label":"local", "service":"up", "reachable":true, "sampled_ms":1, "probe":snapshot.probe, "headroom":null, "error":null, "jobs":[] }])).unwrap();
        assert!(
            notify(
                &fleet,
                &reports,
                "w1:p0",
                "service-health",
                "message",
                "healthy".into(),
                1
            )
            .unwrap()
        );
        assert_eq!(received(&rig.ledger, "term_owner").unwrap().len(), 1);
    }

    #[test]
    fn oversized_text_is_refused() {
        let rig = Rig::new(&["term_a"]);
        claim(&rig.ledger, "term_a", &Rig::reader(), 0).unwrap();
        let mut record = Rig::record("term_a", "r1");
        record.text = "x".repeat(MAX_TEXT + 1);
        assert!(
            put(&rig.ledger, record)
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
    }
}
