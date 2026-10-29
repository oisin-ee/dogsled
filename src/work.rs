//! Placement-backed actions, pane creation, and caller-owned admission tickets.

mod placement;

fn placement(fleet: &Fleet, request: &Request, host: Option<&str>, now: u64) -> Result<Placement> {
    let validity = request_validity(fleet.ledger, request)?;
    let placed = commands::place_with(fleet, host, &request.id, now, validity, Some(request))
        .map_err(prelaunch_error)?;
    if let Placement::Full { hosts, .. } = &placed {
        defer(fleet, request, hosts, now)?;
    }
    Ok(placed)
}
pub(crate) use placement::{
    PlacementPhase, PlacementProgress, resume as resume_placement,
    validate_completion as validate_placement_completion,
};

use crate::checkout::Repository;
use crate::commands::{self, Placement};
use crate::error::{Error, Result};
use crate::hosts::{Fleet, Host};
use crate::items::{self, Item};
use crate::jobs::{self, Job};
use crate::ledger::{Ledger, new_id};
use crate::shell::{Shell, quote};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Action {
    Place,
    Job {
        cmd: Vec<String>,
        timeout_ms: Option<u64>,
        idle_timeout_ms: Option<u64>,
        /// The request's own creation time, stamped once at its origin.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        validity: Option<crate::jobs::Validity>,
    },
    Agent {
        kind: String,
        name: String,
        message: String,
        /// A Pi profile, resolved on the launch host.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<String>,
    },
}

/// The harnesses that take their first task as a launch argument.
pub const KINDS: [&str; 3] = ["pi", "claude", "codex"];
/// Launch attempts before a ticket fails.
const LAUNCH_TRIES: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    /// Immutable origin validity, outside the action replaced by unit commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validity: Option<jobs::Validity>,
    pub action: Action,
    pub host: Option<String>,
    pub branch: Option<String>,
    pub base: Option<String>,
    pub cwd: String,
    pub repository: Option<Repository>,
    pub workspace: Option<String>,
    pub workspace_name: Option<String>,
    pub owner: String,
    /// The hostname of the machine where `owner` asked for the work. A pane ID is unique
    /// only on its own host. Requests written before this field have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_host: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Launch {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validity: Option<jobs::Validity>,
    /// New launch receipts retain this format marker inside their saved request.
    #[serde(
        default,
        deserialize_with = "crate::prepare::receipt_version",
        skip_serializing_if = "Option::is_none"
    )]
    pub preparation_protocol: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preparation: Option<Box<crate::prepare::disposition::Binding>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<crate::unit_transport::Seed>,
    pub action: Action,
    pub host: String,
    pub cwd: String,
    pub pane: Option<String>,
    /// Terminal identity captured by preparation, before transport to this host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
    pub workspace: Option<String>,
    pub workspace_name: Option<String>,
    pub owner: String,
    /// The originating owner's hostname, copied from its request. Launch records written
    /// before this field have none, and nothing can prove their owner's host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_host: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ticket {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_ms: Option<u64>,
    pub request: Request,
    pub created_ms: u64,
    pub state: String,
    pub waiting: bool,
    pub waiting_until_ms: Option<u64>,
    /// The last host reservation refusals, not a later status snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_reason: Option<String>,
    pub notified: bool,
    pub result: Option<Value>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<crate::diagnostic::Diagnostic>,
    /// Failed launch attempts, and when the next may run.
    #[serde(default)]
    pub retry: crate::backoff::Backoff,
}

pub(crate) fn queue_records(ledger: &Ledger) -> Result<Vec<Value>> {
    crate::store::read(ledger, |tx| {
        crate::store::rows(tx, crate::store::Table::Tickets)
    })
}

fn save_queue(ledger: &Ledger, tickets: &[Ticket]) -> Result<()> {
    crate::store::write(ledger, |tx| {
        crate::store::sync(tx, crate::store::Table::Tickets, tickets)
    })
}

/// Job tickets expire only with a conclusively terminal local job record. Missing
/// execution evidence and non-job tickets are kept. Orphan sidecars expire only
/// without a ticket, job record, or launch receipt.
pub(crate) fn expire_requests(ledger: &Ledger, now: u64) -> Result<()> {
    ledger.with_lock("queue", || {
        let mut tickets = queue(ledger)?;
        let mut expired = Vec::new();
        for ticket in &tickets {
            if matches!(ticket.request.action, Action::Job { .. })
                && matches!(ticket.state.as_str(), "admitted" | "failed" | "cancelled")
                && now.saturating_sub(ticket.created_ms) > jobs::RECORD_KEEP_MS
                && jobs::record(ledger, &ticket.request.id)?
                    .is_some_and(|j| matches!(jobs::expiry(&j, now), jobs::Expiry::Record))
            {
                if cancellation(ledger, &ticket.request)?
                    .is_some_and(|saved| saved.release == "pending")
                {
                    continue;
                }
                expired.push(ticket.id.clone());
                match std::fs::remove_file(
                    ledger
                        .child("cancellations")
                        .dir
                        .join(format!("{}.json", ticket.request.id)),
                ) {
                    Ok(()) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error.into()),
                }
            }
        }
        if !expired.is_empty() {
            tickets.retain(|t| !expired.contains(&t.id));
            save_queue(ledger, &tickets)?;
        }
        let sidecars = ledger.child("job-validity");
        let entries = match std::fs::read_dir(&sidecars.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let nonjob_owner = ledger
                .child("admissions")
                .read::<Value>(&format!("{id}.json"))?
                .is_some_and(|admission| admission["request"]["action"]["type"] != "job");
            if nonjob_owner
                || [
                    "preparation-receipts",
                    "placement-grants",
                    "preparation-expectations",
                ]
                .iter()
                .any(|directory| {
                    ledger
                        .child(directory)
                        .dir
                        .join(format!("{id}.json"))
                        .exists()
                })
                || tickets.iter().any(|t| t.request.id == id)
                || ledger
                    .child("launches")
                    .read::<Value>(&format!("{id}.json"))?
                    .is_some()
            {
                continue;
            }
            sidecars.with_lock(id, || {
                let Some(validity) = jobs::request_validity(ledger, id)? else {
                    return Ok(());
                };
                if now.saturating_sub(validity.created_ms) > jobs::RECORD_KEEP_MS
                    && jobs::record(ledger, id)?
                        .is_none_or(|j| matches!(jobs::expiry(&j, now), jobs::Expiry::Record))
                {
                    match std::fs::remove_file(&path) {
                        Ok(()) => (),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                        Err(e) => return Err(e.into()),
                    }
                }
                Ok(())
            })?;
        }
        Ok(())
    })
}

pub(crate) fn ticket_completion(ledger: &Ledger, ticket: &Ticket) -> Result<Option<u64>> {
    if ticket.finished_ms.is_none() {
        return Ok(None);
    }
    let pending =
        cancellation(ledger, &ticket.request)?.is_some_and(|saved| saved.release == "pending");
    jobs::completion_time(
        ledger,
        ticket.finished_ms,
        Some(ticket.created_ms),
        matches!(ticket.state.as_str(), "admitted" | "failed" | "cancelled") && !pending,
    )
}

pub fn queue(ledger: &Ledger) -> Result<Vec<Ticket>> {
    let mut tickets: Vec<Ticket> = queue_records(ledger)?
        .into_iter()
        .map(serde_json::from_value)
        .collect::<std::result::Result<_, _>>()?;
    for ticket in &mut tickets {
        if ticket
            .waiting_until_ms
            .is_some_and(|end| crate::ledger::now_ms() >= end)
        {
            ticket.waiting = false;
        }
    }
    Ok(tickets)
}

/// Inline evidence is transport only. After ingress, the immutable sidecar is the
/// authority, even when an older queue writer strips or changes the inline copy.
pub(crate) fn request_validity(
    ledger: &Ledger,
    request: &Request,
) -> Result<Option<jobs::Validity>> {
    let saved = jobs::request_validity(ledger, &request.id)?;
    if !matches!(request.action, Action::Job { .. }) {
        jobs::match_request_validity(saved, request.validity)?;
    }
    Ok(saved)
}

pub(crate) fn check_launch_validity(ledger: &Ledger, launch: &Launch) -> Result<()> {
    jobs::check_validity(ledger, launch_validity(ledger, launch)?.as_ref())
        .map_err(|error| validity_refusal(error, &launch.id))
}

pub(crate) fn launch_validity(ledger: &Ledger, launch: &Launch) -> Result<Option<jobs::Validity>> {
    let saved = jobs::request_validity(ledger, &launch.id)?;
    if !matches!(launch.action, Action::Job { .. }) {
        jobs::match_request_validity(saved, launch.validity)?;
    }
    Ok(saved)
}

fn record_request_validity(ledger: &Ledger, request: &Request) -> Result<()> {
    if let Action::Job { validity, .. } = &request.action
        && !queue_records(ledger)?
            .iter()
            .any(|t| t["request"]["id"] == request.id)
    {
        if jobs::request_validity(ledger, &request.id)?.is_none() {
            jobs::check_validity(ledger, validity.as_ref())?;
        }
        jobs::record_request_validity(ledger, &request.id, *validity)?;
    }
    Ok(())
}

/// Check a request against this host's clock. A request that may not start new execution
/// is refused here, before any placement, preparation, dispatch, or ownership change.
pub(crate) fn check_request_validity(ledger: &Ledger, request: &Request) -> Result<()> {
    crate::jobs::check_validity(ledger, request_validity(ledger, request)?.as_ref())
        .map_err(|error| validity_refusal(error, &request.id))
}

fn validity_refusal(error: Error, id: &str) -> Error {
    let error = match error {
        Error::Refused { code, detail } => Error::Refused {
            code,
            detail: format!(
                "{detail}; request {id}: unresolved preparation remains held; inspect preparation and use request-scoped retirement"
            ),
        },
        error => error,
    };
    error.correlate(Some(id))
}

pub fn enqueue(ledger: &Ledger, request: Request, waiting: bool, now: u64) -> Result<String> {
    ledger.with_lock("queue", || {
        jobs::validate_id(&request.id)?;
        let mut tickets = queue(ledger)?;
        if !tickets.iter().any(|t| t.request.id == request.id) {
            // Publish the atomic sidecar before the ticket becomes visible. Existing
            // tickets never recover missing evidence from their inline copy.
            record_request_validity(ledger, &request)?;
        }
        check_request_validity(ledger, &request)?;
        if let Some(ticket) = tickets.iter_mut().find(|t| t.request.id == request.id) {
            let mut queued = ticket.request.clone();
            if let Action::Job { validity, .. } = &mut queued.action {
                *validity = jobs::request_validity(ledger, &queued.id)?;
            }
            if serde_json::to_value(&queued)? != serde_json::to_value(&request)? {
                return Err(crate::unit_transport::conflict("queued request changed"));
            }
            let id = ticket.id.clone();
            // Placement may have queued the refusal before the caller selected a wait.
            if waiting && !ticket.waiting {
                ticket.waiting = true;
                ticket.waiting_until_ms = waiting_until(ledger, now);
                save_queue(ledger, &tickets)?;
            }
            return Ok(id);
        }
        let id = new_id();
        tickets.push(Ticket {
            id: id.clone(),
            finished_ms: None,
            request,
            created_ms: now,
            state: "pending".into(),
            waiting,
            waiting_until_ms: waiting.then(|| waiting_until(ledger, now)).flatten(),
            wait_reason: None,
            notified: false,
            result: None,
            error: None,
            diagnostic: None,
            retry: crate::backoff::Backoff::default(),
        });
        save_queue(ledger, &tickets)?;
        Ok(id)
    })
}

fn waiting_until(ledger: &Ledger, now: u64) -> Option<u64> {
    ledger.budget.deadline.map(|end| {
        now.saturating_add(
            end.saturating_duration_since(std::time::Instant::now())
                .as_millis() as u64,
        )
    })
}

/// Save the actual placement refusals even when the first caller has not queued yet.
fn defer(
    fleet: &Fleet,
    request: &Request,
    hosts: &[crate::hosts::HostReport],
    now: u64,
) -> Result<()> {
    let pinned = request.host.as_deref().map(|h| fleet.find(h)).transpose()?;
    let reason = hosts
        .iter()
        .filter(|h| pinned.is_none_or(|p| p.id == h.host.id))
        .map(|h| {
            let reason = h
                .headroom
                .as_ref()
                .and_then(|r| r.reason.as_deref())
                .or(h.error.as_deref())
                .unwrap_or("reservation refused");
            format!("{}: {reason}", h.host.label)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let id = enqueue(fleet.ledger, request.clone(), false, now)?;
    update_ticket(fleet.ledger, &id, |t| {
        t.wait_reason = (!reason.is_empty()).then_some(reason);
    })?;
    Ok(())
}

/// Queue `request` after its first launch failed, so the service retries it with backoff.
pub fn enqueue_retry(
    ledger: &Ledger,
    request: Request,
    waiting: bool,
    error: &Error,
    now: u64,
) -> Result<String> {
    let id = enqueue(ledger, request, waiting, now)?;
    update_ticket(ledger, &id, |t| {
        t.retry.fail(now, &t.id);
        t.error = Some(format!("attempt 1: {error}"));
        t.diagnostic = error.diagnostic().cloned();
    })?;
    Ok(id)
}

/// The expectation is captured once under admission. Release progress never replaces it.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Cancellation {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completion_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    finished_ms: Option<u64>,
    operation_id: String,
    ticket: Ticket,
    admission: Option<Value>,
    preparation: Option<crate::prepare::PreparationLookup>,
    placement: Option<crate::prepare::LookupRequest>,
    outcome: crate::prepare::disposition::Outcome,
    release: String,
    intent: Option<crate::prepare::disposition::Request>,
    acknowledgement: Option<crate::prepare::disposition::Acknowledgement>,
    error: Option<String>,
}

fn cancellation(ledger: &Ledger, request: &Request) -> Result<Option<Cancellation>> {
    let saved = ledger
        .child("cancellations")
        .read::<Cancellation>(&format!("{}.json", request.id))?;
    if let Some(saved) = &saved {
        jobs::validate_id(&saved.operation_id)?;
        if saved.version != 1
            || serde_json::to_value(&saved.ticket.request)? != serde_json::to_value(request)?
        {
            return Err(crate::unit_transport::conflict(
                "cancellation expectation changed or unsupported",
            ));
        }
    }
    Ok(saved)
}

/// Every caller owns admission before checking this durable prohibition.
pub(crate) fn admission_prohibited(ledger: &Ledger, request: &Request) -> Result<bool> {
    Ok(crate::preparation_retirement::prohibited(ledger, request)?
        || cancellation(ledger, request)?.is_some())
}

pub(crate) fn allow_admission(ledger: &Ledger, request: &Request) -> Result<()> {
    if admission_prohibited(ledger, request)? {
        return Err(crate::unit_transport::conflict(
            "request cancelled; future admission prohibited",
        ));
    }
    Ok(())
}

fn capture_cancellation(ledger: &Ledger, ticket: &Ticket) -> Result<Cancellation> {
    let admission = ledger
        .child("admissions")
        .read::<Value>(&format!("{}.json", ticket.request.id))?;
    capture_cancellation_from(ledger, ticket, admission)
}

fn capture_cancellation_from(
    ledger: &Ledger,
    ticket: &Ticket,
    admission: Option<Value>,
) -> Result<Cancellation> {
    use crate::prepare::disposition::Outcome;
    let request = &ticket.request;
    if let Some(saved) = cancellation(ledger, request)? {
        if saved.ticket.id != ticket.id {
            return Err(crate::unit_transport::conflict(
                "cancellation ticket changed",
            ));
        }
        return Ok(saved);
    }
    let expected_request = serde_json::to_value(request)?;
    let attempt = admission
        .clone()
        .map(serde_json::from_value::<Attempt>)
        .transpose()?;
    if attempt
        .as_ref()
        .is_some_and(|a| a.request != expected_request)
    {
        return Err(crate::unit_transport::conflict(
            "cancellation admission request changed",
        ));
    }
    let launches = ledger.child("launches");
    let no_launch = attempt
        .as_ref()
        .is_some_and(|a| a.launch.is_none() && a.result.is_none())
        && ticket.result.is_none()
        && launches
            .read::<Value>(&format!("{}.json", request.id))?
            .is_none()
        && launches
            .read::<Value>(&format!("{}.progress.json", request.id))?
            .is_none();
    let outcome = if no_launch {
        Outcome::NotStarted
    } else if attempt.as_ref().is_some_and(|a| a.launch.is_some()) {
        Outcome::PossiblyExecuted
    } else {
        Outcome::Unknown
    };
    let operation_id = new_id();
    let mut preparation =
        ledger
            .child("preparation-expectations")
            .read::<crate::prepare::PreparationLookup>(&format!("{}.json", request.id))?;
    if let Some(query) = &mut preparation {
        if serde_json::to_value(&query.request)? != expected_request
            || attempt.as_ref().and_then(|a| a.host.as_ref()) != Some(&query.host)
        {
            return Err(crate::unit_transport::conflict(
                "cancellation preparation expectation changed",
            ));
        }
        query.binding = attempt
            .as_ref()
            .and_then(|a| a.launch.as_ref())
            .and_then(|l| l.preparation.as_deref().cloned());
    }
    let placement = cancellation_placement(request, attempt.as_ref(), admission.as_ref())?;
    cancellation_intent(
        ledger,
        ticket,
        admission,
        preparation,
        placement,
        outcome,
        operation_id,
    )
}

fn cancellation_placement(
    request: &Request,
    attempt: Option<&Attempt>,
    admission: Option<&Value>,
) -> Result<Option<crate::prepare::LookupRequest>> {
    Ok(if let Some(a) = &attempt {
        if let Some(p) = &a.placement {
            let transport = p.expectation(request)?;
            let envelope = admission
                .as_ref()
                .and_then(|a| {
                    a.pointer("/placement/preparation")
                        .or_else(|| a.pointer("/placement/prepared/preparation"))
                        .or_else(|| a.pointer("/placement/saved/prepared/preparation"))
                        .or_else(|| a.pointer("/placement/failed/preparation"))
                })
                .filter(|e| !e.is_null());
            match (transport, envelope) {
                (Some(placement), Some(envelope)) => Some(crate::prepare::LookupRequest {
                    version: crate::prepare::disposition::VERSION,
                    placement,
                    preparation: serde_json::from_value(envelope.clone())?,
                    binding: a
                        .launch
                        .as_ref()
                        .and_then(|l| l.preparation.as_deref().cloned()),
                }),
                _ => None,
            }
        } else {
            None
        }
    } else {
        None
    })
}

fn cancellation_intent(
    ledger: &Ledger,
    ticket: &Ticket,
    admission: Option<Value>,
    preparation: Option<crate::prepare::PreparationLookup>,
    placement: Option<crate::prepare::LookupRequest>,
    outcome: crate::prepare::disposition::Outcome,
    operation_id: String,
) -> Result<Cancellation> {
    let request = &ticket.request;
    let attempt = admission
        .clone()
        .map(serde_json::from_value::<Attempt>)
        .transpose()?;
    // A placement may already have persisted its C3 release. Preserve that exact intent.
    let existing_intent = admission
        .as_ref()
        .and_then(|a| {
            a.pointer("/placement/saved/disposition")
                .or_else(|| a.pointer("/placement/failed/disposition"))
        })
        .filter(|v| !v.is_null());
    let intent = if let Some(intent) = existing_intent {
        Some(serde_json::from_value(intent.clone())?)
    } else {
        attempt
            .as_ref()
            .and_then(|a| a.launch.as_ref())
            .and_then(|l| l.preparation.as_ref())
            .map(|binding| crate::prepare::disposition::Request {
                version: crate::prepare::disposition::VERSION,
                binding: *binding.clone(),
                policy: crate::prepare::disposition::Policy::Prohibit {
                    basis: crate::prepare::disposition::Basis::Cancelled {
                        operation_id: operation_id.clone(),
                        request_id: request.id.clone(),
                    },
                },
                outcome: outcome.clone(),
            })
    };
    let pending = intent.is_some()
        || request.branch.is_some() && attempt.as_ref().is_some_and(|a| a.host.is_some());
    let mut finished_ms = None;
    if !pending {
        jobs::finish_once(ledger, &mut finished_ms, Some(ticket.created_ms))?;
    }
    let saved = Cancellation {
        version: 1,
        completion_version: Some(1),
        finished_ms,
        operation_id,
        ticket: ticket.clone(),
        admission,
        preparation,
        placement,
        outcome,
        release: if pending { "pending" } else { "not_needed" }.into(),
        intent,
        acknowledgement: None,
        error: pending.then(|| "preparation release pending".into()),
    };
    ledger
        .child("cancellations")
        .write(&format!("{}.json", request.id), &saved)?;
    crate::unit_request::crash_point("cancellation-intent-saved");
    Ok(saved)
}

fn validate_cancellation_admission(request: &Request, admission: Option<&Value>) -> Result<()> {
    let Some(admission) = admission else {
        return Ok(());
    };
    let attempt: Attempt = serde_json::from_value(admission.clone())?;
    if attempt.request != serde_json::to_value(request)? {
        return Err(crate::unit_transport::conflict(
            "cancellation request changed",
        ));
    }
    if let Some(progress) = &attempt.placement {
        if let Some(transport) = progress.expectation(request)?
            && transport.expected_grant.is_some()
            && attempt.host.as_ref() != Some(&transport.envelope.host)
        {
            return Err(crate::unit_transport::conflict(
                "cancellation placement host changed",
            ));
        }
        match &progress.phase {
            placement::PlacementPhase::ResultSaved { saved }
            | placement::PlacementPhase::ReleaseAcknowledged { saved, .. }
            | placement::PlacementPhase::Completed { saved, .. } => {
                placement::validate_saved(request, saved)?;
            }
            _ => (),
        }
    }
    Ok(())
}

fn project_cancellation(ledger: &Ledger, saved: &Cancellation) -> Result<()> {
    if let Some(finished) = saved.finished_ms {
        let attempts = ledger.child("admissions");
        let file = format!("{}.json", saved.ticket.request.id);
        if let Some(mut attempt) = attempts.read::<Attempt>(&file)?
            && attempt.unit.is_none()
            && attempt.finished_ms.is_none()
        {
            attempt.finished_ms = Some(finished);
            attempts.write(&file, &attempt)?;
        }
    }
    ledger.with_lock("queue", || {
        let mut tickets = queue(ledger)?;
        let Some(ticket) = tickets.iter_mut().find(|t| t.id == saved.ticket.id) else {
            return if saved.release == "pending" {
                Err(crate::unit_transport::conflict(
                    "cancelled ticket disappeared; release remains held",
                ))
            } else {
                Ok(())
            };
        };
        if serde_json::to_value(&ticket.request)? != serde_json::to_value(&saved.ticket.request)? {
            return Err(crate::unit_transport::conflict(
                "cancelled ticket request changed",
            ));
        }
        if saved.ticket.state != "failed" {
            ticket.error.clone_from(&saved.error);
        }
        ticket.state = if saved.ticket.state == "failed" {
            "failed"
        } else {
            "cancelled"
        }
        .into();
        if saved.release != "pending" && ticket.finished_ms.is_none() {
            ticket.finished_ms = saved.finished_ms;
        }
        save_queue(ledger, &tickets)
    })
}

pub fn cancel(ledger: &Ledger, id: &str) -> Result<Value> {
    let original = ledger.with_lock("queue", || {
        queue(ledger)?
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| Error::Invalid(format!("unknown ticket {id}")))
    })?;
    launch_test_barrier("CANCEL_BEFORE_ADMISSION")?;
    ledger.child("admissions").with_lock(&original.request.id, || {
        launch_test_barrier("CANCEL_ENTERED_ADMISSION")?;
        let ticket = ledger.with_lock("queue", || queue(ledger)?.into_iter().find(|t| t.id == id)
            .ok_or_else(|| Error::Invalid(format!("unknown ticket {id}"))))?;
        if serde_json::to_value(&original.request)? != serde_json::to_value(&ticket.request)? {
            return Err(crate::unit_transport::conflict("ticket changed while cancelling"));
        }
        if ticket.state == "admitted" {
            return Ok(json!({"status":"too_late","ticket":id,"result":ticket.result}));
        }
        if !matches!(ticket.state.as_str(), "pending" | "admitting" | "cancelled") {
            return Err(Error::Invalid(format!("ticket {id} is {}", ticket.state)));
        }
        let admission = ledger.child("admissions").read::<Value>(&format!("{}.json", ticket.request.id))?;
        if let Err(error) = validate_cancellation_admission(&ticket.request, admission.as_ref()) {
            let mut saved = capture_cancellation_from(ledger, &ticket, admission)?;
            saved.release = "pending".into();
            saved.intent = None;
            saved.error = Some(error.to_string());
            ledger.child("cancellations").write(&format!("{}.json", ticket.request.id), &saved)?;
            project_cancellation(ledger, &saved)?;
            return Err(error);
        }
        if !admission_prohibited(ledger, &ticket.request)?
            && let Some(result) = completed_admission(&ticket.request, admission.as_ref())? {
            // A saved placement result is final to the caller, but admission still owns
            // its unacknowledged release and must keep replaying the original journal.
            let release_pending = admission.as_ref().and_then(|a| a.pointer("/placement/phase")).and_then(Value::as_str) == Some("result_saved");
            update_ticket(ledger, id, |t| { t.state = if release_pending { "pending" } else { "admitted" }.into(); t.result = Some(result.clone()); t.error = release_pending.then(|| "placement release pending".into()); t.diagnostic = None; t.wait_reason = None; })?;
            return Ok(json!({"status":"too_late","ticket":id,"result":result}));
        }
        let saved = capture_cancellation_from(ledger, &ticket, admission)?;
        project_cancellation(ledger, &saved)?;
        Ok(json!({"cancelled":id,"status":"cancelled","operation_id":saved.operation_id,
            "future_admission":"prohibited","outcome":saved.outcome,"release":saved.release,"error":saved.error}))
    })
}

fn completed_admission(request: &Request, admission: Option<&Value>) -> Result<Option<Value>> {
    let Some(admission) = admission else {
        return Ok(None);
    };
    if admission["request"] != serde_json::to_value(request)? {
        return Err(crate::unit_transport::conflict(
            "admission request changed while cancelling",
        ));
    }
    if matches!(request.action, Action::Place) {
        return Ok(admission
            .pointer("/placement/saved/result")
            .filter(|result| !result.is_null())
            .cloned());
    }
    Ok(admission.get("result").filter(|r| !r.is_null()).cloned())
}

/// An admission-locked witness, saved before the existing provider settlement event.
pub(crate) fn no_launch_proof(ledger: &Ledger, ticket: &Ticket) -> Result<bool> {
    if crate::preparation_retirement::prohibited(ledger, &ticket.request)?
        || crate::prepare::disposition::retired(ledger, &ticket.request.id)?
    {
        return Ok(false);
    }
    let saved = if let Some(saved) = cancellation(ledger, &ticket.request)? {
        saved
    } else if ticket.state == "failed" {
        capture_cancellation(ledger, ticket)?
    } else {
        return Ok(false);
    };
    if saved.intent.as_ref().is_some_and(|intent| {
        matches!(
            intent.policy,
            crate::prepare::disposition::Policy::Prohibit {
                basis: crate::prepare::disposition::Basis::Retired { .. }
            }
        )
    }) {
        return Ok(false);
    }
    let current = ledger
        .child("admissions")
        .read::<Value>(&format!("{}.json", ticket.request.id))?;
    let matches = ["request", "host", "launch", "result", "placement"]
        .iter()
        .all(|field| {
            current.as_ref().map(|a| &a[field]) == saved.admission.as_ref().map(|a| &a[field])
        });
    Ok(saved.ticket.id == ticket.id
        && matches
        && saved.outcome == crate::prepare::disposition::Outcome::NotStarted)
}

fn repair_cancellations(fleet: &Fleet) -> Result<()> {
    let directory = fleet.ledger.child("cancellations");
    let entries = match std::fs::read_dir(&directory.dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let result = jobs::validate_id(id).and_then(|()| {
            fleet.ledger.child("admissions").with_lock(id, || {
                let saved = directory
                    .read::<Cancellation>(&format!("{id}.json"))?
                    .ok_or_else(|| crate::unit_transport::conflict("cancellation disappeared"))?;
                let mut saved = cancellation(fleet.ledger, &saved.ticket.request)?
                    .ok_or_else(|| crate::unit_transport::conflict("cancellation disappeared"))?;
                if let Err(error) = project_cancellation(fleet.ledger, &saved) {
                    saved.error = Some(error.to_string());
                    directory.write(&format!("{id}.json"), &saved)?;
                    return Err(error);
                }
                if saved.release != "pending" {
                    return Ok(());
                }
                let result = release_cancelled(fleet, &mut saved);
                if let Err(error) = result {
                    saved.error = Some(error.to_string());
                }
                if saved.release != "pending" && saved.completion_version == Some(1) {
                    jobs::finish_once(
                        fleet.ledger,
                        &mut saved.finished_ms,
                        Some(saved.ticket.created_ms),
                    )?;
                }
                directory.write(&format!("{id}.json"), &saved)?;
                project_cancellation(fleet.ledger, &saved)
            })
        });
        if let Err(error) = result {
            eprintln!("dogsled cancellation {id}: {error}");
        }
    }
    Ok(())
}

fn release_cancelled(fleet: &Fleet, saved: &mut Cancellation) -> Result<()> {
    let request = &saved.ticket.request;
    let attempt = fleet
        .ledger
        .child("admissions")
        .read::<Value>(&format!("{}.json", request.id))?;
    for field in ["request", "host", "launch", "result", "placement"] {
        if attempt.as_ref().map(|a| &a[field]) != saved.admission.as_ref().map(|a| &a[field]) {
            return Err(crate::unit_transport::conflict(
                "cancelled admission evidence changed",
            ));
        }
    }
    let host: Host =
        serde_json::from_value(
            saved
                .admission
                .as_ref()
                .ok_or_else(|| crate::unit_transport::conflict("missing cancellation admission"))?
                ["host"]
                .clone(),
        )?;
    if *fleet.find(&host.id)? != host {
        return Err(crate::unit_transport::conflict(
            "cancelled host target changed",
        ));
    }
    validate_cancellation_admission(request, saved.admission.as_ref())?;
    // Durable release proof survives host loss. Validate it before any lookup can
    // replace the captured intent or require the host to repeat its acknowledgement.
    if saved.acknowledgement.is_some() {
        return acknowledge_cancellation(fleet, &host, saved);
    }
    resolve_cancelled_intent(fleet, saved)?;
    if saved.release != "pending" {
        return Ok(());
    }
    match acknowledge_cancellation(fleet, &host, saved) {
        Ok(()) => Ok(()),
        Err(error) => {
            let previous = saved.intent.clone();
            resolve_cancelled_intent(fleet, saved)?;
            if saved.intent == previous {
                Err(error)
            } else {
                acknowledge_cancellation(fleet, &host, saved)
            }
        }
    }
}

fn resolve_cancelled_intent(fleet: &Fleet, saved: &mut Cancellation) -> Result<()> {
    use crate::prepare::{self, LookupOutcome, LookupState, disposition};
    let request = &saved.ticket.request;
    let observed = if let Some(query) = &saved.placement {
        prepare::lookup(fleet, query)?
    } else if let Some(query) = &saved.preparation {
        prepare::lookup_preparation(fleet, query)?
    } else {
        return Err(crate::unit_transport::conflict(
            "missing historical preparation identity; release requires inspection",
        ));
    };
    let binding = match observed {
        LookupState::ReceiptOnly {
            outcome: LookupOutcome::Prepared { prepared },
        } => prepared.binding.map(|b| *b),
        LookupState::ReceiptOnly {
            outcome: LookupOutcome::Failed { binding, .. },
        } => binding.map(|b| *b),
        LookupState::Absent => {
            saved.release = "not_needed".into();
            saved.error = None;
            return Ok(());
        }
        LookupState::Disposition { receipt, .. } => {
            saved.intent = Some(receipt.request);
            fleet
                .ledger
                .child("cancellations")
                .write(&format!("{}.json", request.id), saved)?;
            return Ok(());
        }
        _ => {
            return Err(crate::unit_transport::conflict(
                "preparation unresolved; release remains pending",
            ));
        }
    };
    let Some(binding) = binding else {
        saved.release = "not_needed".into();
        saved.error = None;
        return Ok(());
    };
    if let Some(intent) = &saved.intent {
        if intent.binding != binding {
            return Err(crate::unit_transport::conflict(
                "cancellation disposition binding changed",
            ));
        }
        return Ok(());
    }
    saved.intent = Some(disposition::Request {
        version: disposition::VERSION,
        binding,
        policy: disposition::Policy::Prohibit {
            basis: disposition::Basis::Cancelled {
                operation_id: saved.operation_id.clone(),
                request_id: request.id.clone(),
            },
        },
        outcome: saved.outcome.clone(),
    });
    fleet
        .ledger
        .child("cancellations")
        .write(&format!("{}.json", request.id), saved)?;
    Ok(())
}

fn acknowledge_cancellation(fleet: &Fleet, host: &Host, saved: &mut Cancellation) -> Result<()> {
    use crate::prepare;
    let request = &saved.ticket.request;
    let intent = saved
        .intent
        .as_ref()
        .ok_or_else(|| crate::unit_transport::conflict("missing cancellation disposition"))?;
    if intent.binding.request_id != request.id
        || intent.binding.claim.request_id != request.id
        || request.branch.as_ref() != Some(&intent.binding.request.1)
        || request.base != intent.binding.request.2
        || saved
            .preparation
            .as_ref()
            .and_then(|q| q.binding.as_ref())
            .is_some_and(|binding| *binding != intent.binding)
        || saved
            .placement
            .as_ref()
            .and_then(|q| q.binding.as_ref())
            .is_some_and(|binding| *binding != intent.binding)
    {
        return Err(crate::unit_transport::conflict(
            "cancellation disposition binding changed",
        ));
    }
    if let Some(ack) = &saved.acknowledgement {
        prepare::validate_ack(intent, ack)?;
    } else {
        saved.acknowledgement = Some(prepare::dispose(fleet, host, intent)?);
        fleet
            .ledger
            .child("cancellations")
            .write(&format!("{}.json", request.id), saved)?;
        crate::unit_request::crash_point("cancellation-acknowledgement-saved");
    }
    saved.release = "acknowledged".into();
    saved.error = None;
    Ok(())
}

pub fn workspace(shell: &dyn Shell, id: &str) -> Result<String> {
    let value = herdr(shell, &["workspace".into(), "list".into()])?;
    value
        .pointer("/result/workspaces")
        .and_then(Value::as_array)
        .and_then(|rows| rows.iter().find(|w| w["workspace_id"] == id))
        .and_then(|w| w["label"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| Error::Invalid(format!("caller workspace {id} has no label")))
}

fn herdr(shell: &dyn Shell, args: &[String]) -> Result<Value> {
    let out = shell.run_ok("herdr", args)?;
    Ok(serde_json::from_str(&out.stdout)?)
}

pub(crate) fn pane(shell: &dyn Shell, launch: &Launch) -> Result<(String, Option<String>)> {
    if !std::path::Path::new(&launch.cwd).is_dir() {
        return Err(Error::Invalid(format!(
            "{}: cwd does not exist: {}",
            launch.host, launch.cwd
        )));
    }
    if let Some(pane) = &launch.pane {
        return Ok((pane.clone(), launch.terminal.clone()));
    }
    // Work in a repository opens where that repository is shown, never in the caller's
    // unrelated workspace.
    let mut args = match crate::worktree::home(shell, &launch.cwd)? {
        crate::worktree::Home::Shown(id) => tab(id),
        crate::worktree::Home::Unshown => vec!["workspace".into(), "create".into()],
        crate::worktree::Home::Outside => callers(shell, launch)?,
    };
    args.extend(["--cwd".into(), launch.cwd.clone(), "--no-focus".into()]);
    let value = herdr(shell, &args)?;
    let pane = value
        .pointer("/result/root_pane/pane_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::Invalid(format!("herdr create returned no root pane: {value}")))?;
    let terminal = value
        .pointer("/result/root_pane/terminal_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned);
    Ok((pane, terminal))
}

/// The pane's live terminal identity, or an error when the pane is not listed with one.
/// A pane that is not listed has no proven identity, and a pane that is listed without a
/// terminal id has none either.
fn live_terminal(shell: &dyn Shell, pane: &str) -> Result<String> {
    crate::cleanup::pane_snapshot(shell)?
        .1
        .get(pane)
        .cloned()
        .ok_or_else(|| Error::Invalid(format!("{pane}: cannot establish launch terminal identity")))
}

/// Retire a pane this launch allocated. Only that pane is closed, and only after the same
/// guards the launch cleanup uses: the caller's own pane, a focused pane, a pane an agent
/// owns, a busy pane, and a pane whose terminal identity changed are never closed here.
///
/// The terminal identity must be proven before the pane closes. The launch recorded one,
/// the pane is listed, and the listed terminal equals the recorded one. A missing recorded
/// identity, an unlisted pane, or a different live terminal is not proof, so the pane
/// stays open.
pub(crate) fn retire_pane(shell: &dyn Shell, pane: &str, terminal: Option<&str>) -> Result<()> {
    if std::env::var("HERDR_PANE_ID").ok().as_deref() == Some(pane)
        || crate::cleanup::current_panes(shell)?
            .iter()
            .any(|p| p.pane_id == pane && p.focused)
    {
        return Err(Error::Invalid(format!("{pane}: caller or focused pane")));
    }
    let Some(expected) = terminal.filter(|id| !id.is_empty()) else {
        return Err(Error::Invalid(format!(
            "{pane}: the launch recorded no terminal identity"
        )));
    };
    if live_terminal(shell, pane)? != expected {
        return Err(Error::Invalid(format!("{pane}: launch terminal changed")));
    }
    if crate::probe::agent_list(shell)?
        .iter()
        .any(|agent| agent.pane == pane)
    {
        return Err(Error::Invalid(format!(
            "{pane}: an agent now owns the pane"
        )));
    }
    if crate::probe::foreground(shell, pane)?.is_some() {
        return Err(Error::Invalid(format!("{pane}: the pane is busy")));
    }
    // Inspection must not authorize closing a replacement terminal.
    if live_terminal(shell, pane)? != expected {
        return Err(Error::Invalid(format!("{pane}: launch terminal changed")));
    }
    shell.run_ok("herdr", &["pane".into(), "close".into(), pane.into()])?;
    Ok(())
}

fn tab(workspace: String) -> Vec<String> {
    vec![
        "tab".into(),
        "create".into(),
        "--workspace".into(),
        workspace,
    ]
}

/// A tab in the caller's workspace, or a new workspace with the caller's workspace name.
fn callers(shell: &dyn Shell, launch: &Launch) -> Result<Vec<String>> {
    let workspace = if let Some(id) = &launch.workspace {
        Some(id.clone())
    } else {
        let name = launch
            .workspace_name
            .as_ref()
            .ok_or_else(|| Error::Invalid("caller workspace name is required".into()))?;
        let value = herdr(shell, &["workspace".into(), "list".into()])?;
        value
            .pointer("/result/workspaces")
            .and_then(Value::as_array)
            .and_then(|rows| rows.iter().find(|w| w["label"] == *name))
            .and_then(|w| w["workspace_id"].as_str())
            .map(str::to_owned)
    };
    Ok(match workspace {
        Some(id) => tab(id),
        None => vec![
            "workspace".into(),
            "create".into(),
            "--label".into(),
            launch
                .workspace_name
                .clone()
                .ok_or_else(|| Error::Invalid("missing workspace name".into()))?,
        ],
    })
}

/// The request facts a job launch receipt keeps. The receipt names the command it
/// dispatched, so the receipt alone proves which job it belongs to. The job record keeps
/// the same facts, and the prune removes that record.
fn request_evidence(
    ledger: &Ledger,
    launch: &Launch,
    cmd: &[String],
    timeout_ms: Option<u64>,
    idle_timeout_ms: Option<u64>,
) -> Result<Value> {
    let mut evidence = json!({"cmd":cmd,"cwd":launch.cwd,"owner":launch.owner,
        "timeout_ms":timeout_ms,"idle_timeout_ms":idle_timeout_ms});
    // The validity evidence joins the receipt only when the request carries it. A legacy
    // receipt keeps its own shape, and a new request never correlates to a receipt that
    // names no request time.
    if let Some(validity) = launch_validity(ledger, launch)? {
        evidence["validity"] = json!(validity);
    }
    if let Some(version) = launch.preparation_protocol {
        evidence["preparation_protocol"] = json!(version);
    }
    Ok(evidence)
}

/// The saved receipt of a job launch must name this request and the command it dispatched.
/// A conflicting same-id request, a receipt for another pane, and a receipt that names a
/// different command or options all refuse: a receipt never replays for another job.
///
/// The job record is a second check, not a prerequisite. The prune removes a terminal job
/// record and keeps its receipt, so the receipt's own evidence carries the replay. A
/// receipt that carries the request evidence but no pane is not complete: it names no
/// dispatch, so a replay without the job record refuses.
pub(crate) fn correlate(
    ledger: &Ledger,
    launch: &Launch,
    cmd: &[String],
    timeout_ms: Option<u64>,
    idle_timeout_ms: Option<u64>,
    saved: &Value,
) -> Result<()> {
    if saved["id"] != json!(launch.id) {
        return Err(Error::Invalid(format!(
            "job {} has a saved launch receipt for another request",
            launch.id
        )));
    }
    if saved["host"] != json!(launch.host) {
        return Err(Error::Invalid(format!(
            "job {} has a saved launch receipt for another host",
            launch.id
        )));
    }
    // The receipt's own evidence is the first check. A receipt that names another command
    // or another request dispatched another job, whether or not its job record remains.
    let evidence = request_evidence(ledger, launch, cmd, timeout_ms, idle_timeout_ms)?;
    if let Some(request) = saved.get("request")
        && *request != evidence
    {
        return Err(Error::Invalid(format!(
            "job {} has a saved launch receipt for another command or request",
            launch.id
        )));
    }
    // The job record is a second check, not a prerequisite. The prune removes a terminal
    // job record and keeps its receipt, so the receipt carries the replay alone.
    let Some(job) = jobs::record(ledger, &launch.id)? else {
        // Without the record, the receipt must be complete on its own: it names the
        // request it dispatched and the pane it dispatched into. A receipt that predates
        // the request evidence proves nothing about what it dispatched.
        if saved.get("request").is_none() {
            return Err(Error::Invalid(format!(
                "job {} has a legacy launch receipt without request evidence; inspect that host before resubmitting",
                launch.id
            )));
        }
        // A complete receipt also names the pane it dispatched into. Without it the receipt
        // proves no dispatch, so a replay refuses.
        if saved
            .get("pane")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(Error::Invalid(format!(
                "job {} has a launch receipt without a pane; inspect that host before resubmitting",
                launch.id
            )));
        }
        return Ok(());
    };
    if job.id != launch.id {
        return Err(Error::Invalid(format!(
            "job {} claim belongs to job {}",
            launch.id, job.id
        )));
    }
    if saved["pane"] != json!(job.pane) {
        return Err(Error::Invalid(format!(
            "job {} has a saved launch receipt for another pane",
            launch.id
        )));
    }
    if job.cmd != cmd
        || job.cwd != launch.cwd
        || job.host != launch.host
        || job.owner != launch.owner
        || job.timeout_ms != timeout_ms
        || job.idle_timeout_ms != idle_timeout_ms
        || job.validity != launch_validity(ledger, launch)?
    {
        return Err(Error::Invalid(format!(
            "job {} has a saved launch receipt for another command or request",
            launch.id
        )));
    }
    Ok(())
}

fn require_unclaimed(ledger: &Ledger, id: &str) -> Result<()> {
    let existing = match jobs::ownership(ledger, id)? {
        jobs::Ownership::Unclaimed => None,
        jobs::Ownership::Uncertain(held) => {
            return Err(Error::Invalid(format!(
                "job {} has an unresolved claim; inspect that host before resubmitting: {}",
                id,
                held.reason.unwrap_or_else(|| "outcome unknown".into())
            )));
        }
        jobs::Ownership::Pending(_)
        | jobs::Ownership::Started(_)
        | jobs::Ownership::Terminal(_) => Some(()),
    };
    if existing.is_some() {
        // A claim is not a dispatch receipt. Without a saved receipt that proves
        // this request dispatched, the claim proves only that something was
        // claimed: this launch refuses, allocates nothing, dispatches nothing,
        // overwrites nothing, saves no receipt, and releases no preparation.
        return Err(Error::Invalid(format!(
            "job {id} has a claim but no saved launch receipt; inspect that host before resubmitting"
        )));
    }
    Ok(())
}

/// The launch of a job on this host. The claim is published before the pane side effect, and
/// the wrapper that runs in that pane reads the claim before it runs anything.
fn launch_job(
    ledger: &Ledger,
    shell: &dyn Shell,
    launch: &Launch,
    cmd: &[String],
    timeout_ms: Option<u64>,
    idle_timeout_ms: Option<u64>,
    checkout: Option<&crate::cleanup::Tree>,
) -> Result<Value> {
    let job = Job {
        id: launch.id.clone(),
        cmd: cmd.to_vec(),
        cwd: launch.cwd.clone(),
        host: launch.host.clone(),
        owner: launch.owner.clone(),
        pane: String::new(),
        state: jobs::CLAIMING.into(),
        exit: None,
        signal: None,
        reason: None,
        started_ms: None,
        ended_ms: None,
        timeout_ms,
        idle_timeout_ms,
        validity: launch_validity(ledger, launch)?,
        startup: Some(crate::prepare::disposition::startup_claim(
            ledger, launch, checkout,
        )?),
        output_expired: false,
    };
    // The job's ownership is read before any pane side effect. A job that already
    // has an owner keeps it: this launch never takes it over, never allocates a
    // second pane, and never dispatches the command again.
    require_unclaimed(ledger, &launch.id)?;
    // The intent is published before the pane side effect. It is not
    // executable: an allocation that stops here stays uncertain, and no other
    // allocation may take the job over.
    jobs::publish_intent(ledger, &job)?;
    let (pane, terminal) = pane(shell, launch)?;
    // Allocation blocks, so the request can expire during it. The recheck runs before the
    // executable claim is published and before the command is dispatched. A refusal writes
    // no receipt, releases no preparation, and retires only the pane this attempt
    // allocated.
    if let Err(refusal) = jobs::check_validity(ledger, job.validity.as_ref()) {
        if launch.pane.is_none()
            && let Err(error) = retire_pane(shell, &pane, terminal.as_deref())
        {
            eprintln!("dogsled pane close: {error}");
        }
        return Err(refusal);
    }
    // The pane is known, so the claim becomes executable. The wrapper reads it
    // before it runs anything.
    let mut claim = job.clone();
    claim.pane.clone_from(&pane);
    claim.state = jobs::PENDING.into();
    jobs::publish_claim(ledger, &claim)?;
    let executable = std::env::current_exe()?.display().to_string();
    // The wrapper carries the protocol it speaks and the request's own creation time, so
    // the wrapper checks the same age the launch checked.
    let command = format!(
        "DOGSLED_STATE={} {} exec --job-validity {} {} -- {}",
        quote(&ledger.dir.display().to_string()),
        quote(&executable),
        crate::jobs::VALIDITY_PROTOCOL_VERSION,
        quote(&launch.id),
        cmd.iter().map(|c| quote(c)).collect::<Vec<_>>().join(" ")
    );
    if let Err(error) = shell.run_ok(
        "herdr",
        &["pane".into(), "run".into(), pane.clone(), command],
    ) {
        // The dispatch failed. The wrapper's own record is the evidence: a
        // wrapper that already started or ended keeps its state, and nothing
        // overwrites it with the pending copy. A startup that was never proved
        // stays uncertain, so no later execution and no safe retry is invented.
        jobs::uncertain(ledger, &claim, error.to_string())?;
        return Err(error);
    }
    // Stamped on the pane's host after the pane exists and the command is dispatched. A
    // snapshot sampled before it may predate the pane, so it cannot prove the pane gone.
    Ok(json!({"id":launch.id,"host":launch.host,"pane":pane,
        "launch_confirmed_ms":crate::ledger::now_ms(),
        "request":request_evidence(ledger, launch, cmd, timeout_ms, idle_timeout_ms)?}))
}

/// Transport ingress records evidence before the execution path reads it. Internal
/// launches do not reconstitute a missing sidecar from a rewritten queue action.
pub fn receive_launch(
    ledger: &Ledger,
    shell: &dyn Shell,
    launch: &Launch,
    now: u64,
    peer_validity: Option<u32>,
) -> Result<Value> {
    if let Action::Job { validity, .. } = &launch.action
        && peer_validity == Some(jobs::VALIDITY_PROTOCOL_VERSION)
        && !queue_records(ledger)?
            .iter()
            .any(|t| t["request"]["id"] == launch.id)
    {
        jobs::validate_id(&launch.id)?;
        ledger
            .child("preparation-receipts")
            .with_lock(&launch.id, || {
                // Ingress can publish validity. Validate the disposition before that first write;
                // a final correlated receipt remains a read-only replay instead.
                if crate::prepare::validate_launch_binding(ledger, launch)?.legacy_final {
                    return Ok(());
                }
                crate::prepare::disposition::replay_guarded(ledger, shell, launch)?;
                if replay_launch(ledger, launch)?.is_some() {
                    return Ok(());
                }
                crate::prepare::disposition::allow_launch(ledger, launch)?;
                match jobs::request_validity(ledger, &launch.id)? {
                    Some(saved) if validity.is_some_and(|incoming| saved != incoming) => {
                        return Err(Error::Invalid(format!(
                            "job {} request validity changed",
                            launch.id
                        )));
                    }
                    None if ledger
                        .child("launches")
                        .read::<Value>(&format!("{}.json", launch.id))?
                        .is_none() =>
                    {
                        jobs::check_validity(ledger, validity.as_ref())?;
                        jobs::record_request_validity(ledger, &launch.id, *validity)?;
                    }
                    _ => (),
                }
                Ok(())
            })?;
    }
    launch_with_ingress(ledger, shell, launch, now, peer_validity, true)
}

pub fn launch(
    ledger: &Ledger,
    shell: &dyn Shell,
    launch: &Launch,
    now: u64,
    peer_validity: Option<u32>,
) -> Result<Value> {
    launch_with_ingress(ledger, shell, launch, now, peer_validity, false)
}

fn launch_with_ingress(
    ledger: &Ledger,
    shell: &dyn Shell,
    launch: &Launch,
    _now: u64,
    peer_validity: Option<u32>,
    ingress: bool,
) -> Result<Value> {
    jobs::validate_id(&launch.id)?;
    if launch.unit.is_some() && !matches!(launch.action, Action::Agent { .. }) {
        return Err(crate::unit_transport::conflict(
            "seed requires an agent launch",
        ));
    }
    // A job launch needs the validity protocol. A peer that speaks another version, or
    // none, never creates a claim or a pane.
    if matches!(launch.action, Action::Job { .. })
        && peer_validity != Some(jobs::VALIDITY_PROTOCOL_VERSION)
    {
        return Err(Error::Refused {
            code: crate::error::Refusal::JobValidityUnsupported,
            detail: crate::error::Refusal::JobValidityUnsupported
                .message()
                .to_owned(),
        });
    }
    // Complete ownership separately; the final launch receipt itself remains read-only.
    let replay = replay_launch(ledger, launch);
    if !matches!(replay, Ok(None)) {
        ledger
            .child("preparation-receipts")
            .with_lock(&launch.id, || {
                if crate::prepare::validate_launch_binding(ledger, launch)?.legacy_final {
                    return Ok(());
                }
                crate::prepare::disposition::replay_guarded(ledger, shell, launch)
            })?;
        return replay?.ok_or_else(|| Error::Invalid("final launch receipt disappeared".into()));
    }
    if !matches!(launch.action, Action::Job { .. })
        && peer_validity != Some(jobs::REQUEST_VALIDITY_PROTOCOL_VERSION)
    {
        return Err(Error::Invalid(
            "unsupported non-job request validity protocol".into(),
        ));
    }
    if matches!(launch.action, Action::Job { .. }) {
        require_unclaimed(ledger, &launch.id)?;
    }
    launch_test_barrier("BEFORE_GATE")?;
    ledger
        .child("preparation-receipts")
        .with_lock(&launch.id, || {
            launch_test_barrier("ENTERED_GATE")?;
            if crate::prepare::validate_launch_binding(ledger, launch)?.legacy_final {
                return replay_launch(ledger, launch)?
                    .ok_or_else(|| crate::worktree::refuse("historical final launch disappeared"));
            }
            // The preparation gate can wait behind a completed dispatch.
            crate::prepare::disposition::replay_guarded(ledger, shell, launch)?;
            if let Some(result) = replay_launch(ledger, launch)? {
                return Ok(result);
            }
            crate::prepare::disposition::allow_launch(ledger, launch)?;
            if ingress && !matches!(launch.action, Action::Job { .. }) {
                jobs::receive_request_validity(ledger, &launch.id, launch.validity)?;
            }
            let result = launch_owned(ledger, shell, launch);
            launch_test_barrier("COMPLETED_GATE")?;
            result
        })
}

/// Debug CLI fixtures stop at real lock boundaries, without sleeps or production controls.
pub(crate) fn launch_test_barrier(phase: &str) -> Result<()> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os(format!("DOGSLED_TEST_LAUNCH_{phase}")) {
        use std::io::BufRead as _;
        let path = std::path::PathBuf::from(path);
        std::fs::write(path.with_extension("ready"), b"ready\n")?;
        let mut go = std::io::BufReader::new(std::fs::File::open(path.with_extension("go"))?);
        go.read_line(&mut String::new())?;
    }
    let _ = phase;
    Ok(())
}

fn launch_owned(ledger: &Ledger, shell: &dyn Shell, launch: &Launch) -> Result<Value> {
    let job = matches!(launch.action, Action::Job { .. });
    if job && !std::path::Path::new(&launch.cwd).is_dir() {
        return Err(crate::worktree::refuse("execution directory disappeared"));
    }
    let (_, saved) = launch_checkout(ledger, launch)?;
    let before = if job {
        crate::worktree::snapshot(shell, &launch.cwd)?
    } else {
        None
    };
    let pin = before
        .as_ref()
        .map(crate::worktree::Linked::pin)
        .transpose()?;
    let repo = saved.as_ref().map(|t| std::path::PathBuf::from(&t.repo));
    let common = crate::worktree::common_dir(shell, std::path::Path::new(&launch.cwd));
    if repo.is_none() && before.is_none() && common.is_err() {
        return launch_guarded(ledger, shell, launch, None);
    }
    let root = repo
        .as_deref()
        .unwrap_or_else(|| std::path::Path::new(&launch.cwd));
    crate::worktree::with_lifecycle(shell, ledger, root, |guard| {
        // Another invocation may have completed or captured this request while we waited.
        // Re-read every owner before fresh discovery can create a generation marker.
        if let Some(result) = replay_launch(ledger, launch)? {
            ledger.with_lock("done", || {
                ledger.child("launches").with_lock(&launch.id, || {
                    crate::prepare::disposition::transfer_guarded(ledger, launch, &result)
                })
            })?;
            return Ok(result);
        }
        if matches!(launch.action, Action::Job { .. }) {
            require_unclaimed(ledger, &launch.id)?;
        }
        crate::prepare::disposition::allow_launch(ledger, launch)?;
        let (recorded, current) = launch_checkout(ledger, launch)?;
        if !recorded {
            check_launch_validity(ledger, launch)?;
        }
        if current != saved {
            return Err(crate::worktree::refuse(
                "launch checkout changed while waiting",
            ));
        }
        if recorded && saved.is_none() {
            return Err(crate::worktree::refuse(
                "recorded launch has no checkout identity",
            ));
        }
        let live = if let Some(saved) = &saved {
            let live = crate::worktree::snapshot(shell, &launch.cwd)?
                .ok_or_else(|| crate::worktree::refuse("prepared checkout disappeared"))?;
            crate::worktree::validate_identity(shell, &live)?;
            if saved.identity != live.identity
                || saved.branch != live.branch
                || !crate::worktree::same(&saved.path, &live.path)
            {
                return Err(crate::worktree::refuse(
                    "prepared checkout identity changed",
                ));
            }
            Some(live)
        } else if let Some(before) = &before {
            let pin = pin
                .as_ref()
                .ok_or_else(|| crate::worktree::refuse("checkout is unpinned"))?;
            pin.validate(before)?;
            let observed = crate::worktree::snapshot(shell, &launch.cwd)?
                .ok_or_else(|| crate::worktree::refuse("checkout disappeared"))?;
            if before.identity.is_some()
                && before.tree().drift(&observed.tree()) != crate::cleanup::Drift::Same
            {
                return Err(crate::worktree::refuse(
                    "checkout identity changed before launch",
                ));
            }
            if before.gitdir != observed.gitdir
                || before.head != observed.head
                || before.branch != observed.branch
            {
                return Err(crate::worktree::refuse("checkout changed before launch"));
            }
            // Both directory inodes are pinned across the wait. Fresh discovery may create
            // the first generation, but must never replace a known pre-lock identity.
            let live = guard.discover(shell, &launch.cwd)?;
            pin.validate(before)?;
            live
        } else if job {
            return Err(crate::worktree::refuse("repository checkout is unreadable"));
        } else {
            guard.discover(shell, &launch.cwd)?
        };
        let checkout = live.as_ref().map(crate::worktree::Linked::tree);
        launch_guarded(ledger, shell, launch, checkout.as_ref())
    })
}

fn replay_launch(ledger: &Ledger, launch: &Launch) -> Result<Option<Value>> {
    if matches!(launch.action, Action::Agent { .. }) {
        return crate::launch::replay(ledger, launch);
    }
    if let Some(value) = ledger
        .child("launches")
        .read::<Value>(&format!("{}.json", launch.id))?
    {
        if let Action::Job {
            cmd,
            timeout_ms,
            idle_timeout_ms,
            ..
        } = &launch.action
        {
            correlate(ledger, launch, cmd, *timeout_ms, *idle_timeout_ms, &value)?;
        }
        return Ok(Some(value));
    }
    Ok(None)
}

fn launch_checkout(
    ledger: &Ledger,
    launch: &Launch,
) -> Result<(bool, Option<crate::cleanup::Tree>)> {
    let preparation = crate::prepare::saved_tree(ledger, &launch.cwd, Some(&launch.id))?;
    let (recorded, captured) = crate::launch::checkout_for_request(ledger, launch)?;
    if preparation
        .as_ref()
        .zip(captured.as_ref())
        .is_some_and(|(prepared, saved)| prepared.identity != saved.identity)
    {
        return Err(crate::worktree::refuse(
            "preparation and launch checkout identities differ",
        ));
    }
    Ok((recorded, preparation.or(captured)))
}

fn launch_guarded(
    ledger: &Ledger,
    shell: &dyn Shell,
    launch: &Launch,
    checkout: Option<&crate::cleanup::Tree>,
) -> Result<Value> {
    if matches!(launch.action, Action::Agent { .. }) {
        let result = crate::launch::agent_guarded(ledger, shell, launch, checkout);
        ledger.with_lock("done", || {
            ledger
                .child("launches")
                .with_lock(&launch.id, || match &result {
                    Ok(value) => {
                        crate::prepare::disposition::transfer_guarded(ledger, launch, value)
                    }
                    Err(_) => crate::prepare::disposition::agent_refused_guarded(ledger, launch),
                })
        })?;
        return result;
    }
    let receipts = ledger.child("launches");
    ledger.with_lock("done", || {
        receipts.with_lock(&launch.id, || {
            let file = format!("{}.json", launch.id);
            if let Some(value) = receipts.read::<Value>(&file)? {
                // A saved receipt is the only evidence that this request dispatched. It
                // replays only when it correlates to this request and to the recorded job.
                if let Action::Job {
                    cmd,
                    timeout_ms,
                    idle_timeout_ms,
                    ..
                } = &launch.action
                {
                    // A correlated receipt replays after expiry. It is read-only: it returns
                    // the saved result and dispatches nothing.
                    correlate(ledger, launch, cmd, *timeout_ms, *idle_timeout_ms, &value)?;
                }
                crate::prepare::disposition::transfer_guarded(ledger, launch, &value)?;
                return Ok(value);
            }
            // New execution checks the request's age before it claims anything, allocates a
            // pane, or dispatches the command.
            if matches!(launch.action, Action::Job { .. }) {
                jobs::check_validity(ledger, launch_validity(ledger, launch)?.as_ref())?;
            }
            let mut result = match &launch.action {
                Action::Job {
                    cmd,
                    timeout_ms,
                    idle_timeout_ms,
                    ..
                } => launch_job(
                    ledger,
                    shell,
                    launch,
                    cmd,
                    *timeout_ms,
                    *idle_timeout_ms,
                    checkout,
                )?,
                Action::Agent { .. } => {
                    return Err(Error::Invalid(
                        "agent launch must use its receipt owner".into(),
                    ));
                }
                Action::Place => return Err(Error::Invalid("place does not launch a pane".into())),
            };
            if let Some(checkout) = checkout {
                result["checkout"] = serde_json::to_value(checkout)?;
            }
            receipts.write(&file, &result)?;
            crate::prepare::disposition::transfer_guarded(ledger, launch, &result)?;
            Ok(result)
        })
    })
}

/// Herdr types `agent start` into the pane as one line. A terminal in canonical mode refuses a
/// line over 1024 bytes (`agent_start_input_failed`). The task and the child contract never go on
/// that line, so its length does not depend on them; what remains is the name, the profile's
/// flags and paths on the launch host. This budget refuses a line those make too long. The
/// margin covers the newline and Herdr's own quoting, which can differ from ours.
pub const LINE_BUDGET: usize = 960;

/// The bytes of the line Herdr types for `kind` with `args`, with each argument quoted for a
/// POSIX shell.
#[must_use]
pub fn line_len(kind: &str, args: &[String]) -> usize {
    kind.len()
        + args
            .iter()
            .map(|arg| 1 + crate::shell::quote(arg).len())
            .sum::<usize>()
}

/// The files on the launch host that hold the text a launch does not type: the task and the
/// child contract. They outlive the pane for review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Files {
    pub task: String,
    pub contract: String,
}

impl Files {
    /// The paths for launch `id` in the `launches` directory `dir`. Nothing is written.
    #[must_use]
    pub fn at(dir: &std::path::Path, id: &str) -> Self {
        let path = |suffix: &str| dir.join(format!("{id}.{suffix}")).display().to_string();
        Self {
            task: path("task.md"),
            contract: path("contract.md"),
        }
    }

    /// Write the task and the child contract for launch `id`, and return their paths.
    pub fn write(dir: &std::path::Path, id: &str, message: &str) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let files = Self::at(dir, id);
        std::fs::write(&files.task, message)?;
        std::fs::write(&files.contract, crate::done::CONTRACT)?;
        Ok(files)
    }
}

/// The harness arguments after `--`: `lead` (the unit seed flags), Pi's session name and profile
/// flags, the child contract (`crate::done::CONTRACT`) as system instructions, then the task.
/// The task is always read from `files.task`: Pi's `@file`, or a pointer for Claude and Codex.
/// Pi and Claude read the contract from `files.contract`. Codex has no file form for
/// `developer_instructions`, so its contract stays inline; that text is fixed. No argument
/// holds task text, so the line is the same for any task. A line over `LINE_BUDGET` is refused
/// here, before Herdr can refuse it in the pane.
pub fn agent_args(
    kind: &str,
    name: &str,
    lead: &[String],
    files: &Files,
    profile: Option<&crate::profiles::Profile>,
) -> Result<Vec<String>> {
    let args = agent_argv(kind, name, lead, files, profile)?;
    let len = line_len(kind, &args);
    if len > LINE_BUDGET {
        return Err(line_budget_error(kind, len));
    }
    Ok(args)
}

/// Encode argv without dispatch. The launch owner records a typed budget refusal before
/// allocation; every dispatch still uses `agent_args` to enforce the limit.
pub(crate) fn agent_argv(
    kind: &str,
    name: &str,
    lead: &[String],
    files: &Files,
    profile: Option<&crate::profiles::Profile>,
) -> Result<Vec<String>> {
    let mut args = lead.to_vec();
    let task = match kind {
        "pi" => {
            args.extend(["--name".to_owned(), name.to_owned()]);
            args.extend(
                profile
                    .map(crate::profiles::Profile::pi_flags)
                    .unwrap_or_default(),
            );
            args.extend(["--append-system-prompt".to_owned(), files.contract.clone()]);
            format!("@{}", files.task)
        }
        "claude" | "codex" if profile.is_none() => {
            if kind == "claude" {
                args.extend([
                    "--append-system-prompt-file".to_owned(),
                    files.contract.clone(),
                ]);
            } else {
                // Codex adds `developer_instructions` to its own; the value is TOML.
                args.extend([
                    "-c".to_owned(),
                    format!(
                        "developer_instructions={}",
                        toml::Value::String(crate::done::CONTRACT.to_owned())
                    ),
                ]);
            }
            format!(
                "Your task is in {}. Read that file and do the task.",
                files.task
            )
        }
        "claude" | "codex" => {
            return Err(Error::Invalid(
                "profiles configure Pi children; omit --profile or use --kind pi".into(),
            ));
        }
        other => {
            return Err(Error::Invalid(format!(
                "kind {other} cannot take its task at launch; use one of {}",
                KINDS.join(", ")
            )));
        }
    };
    args.push(task);
    Ok(args)
}

pub(crate) fn line_budget_error(kind: &str, len: usize) -> Error {
    Error::Invalid(format!(
        "the {kind} launch line is {len} bytes, over the {LINE_BUDGET}-byte budget: Herdr \
         types it as one terminal line, which holds at most 1024 bytes; the task is not on \
         it, so shorten the name, the profile's flags or the state directory's path"
    ))
}

/// A new request runs now only when no ticket that could take one of its hosts waits ahead of
/// it; otherwise the caller enqueues it, and admission serves the queue in order. `None` means
/// queue it.
pub fn submit(fleet: &Fleet, request: &Request, now: u64) -> Result<Option<Value>> {
    record_request_validity(fleet.ledger, request)?;
    launch_test_barrier("PLACEMENT_BEFORE_ADMISSION")?;
    let attempts = fleet.ledger.child("admissions");
    attempts.with_lock(&request.id, || {
        allow_admission(fleet.ledger, request)?;
        if !placement::reserved(&attempts, request)? && waits_behind(fleet, request)? {
            placement::record(&attempts, request)?;
            return Ok(None);
        }
        perform_locked(fleet, request, now)
    })
}

/// The hosts a request may use: its pinned host, or every host.
fn candidates(fleet: &Fleet, request: &Request) -> Vec<String> {
    match request.host.as_deref() {
        Some(host) => vec![
            fleet
                .find(host)
                .map_or_else(|_| host.to_owned(), |h| h.id.clone()),
        ],
        None => fleet.hosts.iter().map(|h| h.id.clone()).collect(),
    }
}

/// Order holds per host. A pending ticket keeps `request` back only when the two could take the
/// same host, so a full peer never holds back work for a host with room.
pub(crate) fn waits_behind(fleet: &Fleet, request: &Request) -> Result<bool> {
    let wanted = candidates(fleet, request);
    Ok(queue(fleet.ledger)?
        .iter()
        .filter(|t| t.state == "pending" && t.request.id != request.id)
        .any(|t| {
            candidates(fleet, &t.request)
                .iter()
                .any(|h| wanted.contains(h))
        }))
}

/// Caller-owned progress is durable before any preparation or launch side effect. An
/// interrupted preparation cannot be adopted: only a saved launch may be resumed.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Attempt {
    pub request: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<PlacementProgress>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<Host>,
    pub launch: Option<Launch>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<crate::diagnostic::Diagnostic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<crate::unit_request::Prepared>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

#[cfg(test)]
pub fn perform(fleet: &Fleet, request: &Request, now: u64) -> Result<Option<Value>> {
    jobs::validate_id(&request.id)?;
    let attempts = fleet.ledger.child("admissions");
    attempts.with_lock(&request.id, || perform_locked(fleet, request, now))
}

/// The caller owns the request's admission lock. Commit uses this path so the lock is
/// never reacquired while `runtime::start` enters admission.
pub(crate) fn admission_completion(ledger: &Ledger, id: &str) -> Result<Option<u64>> {
    jobs::validate_id(id)?;
    let Some(attempt) = ledger
        .child("admissions")
        .read::<Attempt>(&format!("{id}.json"))?
    else {
        return Ok(None);
    };
    if attempt.finished_ms.is_none() {
        return Ok(None);
    }
    let request: Request = serde_json::from_value(attempt.request.clone())?;
    let terminal = if let Some(unit) = &attempt.unit {
        let terminal = usize::from(unit.abandoned.is_some())
            + usize::from(unit.returned.is_some())
            + usize::from(unit.stopped.is_some());
        terminal == 1 && (unit.settlement_ack.is_some() || !unit.custody_acks.is_empty())
    } else if cancellation(ledger, &request)?.is_some() {
        true
    } else if let Some(progress) = &attempt.placement {
        progress.version == crate::node::PLACEMENT_VERSION
            && matches!(
                progress.phase,
                PlacementPhase::Completed { .. } | PlacementPhase::PreparationFailed { .. }
            )
    } else {
        attempt.launch.as_ref().is_some_and(|launch| {
            items::list(ledger).is_ok_and(|items| {
                items.iter().any(|item| {
                    item.id == id
                        && item.host == launch.host
                        && item.closed_ms == attempt.finished_ms
                        && item.resource_closed_ms.is_none()
                })
            })
        }) || cancellation(ledger, &request)?.is_some()
    };
    jobs::completion_time(
        ledger,
        attempt.finished_ms,
        request.validity.map(|v| v.created_ms),
        terminal,
    )
}

fn retained_admission(fleet: &Fleet, request: &Request) -> Result<Option<Value>> {
    if !matches!(request.action, Action::Job { .. }) {
        let attempt = fleet
            .ledger
            .child("admissions")
            .read::<Attempt>(&format!("{}.json", request.id))?;
        if let Some(attempt) = attempt {
            if attempt.request != serde_json::to_value(request)? {
                return Err(crate::unit_transport::conflict("admission request changed"));
            }
            if matches!(request.action, Action::Place) {
                if let Some(result) = placement::retained(fleet, &attempt, request)? {
                    return Ok(Some(result));
                }
            } else if matches!(request.action, Action::Agent { .. })
                && let Some(result) = attempt.result
            {
                return Ok(Some(result));
            }
        }
    }
    Ok(None)
}

fn restored_request(ledger: &Ledger, request: &Request) -> Result<Request> {
    let mut request = request.clone();
    if let Action::Job { validity, .. } = &mut request.action {
        *validity = jobs::request_validity(ledger, &request.id)?;
    }
    Ok(request)
}

pub(crate) fn perform_locked(fleet: &Fleet, request: &Request, now: u64) -> Result<Option<Value>> {
    admission_completion(fleet.ledger, &request.id)?;
    if let Some(result) = retained_admission(fleet, request)? {
        return Ok(Some(result));
    }
    // Admission checks the request's age before it places, prepares, or dispatches
    // anything. A request that may not start new execution is refused here.
    allow_admission(fleet.ledger, request)?;
    if !matches!(request.action, Action::Place) {
        check_request_validity(fleet.ledger, request)?;
    }
    if matches!(request.action, Action::Place) {
        let mut result = placement::perform(fleet, request, now)?;
        if let Some(value) = &mut result
            && value["status"] == "pending_release"
        {
            let ticket = enqueue(fleet.ledger, request.clone(), false, now)?;
            update_ticket(fleet.ledger, &ticket, |t| {
                t.result = Some(value.clone());
            })?;
            value["ticket"] = json!(ticket);
        }
        return Ok(result);
    }
    let restored = restored_request(fleet.ledger, request)?;
    let request = &restored;
    let attempts = fleet.ledger.child("admissions");
    let file = format!("{}.json", request.id);
    if let Some(mut attempt) = attempts.read::<Attempt>(&file)? {
        if attempt.request != serde_json::to_value(request)? {
            return Err(Error::Invalid(format!(
                "request {} changed after placement",
                request.id
            )));
        }
        if let Some(unit) = &attempt.unit {
            if unit.committed.is_none() || unit.abandoned.is_some() {
                return Err(crate::unit_transport::conflict("request is not committed"));
            }
            if let Some(result) = &attempt.result {
                return Ok(Some(result.clone()));
            }
        }
        if attempt.launch.is_some() && !matches!(request.action, Action::Agent { .. }) {
            return Err(Error::Invalid(format!(
                "request {} already attempted a non-reconcilable launch; inspect before resubmitting",
                request.id
            )));
        }
        if let Some(host) = attempt.host.clone() {
            if attempt.launch.is_none() {
                // A returned remote failure is safe to repeat: checkout validates identity,
                // and preparation replays its request-keyed receipt. An interrupted local
                // preparation, or one with no returned error, still requires inspection.
                let retry_queued = queue(fleet.ledger)?.iter().any(|t| {
                    t.request.id == request.id
                        && t.retry.attempts > 0
                        && matches!(t.state.as_str(), "pending" | "admitting")
                });
                if host.is_local() || attempt.error.is_none() || !retry_queued {
                    return Err(Error::Invalid(format!(
                        "request {} stopped during preparation on {}; inspect that host before resubmitting: {}",
                        request.id,
                        host.label,
                        attempt
                            .error
                            .as_deref()
                            .unwrap_or("preparation outcome is unknown")
                    )));
                }
                if matches!(
                    placement(fleet, request, Some(&host.id), now)?,
                    Placement::Full { .. }
                ) {
                    return Ok(None);
                }
                let prepared = prepare_launch(fleet, request, &host);
                let launch = match prepared {
                    Ok(launch) => launch,
                    Err(error) => {
                        attempt.error = Some(error.to_string());
                        attempt.diagnostic = error.diagnostic().cloned();
                        attempts.write(&file, &attempt)?;
                        return Err(prelaunch_error(error));
                    }
                };
                attempt.launch = Some(launch);
                attempt.error = None;
                attempt.diagnostic = None;
                attempts.write(&file, &attempt)?;
            }
            let launch = attempt
                .launch
                .as_ref()
                .ok_or_else(|| Error::Invalid("missing prepared launch".into()))?;
            let result = finish(fleet, request, &host, launch, now)?;
            save_result(&attempts, &file, attempt, &result)?;
            return Ok(Some(result));
        }
        if attempt.unit.is_none() {
            return Err(Error::Invalid("admission has no selected host".into()));
        }
    }
    begin(fleet, request, &attempts, &file, now)
}

fn save_result(attempts: &Ledger, file: &str, mut attempt: Attempt, result: &Value) -> Result<()> {
    attempt.result = Some(result.clone());
    attempts.write(file, &attempt)?;
    crate::unit_request::crash_point("admission-result-saved");
    Ok(())
}

fn begin(
    fleet: &Fleet,
    request: &Request,
    attempts: &Ledger,
    file: &str,
    now: u64,
) -> Result<Option<Value>> {
    let placed = placement(fleet, request, request.host.as_deref(), now)?;
    let Placement::Placed { host, .. } = placed else {
        return Ok(None);
    };
    let mut attempt = Attempt {
        request: serde_json::to_value(request)?,
        finished_ms: None,
        placement: None,
        host: Some(host.clone()),
        launch: None,
        error: None,
        diagnostic: None,
        unit: attempts.read::<Attempt>(file)?.and_then(|a| a.unit),
        result: None,
    };
    attempts.write(file, &attempt)?;
    let prepared = prepare_launch(fleet, request, &host);
    let launch_request = match prepared {
        Ok(launch) => launch,
        Err(error) => {
            attempt.error = Some(error.to_string());
            attempt.diagnostic = error.diagnostic().cloned();
            attempts.write(file, &attempt)?;
            return Err(prelaunch_error(error));
        }
    };
    attempt.launch = Some(launch_request.clone());
    attempts.write(file, &attempt)?;
    let result = finish(fleet, request, &host, &launch_request, now)?;
    save_result(attempts, file, attempt, &result)?;
    Ok(Some(result))
}

/// Only transport errors are retryable before a launch receipt exists. Local setup errors
/// and the peer's structured refusals remain terminal.
fn prelaunch_error(error: Error) -> Error {
    if transient_ssh(&error) {
        Error::Launch(error.to_string())
    } else {
        error
    }
}

fn prepare_launch(fleet: &Fleet, request: &Request, host: &Host) -> Result<Launch> {
    let checkout = request
        .repository
        .as_ref()
        .map(|repo| {
            crate::checkout::ensure(fleet, host, repo, request)
                .map_err(|error| error.correlate(Some(&request.id)).on_host(&host.label))
        })
        .transpose()?;
    let prepared = if let Some(branch) = &request.branch {
        let repo = request
            .repository
            .as_ref()
            .ok_or_else(|| Error::Invalid("branch requires repository".into()))?;
        let query = crate::prepare::PreparationLookup {
            version: crate::prepare::disposition::VERSION,
            request: request.clone(),
            host: host.clone(),
            preparation: crate::prepare::envelope(
                fleet,
                host,
                &repo.path,
                branch,
                request.base.as_deref(),
                &request.id,
            )?,
            binding: None,
        };
        let expectations = fleet.ledger.child("preparation-expectations");
        let file = format!("{}.json", request.id);
        if let Some(saved) = expectations.read::<crate::prepare::PreparationLookup>(&file)? {
            if serde_json::to_value(&saved)? != serde_json::to_value(&query)? {
                return Err(crate::unit_transport::conflict(
                    "preparation expectation changed",
                ));
            }
        } else {
            expectations.write(&file, &query)?;
        }
        crate::prepare::run(
            fleet,
            host,
            &repo.path,
            branch,
            request.base.as_deref(),
            &request.id,
        )
        .map_err(|error| error.correlate(Some(&request.id)).on_host(&host.label))?
    } else {
        crate::prepare::Prepared::default()
    };
    prepared_launch(fleet, request, host, checkout, prepared)
}

fn prepared_launch(
    fleet: &Fleet,
    request: &Request,
    host: &Host,
    checkout: Option<String>,
    prepared: crate::prepare::Prepared,
) -> Result<Launch> {
    let cwd = if request.branch.is_some() {
        prepared
            .path
            .clone()
            .ok_or_else(|| Error::Invalid("the prepared worktree has no path".into()))?
    } else if host.is_local() {
        request.cwd.clone()
    } else if let (Some(checkout), Some(repo)) = (checkout, &request.repository) {
        let relative = std::path::Path::new(&request.cwd)
            .strip_prefix(&repo.working_root)
            .map_err(|_| Error::Invalid("repository cwd is outside its working root".into()))?;
        std::path::Path::new(&checkout)
            .join(relative)
            .display()
            .to_string()
    } else {
        request.cwd.clone()
    };
    let unit = fleet
        .ledger
        .child("admissions")
        .read::<Attempt>(&format!("{}.json", request.id))?
        .and_then(|a| a.unit)
        .and_then(|u| u.committed)
        .map(crate::unit_transport::seed_from_commit);
    crate::unit_request::crash_point("preparation-returned-before-launch");
    let mut action = request.action.clone();
    if let Action::Job { validity, .. } = &mut action {
        *validity = jobs::request_validity(fleet.ledger, &request.id)?;
    }
    Ok(Launch {
        id: request.id.clone(),
        validity: request.validity,
        preparation_protocol: prepared
            .binding
            .as_ref()
            .map(|_| crate::prepare::disposition::VERSION),
        preparation: prepared.binding,
        unit,
        action,
        host: host.id.clone(),
        cwd,
        pane: if matches!(request.action, Action::Agent { .. } | Action::Place) {
            prepared.pane_id
        } else {
            None
        },
        terminal: if matches!(request.action, Action::Agent { .. }) {
            prepared.terminal_id
        } else {
            None
        },
        workspace: host.is_local().then(|| request.workspace.clone()).flatten(),
        workspace_name: request.workspace_name.clone(),
        owner: request.owner.clone(),
        owner_host: request.owner_host.clone(),
    })
}

fn finish(
    fleet: &Fleet,
    request: &Request,
    host: &Host,
    launch_request: &Launch,
    now: u64,
) -> Result<Value> {
    let launched = if host.is_local() {
        launch(
            fleet.ledger,
            fleet.shell,
            launch_request,
            now,
            Some(if matches!(request.action, Action::Job { .. }) {
                jobs::VALIDITY_PROTOCOL_VERSION
            } else {
                jobs::REQUEST_VALIDITY_PROTOCOL_VERSION
            }),
        )
    } else {
        let mut args = vec![
            "launch".into(),
            "--preparation-protocol".into(),
            crate::prepare::disposition::VERSION.to_string(),
            "--request".into(),
            serde_json::to_string(launch_request)?,
        ];
        // The validity protocol rides with a job launch. A peer that does not speak it
        // rejects the flag, and the caller reports an unsupported protocol.
        if matches!(request.action, Action::Job { .. }) {
            args.extend([
                "--job-validity".into(),
                crate::jobs::VALIDITY_PROTOCOL_VERSION.to_string(),
            ]);
        } else {
            args.extend([
                "--request-validity".into(),
                jobs::REQUEST_VALIDITY_PROTOCOL_VERSION.to_string(),
            ]);
        }
        fleet.remote_setup(host, &args)
    };
    let response = launched.map_err(|e| {
        if retryable(&e) && matches!(request.action, Action::Agent { .. }) {
            Error::Launch(e.to_string())
        } else {
            e
        }
    })?;
    let result = if launch_request.preparation.is_some() {
        let response = if host.is_local() {
            crate::prepare::launch_response(fleet.ledger, launch_request, response)?
        } else {
            response
        };
        crate::prepare::accept_launch(fleet, host, launch_request, response)?
    } else {
        response
    };
    let pane = result["pane"]
        .as_str()
        .ok_or_else(|| Error::Invalid("launch returned no pane".into()))?;
    // Queued and still-launching requests have no item: only a successful receipt
    // reaches this point. Preserve its host-clock confirmation, not admission's `now`.
    // Only a child can be orphaned, so only a child records an owner session.
    let owner_session = match request.action {
        Action::Agent { .. } => owner_session(fleet, request),
        _ => None,
    };
    let item = item(request, host.id.clone(), pane, &result, now, owner_session)?;
    record_child(fleet.ledger, &item, result)
}

/// Record the launched child and its custody, and return the launch result.
///
/// The custody starts at the origin: the owner holds the child until a lead claims it.
/// A custody record that cannot be read or written is a diagnostic only. The child is
/// running, and a claim derives the origin record from the item when the custody file
/// holds none, so a custody failure never turns confirmed execution into a failed
/// launch.
pub(crate) fn record_child(ledger: &Ledger, item: &Item, result: Value) -> Result<Value> {
    items::record(ledger, item.clone())?;
    if item.kind == "agent"
        && let Err(error) = crate::custody::ensure(ledger, item)
    {
        eprintln!(
            "dogsled: custody record for {} failed: {error}; a claim derives it from the item",
            item.id
        );
    }
    Ok(result)
}

/// The owner's harness session identity, when this host can prove it.
///
/// The session-ended rule needs a recorded identity. A host that cannot prove the owner
/// is on this host records none, so the rule never applies to that item.
fn owner_session(fleet: &Fleet, request: &Request) -> Option<String> {
    let hostname = crate::inbox::hostname();
    if request.owner_host.as_deref() != Some(hostname.as_str()) {
        return None;
    }
    crate::probe::agent_list(fleet.shell)
        .ok()?
        .into_iter()
        .find(|agent| agent.pane == request.owner)
        .and_then(|agent| agent.agent_session)
}

/// The owner's record of what `request` started. The profile's role wins; otherwise a child
/// with a branch is a writer.
fn item(
    request: &Request,
    host: String,
    pane: &str,
    result: &Value,
    now: u64,
    owner_session: Option<String>,
) -> Result<Item> {
    let text = |key: &str| result[key].as_str().map(str::to_owned);
    let (kind, name, harness) = match &request.action {
        // A job is named for its program, so the board shows it before any snapshot has it.
        Action::Job { cmd, .. } => (
            "job",
            cmd.first()
                .map(|c| c.rsplit('/').next().unwrap_or(c).to_owned()),
            None,
        ),
        Action::Agent { name, kind, .. } => ("agent", Some(name.clone()), Some(kind.clone())),
        Action::Place => return Err(Error::Invalid("place cannot create an item".into())),
    };
    let role = harness.as_ref().map(|_| {
        text("role").unwrap_or_else(|| {
            if request.branch.is_some() {
                "writer"
            } else {
                "reader"
            }
            .into()
        })
    });
    Ok(Item {
        id: request.id.clone(),
        kind: kind.into(),
        host,
        pane: pane.into(),
        owner_pane: request.owner.clone(),
        owner_session,
        name,
        created_ms: now,
        launch_confirmed_ms: result["launch_confirmed_ms"].as_u64(),
        closed_ms: None,
        harness,
        role,
        profile: text("profile"),
        model: text("model"),
        ..Item::default()
    })
}

fn update_ticket(ledger: &Ledger, id: &str, update: impl FnOnce(&mut Ticket)) -> Result<Ticket> {
    ledger.with_lock("queue", || {
        let mut tickets = queue(ledger)?;
        let ticket = tickets
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(|| Error::Invalid(format!("unknown ticket {id}")))?;
        let before = ticket.state.clone();
        update(ticket);
        if before != ticket.state
            && matches!(ticket.state.as_str(), "admitted" | "failed" | "cancelled")
            && cancellation(ledger, &ticket.request)?.is_none_or(|saved| saved.release != "pending")
        {
            jobs::finish_once(ledger, &mut ticket.finished_ms, Some(ticket.created_ms))?;
            ticket_completion(ledger, ticket)?;
        }
        let result = ticket.clone();
        save_queue(ledger, &tickets)?;
        Ok(result)
    })
}

fn report_admission_failure(ledger: &Ledger, ticket: &Ticket, error: &Error) -> Result<()> {
    eprintln!("dogsled admission {}: {error}", ticket.request.id);
    update_ticket(ledger, &ticket.id, |t| {
        t.error = Some(error.to_string());
    })?;
    Ok(())
}

fn admit(fleet: &Fleet, now: u64) -> Result<()> {
    // Hosts that had no room for an earlier ticket in this pass. A later ticket that can use
    // only these hosts waits, so it cannot pass an earlier one; any other ticket is still served.
    let mut full: Vec<String> = Vec::new();
    for ticket in queue(fleet.ledger)? {
        let expected_request = serde_json::to_value(&ticket.request)?;
        let result = fleet.ledger.child("admissions").with_lock(&ticket.request.id, || {
        let Some(mut ticket) = queue(fleet.ledger)?.into_iter().find(|t| t.id == ticket.id) else { return Ok(()); };
        if serde_json::to_value(&ticket.request)? != expected_request { return Err(crate::unit_transport::conflict("queued request changed before admission")); }
        if let Some(saved) = cancellation(fleet.ledger, &ticket.request)? {
            project_cancellation(fleet.ledger, &saved)?;
            return Ok(());
        }
        if ticket.state == "admitting" {
            let saved = fleet
                .ledger
                .child("admissions")
                .read::<Attempt>(&format!("{}.json", ticket.request.id))?;
            let recoverable = (matches!(ticket.request.action, Action::Place)
                && saved.as_ref().is_some_and(|a| {
                    a.placement
                        .as_ref()
                        .is_some_and(|p| p.version == crate::node::PLACEMENT_VERSION)
                }))
                || matches!(ticket.request.action, Action::Agent { .. })
                    && saved.as_ref().is_some_and(|a| {
                        a.launch.is_some()
                            || (a.unit.is_some() && a.host.is_none())
                            || (ticket.retry.attempts > 0
                                && a.error.is_some()
                                && a.host.as_ref().is_some_and(|h| !h.is_local()))
                    });
            ticket = update_ticket(fleet.ledger, &ticket.id, |t| {
                t.state = if recoverable { "pending" } else { "failed" }.into();
                t.diagnostic = saved.as_ref().and_then(|a| a.diagnostic.clone());
                let message = "service stopped during admission; inspect the selected host before resubmitting";
                t.error = Some(t.diagnostic.as_ref().map_or_else(
                    || message.into(),
                    |diagnostic| format!("{message}; {diagnostic}"),
                ));
            })?;
        }
        if ticket.state != "pending" || !ticket.retry.ready(now) {
            return Ok(());
        }
        let hosts = candidates(fleet, &ticket.request);
        if hosts.iter().all(|h| full.contains(h)) {
            return Ok(());
        }
        let claimed = update_ticket(fleet.ledger, &ticket.id, |t| {
            if t.state == "pending" {
                t.state = "admitting".into();
            }
        })?;
        if claimed.state != "admitting" {
            return Ok(());
        }
        // No store lock spans SSH, cloning, hooks, or agent readiness.
        let outcome = perform_locked(fleet, &claimed.request, now);
        launch_test_barrier("BEFORE_FINALIZATION")?;
        let no_room = matches!(&outcome, Ok(None));
        update_ticket(fleet.ledger, &ticket.id, |t| match outcome {
            Ok(Some(result)) if result["status"] == "pending_release" => {
                t.state = "pending".into();
                t.result = Some(result);
                t.error = Some("placement release pending; resume the original request".into());
            }
            Ok(Some(result)) => {
                t.state = "admitted".into();
                t.result = Some(result);
                t.wait_reason = None;
                t.error = None;
                t.diagnostic = None;
            }
            Ok(None) => t.state = "pending".into(),
            // A refused request fails at once; a failed launch, SSH, or Herdr call is retried.
            Err(error @ Error::Launch(_)) if t.retry.attempts + 1 < LAUNCH_TRIES => {
                t.state = "pending".into();
                t.retry.fail(now, &t.id);
                t.error = Some(format!("attempt {}: {error}", t.retry.attempts));
                t.diagnostic = error.diagnostic().cloned();
            }
            Err(error) => {
                t.state = "failed".into();
                t.diagnostic = error.diagnostic().cloned();
                t.error = Some(if t.retry.attempts > 0 {
                    format!("{error} (after {} attempts)", t.retry.attempts + 1)
                } else {
                    error.to_string()
                });
            }
        })?;
        if no_room {
            full.extend(hosts);
        }
        Ok(())
        });
        if let Err(error) = result {
            report_admission_failure(fleet.ledger, &ticket, &error)?;
        }
        if fleet.hosts.iter().all(|h| full.contains(&h.id)) {
            break;
        }
    }
    Ok(())
}

/// A launch that failed after placement: Herdr did not start the agent in time, or the SSH
/// launch call failed. Refusals and deadlines are not retried.
fn retryable(error: &Error) -> bool {
    transient_ssh(error) || matches!(error.root(), Error::Command { .. } | Error::Io(_))
}

/// Retry only SSH transport categories. Authentication, remote commands, malformed replies,
/// structured refusals, and deadlines are terminal, even when wrapped in startup diagnostics.
fn transient_ssh(error: &Error) -> bool {
    use crate::hosts::Failure;
    matches!(
        error.root(),
        Error::Ssh {
            failure: Failure::Timeout
                | Failure::ConnectionRefused
                | Failure::HostNotResolved
                | Failure::Other,
            refusal: None,
            ..
        }
    )
}

/// Owner-specific closure evidence remains distinct from unit settlement and resources.
fn reconcile_finished(ledger: &Ledger) -> Result<()> {
    let attempts = ledger.child("admissions");
    for item in items::list(ledger)? {
        let Some(closed) = item.closed_ms.filter(|_| item.resource_closed_ms.is_none()) else {
            continue;
        };
        jobs::validate_id(&item.id)?;
        attempts.with_lock(&item.id, || {
            let file = format!("{}.json", item.id);
            let Some(mut attempt) = attempts.read::<Attempt>(&file)? else {
                return Ok(());
            };
            if attempt.unit.is_some() || attempt.placement.is_some() {
                return Ok(());
            }
            let Some(launch) = &attempt.launch else {
                return Ok(());
            };
            if launch.id != item.id
                || launch.host != item.host
                || attempt
                    .result
                    .as_ref()
                    .is_none_or(|result| result["pane"].as_str() != Some(&item.pane))
            {
                return Ok(());
            }
            let effective = jobs::advance_clock(ledger, crate::ledger::now_ms().max(closed))?;
            if attempt
                .finished_ms
                .is_some_and(|finished| finished != closed || finished > effective)
            {
                return Err(Error::Invalid(
                    "closed admission completion timestamp changed".into(),
                ));
            }
            if attempt.finished_ms.is_none() {
                attempt.finished_ms = Some(closed);
                attempts.write(&file, &attempt)?;
            }
            crate::launch::execution_closed(ledger, &item)?;
            Ok(())
        })?;
    }
    Ok(())
}

pub fn tick(fleet: &Fleet, now: u64) -> Result<()> {
    reconcile_finished(fleet.ledger)?;
    let reports = fleet.status(now)?;
    launch_test_barrier("PLACEMENT_SERVICE_BEFORE_ADMISSION")?;
    fleet.ledger.with_lock("admission", || {
        crate::preparation_retirement::recover(fleet)?;
        repair_cancellations(fleet)?;
        crate::unit_request::recover(fleet.ledger, now)?;
        admit(fleet, now)?;
        for ticket in queue(fleet.ledger)? {
            if ticket.waiting
                || ticket.notified
                || !matches!(ticket.state.as_str(), "admitted" | "failed")
            {
                continue;
            }
            let result = ticket.result.as_ref();
            let selected = fleet
                .ledger
                .child("admissions")
                .read::<Attempt>(&format!("{}.json", ticket.request.id))?
                .and_then(|a| a.host);
            let host = result
                .and_then(|r| r["host"].as_str().or_else(|| r["host"]["id"].as_str()))
                .or_else(|| selected.as_ref().map(|h| h.label.as_str()))
                .or_else(|| ticket.diagnostic.as_ref().and_then(|d| d.host.as_deref()))
                .or(ticket.request.host.as_deref())
                .unwrap_or("unselected");
            let summary = ticket
                .error
                .clone()
                .unwrap_or_else(|| result.map_or_else(String::new, Value::to_string));
            let text = format!(
                "dogsled: ticket {} {} on {host}: {summary}",
                ticket.id, ticket.state
            );
            // The owner's inbox reader shows it. An owner without a reader hears nothing until
            // it has one; nothing types into its terminal.
            let id = format!("ticket-{}", ticket.id);
            if !crate::inbox::notify(
                fleet,
                &reports,
                &ticket.request.owner,
                &id,
                "ticket",
                text,
                now,
            )? {
                continue;
            }
            update_ticket(fleet.ledger, &ticket.id, |t| t.notified = true)?;
        }
        Ok(())
    })
}

pub fn job_host<'a>(fleet: &'a Fleet<'_>, id: &str, now: u64) -> Result<&'a Host> {
    jobs::validate_id(id)?;
    if jobs::get(fleet.ledger, id).is_ok() {
        return fleet.find("local");
    }
    if let Some(item) = items::list(fleet.ledger)?
        .iter()
        .find(|i| i.kind == "job" && i.id == id)
    {
        return fleet.find(&item.host);
    }
    for host in fleet.status(now)? {
        if host.jobs.iter().any(|j| j.id == id) {
            return fleet.find(&host.host.id);
        }
    }
    Err(Error::Invalid(format!("unknown job {id}")))
}

#[cfg(test)]
mod line_limit_tests {
    use super::*;

    fn files() -> Files {
        Files::at(
            std::path::Path::new("/state/launches"),
            "r-1792603096562-36969-335",
        )
    }

    fn build(kind: &str, name: &str) -> Result<Vec<String>> {
        agent_args(kind, name, &[], &files(), None)
    }

    #[test]
    fn the_line_is_the_same_for_every_task_and_fits() {
        for kind in KINDS {
            let args = build(kind, "cleanup-identity-rondo").unwrap();
            assert!(line_len(kind, &args) <= LINE_BUDGET, "{kind}");
            assert!(
                args.last()
                    .unwrap()
                    .contains("r-1792603096562-36969-335.task.md")
            );
        }
    }

    #[test]
    fn pi_and_claude_read_the_contract_from_a_file() {
        for (kind, flag) in [
            ("pi", "--append-system-prompt"),
            ("claude", "--append-system-prompt-file"),
        ] {
            let args = build(kind, "n").unwrap();
            let at = args.iter().position(|a| a == flag).unwrap();
            assert_eq!(args[at + 1], files().contract, "{kind}");
            assert!(
                !args.iter().any(|a| a.contains("Dogsled started you")),
                "{kind}"
            );
        }
    }

    #[test]
    fn a_long_name_is_refused() {
        let error = build("pi", &"n".repeat(2_000)).unwrap_err().to_string();
        assert!(error.contains("budget"), "{error}");
    }

    #[test]
    fn a_long_state_path_is_refused_where_the_files_live() {
        let dir = std::path::PathBuf::from(format!("/{}", "d".repeat(1_000)));
        let files = Files::at(&dir, "r-1");
        assert!(agent_args("pi", "n", &[], &files, None).is_err());
    }
}
