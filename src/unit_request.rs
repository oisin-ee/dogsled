//! Unit requests extend admissions, not task state. The admission lock owns commitment,
//! abandonment, and launch transitions. The service repairs the commit/enqueue crash gap.

use crate::error::{Error, Result};
use crate::hosts::Fleet;
use crate::ledger::Ledger;
use crate::unit_transport::{
    CommitInput, ExecutionRef, NotStarted, NotStartedResult, Placement, PrepareInput, UnitRef,
    conflict,
};
use crate::work::{self, Action, Attempt, Request};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Serialize, Deserialize)]
pub struct Prepared {
    pub origin: UnitRef,
    pub execution: ExecutionRef,
    pub placement: Placement,
    /// The originating invocation is retained, never treated as expired by time alone.
    pub tool_call_id: String,
    pub committed: Option<CommitInput>,
    pub abandoned: Option<NotStarted>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returned: Option<crate::unit_transport::Returned>,
    /// Retain full immutable evidence so item repair survives host unavailability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_receipt: Option<crate::unit_reply::Receipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settlement_ack: Option<String>,
}

fn owner() -> Result<String> {
    std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| conflict("HERDR_PANE_ID is required"))
}

fn records(ledger: &Ledger) -> Result<Vec<String>> {
    let entries = match std::fs::read_dir(&ledger.dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().extension().is_some_and(|ext| ext == "json") {
                files.push(name);
            }
        }
    }
    files.sort();
    Ok(files)
}

pub fn prepare(fleet: &Fleet, input: PrepareInput, tool_call_id: String) -> Result<ExecutionRef> {
    input.origin.validate()?;
    if tool_call_id.is_empty() {
        return Err(conflict("tool-call-id must be nonempty"));
    }
    if input.placement.base.is_some() && input.placement.branch.is_none() {
        return Err(conflict("base requires branch"));
    }
    let owner = owner()?;
    let attempts = fleet.ledger.child("admissions");
    let lock = format!(
        "owner-{}",
        crate::unit_transport::digest_hex(owner.as_bytes())
    );
    attempts.with_lock(&lock, || {
        for file in records(&attempts)? {
            if let Some(attempt) = attempts.read::<Attempt>(&file)?
                && let Some(unit) = attempt.unit
                && unit.execution.owner_id == owner
                && unit.origin == input.origin
            {
                if unit.placement != input.placement {
                    return Err(conflict("placement changed for this origin"));
                }
                return Ok(unit.execution);
            }
        }
        let args = crate::cli::WorkArgs {
            host: input.placement.host.clone(),
            branch: input.placement.branch.clone(),
            base: input.placement.base.clone(),
            cwd: None,
            wait: false,
            max: 0,
        };
        // Discovery is read-only. No capacity reservation, checkout, or pane before commit.
        let request = crate::runtime::request(
            fleet,
            &args,
            Action::Agent {
                kind: "pi".into(),
                name: String::new(),
                message: String::new(),
                profile: None,
            },
        )?;
        let execution = ExecutionRef {
            executor: "dogsled".into(),
            owner_id: owner,
            request_id: request.id.clone(),
        };
        let attempt = Attempt {
            request: serde_json::to_value(request)?,
            host: None,
            launch: None,
            error: None,
            result: None,
            unit: Some(Prepared {
                origin: input.origin,
                execution: execution.clone(),
                placement: input.placement,
                tool_call_id,
                committed: None,
                abandoned: None,
                returned: None,
                terminal_receipt: None,
                settlement_ack: None,
            }),
        };
        attempts.write(&format!("{}.json", execution.request_id), &attempt)?;
        Ok(execution)
    })
}

fn load(attempts: &Ledger, execution: &ExecutionRef) -> Result<Attempt> {
    crate::jobs::validate_id(&execution.request_id)?;
    if execution.executor != "dogsled" || execution.owner_id != owner()? {
        return Err(conflict("execution owner does not match caller"));
    }
    let attempt = attempts
        .read::<Attempt>(&format!("{}.json", execution.request_id))?
        .ok_or_else(|| conflict("unknown prepared request"))?;
    let unit = attempt
        .unit
        .as_ref()
        .ok_or_else(|| conflict("not a unit request"))?;
    if unit.execution != *execution {
        return Err(conflict("execution does not match prepared request"));
    }
    Ok(attempt)
}

pub fn commit(fleet: &Fleet, input: &CommitInput) -> Result<Value> {
    crate::jobs::validate_id(&input.execution.request_id)?;
    let attempts = fleet.ledger.child("admissions");
    attempts.with_lock(&input.execution.request_id, || {
        let mut attempt = load(&attempts, &input.execution)?;
        let unit = attempt
            .unit
            .as_mut()
            .ok_or_else(|| conflict("not a unit request"))?;
        if unit.abandoned.is_some() {
            return Err(conflict("prepared request was permanently abandoned"));
        }
        if let Some(committed) = &unit.committed {
            if committed != input {
                return Err(conflict("committed payload changed"));
            }
        } else {
            let profile = input
                .exported
                .validate(&input.binding, &unit.origin, &unit.execution)?;
            let mut request: Request = serde_json::from_value(attempt.request.clone())?;
            request.action = crate::runtime::agent_action(
                fleet,
                "pi".into(),
                crate::unit_transport::agent_name(&unit.origin, &unit.execution)?,
                input.exported.brief.clone(),
                Some(profile.into()),
                request.host.as_deref(),
            )
            .map_err(|error| Error::Invalid(format!("PROFILE_INVALID: {error}")))?;
            attempt.request = serde_json::to_value(request)?;
            unit.committed = Some(input.clone());
            attempts.write(&format!("{}.json", input.execution.request_id), &attempt)?;
            crash_point("committed");
        }
        if let Some(receipt) = receipt(fleet.ledger, &attempt)? {
            return Ok(receipt);
        }
        let request = serde_json::from_value(attempt.request.clone())?;
        // No recursive admission lock: runtime uses perform_locked for this request.
        let result = crate::runtime::start_locked(fleet, request);
        crash_point("started");
        let current = load(&attempts, &input.execution)?;
        match result {
            Ok(_) => receipt(fleet.ledger, &current)?
                .ok_or_else(|| conflict("start produced no receipt")),
            Err(error) => Err(error),
        }
    })
}

fn crash_point(point: &str) {
    // Debug CLI fixtures terminate at real persistence boundaries. Release has no failpoint.
    #[cfg(debug_assertions)]
    if std::env::var("DOGSLED_TEST_UNIT_CRASH").as_deref() == Ok(point) {
        std::process::exit(86);
    }
    let _ = point;
}

fn receipt(ledger: &Ledger, attempt: &Attempt) -> Result<Option<Value>> {
    let unit = attempt
        .unit
        .as_ref()
        .ok_or_else(|| conflict("not a unit request"))?;
    let mut receipt = json!({"execution":unit.execution,"origin":unit.origin});
    if let Some(result) = &attempt.result {
        receipt["disposition"] = json!("launched");
        receipt["host"] = result["host"].clone();
        receipt["pane"] = result["pane"].clone();
        return Ok(Some(receipt));
    }
    if let Some(ticket) = work::queue(ledger)?
        .iter()
        .find(|t| t.request.id == unit.execution.request_id)
    {
        receipt["ticket"] = json!(ticket.id);
        receipt["disposition"] = json!(
            if matches!(ticket.state.as_str(), "pending" | "admitting") {
                "queued"
            } else {
                "unresolved"
            }
        );
        return Ok(Some(receipt));
    }
    Ok(None)
}

pub fn observe(ledger: &Ledger, execution: &ExecutionRef) -> Result<Value> {
    let attempt = load(&ledger.child("admissions"), execution)?;
    let unit = attempt
        .unit
        .as_ref()
        .ok_or_else(|| conflict("not a unit request"))?;
    let receipt = receipt(ledger, &attempt)?;
    let phase = if unit.returned.is_some() {
        "returned"
    } else if unit.abandoned.is_some() {
        "not_started"
    } else if unit.committed.is_none() {
        "prepared"
    } else if attempt.result.is_some() {
        let item = crate::items::list(ledger)?
            .into_iter()
            .find(|item| item.id == execution.request_id);
        // A captured reply is prose, not an execution seal or terminal acknowledgement.
        if item
            .as_ref()
            .is_none_or(|item| item.reply.is_some() || item.closed_ms.is_some())
        {
            "unresolved"
        } else {
            "running"
        }
    } else if receipt
        .as_ref()
        .is_some_and(|r| r["disposition"] == "queued")
    {
        "queued"
    } else if receipt.is_some() {
        "unresolved"
    } else if attempt.host.is_none() {
        "launching"
    } else {
        "unresolved"
    };
    Ok(
        json!({"execution":execution,"origin":unit.origin,"phase":phase,"receipt":receipt,"settlement":settlement(unit)?}),
    )
}

pub fn abandon(ledger: &Ledger, execution: &ExecutionRef) -> Result<NotStarted> {
    crate::jobs::validate_id(&execution.request_id)?;
    let attempts = ledger.child("admissions");
    attempts.with_lock(&execution.request_id, || {
        let mut attempt = load(&attempts, execution)?;
        let unit = attempt
            .unit
            .as_mut()
            .ok_or_else(|| conflict("not a unit request"))?;
        if unit.committed.is_some() {
            return Err(conflict("committed requests cannot be abandoned"));
        }
        let event = unit
            .abandoned
            .get_or_insert_with(|| NotStarted {
                version: 1,
                event_id: format!("{}-not-started", execution.request_id),
                origin: unit.origin.clone(),
                execution: execution.clone(),
                result: NotStartedResult::NotStarted {
                    reason: "prepared request abandoned before commitment".into(),
                },
            })
            .clone();
        attempts.write(&format!("{}.json", execution.request_id), &attempt)?;
        Ok(event)
    })
}

/// Invoked by the existing admission service. Never abandons a prepared request based on
/// elapsed time, and never allocates another request or another ticket for a committed ID.
pub fn recover(ledger: &Ledger, now: u64) -> Result<()> {
    let attempts = ledger.child("admissions");
    for file in records(&attempts)? {
        let Some(id) = file.strip_suffix(".json") else {
            continue;
        };
        crate::jobs::validate_id(id)?;
        attempts.with_lock(id, || {
            let Some(attempt) = attempts.read::<Attempt>(&file)? else {
                return Ok(());
            };
            if attempt
                .unit
                .as_ref()
                .is_some_and(|u| u.committed.is_some() && u.abandoned.is_none())
                && attempt.result.is_none()
            {
                work::enqueue(ledger, serde_json::from_value(attempt.request)?, false, now)?;
            }
            Ok(())
        })?;
    }
    Ok(())
}

fn settlement(unit: &Prepared) -> Result<Value> {
    Ok(if let Some(event) = &unit.returned {
        serde_json::to_value(event)?
    } else {
        serde_json::to_value(&unit.abandoned)?
    })
}

/// Existing inbox/refresh lane imports immutable host evidence. Missing or unreachable
/// hosts prove nothing; this never manufactures stopped from probes or closed items.
pub fn refresh(fleet: &Fleet) -> Result<()> {
    let attempts = fleet.ledger.child("admissions");
    for file in records(&attempts)? {
        let Some(attempt) = attempts.read::<Attempt>(&file)? else {
            continue;
        };
        let Some(unit) = attempt.unit.as_ref() else {
            continue;
        };
        if unit.committed.is_none() || unit.abandoned.is_some() {
            continue;
        }
        let Some(host) = attempt.host.as_ref() else {
            continue;
        };
        let receipt = if host.is_local() {
            crate::launch::unit_receipt(fleet.ledger, &unit.execution)
        } else {
            fleet.remote(
                host,
                &[
                    "unit-receipt".into(),
                    "--input".into(),
                    json!({"execution":unit.execution}).to_string(),
                ],
            )
        };
        let receipt: crate::unit_reply::Receipt = match receipt {
            Ok(Some(receipt)) => receipt,
            Ok(None) => match &unit.terminal_receipt {
                Some(saved) => saved.clone(),
                None => continue,
            },
            Err(error) => {
                eprintln!(
                    "dogsled unit evidence {}: {error}",
                    unit.execution.request_id
                );
                match &unit.terminal_receipt {
                    Some(saved) => saved.clone(),
                    None => continue,
                }
            }
        };
        import_receipt(fleet.ledger, &file, receipt)?;
    }
    Ok(())
}

fn import_receipt(ledger: &Ledger, file: &str, receipt: crate::unit_reply::Receipt) -> Result<()> {
    let attempts = ledger.child("admissions");
    let id = receipt.ack.execution.request_id.clone();
    attempts.with_lock(&id, || {
        let mut current = attempts
            .read::<Attempt>(file)?
            .ok_or_else(|| conflict("admission disappeared"))?;
        let unit = current
            .unit
            .as_mut()
            .ok_or_else(|| conflict("unit admission disappeared"))?;
        let committed = unit
            .committed
            .as_ref()
            .ok_or_else(|| conflict("unit is not committed"))?;
        if receipt.ack.execution != unit.execution
            || receipt.ack.unit_ref != unit.origin
            || receipt.ack.digest != committed.exported.digest
            || receipt.session_id != unit.execution.request_id
        {
            return Err(conflict("host terminal evidence does not match commitment"));
        }
        let event = crate::unit_transport::Returned {
            version: 1,
            event_id: format!("{}-returned", unit.execution.request_id),
            binding: committed.binding.clone(),
            result: crate::unit_transport::ReturnedResult::Returned {
                evidence: crate::unit_transport::Evidence {
                    execution: unit.execution.clone(),
                    result_id: format!("{}-{}", unit.execution.request_id, receipt.reply_entry_id),
                    summary: receipt.text.chars().take(2048).collect(),
                },
            },
        };
        if unit.returned.as_ref().is_some_and(|saved| saved != &event)
            || unit
                .terminal_receipt
                .as_ref()
                .is_some_and(|saved| saved != &receipt)
        {
            return Err(conflict("contradictory terminal evidence"));
        }
        if unit.returned.is_none() || unit.terminal_receipt.is_none() {
            unit.returned = Some(event);
            unit.terminal_receipt = Some(receipt.clone());
            attempts.write(file, &current)?;
        }
        crash_point("settlement-persisted");
        // This projection is replayable even if finish() has not registered the item yet.
        // Identical replay preserves notification acknowledgement under the item lock.
        crate::items::set_reply(
            ledger,
            &receipt.ack.execution.request_id,
            crate::items::Reply {
                seq: 0,
                result_id: Some(format!(
                    "{}-{}",
                    receipt.ack.execution.request_id, receipt.reply_entry_id
                )),
                text: receipt.text,
                captured_ms: crate::ledger::now_ms(),
                reported_ms: None,
                // A terminal receipt is the child's own word that the unit returned.
                done: true,
            },
        )?;
        crash_point("reply-persisted");
        Ok(())
    })
}

pub fn managed_returned(ledger: &Ledger, id: &str) -> Result<Option<bool>> {
    crate::jobs::validate_id(id)?;
    Ok(ledger
        .child("admissions")
        .read::<Attempt>(&format!("{id}.json"))?
        .and_then(|a| a.unit)
        .map(|u| u.returned.is_some()))
}

fn pending_preparations(ledger: &Ledger) -> Result<Value> {
    let owner = owner()?;
    let attempts = ledger.child("admissions");
    let mut pending = Vec::new();
    for file in records(&attempts)? {
        if let Some(attempt) = attempts.read::<Attempt>(&file)?
            && let Some(unit) = attempt.unit
            && unit.execution.owner_id == owner
            && unit.committed.is_none()
            && unit.abandoned.is_none()
        {
            pending.push(json!({"execution": unit.execution, "origin": unit.origin, "toolCallId": unit.tool_call_id}));
        }
    }
    Ok(json!(pending))
}

pub fn settlements(fleet: &Fleet) -> Result<Value> {
    refresh(fleet)?;
    let owner = owner()?;
    let attempts = fleet.ledger.child("admissions");
    let mut pending = Vec::new();
    for file in records(&attempts)? {
        if let Some(attempt) = attempts.read::<Attempt>(&file)?
            && let Some(unit) = attempt.unit
            && unit.execution.owner_id == owner
            && unit.settlement_ack.is_none()
        {
            let event = settlement(&unit)?;
            if !event.is_null() {
                pending.push(event);
            }
        }
    }
    Ok(json!(pending))
}

fn ack_settlement(ledger: &Ledger, input: &crate::unit_transport::SettlementAck) -> Result<Value> {
    crate::jobs::validate_id(&input.execution.request_id)?;
    let attempts = ledger.child("admissions");
    attempts.with_lock(&input.execution.request_id, || {
        let mut attempt = load(&attempts, &input.execution)?;
        let unit = attempt
            .unit
            .as_mut()
            .ok_or_else(|| conflict("not a unit request"))?;
        let event = settlement(unit)?;
        if event["eventId"].as_str() != Some(input.event_id.as_str()) {
            return Err(conflict(
                "settlement acknowledgement does not match pending event",
            ));
        }
        let applied = unit.settlement_ack.is_none();
        unit.settlement_ack = Some(input.event_id.clone());
        attempts.write(&format!("{}.json", input.execution.request_id), &attempt)?;
        Ok(json!({"applied":applied}))
    })
}

fn decode<T: serde::de::DeserializeOwned>(input: &str) -> Result<T> {
    serde_json::from_str(input)
        .map_err(|error| conflict(&format!("invalid unit envelope: {error}")))
}

pub fn run(fleet: &Fleet, command: crate::cli::UnitCommand) -> Result<std::process::ExitCode> {
    match dispatch(fleet, command) {
        Ok(value) => {
            crate::print(true, &value, String::new)?;
            Ok(std::process::ExitCode::SUCCESS)
        }
        Err(error) => {
            let message = error.to_string();
            let code = ["REQUEST_CONFLICT", "PROFILE_INVALID", "LAUNCH_UNRESOLVED"]
                .into_iter()
                .find(|code| message.starts_with(code))
                .unwrap_or("LAUNCH_UNRESOLVED");
            crate::print(
                true,
                &json!({"error":{"code":code,"message":message}}),
                String::new,
            )?;
            Ok(error.exit_code())
        }
    }
}

fn dispatch(fleet: &Fleet, command: crate::cli::UnitCommand) -> Result<Value> {
    use crate::cli::UnitCommand;
    match command {
        UnitCommand::Prepared => pending_preparations(fleet.ledger),
        UnitCommand::Prepare {
            input,
            tool_call_id,
        } => Ok(serde_json::to_value(prepare(
            fleet,
            decode(&input)?,
            tool_call_id,
        )?)?),
        UnitCommand::Commit { input } => commit(fleet, &decode(&input)?),
        UnitCommand::Observe { input } => {
            refresh(fleet)?;
            observe(
                fleet.ledger,
                &decode::<crate::unit_transport::ExecutionInput>(&input)?.execution,
            )
        }
        UnitCommand::TerminalAck { input } => Ok(serde_json::to_value(
            crate::launch::terminal_ack(fleet.ledger, fleet.shell, &decode(&input)?)?,
        )?),
        UnitCommand::Settlements => settlements(fleet),
        UnitCommand::AckSettlement { input } => ack_settlement(fleet.ledger, &decode(&input)?),
        UnitCommand::AbandonPrepared { input } => Ok(serde_json::to_value(abandon(
            fleet.ledger,
            &decode::<crate::unit_transport::ExecutionInput>(&input)?.execution,
        )?)?),
    }
}
