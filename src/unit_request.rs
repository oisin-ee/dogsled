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
                format!("unit-{}-r{}", unit.origin.unit_id, unit.origin.revision),
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
    let phase = if unit.abandoned.is_some() {
        "not_started"
    } else if unit.committed.is_none() {
        "prepared"
    } else if attempt.result.is_some() {
        let item = crate::items::list(ledger)?
            .into_iter()
            .find(|item| item.id == execution.request_id);
        if item.as_ref().is_some_and(|item| item.reply.is_some()) {
            "returned"
        } else if item.as_ref().is_some_and(|item| item.closed_ms.is_some()) {
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
        json!({"execution":execution,"origin":unit.origin,"phase":phase,"receipt":receipt,"settlement":unit.abandoned}),
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
        UnitCommand::Prepare {
            input,
            tool_call_id,
        } => Ok(serde_json::to_value(prepare(
            fleet,
            decode(&input)?,
            tool_call_id,
        )?)?),
        UnitCommand::Commit { input } => commit(fleet, &decode(&input)?),
        UnitCommand::Observe { input } => observe(
            fleet.ledger,
            &decode::<crate::unit_transport::ExecutionInput>(&input)?.execution,
        ),
        UnitCommand::AbandonPrepared { input } => Ok(serde_json::to_value(abandon(
            fleet.ledger,
            &decode::<crate::unit_transport::ExecutionInput>(&input)?.execution,
        )?)?),
    }
}
