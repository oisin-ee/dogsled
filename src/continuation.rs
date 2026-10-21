//! Owner-side continuation of a seeded worker whose launch record predates `owner_host`.
//!
//! Such a record cannot prove which machine its owner pane ID belongs to, so ordinary input
//! refuses every sender. This operation is bounded to the origin host: the caller must be the
//! owner pane on the host whose ledger holds the original admission, the admission must have
//! selected this host, and the caller's live Pi session must be the admission's origin session.
//! Nothing is backfilled, relaunched, sealed, returned, or settled.
//!
//! Lock order: `admissions/<request>`, then `done`, then `launches/<request>`, then
//! `inbox/<terminal>`. No lock is taken twice, and no lock is taken out of this order.

use crate::error::{Error, Result};
use crate::inbox::{Record, Sender, Sent, State};
use crate::ledger::Ledger;
use crate::probe::Agent;
use crate::unit_transport::{ExecutionRef, Seed, conflict, digest_hex};
use crate::work::{Attempt, Launch, Request};
use serde_json::{Value, json};

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
    let stored = ledger.child("admissions").with_lock(&id, || {
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
                let record = Record {
                    id: format!("{series}{}", &mark_id[..24]),
                    terminal: terminal.clone(),
                    pane: target.into(),
                    from: from.clone(),
                    kind: kind.into(),
                    text: text.into(),
                    created_ms: now,
                    state: State::Pending,
                    lease: crate::backoff::Backoff::default(),
                    reason: None,
                    done_ms: None,
                };
                let stored = crate::inbox::put_continuation(ledger, record, &series)?;
                // The input is durable; the done mark is still in place.
                crate::unit_request::crash_point("legacy-continue-stored");
                Ok(stored)
            })
        })
    })?;
    let sent = Sent {
        id: stored.id,
        host: crate::hosts::LOCAL.into(),
        host_label: crate::hosts::LOCAL.into(),
        terminal,
        pane: target.into(),
        name: worker.name.clone(),
        harness: worker.agent.clone(),
        from,
        kind: kind.into(),
        created_ms: stored.created_ms,
        state: stored.state,
        reason: None,
        check: crate::backoff::Backoff::default(),
    };
    crate::inbox::record_sent(ledger, &sent, now)?;
    Ok(sent)
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
