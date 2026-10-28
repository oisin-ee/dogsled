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
    /// The custody claims whose custodian acknowledged the saved settlement. The owner's
    /// `settlement_ack` never covers a later custodian.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custody_acks: Vec<String>,
    /// The acceptance a custodian recorded for the returned result, one per custody claim.
    /// It is a review fact only: it never completes or changes the original task.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reviews: Vec<crate::unit_transport::Review>,
    /// The settlement for host proof that the worker ended without a sealed reply. Final:
    /// refresh no longer reads its host, and later returned or refusal evidence is rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<crate::unit_transport::Returned>,
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
            diagnostic: None,
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
                custody_acks: Vec::new(),
                reviews: Vec::new(),
                stopped: None,
            }),
        };
        attempts.write(&format!("{}.json", execution.request_id), &attempt)?;
        Ok(execution)
    })
}

/// Read the record and check that it belongs to `execution`. It authorizes nobody.
fn correlate(attempts: &Ledger, execution: &ExecutionRef) -> Result<Attempt> {
    crate::jobs::validate_id(&execution.request_id)?;
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

/// Commit and abandonment belong to the origin owner alone.
fn load(attempts: &Ledger, execution: &ExecutionRef) -> Result<Attempt> {
    if execution.executor != "dogsled" || execution.owner_id != owner()? {
        return Err(conflict("execution owner does not match caller"));
    }
    correlate(attempts, execution)
}

/// The caller as custody knows it: its pane and its harness session.
fn caller(fleet: &Fleet) -> Result<(String, Option<String>)> {
    let pane = owner()?;
    let session = crate::probe::agent_list(fleet.shell)
        .ok()
        .and_then(|agents| agents.into_iter().find(|agent| agent.pane == pane))
        .and_then(|agent| agent.agent_session);
    Ok((pane, session))
}

/// The claim that makes the caller the custodian of `item`, when the custody moved to it.
/// An unclaimed custody stays with the origin owner and its own delivery path.
fn claim_of(
    custody: Option<&crate::custody::Custody>,
    pane: &str,
    session: Option<&str>,
) -> Option<(String, u64)> {
    let custody = custody?;
    let claim = custody.latest_claim()?;
    // A recorded custodian session must be readable now and equal. A reused pane with
    // another session, or an unreadable session, is not the custodian.
    let session_ok = match custody.custodian_session.as_deref() {
        Some(recorded) => session == Some(recorded),
        None => true,
    };
    (custody.host == crate::inbox::hostname() && custody.custodian_pane == pane && session_ok)
        .then(|| (claim.id.clone(), custody.revision))
}

pub fn commit(fleet: &Fleet, input: &CommitInput) -> Result<Value> {
    commit_using(fleet, input, crate::runtime::start_locked)
}

/// MCP commits durably; the service owns preparation and startup confirmation.
pub fn commit_queued(fleet: &Fleet, input: &CommitInput) -> Result<Value> {
    commit_using(fleet, input, crate::runtime::start_queued)
}

fn commit_using(
    fleet: &Fleet,
    input: &CommitInput,
    start: fn(&Fleet, Request) -> Result<Value>,
) -> Result<Value> {
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
        // The synchronous CLI uses perform_locked; MCP only persists the ticket.
        let result = start(fleet, request);
        crash_point("started");
        let current = load(&attempts, &input.execution)?;
        match result {
            Ok(_) => receipt(fleet.ledger, &current)?
                .ok_or_else(|| conflict("start produced no receipt")),
            Err(error) => Err(error),
        }
    })
}

pub(crate) fn crash_point(point: &str) {
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
        if let Some(error) = &ticket.error {
            receipt["error"] = json!(error);
        }
        if let Some(diagnostic) = &ticket.diagnostic {
            receipt["diagnostic"] = json!(diagnostic);
        }
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
    } else if unit.stopped.is_some() {
        "stopped"
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
    let mut value = json!({"execution":execution,"origin":unit.origin,"phase":phase,"receipt":receipt,"settlement":settlement(unit)?});
    if let Some(error) = &attempt.error {
        value["error"] = json!(error);
    }
    if let Some(diagnostic) = &attempt.diagnostic {
        value["diagnostic"] = json!(diagnostic);
    }
    Ok(value)
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
    } else if let Some(event) = &unit.stopped {
        serde_json::to_value(event)?
    } else {
        serde_json::to_value(&unit.abandoned)?
    })
}

/// The reason a committed unit never started, when its own durable facts prove it: the ticket
/// that carried it ended `failed` or `cancelled`, and no launch request was ever saved. A saved
/// launch may have reached a host, so its outcome stays unknown until host evidence settles it.
fn unlaunched(attempt: &Attempt, ticket: &work::Ticket) -> Option<String> {
    let unit = attempt.unit.as_ref()?;
    if unit.committed.is_none()
        || unit.abandoned.is_some()
        || unit.returned.is_some()
        || unit.stopped.is_some()
        || unit.terminal_receipt.is_some()
        || attempt.launch.is_some()
        || attempt.result.is_some()
        || ticket.request.id != unit.execution.request_id
    {
        return None;
    }
    match ticket.state.as_str() {
        "failed" => Some(format!(
            "launch failed before the worker started: ticket {}: {}",
            ticket.id,
            ticket.error.as_deref().unwrap_or("no error recorded")
        )),
        "cancelled" => Some(format!(
            "launch cancelled before the worker started: ticket {}",
            ticket.id
        )),
        _ => None,
    }
}

/// Settle each committed unit whose ticket ended without a launch as `not_started`, so the
/// owner's provider releases its claim and can retry. Failed and cancelled tickets are final:
/// `enqueue` reuses them, `commit` returns their receipt, and admission never claims them, so
/// no later launch can contradict this event. The admission lock orders it with commit.
pub fn settle_unlaunched(ledger: &Ledger) -> Result<()> {
    let tickets = work::queue(ledger)?;
    if !tickets
        .iter()
        .any(|t| matches!(t.state.as_str(), "failed" | "cancelled"))
    {
        return Ok(());
    }
    let attempts = ledger.child("admissions");
    for ticket in tickets
        .iter()
        .filter(|t| matches!(t.state.as_str(), "failed" | "cancelled"))
    {
        let id = &ticket.request.id;
        let file = format!("{id}.json");
        if crate::jobs::validate_id(id).is_err() {
            continue;
        }
        attempts.with_lock(id, || {
            let Some(mut attempt) = attempts.read::<Attempt>(&file)? else {
                return Ok(());
            };
            let Some(reason) = unlaunched(&attempt, ticket) else {
                return Ok(());
            };
            let Some(unit) = attempt.unit.as_mut() else {
                return Ok(());
            };
            unit.abandoned = Some(NotStarted {
                version: 1,
                event_id: format!("{}-not-started", unit.execution.request_id),
                origin: unit.origin.clone(),
                execution: unit.execution.clone(),
                result: NotStartedResult::NotStarted { reason },
            });
            attempts.write(&file, &attempt)
        })?;
    }
    Ok(())
}

/// Which committed unit admissions a refresh asks hosts about.
#[derive(Clone, Copy)]
pub enum Scope<'a> {
    All,
    Owner(&'a str),
    Execution(&'a ExecutionRef),
}

impl Scope<'_> {
    fn includes(self, execution: &ExecutionRef) -> bool {
        match self {
            Self::All => true,
            Self::Owner(owner) => execution.owner_id == owner,
            Self::Execution(wanted) => execution == wanted,
        }
    }
}

/// Existing inbox/refresh lane imports immutable host evidence. Missing or unreachable
/// hosts prove nothing; this never manufactures stopped from owner-side probes or closed
/// items. Only the execution host proves a stop, and the owner imports that proof.
/// An acknowledged settlement needs no repair: the owner acknowledges only after
/// `settlements` ran a refresh, so such units are skipped in every scope. A saved stop is
/// final, so its unit is skipped too.
/// Every in-scope remote unit on one host is read with one ssh call; units outside
/// the scope or already acknowledged never enter a host's batch.
pub fn refresh(fleet: &Fleet, scope: Scope<'_>) -> Result<()> {
    settle_unlaunched(fleet.ledger)?;
    let attempts = fleet.ledger.child("admissions");
    let mut remote: Vec<(crate::hosts::Host, Vec<Pending>)> = Vec::new();
    for file in records(&attempts)? {
        let Some(attempt) = attempts.read::<Attempt>(&file)? else {
            continue;
        };
        let Some(unit) = attempt.unit.as_ref() else {
            continue;
        };
        if unit.committed.is_none()
            || unit.settlement_ack.is_some()
            || unit.stopped.is_some()
            || !scope.includes(&unit.execution)
        {
            continue;
        }
        let Some(host) = attempt.host.as_ref() else {
            continue;
        };
        let pending = Pending {
            file,
            execution: unit.execution.clone(),
            saved: unit.terminal_receipt.clone(),
        };
        if host.is_local() {
            let receipt =
                crate::launch::unit_receipt(fleet.ledger, fleet.shell, &pending.execution);
            // A failed, timed-out or unparsable pane snapshot proves nothing and belongs to
            // this request, as a remote host's `failed` entry does.
            if let Err(error) = &receipt
                && matches!(
                    error.root(),
                    Error::Command { .. } | Error::Deadline(_) | Error::Parse { .. }
                )
            {
                eprintln!(
                    "dogsled unit evidence {}: host could not read evidence: {error}",
                    pending.execution.request_id
                );
                continue;
            }
            settle(fleet.ledger, &pending, receipt)?;
        } else if let Some((_, units)) = remote.iter_mut().find(|(known, _)| known == host) {
            units.push(pending);
        } else {
            remote.push((host.clone(), vec![pending]));
        }
    }
    for (host, units) in remote {
        let entries: Vec<crate::launch::ReceiptEntry> = match fleet.remote(
            &host,
            &[
                "unit-receipts".into(),
                "--input".into(),
                json!({ "executions": units.iter().map(|unit| &unit.execution).collect::<Vec<_>>() }).to_string(),
            ],
        ) {
            Ok(entries) => entries,
            // An unreachable host, or an old peer without `unit-receipts`, proves nothing.
            // Skip only this host; never fall back to a saved receipt or a per-unit read.
            Err(error)
                if matches!(
                    error.root(),
                    Error::Ssh { .. } | Error::Parse { .. } | Error::Deadline(_)
                ) =>
            {
                let ids: Vec<_> = units
                    .iter()
                    .map(|unit| unit.execution.request_id.as_str())
                    .collect();
                eprintln!("dogsled unit evidence {}: {error}", ids.join(" "));
                continue;
            }
            Err(error) => return Err(error),
        };
        for unit in &units {
            let id = &unit.execution.request_id;
            let Some(entry) = entries.iter().find(|entry| entry.request_id == *id) else {
                eprintln!("dogsled unit evidence {id}: ssh {}: no entry", host.label);
                continue;
            };
            let receipt = match (&entry.evidence, &entry.invalid, &entry.failed) {
                (evidence, None, None) => Ok(evidence.clone()),
                (None, Some(reason), None) => Err(Error::Invalid(reason.clone())),
                // Remote error text can carry secrets; log only the category.
                (None, None, Some(_)) => {
                    eprintln!(
                        "dogsled unit evidence {id}: ssh {}: host could not read evidence",
                        host.label
                    );
                    continue;
                }
                _ => {
                    eprintln!(
                        "dogsled unit evidence {id}: ssh {}: malformed entry",
                        host.label
                    );
                    continue;
                }
            };
            settle(fleet.ledger, unit, receipt)?;
        }
    }
    Ok(())
}

/// One committed unit whose host evidence `refresh` reads.
struct Pending {
    file: String,
    execution: ExecutionRef,
    saved: Option<crate::unit_reply::Receipt>,
}

/// Import one unit's host evidence. Rejection belongs to this request, not every
/// admission sharing refresh.
fn settle(
    ledger: &Ledger,
    unit: &Pending,
    receipt: Result<Option<crate::unit_reply::HostEvidence>>,
) -> Result<()> {
    let receipt: crate::unit_reply::HostEvidence = match receipt {
        Ok(Some(receipt)) => receipt,
        Ok(None) => match &unit.saved {
            Some(saved) => crate::unit_reply::HostEvidence::Returned(Box::new(saved.clone())),
            None => return Ok(()),
        },
        // Never use rejected/unknown host evidence to authorize a fallback import.
        Err(error @ Error::Invalid(_)) => {
            eprintln!(
                "dogsled unit evidence {}: {error}",
                unit.execution.request_id
            );
            return Ok(());
        }
        // Local storage, serialization and lock failures remain operation failures.
        Err(error) => return Err(error),
    };
    let imported = match receipt {
        crate::unit_reply::HostEvidence::Returned(receipt) => {
            import_receipt(ledger, &unit.file, *receipt)
        }
        crate::unit_reply::HostEvidence::Refused(receipt) => {
            import_refusal(ledger, &unit.file, *receipt)
        }
        crate::unit_reply::HostEvidence::Stopped(stopped) => {
            import_stopped(ledger, &unit.file, *stopped)
        }
    };
    match imported {
        Ok(()) => Ok(()),
        Err(error @ Error::Invalid(_)) => {
            eprintln!(
                "dogsled unit evidence {}: {error}",
                unit.execution.request_id
            );
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn import_refusal(ledger: &Ledger, file: &str, receipt: crate::unit_reply::Refusal) -> Result<()> {
    let attempts = ledger.child("admissions");
    let id = &receipt.launch.id;
    crate::jobs::validate_id(id)?;
    if file != format!("{id}.json") {
        return Err(conflict("refusal belongs to another admission"));
    }
    attempts.with_lock(id, || {
        let mut current = attempts
            .read::<Attempt>(file)?
            .ok_or_else(|| conflict("admission disappeared"))?;
        if current.launch.as_ref() != Some(&receipt.launch)
            || current
                .host
                .as_ref()
                .is_none_or(|host| host.id != receipt.launch.host)
            || current.result.is_some()
            || receipt.failed.is_empty()
        {
            return Err(conflict("host refusal does not match selected launch"));
        }
        let unit = current
            .unit
            .as_mut()
            .ok_or_else(|| conflict("unit admission disappeared"))?;
        let committed = unit
            .committed
            .as_ref()
            .ok_or_else(|| conflict("unit is not committed"))?;
        let seed = receipt
            .launch
            .unit
            .as_ref()
            .ok_or_else(|| conflict("refusal seed missing"))?;
        seed.validate(&receipt.launch.id, &receipt.launch.owner)?;
        if seed != &crate::unit_transport::Seed::from(committed.clone())
            || seed.execution != unit.execution
            || seed.binding.contract != unit.origin
            || unit.returned.is_some()
            || unit.stopped.is_some()
            || unit.terminal_receipt.is_some()
        {
            return Err(conflict("contradictory or mismatched refusal evidence"));
        }
        let event = NotStarted {
            version: 1,
            event_id: format!("{}-not-started", unit.execution.request_id),
            origin: unit.origin.clone(),
            execution: unit.execution.clone(),
            result: NotStartedResult::NotStarted {
                reason: receipt.failed,
            },
        };
        if unit.abandoned.as_ref().is_some_and(|saved| saved != &event) {
            return Err(conflict("contradictory refusal evidence"));
        }
        if unit.abandoned.is_none() {
            unit.abandoned = Some(event);
            attempts.write(file, &current)?;
        }
        crash_point("refusal-settlement-persisted");
        Ok(())
    })
}

/// Import the host's proof that a confirmed worker stopped without a sealed reply. The checks
/// match `import_refusal`, except that the launch was confirmed, so the admission may hold a
/// result. Returned or refusal evidence already saved contradicts it.
fn import_stopped(ledger: &Ledger, file: &str, stopped: crate::unit_reply::Stopped) -> Result<()> {
    let attempts = ledger.child("admissions");
    let id = &stopped.launch.id;
    crate::jobs::validate_id(id)?;
    if file != format!("{id}.json") {
        return Err(conflict("stop belongs to another admission"));
    }
    attempts.with_lock(id, || {
        let mut current = attempts
            .read::<Attempt>(file)?
            .ok_or_else(|| conflict("admission disappeared"))?;
        if current.launch.as_ref() != Some(&stopped.launch)
            || current
                .host
                .as_ref()
                .is_none_or(|host| host.id != stopped.launch.host)
            || stopped.reason.is_empty()
        {
            return Err(conflict("host stop does not match selected launch"));
        }
        let unit = current
            .unit
            .as_mut()
            .ok_or_else(|| conflict("unit admission disappeared"))?;
        let committed = unit
            .committed
            .as_ref()
            .ok_or_else(|| conflict("unit is not committed"))?;
        let seed = stopped
            .launch
            .unit
            .as_ref()
            .ok_or_else(|| conflict("stop seed missing"))?;
        seed.validate(&stopped.launch.id, &stopped.launch.owner)?;
        if seed != &crate::unit_transport::Seed::from(committed.clone())
            || seed.execution != unit.execution
            || seed.binding.contract != unit.origin
            || unit.abandoned.is_some()
            || unit.returned.is_some()
            || unit.terminal_receipt.is_some()
        {
            return Err(conflict("contradictory or mismatched stop evidence"));
        }
        let event = crate::unit_transport::Returned {
            version: 1,
            event_id: format!("{}-stopped", unit.execution.request_id),
            binding: committed.binding.clone(),
            result: crate::unit_transport::ReturnedResult::Stopped {
                reason: stopped.reason,
            },
        };
        if unit.stopped.as_ref().is_some_and(|saved| saved != &event) {
            return Err(conflict("contradictory stop evidence"));
        }
        if unit.stopped.is_none() {
            unit.stopped = Some(event);
            attempts.write(file, &current)?;
        }
        Ok(())
    })
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
        if unit.abandoned.is_some()
            || unit.stopped.is_some()
            || receipt.ack.execution != unit.execution
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
    let owner = owner()?;
    let (pane, session) = caller(fleet)?;
    refresh(fleet, Scope::Owner(&owner))?;
    let attempts = fleet.ledger.child("admissions");
    let mut pending = Vec::new();
    let mut inherited = Vec::new();
    for file in records(&attempts)? {
        if let Some(attempt) = attempts.read::<Attempt>(&file)?
            && let Some(unit) = attempt.unit
        {
            let custody = crate::custody::get(fleet.ledger, &unit.execution.request_id)?;
            if custody.as_ref().is_some_and(|c| c.latest_claim().is_some()) {
                if let Some((claim, revision)) =
                    claim_of(custody.as_ref(), &pane, session.as_deref())
                {
                    inherited.push((unit, claim, revision));
                }
            } else if unit.execution.owner_id == owner && unit.settlement_ack.is_none() {
                let event = settlement(&unit)?;
                if !event.is_null() {
                    pending.push(event);
                }
            }
        }
    }
    // The custodian reads what the origin owner never acknowledged and what it did.
    for (unit, _, _) in &inherited {
        refresh(fleet, Scope::Execution(&unit.execution))?;
    }
    for (unit, claim, revision) in inherited {
        if unit.custody_acks.contains(&claim) {
            continue;
        }
        let unit = attempts
            .read::<Attempt>(&format!("{}.json", unit.execution.request_id))?
            .and_then(|attempt| attempt.unit)
            .unwrap_or(unit);
        let event = settlement(&unit)?;
        if !event.is_null() {
            let delivery = crate::unit_transport::Delivery {
                version: 1,
                custody: crate::unit_transport::CustodyRef {
                    item_id: unit.execution.request_id.clone(),
                    claim_id: claim,
                    revision,
                    recipient: pane.clone(),
                },
                settlement: event,
            };
            pending.push(serde_json::to_value(delivery)?);
        }
    }
    Ok(json!(pending))
}

fn ack_settlement(fleet: &Fleet, input: &crate::unit_transport::SettlementAck) -> Result<Value> {
    crate::jobs::validate_id(&input.execution.request_id)?;
    let attempts = fleet.ledger.child("admissions");
    let Some(reference) = &input.custody else {
        // A legacy acknowledgement carries no custody reference, so it may only speak for
        // an item whose custody never moved. The check and the write hold the custody
        // lock, so a claim cannot slip in between them.
        return attempts.with_lock(&input.execution.request_id, || {
            crate::custody::locked(fleet.ledger, &input.execution.request_id, |custody| {
                if custody.is_some_and(|record| !record.is_unclaimed()) {
                    return Err(conflict(
                        "custody was claimed; the acknowledgement must carry the custody reference",
                    ));
                }
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
        });
    };
    let (pane, session) = caller(fleet)?;
    attempts.with_lock(&input.execution.request_id, || {
        let mut attempt = correlate(&attempts, &input.execution)?;
        let event = settlement(
            attempt
                .unit
                .as_ref()
                .ok_or_else(|| conflict("not a unit request"))?,
        )?;
        if event["eventId"].as_str() != Some(input.event_id.as_str()) {
            return Err(conflict(
                "settlement acknowledgement does not match pending event",
            ));
        }
        // The reference is checked against live custody under the custody lock, and the
        // acknowledgement is written inside it, so a claim cannot slip in between.
        crate::custody::locked(fleet.ledger, &input.execution.request_id, |custody| {
            let live = claim_of(custody, &pane, session.as_deref());
            if live
                .as_ref()
                .map(|(claim, revision)| (claim.as_str(), *revision))
                != Some((reference.claim_id.as_str(), reference.revision))
                || reference.item_id != input.execution.request_id
                || reference.recipient != pane
            {
                return Err(conflict("custody reference does not match live custody"));
            }
            let unit = attempt
                .unit
                .as_mut()
                .ok_or_else(|| conflict("not a unit request"))?;
            let applied = !unit.custody_acks.contains(&reference.claim_id);
            if applied {
                unit.custody_acks.push(reference.claim_id.clone());
                attempts.write(&format!("{}.json", input.execution.request_id), &attempt)?;
            }
            Ok(json!({"applied":applied}))
        })
    })
}

/// Record that the current custodian accepted the exact returned result.
///
/// The reviewer is the live custodian, never caller-supplied identity. Stopped and
/// not-started outcomes are not results. A matching replay changes nothing and a
/// conflicting one for the same claim is refused. Task state is never touched.
// The command surface for a fresh claimant arrives with O4.
#[allow(dead_code)]
pub fn record_review(fleet: &Fleet, input: &crate::unit_transport::Review) -> Result<Value> {
    let (pane, session) = caller(fleet)?;
    refresh(fleet, Scope::Execution(&input.execution))?;
    record_review_as(fleet, &pane, session.as_deref(), input)
}

fn record_review_as(
    fleet: &Fleet,
    pane: &str,
    session: Option<&str>,
    input: &crate::unit_transport::Review,
) -> Result<Value> {
    use crate::unit_transport::ReturnedResult;
    crate::jobs::validate_id(&input.execution.request_id)?;
    let attempts = fleet.ledger.child("admissions");
    attempts.with_lock(&input.execution.request_id, || {
        let mut attempt = correlate(&attempts, &input.execution)?;
        let unit = attempt
            .unit
            .as_ref()
            .ok_or_else(|| conflict("not a unit request"))?;
        let accepted = match unit.returned.as_ref().map(|event| &event.result) {
            Some(ReturnedResult::Returned { evidence }) => evidence.result_id == input.result_id,
            _ => false,
        };
        if !accepted {
            return Err(conflict("no saved returned result matches the review"));
        }
        crate::custody::locked(fleet.ledger, &input.execution.request_id, |custody| {
            let live = claim_of(custody, pane, session);
            if live
                .as_ref()
                .map(|(claim, revision)| (claim.as_str(), *revision))
                != Some((input.claim_id.as_str(), input.revision))
                || input.reviewer != pane
            {
                return Err(conflict("review does not match live custody"));
            }
            let unit = attempt
                .unit
                .as_mut()
                .ok_or_else(|| conflict("not a unit request"))?;
            if let Some(saved) = unit.reviews.iter().find(|r| r.claim_id == input.claim_id) {
                if saved == input {
                    return Ok(json!({"applied":false}));
                }
                return Err(conflict("a different review is already recorded"));
            }
            unit.reviews.push(input.clone());
            attempts.write(&format!("{}.json", input.execution.request_id), &attempt)?;
            Ok(json!({"applied":true}))
        })
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
            let mut body = json!({"error":{"code":code,"message":message}});
            if let Some(diagnostic) = error.diagnostic() {
                body["error"]["diagnostic"] = json!(diagnostic);
            }
            crate::print(true, &body, String::new)?;
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
            let input = decode::<crate::unit_transport::ExecutionInput>(&input)?;
            refresh(fleet, Scope::Execution(&input.execution))?;
            observe(fleet.ledger, &input.execution)
        }
        UnitCommand::TerminalAck { input } => Ok(serde_json::to_value(
            crate::launch::terminal_ack(fleet.ledger, fleet.shell, &decode(&input)?)?,
        )?),
        UnitCommand::Settlements => settlements(fleet),
        UnitCommand::AckSettlement { input } => ack_settlement(fleet, &decode(&input)?),
        UnitCommand::AbandonPrepared { input } => Ok(serde_json::to_value(abandon(
            fleet.ledger,
            &decode::<crate::unit_transport::ExecutionInput>(&input)?.execution,
        )?)?),
    }
}

#[cfg(test)]
mod stopped_tests {
    use super::*;

    const ID: &str = "r-1792657889925-39225-0";

    fn export() -> Value {
        let fixtures: Value =
            serde_json::from_str(include_str!("../tests/fixtures/unit-exports.json")).unwrap();
        let mut export: Value =
            serde_json::from_str(fixtures[0]["canonical"].as_str().unwrap()).unwrap();
        export["digest"] = fixtures[0]["digest"].clone();
        export
    }

    /// A committed admission, with a saved `stopped` settlement when `stopped` is set, and
    /// the host receipt that would otherwise import as `returned`.
    fn admission(stopped: bool) -> (Ledger, crate::unit_reply::Receipt) {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let export = export();
        let execution = json!({"executor":"dogsled","ownerId":"w1:p1","requestId":ID});
        let binding = json!({"contract":export["contract"]["ref"],"execution":execution,"exportDigest":export["digest"]});
        let mut unit = json!({
            "origin":export["contract"]["ref"],"execution":execution,"placement":{},
            "tool_call_id":"call-1","abandoned":null,
            "committed":{"execution":execution,"binding":binding,"exported":export},
        });
        if stopped {
            unit["stopped"] = json!({"version":1,"eventId":format!("{ID}-stopped"),"binding":binding,
                "result":{"kind":"stopped","reason":"worker pane closed without a final reply"}});
        }
        let attempt: Attempt =
            serde_json::from_value(json!({"request":{},"launch":null,"error":null,"unit":unit}))
                .unwrap();
        ledger
            .child("admissions")
            .write(&format!("{ID}.json"), &attempt)
            .unwrap();
        let receipt = crate::unit_reply::Receipt {
            ack: crate::unit_transport::TerminalAck {
                execution: serde_json::from_value(execution).unwrap(),
                unit_ref: serde_json::from_value(export["contract"]["ref"].clone()).unwrap(),
                digest: export["digest"].as_str().unwrap().into(),
                seal_entry_id: "seal".into(),
                session_leaf_id: "leaf".into(),
            },
            session_id: ID.into(),
            reply_entry_id: "reply".into(),
            text: "FINAL".into(),
        };
        (ledger, receipt)
    }

    #[test]
    fn returned_receipt_after_stop_is_rejected_and_the_record_is_unchanged() {
        let file = format!("{ID}.json");
        // Control: the same receipt imports when no stop is saved.
        let (ledger, receipt) = admission(false);
        import_receipt(&ledger, &file, receipt).unwrap();
        let attempt: Attempt = ledger.child("admissions").read(&file).unwrap().unwrap();
        assert!(attempt.unit.unwrap().returned.is_some());
        std::fs::remove_dir_all(&ledger.dir).ok();

        let (ledger, receipt) = admission(true);
        let path = ledger.child("admissions").dir.join(&file);
        let before = std::fs::read(&path).unwrap();
        let error = import_receipt(&ledger, &file, receipt).unwrap_err();
        assert!(matches!(error, Error::Invalid(_)), "{error}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let attempt: Attempt = ledger.child("admissions").read(&file).unwrap().unwrap();
        let unit = attempt.unit.unwrap();
        assert!(unit.returned.is_none() && unit.terminal_receipt.is_none());
        assert_eq!(
            settlement(&unit).unwrap()["eventId"],
            format!("{ID}-stopped")
        );
        std::fs::remove_dir_all(&ledger.dir).ok();
    }

    fn fleet<'a>(
        ledger: &'a Ledger,
        shell: &'a crate::shell::Fake,
        policy: &'a crate::config::Policy,
    ) -> Fleet<'a> {
        Fleet {
            ledger,
            shell,
            policy,
            home: "/".into(),
            hosts: vec![crate::hosts::Host::local()],
        }
    }

    fn claimed(ledger: &Ledger, host: &str, session: &str, revision: u64) {
        ledger
            .write(
                "custody.json",
                &json!([{"item":ID,"host":host,"owner_pane":"w1:p1","custodian_pane":"w2:p1",
                    "custodian_session":session,"revision":revision,
                    "claims":[{"id":format!("claim-{revision}"),"claimant_pane":"w2:p1",
                        "claimed_ms":1,"revision":revision}]}]),
            )
            .unwrap();
    }

    fn review(ledger: &Ledger, evidence: &str) -> crate::unit_transport::Review {
        let attempt: Attempt = ledger
            .child("admissions")
            .read(&format!("{ID}.json"))
            .unwrap()
            .unwrap();
        let unit = attempt.unit.unwrap();
        let Some(crate::unit_transport::ReturnedResult::Returned { evidence: saved }) =
            unit.returned.map(|event| event.result)
        else {
            panic!("no returned result")
        };
        crate::unit_transport::Review {
            claim_id: "claim-1".into(),
            revision: 1,
            execution: unit.execution,
            result_id: saved.result_id,
            review_id: "review-1".into(),
            evidence: evidence.into(),
            reviewer: "w2:p1".into(),
        }
    }

    #[test]
    fn a_review_is_idempotent_conflicts_are_refused_and_it_survives_restart() {
        let (ledger, receipt) = admission(false);
        let file = format!("{ID}.json");
        import_receipt(&ledger, &file, receipt).unwrap();
        claimed(&ledger, &crate::inbox::hostname(), "s2", 1);
        let (shell, policy) = (crate::shell::Fake::new(), crate::config::Policy::default());
        let fleet = fleet(&ledger, &shell, &policy);
        let path = ledger.child("admissions").dir.join(&file);
        let before: Attempt = ledger.child("admissions").read(&file).unwrap().unwrap();
        let first = review(&ledger, "accepted");
        let record = |input| record_review_as(&fleet, "w2:p1", Some("s2"), input);
        assert_eq!(record(&first).unwrap()["applied"], true);
        assert_eq!(record(&first).unwrap()["applied"], false);
        let saved = std::fs::read(&path).unwrap();
        assert!(record(&review(&ledger, "different")).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        // A new read of the ledger, as after a restart, holds the receipt and nothing else.
        let after = Ledger::new(ledger.dir.clone());
        let attempt: Attempt = after.child("admissions").read(&file).unwrap().unwrap();
        let unit = attempt.unit.unwrap();
        assert_eq!(unit.reviews, vec![first]);
        let old = before.unit.unwrap();
        assert_eq!(unit.returned, old.returned);
        assert!(unit.settlement_ack.is_none() && unit.custody_acks.is_empty());
        std::fs::remove_dir_all(&ledger.dir).ok();
    }

    #[test]
    fn a_review_is_refused_without_a_returned_result_or_live_custody() {
        let (shell, policy) = (crate::shell::Fake::new(), crate::config::Policy::default());
        // A stopped outcome is not a returned result.
        let (ledger, receipt) = admission(true);
        let fleet_stopped = fleet(&ledger, &shell, &policy);
        claimed(&ledger, &crate::inbox::hostname(), "s2", 1);
        let (_, other) = admission(false);
        std::fs::remove_dir_all(&other.ack.execution.request_id).ok();
        let mut input = crate::unit_transport::Review {
            claim_id: "claim-1".into(),
            revision: 1,
            execution: receipt.ack.execution.clone(),
            result_id: "FINAL".into(),
            review_id: "review-1".into(),
            evidence: "x".into(),
            reviewer: "w2:p1".into(),
        };
        assert!(record_review_as(&fleet_stopped, "w2:p1", Some("s2"), &input).is_err());
        std::fs::remove_dir_all(&ledger.dir).ok();

        let (ledger, receipt) = admission(false);
        import_receipt(&ledger, &format!("{ID}.json"), receipt).unwrap();
        let fleet = fleet(&ledger, &shell, &policy);
        input = review(&ledger, "x");
        let attempt = |pane: &str, session: Option<&str>, input: &crate::unit_transport::Review| {
            record_review_as(&fleet, pane, session, input).is_err()
        };
        // A foreign host, a session mismatch and an unreadable session are refused.
        claimed(&ledger, "elsewhere", "s2", 1);
        assert!(attempt("w2:p1", Some("s2"), &input));
        claimed(&ledger, &crate::inbox::hostname(), "s2", 1);
        assert!(attempt("w2:p1", Some("other"), &input));
        assert!(attempt("w2:p1", None, &input));
        // The previous custodian, a stale revision and a wrong result are refused.
        assert!(attempt("w1:p1", Some("s2"), &input));
        let mut stale = input.clone();
        stale.revision = 0;
        assert!(attempt("w2:p1", Some("s2"), &stale));
        let mut wrong = input.clone();
        wrong.result_id = "other".into();
        assert!(attempt("w2:p1", Some("s2"), &wrong));
        // Control: the live custodian is accepted.
        assert!(!attempt("w2:p1", Some("s2"), &input));
        std::fs::remove_dir_all(&ledger.dir).ok();
    }
}
