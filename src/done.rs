//! Durable terminal handoffs and turn signals owned by the child's host.
//! Idle without `done` is not consent.
//! Identity is launch/item + pane + harness session, never the restorable Herdr terminal.

use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::probe::Agent;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const FILE: &str = "done.json";

/// One line because Herdr refuses newlines in an argument.
pub const CONTRACT: &str = "Dogsled started you as a child agent. When your whole task is \
complete, call the dogsled `done` tool (or run `dogsled done`) once, then end your turn with \
your final reply. Do not call `done` when you ask a question, wait for input, or own open \
children. Commit your changes first. `done` hands your work back: Dogsled closes your pane \
and agent worktree when this turn ends, keeps your branch, and leaves landing to your owner. \
After that turn, messages cannot reopen you; more work needs a new child.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branch {
    pub repo: String,
    pub name: String,
    pub ahead: u64,
    /// Settled: integrated into the default branch, or no local/origin branch remains.
    pub landed: bool,
}

/// A live working turn. Missing evidence remains missing across restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub state_change_seq: Option<u64>,
    pub state_since_ms: Option<u64>,
}

impl Turn {
    fn of_agent(agent: &Agent) -> Self {
        Self {
            state_change_seq: agent.state_change_seq,
            state_since_ms: agent.state_since_ms,
        }
    }

    fn precedes(&self, agent: &Agent) -> bool {
        let (Some(before), Some(after), Some(since), Some(now)) = (
            self.state_change_seq,
            agent.state_change_seq,
            self.state_since_ms,
            agent.state_since_ms,
        ) else {
            return false;
        };
        agent.state == "working" && (after > before || (after == before && now > since))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signal {
    pub kind: crate::lifecycle::Signal,
    pub text: String,
    pub event_id: String,
    pub turn: Turn,
    pub session: String,
    pub recorded_ms: u64,
    pub cleared_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    pub pane: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<u64>,
    /// Legacy completion number, also used for reply deduplication. Zero means counters reset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed: Option<u64>,
    pub marked_ms: u64,
    /// New done calls retain their turn identity; legacy done marks have no turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_turn: Option<Turn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<Signal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<Branch>,
    /// The child's checkout is detached: it has no branch to land, and its commits must be
    /// held by the default branch or a remote. Recorded so the handoff checks it once.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub detached: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<crate::items::Reply>,
    /// The reply came from the native `done` handoff, so it proves the child finished. A
    /// sealed terminal receipt is a result, not a native `done`, so it never proves a
    /// handoff. Marks from before this field load with `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub native_done: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped_ms: Option<u64>,
}

type Marks = BTreeMap<String, Mark>;

fn has_done(mark: &Mark) -> bool {
    // A legacy mark is a done call. A signal-only record always keeps its signal,
    // including after clearing, and has no done turn.
    mark.done_turn.is_some() || mark.signal.is_none()
}

fn conflict(pane: &str) -> Error {
    Error::Refused {
        code: crate::error::Refusal::SignalConflict,
        detail: format!("{pane}: the turn already has a conflicting signal or done handoff"),
    }
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "wired by -17 turn-signal delivery")
)]
fn unproven(pane: &str) -> Error {
    Error::Refused {
        code: crate::error::Refusal::SignalUnproven,
        detail: format!("{pane}: turn signal identity is unproven; nothing was recorded"),
    }
}

fn waiting(agent: &Agent) -> bool {
    matches!(agent.state.as_str(), "idle" | "done")
}

fn identity(ledger: &Ledger, pane: &str) -> Result<Option<String>> {
    if let Some(id) = crate::launch::identity(ledger, pane)? {
        return Ok(Some(id));
    }
    Ok(crate::items::list(ledger)?
        .into_iter()
        .find(|i| i.pane == pane && i.host == crate::hosts::LOCAL && i.closed_ms.is_none())
        .map(|i| i.id))
}

fn key(mark: &Mark) -> String {
    // JSON tuples avoid ambiguous concatenation, and the key is never a file path.
    serde_json::json!([mark.item, mark.pane, mark.session]).to_string()
}

fn matches(mark: &Mark, agent: &Agent) -> bool {
    mark.pane == agent.pane && mark.session == agent.agent_session
}

fn owners(ledger: &Ledger) -> Result<BTreeMap<String, Vec<String>>> {
    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for item in crate::items::list(ledger)?
        .into_iter()
        .filter(|i| i.closed_ms.is_none())
    {
        owners
            .entry(item.owner_pane)
            .or_default()
            .push(item.name.unwrap_or(item.pane));
    }
    for ticket in crate::work::queue(ledger)?
        .into_iter()
        .filter(|t| matches!(t.state.as_str(), "pending" | "admitting"))
    {
        owners
            .entry(ticket.request.owner)
            .or_default()
            .push(format!("queued {}", ticket.id));
    }
    Ok(owners)
}

/// Preserve unmatched legacy records until their pane is observable. Migration never deletes
/// a mark, even if its old terminal disappeared before the first read by this version.
fn migrate(ledger: &Ledger, marks: Marks, agents: &[Agent]) -> Result<Marks> {
    let mut migrated = Marks::new();
    for (old_key, mut mark) in marks {
        if old_key != key(&mark) {
            if let Some(agent) = agents.iter().find(|a| a.pane == mark.pane) {
                mark.session.clone_from(&agent.agent_session);
                mark.name.clone_from(&agent.name);
                mark.item = identity(ledger, &mark.pane)?;
            } else {
                migrated.insert(old_key, mark);
                continue;
            }
        }
        let new_key = key(&mark);
        if migrated.contains_key(&new_key) {
            migrated.insert(old_key, mark);
        } else {
            migrated.insert(new_key, mark);
        }
    }
    Ok(migrated)
}

/// The pane's unit evidence. An error stays an error: it never reads as a plain pane.
type Evidence = std::result::Result<crate::launch::Execution, String>;

fn evidence(ledger: &Ledger, mark: &Mark, agent: &Agent) -> Evidence {
    crate::launch::execution(ledger, mark.item.as_deref(), agent).map_err(|e| e.to_string())
}

/// `done` can end a plain pane's work. A unit execution ends only when its worker
/// acknowledged the sealed terminal boundary; until then its owner can still continue it.
fn closable(evidence: &Evidence) -> bool {
    matches!(
        evidence,
        Ok(crate::launch::Execution::Plain | crate::launch::Execution::Returned)
    )
}

/// A successful mark was made while working. Its first idle/done observation ends that turn,
/// even after restore with no `completion_seq`. Working/blocked/unknown never authorize close.
/// Once sealed it is terminal: a later turn cannot revoke it, but busy panes still never close.
/// An unreturned unit is not sealed. A seal written before this rule stays, and `terminal`
/// decides.
fn seal(mark: &mut Mark, agent: &Agent, evidence: &Evidence) {
    if has_done(mark) && mark.sealed.is_none() && waiting(agent) && closable(evidence) {
        mark.sealed = Some(agent.completion_seq.unwrap_or(0));
    }
}

fn terminal(mark: &Mark, evidence: &Evidence) -> bool {
    mark.sealed.is_some() && closable(evidence)
}

/// Who offers input to a marked pane.
#[derive(Debug, Clone, Copy)]
pub enum Input<'a> {
    /// The pane's own inbox reader claims records.
    Claim,
    /// A sender puts a record; `from` is the whole sender, because a pane ID is unique only
    /// on its own host.
    Put {
        from: Option<&'a crate::inbox::Sender>,
    },
    /// The local owner continues a unit execution whose launch record predates the owner
    /// host. `continue_legacy` is the only caller: its acceptance proves the original
    /// admission and the owner's live session under the launch lock.
    Legacy { from: &'a crate::inbox::Sender },
}

pub fn observe(ledger: &Ledger, agents: &mut [Agent]) -> Result<()> {
    ledger.with_lock("done", || {
        let before: Marks = ledger.read(FILE)?.unwrap_or_default();
        let mut marks = migrate(ledger, before.clone(), agents)?;
        let owners = owners(ledger)?;
        for agent in agents {
            let mut done = false;
            let mut recorded = false;
            let mut called = false;
            // The terminal evidence the unit return needs. It is one read per pane.
            let mut ready = false;
            let mut active_signal = None;
            for mark in marks.values_mut().filter(|m| matches(m, agent)) {
                if let Some(signal) = &mut mark.signal
                    && signal.session == agent.agent_session.as_deref().unwrap_or("")
                {
                    if signal.cleared_ms.is_none() && signal.turn.precedes(agent) {
                        signal.cleared_ms = Some(crate::ledger::now_ms());
                    }
                    if signal.cleared_ms.is_none() {
                        active_signal = Some(signal.kind.as_str().to_owned());
                    }
                }
                if !has_done(mark) {
                    continue;
                }
                let evidence = evidence(ledger, mark, agent);
                seal(mark, agent, &evidence);
                let closable = terminal(mark, &evidence)
                    && waiting(agent)
                    && !owners.contains_key(&agent.pane);
                done |= closable;
                // The handoff is recorded only with its saved final reply and the
                // launch evidence the unit return needs.
                if closable && mark.reply.is_some() {
                    if !ready {
                        ready = crate::launch::handoff_ready(ledger, &agent.pane)?;
                    }
                    recorded |= ready;
                }
                called = true;
            }
            // `done` stays the guard's turn-end fact: the handoff that records the reply
            // looks for it. The published handoff below is the stricter claim.
            agent.signal = active_signal;
            agent.done = done;
            // The native `done` was accepted, but the handoff is not complete.
            agent.finishing = called && !recorded;
            // The handoff is published from these fresh facts, so a second observation
            // never reads back its own previous value.
            agent.handoff = Some(
                match (recorded, agent.finishing) {
                    (true, _) => "recorded",
                    (false, true) => "accepted",
                    (false, false) => "none",
                }
                .to_owned(),
            );
            // The same scan the done guard uses: open items and pending or admitting
            // child tickets. A turn that ended while the agent owns work waits for it.
            agent.owns_open = owners.contains_key(&agent.pane);
            // The lifecycle owner publishes the state with the agent, so every reader of
            // the host snapshot sees the state this host observed.
            crate::lifecycle::publish(agent, crate::lifecycle::Host::Up);
        }
        if before != marks {
            ledger.write(FILE, &marks)?;
        }
        Ok(())
    })
}

pub fn finished(ledger: &Ledger, agents: &[Agent]) -> Result<BTreeSet<String>> {
    let mut agents = agents.to_vec();
    observe(ledger, &mut agents)?;
    Ok(agents
        .into_iter()
        .filter(|a| a.done)
        .map(|a| a.pane)
        .collect())
}

/// Validate while the caller can still repair its work. No mark is written on a refusal.
pub fn mark(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    pane: &str,
    now: u64,
) -> Result<Mark> {
    let agent = crate::inbox::local_agent(shell, pane)?;
    if agent.state != "working" {
        return Err(Error::Invalid(format!(
            "{pane} is {}; call done during the turn that completes your task",
            agent.state
        )));
    }
    if let Some(open) = owners(ledger)?.get(pane) {
        return Err(Error::Invalid(format!(
            "{pane} still owns open work: {}. Finish or close each child and cancel queued work before calling done",
            open.join(", ")
        )));
    }
    let session = agent.agent_session.clone().ok_or_else(|| {
        Error::Invalid(format!(
            "{pane}: Herdr reports no agent session; cannot record a durable done"
        ))
    })?;
    let item = identity(ledger, pane)?;
    let (branch, detached) = match preflight(shell, &agent)? {
        Preflight::None => (None, false),
        Preflight::Detached => (None, true),
        Preflight::Branch(branch) => (Some(branch), false),
    };
    let mark = Mark {
        pane: pane.into(),
        item,
        session: Some(session),
        name: agent.name.clone(),
        after: agent.state_change_seq,
        sealed: None,
        marked_ms: now,
        done_turn: Some(Turn::of_agent(&agent)),
        signal: None,
        branch,
        detached,
        reply: None,
        native_done: false,
        closed_ms: None,
        dropped_ms: None,
    };
    ledger.with_lock("done", || {
        let mut marks: Marks = ledger.read(FILE)?.unwrap_or_default();
        for existing in marks.values().filter(|m| matches(m, &agent)) {
            if let Some(signal) = &existing.signal
                && signal.cleared_ms.is_none()
                && !signal.turn.precedes(&agent)
            {
                return Err(conflict(pane));
            }
        }
        if let Some(existing) = marks.get(&key(&mark))
            && existing.sealed.is_some()
        {
            return Err(Error::Invalid(format!(
                "{pane} finished; start a new child for more work"
            )));
        }
        marks.insert(key(&mark), mark.clone());
        ledger.write(FILE, &marks)
    })?;
    Ok(mark)
}

/// Record a nonterminal turn signal without checkout or child-work preconditions.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "wired by -17 turn-signal delivery")
)]
pub fn signal(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    pane: &str,
    kind: crate::lifecycle::Signal,
    text: &str,
    now: u64,
) -> Result<Signal> {
    let agent = crate::inbox::local_agent(shell, pane)?;
    if agent.state != "working" {
        return Err(Error::Invalid(format!(
            "{pane} is {}; call {} during the working turn",
            agent.state,
            kind.as_str()
        )));
    }
    let session = agent
        .agent_session
        .as_ref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| unproven(pane))?;
    let turn = Turn::of_agent(&agent);
    let item = identity(ledger, pane)?;
    ledger.with_lock("done", || {
        let mut marks: Marks = ledger.read(FILE)?.unwrap_or_default();
        for mark in marks.values().filter(|m| matches(m, &agent)) {
            if has_done(mark) {
                return Err(conflict(pane));
            }
            if let Some(existing) = &mark.signal {
                if existing.turn == turn {
                    return if existing.kind == kind && existing.text == text {
                        Ok(existing.clone())
                    } else {
                        Err(conflict(pane))
                    };
                }
                if existing.cleared_ms.is_none() && !existing.turn.precedes(&agent) {
                    return Err(unproven(pane));
                }
            }
        }
        let session_hash = crate::unit_transport::digest_hex(session.as_bytes());
        let signal = Signal {
            kind,
            text: text.to_owned(),
            event_id: format!(
                "sig-{pane}-{session_hash}-{}-{}",
                turn.state_change_seq
                    .map_or_else(|| "unknown".into(), |s| s.to_string()),
                turn.state_since_ms
                    .map_or_else(|| "unknown".into(), |s| s.to_string())
            ),
            turn,
            session: session.clone(),
            recorded_ms: now,
            cleared_ms: None,
        };
        let mark = Mark {
            pane: pane.to_owned(),
            item,
            session: Some(session.clone()),
            name: agent.name.clone(),
            after: None,
            sealed: None,
            marked_ms: now,
            done_turn: None,
            signal: Some(signal.clone()),
            branch: None,
            detached: false,
            reply: None,
            native_done: false,
            closed_ms: None,
            dropped_ms: None,
        };
        marks.insert(key(&mark), mark);
        ledger.write(FILE, &marks)?;
        Ok(signal)
    })
}

/// What the child's checkout is: no agent worktree, a detached one with no branch to land, or
/// a branch to land.
#[derive(Debug)]
enum Preflight {
    /// The pane works in no agent worktree.
    None,
    /// A detached checkout. It has no branch, and its commits are already held elsewhere.
    Detached,
    /// A branch to land.
    Branch(Branch),
}

fn preflight(shell: &dyn crate::shell::Shell, agent: &Agent) -> Result<Preflight> {
    let panes = if agent.cwd.is_none() {
        crate::cleanup::current_panes(shell)?
    } else {
        Vec::new()
    };
    let cwd = agent.cwd.as_deref().or_else(|| {
        panes
            .iter()
            .find(|p| p.pane_id == agent.pane)
            .and_then(|p| p.cwd.as_deref())
    });
    let Some(cwd) = cwd else {
        return Ok(Preflight::None);
    };
    let Some(tree) = crate::worktree::linked_or_detached(shell, cwd)? else {
        return Ok(Preflight::None);
    };
    if crate::worktree::lock_reason(shell, &tree.checkout, &tree.path)?.as_deref()
        != Some(crate::worktree::AGENT_LOCK)
    {
        return Ok(Preflight::None);
    }
    let dirty = crate::loss::dirty_paths(shell, &tree.path, false)?;
    if !dirty.is_empty() {
        return Err(Error::Invalid(format!(
            "{} has uncommitted or untracked changes: {}. Commit or preserve these changes before calling done",
            tree.path,
            dirty.join(", ")
        )));
    }
    let repo = tree.checkout.display().to_string();
    // A detached checkout has no branch. None is invented for it, so its commits must be
    // held by the default branch or a refreshed remote ref, exactly as for a branch.
    let detached = tree.branch.is_empty();
    let evidence = if detached {
        crate::loss::Evidence::fetch(shell, &repo)
    } else {
        crate::loss::Evidence::keeping_branch(&repo, &tree.branch)
    };
    if let Some(warning) = &evidence.warning {
        eprintln!("{warning}");
    }
    let head = crate::worktree::head_sha(shell, &tree.path)?;
    let loss = evidence.assess(shell, &head, Some(&tree.path))?;
    if !loss.loses_nothing() {
        return Err(Error::Invalid(format!(
            "{}: {}. Preserve the work before calling done",
            tree.path,
            loss.detail()
        )));
    }
    if detached {
        return Ok(Preflight::Detached);
    }
    let mut branch = Branch {
        repo,
        name: tree.branch,
        ahead: 0,
        landed: false,
    };
    refresh_branch(shell, &mut branch)?;
    Ok(Preflight::Branch(branch))
}

pub fn refresh_branch(shell: &dyn crate::shell::Shell, branch: &mut Branch) -> Result<()> {
    let mut tip = None;
    for reference in [
        format!("refs/heads/{}", branch.name),
        format!("refs/remotes/origin/{}", branch.name),
    ] {
        let exists = shell.run(
            "git",
            &[
                "-C".into(),
                branch.repo.clone(),
                "show-ref".into(),
                "--verify".into(),
                "--quiet".into(),
                reference.clone(),
            ],
        )?;
        match exists.status {
            0 => {
                tip = Some(reference);
                break;
            }
            1 => (),
            _ => {
                return Err(Error::Invalid(format!(
                    "{}: cannot inspect branch {}: {}",
                    branch.repo,
                    branch.name,
                    exists.stderr.trim()
                )));
            }
        }
    }
    let Some(tip) = tip else {
        branch.landed = true;
        branch.ahead = 0;
        return Ok(());
    };
    let landed = crate::loss::Evidence::default_only(&branch.repo)
        .assess(shell, &tip, None)?
        .landed();
    let ahead = if landed {
        0
    } else {
        let base = crate::integration::base(shell, &branch.repo)?;
        shell
            .run_ok(
                "git",
                &[
                    "-C".into(),
                    branch.repo.clone(),
                    "rev-list".into(),
                    "--count".into(),
                    format!("{base}..{tip}"),
                ],
            )?
            .stdout
            .trim()
            .parse()
            .map_err(|_| Error::Invalid(format!("{}: invalid branch commit count", branch.repo)))?
    };
    branch.landed = landed;
    branch.ahead = ahead;
    Ok(())
}

pub fn list(ledger: &Ledger) -> Result<Vec<Mark>> {
    Ok(ledger
        .read::<Marks>(FILE)?
        .unwrap_or_default()
        .into_values()
        .collect())
}

/// Keep explicit branch disposal even when the origin is on another host.
pub fn dropped(ledger: &Ledger, item: &crate::items::Item, now: u64) -> Result<()> {
    ledger.with_lock("done", || {
        let mut marks: Marks = ledger.read(FILE)?.unwrap_or_default();
        for mark in marks.values_mut().filter(|m| {
            m.pane == item.pane
                && m.item.as_deref().is_none_or(|id| id == item.id)
                && m.branch
                    .as_ref()
                    .zip(item.branch.as_ref())
                    .is_some_and(|(a, b)| a.repo == b.repo && a.name == b.name)
        }) {
            mark.dropped_ms.get_or_insert(now);
            if let Some(branch) = &mut mark.branch {
                branch.landed = true;
                branch.ahead = 0;
            }
        }
        ledger.write(FILE, &marks)
    })
}

fn save(ledger: &Ledger, mark: &Mark) -> Result<()> {
    ledger.with_lock("done", || {
        let mut marks: Marks = ledger.read(FILE)?.unwrap_or_default();
        // A message during the same turn may have removed it. Never resurrect it.
        if let std::collections::btree_map::Entry::Occupied(mut entry) = marks.entry(key(mark)) {
            let mut updated = mark.clone();
            // Branch refresh can overlap a live inbox check. Never replace its terminal seal
            // with an older unsealed observation from the start of this handoff pass.
            updated.sealed = entry.get().sealed.or(updated.sealed);
            updated.closed_ms = entry.get().closed_ms.or(updated.closed_ms);
            updated.dropped_ms = entry.get().dropped_ms.or(updated.dropped_ms);
            if updated.dropped_ms.is_some() {
                updated.branch.clone_from(&entry.get().branch);
            }
            entry.insert(updated);
            ledger.write(FILE, &marks)?;
        }
        Ok(())
    })
}

/// Serialize inbox acceptance with sealing. Only new work accepted while the marked turn is
/// still working cancels a plain pane's mark. After idle/done (or an earlier seal), reject all
/// messages, even if Herdr subsequently reports working. No post-turn claim can reopen a
/// finished child.
///
/// A unit execution that has not returned is not finished. Only its execution owner may
/// continue or cancel its mark, by putting a record. Every check runs before `accept`, so a
/// refusal leaves the mark and the inbox unchanged.
pub fn input<T>(
    ledger: &Ledger,
    agent: &Agent,
    input: Input<'_>,
    accept: impl FnOnce() -> Result<(T, bool)>,
) -> Result<T> {
    if matches!(input, Input::Legacy { .. }) {
        return Err(Error::Invalid(
            "legacy continuation needs its own guarded acceptance".into(),
        ));
    }
    guarded(ledger, agent, input, |_| accept())
}

/// Continue the one done mark of a legacy unit execution in `agent`'s pane. A mark must
/// belong to an unreturned launch with no owner host, and `from` must be its owner pane on
/// this host. `accept` gets that exact mark, or `None` when the pane has no mark, while the
/// done lock is held. It must prove the original admission and decide from its durable
/// receipt. It returns whether its acceptance consumed the mark: only then is that mark, and
/// no other, removed, and only if it is unchanged.
pub fn continue_legacy<T>(
    ledger: &Ledger,
    agent: &Agent,
    from: &crate::inbox::Sender,
    accept: impl FnOnce(Option<&Mark>) -> Result<(T, bool)>,
) -> Result<T> {
    guarded(ledger, agent, Input::Legacy { from }, accept)
}

fn guarded<T>(
    ledger: &Ledger,
    agent: &Agent,
    input: Input<'_>,
    accept: impl FnOnce(Option<&Mark>) -> Result<(T, bool)>,
) -> Result<T> {
    ledger.with_lock("done", || {
        let before: Marks = ledger.read(FILE)?.unwrap_or_default();
        let mut marks = migrate(ledger, before.clone(), std::slice::from_ref(agent))?;
        let mut refusal = None;
        let mut continued = false;
        let mut legacy = None;
        let matched = marks.values().filter(|m| matches(m, agent) && has_done(m)).count();
        if matches!(input, Input::Legacy { .. }) && matched > 1 {
            return Err(Error::Invalid(format!(
                "{}: competing done marks; nothing was accepted",
                agent.pane
            )));
        }
        for (key, mark) in marks.iter_mut().filter(|(_, m)| matches(m, agent) && has_done(m)) {
            let evidence = evidence(ledger, mark, agent);
            seal(mark, agent, &evidence);
            let name = mark
                .name
                .as_deref()
                .or(agent.name.as_deref())
                .unwrap_or(&agent.pane)
                .to_owned();
            refusal = if terminal(mark, &evidence) {
                Some(format!("{name} finished; start a new child for more work"))
            } else {
                match (evidence, input) {
                    // Legacy continuation exists only for a launch record that predates the
                    // owner host. Every other execution keeps the ordinary rules.
                    (
                        Ok(crate::launch::Execution::Unreturned {
                            owner,
                            host: None,
                        }),
                        Input::Legacy { from },
                    ) if from.pane.as_deref() == Some(owner.as_str())
                        && from.host == crate::inbox::hostname()
                        && mark.closed_ms.is_none() =>
                    {
                        legacy = Some((key.clone(), mark.clone()));
                        None
                    }
                    (Ok(crate::launch::Execution::Unreturned { host: Some(_), .. }), Input::Legacy { .. }) => Some(format!(
                        "{name}'s launch record names its owner host; continue it with dogsled send; nothing was accepted"
                    )),
                    (Ok(crate::launch::Execution::Plain), Input::Legacy { .. }) => Some(format!(
                        "{name} has no unit execution; continue it with dogsled send; nothing was accepted"
                    )),
                    (Ok(crate::launch::Execution::Plain), _) => None,
                    (Ok(crate::launch::Execution::Returned), _) => Some(format!(
                        "{name} returned its unit; start a new child for more work"
                    )),
                    (Err(error), _) => Some(format!(
                        "{name} called done, and its unit evidence is unavailable: {error}; nothing was accepted"
                    )),
                    // Pane and host must both match the stored originating owner. A launch
                    // record with no owner host proves nobody, so it refuses every sender.
                    (
                        Ok(crate::launch::Execution::Unreturned {
                            owner,
                            host: Some(host),
                        }),
                        Input::Put { from: Some(from) },
                    ) if from.pane.as_deref() == Some(owner.as_str()) && from.host == host => {
                        continued = true;
                        None
                    }
                    (Ok(crate::launch::Execution::Unreturned { owner, .. }), _) => Some(format!(
                        "{name} called done before its unit returned; only its owner {owner} can continue it; nothing was accepted"
                    )),
                }
            };
            if refusal.is_some() {
                break;
            }
        }
        if let Some(refusal) = refusal {
            if marks != before {
                ledger.write(FILE, &marks)?;
            }
            return Err(Error::Invalid(refusal));
        }
        // Legacy input without a refusal reaches here with its one mark, or with no matching
        // mark at all: its receipt decides whether that is a finished retry.
        let (result, received) = accept(legacy.as_ref().map(|(_, mark)| mark))?;
        if let Some((key, mark)) = &legacy {
            // The acceptance consumed this mark. Remove exactly it, and only if unchanged.
            if received && marks.get(key) == Some(mark) {
                marks.remove(key);
            }
        } else if received && (agent.state == "working" || continued) {
            marks.retain(|_, m| !matches(m, agent) || !has_done(m));
        }
        if marks != before {
            ledger.write(FILE, &marks)?;
        }
        Ok(result)
    })
}

/// Child-host action lane, independent of fleet discovery, owner availability and sweeps.
/// Keep the result durably before closing; the owner's children lane imports it afterwards.
pub fn handoff(ledger: &Ledger, shell: &dyn crate::shell::Shell, now: u64) -> Result<()> {
    ledger.with_lock("handoff", || handoff_locked(ledger, shell, now))
}

fn handoff_locked(ledger: &Ledger, shell: &dyn crate::shell::Shell, now: u64) -> Result<()> {
    if list(ledger)?.is_empty() {
        return Ok(());
    }
    let mut agents = crate::probe::agent_list(shell)?;
    observe(ledger, &mut agents)?;
    for mut mark in list(ledger)? {
        if mark.dropped_ms.is_some() {
            continue;
        }
        if let Some(branch) = &mut mark.branch
            && !(mark.closed_ms.is_some() && branch.landed)
        {
            if let Err(error) = refresh_branch(shell, branch) {
                eprintln!("dogsled handoff: branch {}: {error}", branch.name);
            }
            save(ledger, &mark)?;
        }
        if mark.closed_ms.is_some() {
            continue;
        }
        let Some(agent) = agents.iter().find(|a| matches(&mark, a) && a.done) else {
            continue;
        };
        if !crate::launch::handoff_ready(ledger, &mark.pane)? {
            continue;
        }
        if mark.reply.is_none() {
            let reply = if let Some(text) = crate::launch::handoff_reply(ledger, &mark.pane)? {
                crate::sweep::Final {
                    seq: mark.sealed.unwrap_or(0),
                    text,
                }
            } else if let Some(reply) = crate::sweep::local_reply(shell, ledger, &mark.pane)? {
                reply
            } else {
                continue;
            };
            mark.reply = Some(crate::items::Reply {
                seq: reply.seq,
                result_id: None,
                text: reply.text,
                captured_ms: now,
                reported_ms: None,
                done: true,
            });
            // The native `done` handoff produced this reply. It is the only provenance
            // that proves the child finished.
            mark.native_done = true;
            save(ledger, &mark)?;
        }
        if let (Some(id), Some(reply)) = (&mark.item, &mark.reply)
            && !crate::items::list(ledger)?.iter().any(|i| {
                i.id == *id
                    && i.reply
                        .as_ref()
                        .is_some_and(|r| r.result_id.is_some() || (r.seq == reply.seq && r.done))
            })
        {
            crate::items::set_reply_with(ledger, id, reply.clone(), mark.native_done)?;
        }
        if mark.branch.is_none() && !mark.detached {
            match preflight(shell, agent) {
                Ok(Preflight::None) => {
                    save(ledger, &mark)?;
                }
                Ok(Preflight::Detached) => {
                    mark.detached = true;
                    save(ledger, &mark)?;
                }
                Ok(Preflight::Branch(branch)) => {
                    mark.branch = Some(branch);
                    save(ledger, &mark)?;
                }
                Err(error) => {
                    eprintln!("dogsled handoff: {}: {error}", mark.pane);
                    continue;
                }
            }
        }
        match crate::close::local(
            shell,
            ledger,
            &mark.pane,
            &crate::close::Caller::from_env(),
            crate::close::Guard::Writer,
        ) {
            Ok(_) => {
                mark.closed_ms = Some(now);
                save(ledger, &mark)?;
                crate::items::close(ledger, crate::hosts::LOCAL, &mark.pane, now)?;
                eprintln!("dogsled handoff: closed {}", mark.pane);
            }
            Err(error) => eprintln!("dogsled handoff: {}: {error}", mark.pane),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
