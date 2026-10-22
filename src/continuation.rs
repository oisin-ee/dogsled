//! Owner-side continuation of a seeded worker whose launch record predates `owner_host`.
//!
//! Such a record cannot prove which machine its owner pane ID belongs to, so ordinary input
//! refuses every sender. This operation is bounded to the origin host: the caller must be the
//! owner pane on the host whose ledger holds the original admission, the admission must have
//! selected this host, and the caller's live Pi session must be the admission's origin session.
//! Nothing is backfilled, relaunched, sealed, returned, or settled.
//!
//! Lock order: `admissions/<request>`, then `done`, then `launches/<request>`, held for the
//! whole decision. Inside them, `inbox/<terminal>` and then `outbox` are taken one at a time,
//! and neither holds another lock. No lock is taken twice, and no lock is taken out of this
//! order.
//!
//! Replay protection belongs to the unit execution, not to the inbox, which prunes final
//! records after 24 hours. `launches/<request>.continuations.json` keeps one receipt per
//! continuation series (request, owner, kind, and text). The receipt is written before the
//! input exists and names the exact done mark it continues. It is never removed, so the
//! series can never continue another mark.

use crate::done::Mark;
use crate::error::{Error, Result};
use crate::inbox::{Record, Sender, Sent, State};
use crate::ledger::Ledger;
use crate::probe::Agent;
use crate::unit_transport::{ExecutionRef, Seed, conflict, digest_hex};
use crate::work::{Attempt, Launch, Request};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Continue the legacy unit execution in the local pane `target`, as its owner, with `text`.
pub fn legacy(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    target: &str,
    kind: &str,
    text: &str,
    now: u64,
) -> Result<Sent> {
    let owner = std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|pane| !pane.is_empty())
        .ok_or_else(|| Error::Invalid("HERDR_PANE_ID is required".into()))?;
    if owner == target {
        return Err(Error::Invalid(
            "an agent cannot continue its own pane".into(),
        ));
    }
    let worker = crate::inbox::local_agent(shell, target)?;
    let terminal = worker
        .terminal_id
        .clone()
        .ok_or_else(|| Error::Invalid(format!("Herdr reports no terminal id for {target}")))?;
    let id = crate::launch::identity(ledger, target)?.ok_or_else(|| {
        Error::Invalid(format!(
            "{target} has no launch record on this host; nothing was sent"
        ))
    })?;
    crate::jobs::validate_id(&id)?;
    let from = sender(&crate::inbox::local_agent(shell, &owner)?);
    let series = format!(
        "legacy-{}-",
        &digest_hex(json!([id, owner, kind, text]).to_string().as_bytes())[..24]
    );
    let sent = ledger.child("admissions").with_lock(&id, || {
        crate::done::continue_legacy(ledger, &worker, &from, |mark| {
            crate::launch::with_legacy_unreturned(ledger, &id, &worker, |launch| {
                let origin = admission(ledger, &id, &owner, &worker.pane, launch)?;
                // Fresh observations, immediately before the input is stored.
                let current = crate::inbox::local_agent(shell, &owner)?;
                if sender(&current) != from || pi_session(&current)? != origin {
                    return Err(conflict(&format!(
                        "{owner} no longer runs the session that started this unit"
                    )));
                }
                let live = crate::inbox::local_agent(shell, target)?;
                if live.terminal_id != worker.terminal_id
                    || live.agent_session != worker.agent_session
                    || live.agent != worker.agent
                    || live.name != worker.name
                {
                    return Err(conflict(&format!("{target}: worker identity changed")));
                }
                item(ledger, &id, target)?;
                let request = Continuation {
                    request: id.clone(),
                    series: series.clone(),
                    owner: from.clone(),
                    session: origin,
                    worker: Worker::of(&worker),
                    kind: kind.into(),
                    text_sha256: digest_hex(text.as_bytes()),
                };
                if let Some(receipt) = receipts(ledger, &id)?.remove(&series) {
                    return resume(ledger, &request, receipt, mark, text, now);
                }
                let mark = mark.ok_or_else(|| {
                    Error::Invalid(format!(
                        "{target} has no done mark to continue; nothing was accepted"
                    ))
                })?;
                let receipt = Receipt {
                    sent: Sent {
                        id: record_id(&series, mark),
                        host: crate::hosts::LOCAL.into(),
                        host_label: crate::hosts::LOCAL.into(),
                        terminal: terminal.clone(),
                        pane: target.into(),
                        name: worker.name.clone(),
                        harness: worker.agent.clone(),
                        from: from.clone(),
                        kind: kind.into(),
                        created_ms: now,
                        state: State::Pending,
                        reason: None,
                        check: crate::backoff::Backoff::default(),
                    },
                    request,
                    mark: mark.clone(),
                    phase: Phase::Prepared,
                };
                // A record that nothing would deliver is refused before any write.
                crate::inbox::deliverable(ledger, &record(&receipt.sent, text))?;
                // The intent is durable before any input exists: it fences this series to
                // this mark for good.
                write(ledger, &id, &receipt, None)?;
                crate::unit_request::crash_point("legacy-continue-prepared");
                complete(ledger, receipt, text, now)
            })
        })
    })?;
    // The done mark is removed. A retry from here returns this record from the receipt.
    crate::unit_request::crash_point("legacy-continue-removed");
    Ok(sent)
}

/// The receipts file of request `id`: one receipt per continuation series. It belongs to the
/// unit execution, beside its launch record. No path removes or rewrites `launches/` files
/// other than their own writers, and nothing prunes them. Only the holder of the
/// `launches/<id>` lock reads or writes this file.
fn receipts_file(id: &str) -> String {
    format!("{id}.continuations.json")
}

fn receipts(ledger: &Ledger, id: &str) -> Result<BTreeMap<String, Receipt>> {
    Ok(ledger
        .child("launches")
        .read(&receipts_file(id))?
        .unwrap_or_default())
}

/// Store `receipt`. `from` is the phase the stored receipt must have now: `None` for a new
/// series. Every other field stays as it was first written.
fn write(ledger: &Ledger, id: &str, receipt: &Receipt, from: Option<Phase>) -> Result<()> {
    let mut all = receipts(ledger, id)?;
    let stored = all.get(&receipt.request.series);
    let unchanged = |stored: &Receipt| {
        Receipt {
            phase: receipt.phase,
            ..stored.clone()
        } == *receipt
    };
    match (stored, from) {
        (None, None) => (),
        (Some(stored), Some(phase)) if stored.phase == phase && unchanged(stored) => (),
        _ => return Err(conflict("the continuation receipt changed")),
    }
    all.insert(receipt.request.series.clone(), receipt.clone());
    ledger.child("launches").write(&receipts_file(id), &all)
}

/// Finish a prepared continuation while its done mark is in place: store the input, record
/// it in the outbox, and accept it. Each step is idempotent. While the mark is in place the
/// worker's own claim is refused, so the input is still pending and nothing pruned it.
fn complete(ledger: &Ledger, mut receipt: Receipt, text: &str, now: u64) -> Result<(Sent, bool)> {
    crate::inbox::put_continuation(ledger, record(&receipt.sent, text))?;
    crate::unit_request::crash_point("legacy-continue-stored");
    crate::inbox::record_sent(ledger, &receipt.sent, now)?;
    receipt.phase = Phase::Accepted;
    write(
        ledger,
        &receipt.request.request,
        &receipt,
        Some(Phase::Prepared),
    )?;
    crate::unit_request::crash_point("legacy-continue-accepted");
    Ok((receipt.sent, true))
}

/// An identical request whose series already has a receipt. The receipt must match the request
/// exactly. Then:
///
/// - The done mark it names is still in place: an earlier run stopped before it removed the
///   mark. Finish that run, and remove the mark.
/// - It is accepted and no mark is in place: the earlier run finished. Return its record, and
///   restore its outbox entry if that was lost.
/// - Another mark is in place: a stale replay. Refuse it, and keep that mark.
/// - It is prepared and its mark is gone: nothing proves the input was stored. Refuse.
///
/// No case stores input for any mark other than the one the receipt names.
fn resume(
    ledger: &Ledger,
    request: &Continuation,
    receipt: Receipt,
    mark: Option<&Mark>,
    text: &str,
    now: u64,
) -> Result<(Sent, bool)> {
    if receipt.request != *request
        || receipt.sent.id != record_id(&request.series, &receipt.mark)
        || receipt.sent.kind != request.kind
        || receipt.sent.from != request.owner
        || Some(&receipt.sent.terminal) != request.worker.terminal.as_ref()
        || receipt.sent.pane != request.worker.pane
    {
        return Err(conflict(&format!(
            "the continuation receipt {} does not match this request",
            request.series
        )));
    }
    let pane = &request.worker.pane;
    match (mark, receipt.phase) {
        (Some(mark), _) if *mark != receipt.mark => Err(Error::Invalid(format!(
            "{pane} already received this continuation for an earlier done mark; nothing was sent"
        ))),
        (Some(_), Phase::Prepared) => complete(ledger, receipt, text, now),
        (Some(_), Phase::Accepted) => {
            crate::inbox::record_sent(ledger, &receipt.sent, now)?;
            Ok((receipt.sent, true))
        }
        (None, Phase::Accepted) => {
            // The outbox entry was written before the mark was removed. Restore it only if it
            // was lost while its inbox record is still kept, with that record's state. A
            // record that ended and was pruned has no outbox entry to keep. Nothing is put in
            // the inbox again.
            let sent = &receipt.sent;
            if let [outcome] =
                crate::inbox::states(ledger, &sent.terminal, std::slice::from_ref(&sent.id))?
                    .as_slice()
            {
                let mut kept = sent.clone();
                kept.state = outcome.state;
                kept.reason.clone_from(&outcome.reason);
                crate::inbox::record_sent(ledger, &kept, now)?;
            }
            Ok((receipt.sent, false))
        }
        (None, Phase::Prepared) => Err(Error::Invalid(format!(
            "{pane}: the done mark of a prepared continuation is gone; nothing was sent"
        ))),
    }
}

/// The pending inbox record of the continuation `sent`.
fn record(sent: &Sent, text: &str) -> Record {
    Record {
        id: sent.id.clone(),
        terminal: sent.terminal.clone(),
        pane: sent.pane.clone(),
        from: sent.from.clone(),
        kind: sent.kind.clone(),
        text: text.into(),
        created_ms: sent.created_ms,
        state: State::Pending,
        lease: crate::backoff::Backoff::default(),
        reason: None,
        done_ms: None,
    }
}

/// The inbox record ID of a continuation of `series` for the done mark `mark`.
fn record_id(series: &str, mark: &Mark) -> String {
    let mark_id = digest_hex(
        json!([
            mark.item,
            mark.pane,
            mark.session,
            mark.after,
            mark.marked_ms
        ])
        .to_string()
        .as_bytes(),
    );
    format!("{series}{}", &mark_id[..24])
}

/// What a continuation proved: the request, its owner and session, and the worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Continuation {
    request: String,
    series: String,
    owner: Sender,
    /// The admission's origin session, read from the owner's live transcript header.
    session: String,
    worker: Worker,
    kind: String,
    text_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Worker {
    pane: String,
    terminal: Option<String>,
    agent_session: Option<String>,
    agent: Option<String>,
    name: Option<String>,
}

impl Worker {
    fn of(agent: &Agent) -> Self {
        Self {
            pane: agent.pane.clone(),
            terminal: agent.terminal_id.clone(),
            agent_session: agent.agent_session.clone(),
            agent: agent.agent.clone(),
            name: agent.name.clone(),
        }
    }
}

/// The durable intent and acceptance of one continuation: the proof, the exact done mark it
/// continues, the record it sends, and how far it got. Only `phase` ever changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    request: Continuation,
    mark: Mark,
    sent: Sent,
    phase: Phase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    /// The intent is durable. The input may or may not be in the inbox yet; the mark is in
    /// place.
    Prepared,
    /// The input and its outbox entry are durable. The mark may or may not be removed yet.
    Accepted,
}

fn sender(agent: &Agent) -> Sender {
    Sender {
        name: agent.name.clone(),
        pane: Some(agent.pane.clone()),
        terminal: agent.terminal_id.clone(),
        host: crate::inbox::hostname(),
    }
}

/// The session ID in the header of the Pi transcript that Herdr reports for `agent` now.
fn pi_session(agent: &Agent) -> Result<String> {
    if agent.agent.as_deref() != Some("pi") {
        return Err(conflict(&format!("{} is not a Pi agent", agent.pane)));
    }
    let path = agent.transcript.as_deref().ok_or_else(|| {
        conflict(&format!(
            "{}: Herdr reports no session transcript",
            agent.pane
        ))
    })?;
    crate::transcript::pi_session_id(path)
}

/// Prove that this host's ledger holds the original admission of the started launch `launch`,
/// and return the session ID of the owner that committed it.
fn admission(ledger: &Ledger, id: &str, owner: &str, pane: &str, launch: &Value) -> Result<String> {
    let invalid = |detail: &str| conflict(&format!("admission {id}: {detail}"));
    let raw = ledger
        .child("admissions")
        .read::<Value>(&format!("{id}.json"))?
        .ok_or_else(|| invalid("this host has no original admission"))?;
    let attempt: Attempt = serde_json::from_value(raw.clone())?;
    let unit = attempt
        .unit
        .as_ref()
        .ok_or_else(|| invalid("not a unit request"))?;
    let execution = ExecutionRef {
        executor: "dogsled".into(),
        owner_id: owner.into(),
        request_id: id.into(),
    };
    if unit.execution != execution {
        return Err(invalid("the caller is not the execution owner"));
    }
    if unit.abandoned.is_some()
        || unit.returned.is_some()
        || unit.terminal_receipt.is_some()
        || unit.settlement_ack.is_some()
    {
        return Err(invalid("the unit already settled"));
    }
    let committed = unit
        .committed
        .as_ref()
        .ok_or_else(|| invalid("not committed"))?;
    if committed.execution != execution {
        return Err(invalid("the committed execution changed"));
    }
    committed
        .exported
        .validate(&committed.binding, &unit.origin, &unit.execution)?;
    if !attempt
        .host
        .as_ref()
        .is_some_and(crate::hosts::Host::is_local)
    {
        return Err(invalid("the admission did not select this host"));
    }
    let request: Request = serde_json::from_value(attempt.request.clone())?;
    if request.id != id
        || request.owner != owner
        || request.owner_host.is_some()
        || raw["request"].get("owner_host").is_some()
    {
        return Err(invalid("the admitted request does not match"));
    }
    // The saved launch is immutable evidence: it must equal the host's record exactly.
    if raw.get("launch") != Some(launch) || attempt.error.is_some() {
        return Err(invalid("the saved launch does not equal the host launch"));
    }
    let saved: Launch = serde_json::from_value(launch.clone())?;
    if saved.unit != Some(Seed::from(committed.clone())) {
        return Err(invalid(
            "the committed envelope does not equal the launch seed",
        ));
    }
    if attempt
        .result
        .as_ref()
        .is_none_or(|result| result["pane"].as_str() != Some(pane))
    {
        return Err(invalid("the admission did not launch this pane"));
    }
    Ok(unit.origin.session_id.clone())
}

/// The launch's item: open, on this host, and the only open item of its pane.
fn item(ledger: &Ledger, id: &str, pane: &str) -> Result<()> {
    let items = crate::items::list(ledger)?;
    let open: Vec<_> = items
        .iter()
        .filter(|i| i.host == crate::hosts::LOCAL && i.pane == pane && i.closed_ms.is_none())
        .collect();
    match open.as_slice() {
        [item] if item.id == id && items.iter().filter(|i| i.id == id).count() == 1 => Ok(()),
        _ => Err(conflict(&format!(
            "{pane}: the launch item is closed, missing, or competing"
        ))),
    }
}
