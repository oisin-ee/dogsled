//! Host-owned job execution, output capture, and bounded process-group lifetime.

use crate::error::{Error, Result};
use crate::ledger::{Ledger, now_ms};
use crate::shell::{Shell, kill_group};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The recorded creation time of a job request. It is stamped once, at the origin that
/// asked for the work, and every later copy is byte-identical. A request without this
/// evidence never starts new execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Validity {
    pub version: u32,
    pub created_ms: u64,
}

impl Validity {
    /// The version this binary records and accepts.
    pub const VERSION: u32 = 1;
    /// The evidence for a request created at `created_ms`.
    #[must_use]
    pub const fn new(created_ms: u64) -> Self {
        Self {
            version: Self::VERSION,
            created_ms,
        }
    }
}

/// An origin prohibition sent only after admission-locked no-start proof and preparation release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnstartedRelease {
    pub id: String,
    pub validity: Validity,
    pub owner: String,
    pub owner_host: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnstartedAck {
    pub request: UnstartedRelease,
    pub finished_ms: u64,
}

pub(crate) fn reject_unstarted(ledger: &Ledger, id: &str) -> Result<()> {
    if crate::job_store::unstarted_finished(ledger, id)?.is_some() {
        return Err(Error::Invalid(
            "request released without execution; future dispatch prohibited".into(),
        ));
    }
    Ok(())
}

pub(crate) fn release_unstarted(
    ledger: &Ledger,
    request: &UnstartedRelease,
) -> Result<UnstartedAck> {
    validate_id(&request.id)?;
    if request.owner.is_empty()
        || request.owner_host.is_empty()
        || request.validity.version != Validity::VERSION
    {
        return Err(Error::Invalid(
            "unstarted release identity is incomplete".into(),
        ));
    }
    let ack = ledger
        .child("preparation-receipts")
        .with_lock(&request.id, || {
            ledger.child("launches").with_lock(&request.id, || {
                if record(ledger, &request.id)?.is_some()
                    || crate::launch_store::exists(ledger, &request.id)?
                    || crate::prepare::held(ledger, &request.id)?
                {
                    return Err(Error::Invalid(
                        "unstarted release contradicts execution or preparation ownership".into(),
                    ));
                }
                if let Some(saved) = request_validity(ledger, &request.id)? {
                    match_request_validity(Some(saved), Some(request.validity))?;
                } else {
                    check_validity(ledger, Some(&request.validity))?;
                    record_request_validity(ledger, &request.id, Some(request.validity))?;
                }
                crate::job_store::remember_owner(
                    ledger,
                    &request.id,
                    &request.owner,
                    Some(&request.owner_host),
                )?;
                let mut finished = crate::job_store::unstarted_finished(ledger, &request.id)?;
                finish_once(ledger, &mut finished, Some(request.validity.created_ms))?;
                let finished_ms = finished
                    .ok_or_else(|| Error::Invalid("unstarted completion missing".into()))?;
                crate::job_store::finish_unstarted(ledger, &request.id, finished_ms)?;
                Ok(UnstartedAck {
                    request: request.clone(),
                    finished_ms,
                })
            })
        })?;
    crate::job_store::release_reservation(ledger, &request.id)?;
    Ok(ack)
}

/// Version 2 requires recorded request validity and checkout ownership through spawn.
/// Peers advertising an older version cannot safely execute a new job.
pub const VALIDITY_PROTOCOL_VERSION: u32 = 2;

/// How long a recorded request may start new execution: 30 days.
pub const VALIDITY_WINDOW_MS: u64 = 2_592_000_000;
/// How far ahead of this host clock a recorded request may claim to be: 5 minutes.
pub const VALIDITY_SKEW_MS: u64 = 300_000;
/// How long a job's output is kept after the job ended: 24 hours.
pub const LOG_KEEP_MS: u64 = 24 * 60 * 60 * 1000;
/// How long a conclusively terminal job record is kept after the job ended: 30 days.
pub const RECORD_KEEP_MS: u64 = 30 * 24 * 60 * 60 * 1000;
/// What a reader sees when a job's output has expired.
pub const OUTPUT_EXPIRED: &str = "output expired";
/// No permanent identity tombstones are kept.
pub const RESULT_UNAVAILABLE: &str = "result unavailable: expired or unknown";

/// Read the request-owned authority, which old queue writers never touch.
pub fn request_validity(ledger: &Ledger, id: &str) -> Result<Option<Validity>> {
    validate_id(id)?;
    crate::store::read(ledger, |tx| crate::job_store::validity(tx, id)).map_err(|error| match error
    {
        Error::Deadline(_) => error,
        _ => refuse(crate::error::Refusal::JobValidityUnprovable),
    })
}

/// Fresh origin issuance. The marker prevents transport from repairing deleted authority.
pub fn issue_request_validity(
    ledger: &Ledger,
    id: &str,
    owner: &str,
    origin_host: &str,
) -> Result<Validity> {
    validate_id(id)?;
    let validity = origin_validity(ledger)?;
    crate::store::write(ledger, |tx| {
        if crate::job_store::validity(tx, id)?.is_some() || crate::job_store::ingress(tx, id)? {
            return Err(refuse(crate::error::Refusal::JobValidityUnprovable));
        }
        crate::job_store::put_ingress(tx, id)?;
        crate::job_store::put_validity(tx, id, validity)?;
        crate::job_store::remember_owner_in(tx, id, owner, Some(origin_host))?;
        Ok(validity)
    })
}

/// Versioned non-job transport capability. No effectful downgrade is permitted.
pub const REQUEST_VALIDITY_PROTOCOL_VERSION: u32 = 1;

/// Accept validated first host ingress only. Existing owning evidence cannot restore
/// a deleted sidecar from a transport copy.
pub fn receive_request_validity(
    ledger: &Ledger,
    id: &str,
    incoming: Option<Validity>,
) -> Result<()> {
    validate_id(id)?;
    if let Some(saved) = request_validity(ledger, id)? {
        return match_request_validity(Some(saved), incoming);
    }
    if crate::admissions::exists(ledger, id)? || crate::launch_store::exists(ledger, id)? {
        return Err(refuse(crate::error::Refusal::JobValidityUnprovable));
    }
    if crate::preparation_store::exists(ledger, id)? {
        return Err(refuse(crate::error::Refusal::JobValidityUnprovable));
    }
    let holds = crate::job_store::reservations(ledger)?;
    if crate::job_store::selection(ledger, id)?.is_some()
        || crate::job_store::cancellation(ledger, id)?.is_some()
        || holds.iter().any(|hold| hold.id == id)
        || crate::launch_store::progress_value(ledger, id)?.is_some()
        || crate::work::queued_request(ledger, id)?
    {
        return Err(refuse(crate::error::Refusal::JobValidityUnprovable));
    }
    check_validity(ledger, incoming.as_ref())?;
    // This is publication provenance only, never a second validity authority.
    // Publish it first: interrupted issuance refuses rather than recreating authority.
    crate::store::write(ledger, |tx| {
        if let Some(saved) = crate::job_store::validity(tx, id)? {
            return match_request_validity(Some(saved), incoming);
        }
        if crate::job_store::ingress(tx, id)? {
            return Err(refuse(crate::error::Refusal::JobValidityUnprovable));
        }
        crate::job_store::put_ingress(tx, id)?;
        let validity =
            incoming.ok_or_else(|| refuse(crate::error::Refusal::JobValidityUnprovable))?;
        crate::job_store::put_validity(tx, id, validity)
    })
}

/// Compare a non-job transport stamp with its recorded authority. Missing authority
/// never repairs itself from a queue, admission, or launch envelope.
pub fn match_request_validity(saved: Option<Validity>, incoming: Option<Validity>) -> Result<()> {
    match (saved, incoming) {
        (Some(saved), Some(incoming)) if saved == incoming => Ok(()),
        _ => Err(refuse(crate::error::Refusal::JobValidityUnprovable)),
    }
}

/// Only a request origin or transport ingress may create a sidecar. Once published,
/// the same id cannot acquire a different creation time. Ledger writes are atomic.
pub fn record_request_validity(
    ledger: &Ledger,
    id: &str,
    validity: Option<Validity>,
) -> Result<()> {
    crate::store::write(ledger, |tx| record_request_validity_in(tx, id, validity))
}

pub(crate) fn record_request_validity_in(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    validity: Option<Validity>,
) -> Result<()> {
    validate_id(id)?;
    match (crate::job_store::validity(tx, id)?, validity) {
        (Some(saved), Some(incoming)) if saved != incoming => {
            Err(Error::Invalid(format!("job {id} request validity changed")))
        }
        (None, Some(incoming)) => crate::job_store::put_validity(tx, id, incoming),
        _ => Ok(()),
    }
}

/// The host's job validity clock. Its high-water time never moves back, so a host clock
/// that moves back cannot reopen an expired request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clock {
    pub version: u32,
    pub high_water_ms: u64,
}

impl Clock {
    /// The version this host records and accepts.
    pub const VERSION: u32 = 1;
}

/// A refusal with the fixed message its code carries.
fn refuse(code: crate::error::Refusal) -> Error {
    Error::Refused {
        code,
        detail: code.message().to_owned(),
    }
}

/// The newest timestamp in this host's own records: job records, launch receipts,
/// admissions, and queue tickets. A store that cannot be read refuses: the clock cannot
/// start without accounting for every local record.
fn newest_record_ms(ledger: &Ledger) -> Result<u64> {
    let mut newest = 0_u64;
    for job in list(ledger)? {
        for ms in [
            job.validity.map(|v| v.created_ms),
            job.started_ms,
            job.ended_ms,
        ]
        .into_iter()
        .flatten()
        {
            newest = newest.max(ms);
        }
    }
    let mut note = |value: &Value, path: &str| {
        if let Some(ms) = value.pointer(path).and_then(Value::as_u64) {
            newest = newest.max(ms);
        }
    };
    let admissions = crate::admissions::values(ledger)?;
    let mut records = admissions.clone();
    records.extend(crate::launch_store::values(ledger)?);
    records.extend(crate::preparation_store::values(ledger)?);
    records.extend(
        crate::job_store::cancellations(ledger)?
            .into_iter()
            .map(serde_json::to_value)
            .collect::<serde_json::Result<Vec<_>>>()?,
    );
    for value in records {
        note(&value, "/request/validity/created_ms");
        note(&value, "/validity/created_ms");
        note(&value, "/launch/validity/created_ms");
        note(&value, "/preparation/validity/created_ms");
        note(&value, "/envelope/request/validity/created_ms");
        note(&value, "/finished_ms");
        note(&value, "/recorded_ms");
        note(&value, "/accepted_ms");
        note(&value, "/terminal_job/ended_ms");
        note(&value, "/certificate/terminal_evidence/job/ended_ms");
        note(&value, "/certificate/terminal_evidence/closed_ms");
        note(&value, "/origin_release/accepted_ms");
    }
    for value in admissions {
        note(&value, "/request/action/validity/created_ms");
        note(&value, "/launch/action/validity/created_ms");
    }
    for (_, validity) in crate::job_store::validities(ledger)? {
        note(&serde_json::to_value(validity)?, "/created_ms");
    }
    for ticket in crate::work::queue_records(ledger).map_err(|error| match error {
        Error::Deadline(_) => error,
        _ => refuse(crate::error::Refusal::JobClockUnavailable),
    })? {
        note(&ticket, "/created_ms");
        note(&ticket, "/request/action/validity/created_ms");
        note(&ticket, "/request/validity/created_ms");
        note(&ticket, "/finished_ms");
    }
    Ok(newest)
}

/// Read the clock without initializing it; projections must not create authority.
pub(crate) fn recorded_clock(ledger: &Ledger) -> Result<Option<Clock>> {
    crate::store::read(ledger, crate::job_store::clock).map_err(|error| match error {
        Error::Deadline(_) => error,
        _ => refuse(crate::error::Refusal::JobClockUnavailable),
    })
}

/// The time a clock that is not recorded yet starts from: the newest local record.
pub(crate) fn clock_floor(ledger: &Ledger) -> Result<u64> {
    if recorded_clock(ledger)?.is_none() {
        newest_record_ms(ledger)
    } else {
        Ok(0)
    }
}

/// The validity fence is initialized and advanced in one database transaction.
pub fn clock(ledger: &Ledger) -> Result<Clock> {
    let now = now_ms();
    let floor = clock_floor(ledger)?;
    crate::store::write(ledger, |tx| load_clock(tx, now.max(floor)))
}

fn load_clock(tx: &rusqlite::Transaction<'_>, now: u64) -> Result<Clock> {
    match crate::job_store::clock(tx) {
        Ok(Some(clock)) if clock.version == Clock::VERSION => Ok(clock),
        Err(error @ Error::Deadline(_)) => Err(error),
        Ok(Some(_)) | Err(_) => Err(refuse(crate::error::Refusal::JobClockUnavailable)),
        Ok(None) => {
            let clock = Clock {
                version: Clock::VERSION,
                high_water_ms: now,
            };
            crate::job_store::put_clock(tx, &clock).map_err(|error| match error {
                Error::Deadline(_) => error,
                _ => refuse(crate::error::Refusal::JobClockUnavailable),
            })?;
            Ok(clock)
        }
    }
}

/// Stamp the owner's first terminal transition. Retrying validates and preserves the
/// original value; observation of a legacy terminal record must not call this helper.
pub fn finish_once(
    ledger: &Ledger,
    finished: &mut Option<u64>,
    created: Option<u64>,
) -> Result<()> {
    finish_at(advance_clock(ledger, now_ms())?, finished, created)
}

/// [`finish_once`] inside the caller's write transaction. `floor` is [`clock_floor`].
pub(crate) fn finish_once_in(
    tx: &rusqlite::Transaction<'_>,
    floor: u64,
    finished: &mut Option<u64>,
    created: Option<u64>,
) -> Result<()> {
    finish_at(advance_clock_in(tx, now_ms(), floor)?, finished, created)
}

fn finish_at(effective: u64, finished: &mut Option<u64>, created: Option<u64>) -> Result<()> {
    if let Some(saved) = finished {
        if *saved > effective
            || created.is_some_and(|created| saved.saturating_add(VALIDITY_SKEW_MS) < created)
        {
            return Err(Error::Invalid(
                "completion timestamp contradicts its owner evidence; retention refused".into(),
            ));
        }
    } else {
        *finished = Some(effective);
    }
    Ok(())
}

/// Owner-local retention proof. This never repairs legacy timestamps or deletes records.
pub(crate) fn completion_time(
    ledger: &Ledger,
    finished: Option<u64>,
    created: Option<u64>,
    terminal: bool,
) -> Result<Option<u64>> {
    if finished.is_none() {
        return Ok(None);
    }
    completion_at(
        recorded_clock(ledger)?.map(|clock| clock.high_water_ms),
        finished,
        created,
        terminal,
    )
}

/// [`completion_time`] inside the caller's write transaction. Like it, this reads the
/// recorded clock and never initializes one.
pub(crate) fn completion_time_in(
    tx: &rusqlite::Transaction<'_>,
    finished: Option<u64>,
    created: Option<u64>,
    terminal: bool,
) -> Result<Option<u64>> {
    if finished.is_none() {
        return Ok(None);
    }
    let recorded = crate::job_store::clock(tx).map_err(|error| match error {
        Error::Deadline(_) => error,
        _ => refuse(crate::error::Refusal::JobClockUnavailable),
    })?;
    completion_at(
        recorded.map(|clock| clock.high_water_ms),
        finished,
        created,
        terminal,
    )
}

fn completion_at(
    high_water_ms: Option<u64>,
    finished: Option<u64>,
    created: Option<u64>,
    terminal: bool,
) -> Result<Option<u64>> {
    let Some(finished) = finished else {
        return Ok(None);
    };
    let effective = high_water_ms.map_or(now_ms(), |high_water| high_water.max(now_ms()));
    if !terminal
        || finished > effective
        || created.is_some_and(|created| finished.saturating_add(VALIDITY_SKEW_MS) < created)
    {
        return Err(Error::Invalid(
            "completion timestamp contradicts terminal provenance; retention refused".into(),
        ));
    }
    Ok(Some(finished))
}

/// The host's effective time: the later of its high-water time and its wall clock. The
/// high-water time is persisted first, so a host clock that moves back cannot reopen an
/// expired request.
pub fn advance_clock(ledger: &Ledger, now: u64) -> Result<u64> {
    let floor = clock_floor(ledger)?;
    crate::store::write(ledger, |tx| advance_clock_in(tx, now, floor))
}

/// [`advance_clock`] inside the caller's write transaction. `floor` is [`clock_floor`].
pub(crate) fn advance_clock_in(
    tx: &rusqlite::Transaction<'_>,
    now: u64,
    floor: u64,
) -> Result<u64> {
    let clock = load_clock(tx, now.max(floor))?;
    let effective = clock.high_water_ms.max(now);
    if effective != clock.high_water_ms {
        crate::job_store::put_clock(
            tx,
            &Clock {
                version: Clock::VERSION,
                high_water_ms: effective,
            },
        )
        .map_err(|error| match error {
            Error::Deadline(_) => error,
            _ => refuse(crate::error::Refusal::JobClockUnavailable),
        })?;
    }
    Ok(effective)
}

/// Check a recorded request against this host's clock. The clock advances first, so a
/// refusal records the time it was refused at. A request without evidence, an old
/// request, and a request from the future each refuse with their own code.
pub fn check_validity(ledger: &Ledger, validity: Option<&Validity>) -> Result<()> {
    check_validity_at(ledger, validity, crate::ledger::now_ms())
}

/// Check a recorded request against this host's clock at `now`.
pub fn check_validity_at(ledger: &Ledger, validity: Option<&Validity>, now: u64) -> Result<()> {
    // The clock advances first, so a refusal is recorded at the time it was refused.
    let effective = advance_clock(ledger, now)?;
    let Some(validity) = validity else {
        return Err(refuse(crate::error::Refusal::JobValidityUnprovable));
    };
    if validity.version != Validity::VERSION {
        return Err(refuse(crate::error::Refusal::JobValidityUnprovable));
    }
    // A request from the future is refused against the wall clock, so a host clock that
    // moved back does not refuse a request that was made while it was correct.
    if validity.created_ms > now.saturating_add(VALIDITY_SKEW_MS) {
        return Err(refuse(crate::error::Refusal::JobFromFuture));
    }
    // The age is measured against the effective time, so a host clock that moved back
    // cannot reopen an expired request.
    if effective.saturating_sub(validity.created_ms) > VALIDITY_WINDOW_MS {
        return Err(refuse(crate::error::Refusal::JobExpired));
    }
    Ok(())
}

/// The validity evidence a new request carries. The origin stamps it once, and refuses
/// issuance when this host's clock is behind its own high-water time.
pub fn origin_validity(ledger: &Ledger) -> Result<Validity> {
    let now = crate::ledger::now_ms();
    let clock = clock(ledger)?;
    if clock.high_water_ms > now.saturating_add(VALIDITY_SKEW_MS) {
        return Err(refuse(crate::error::Refusal::JobClockUnavailable));
    }
    Ok(Validity::new(now))
}

/// Startup authority captured before pane allocation. Unknown versions/scopes stay
/// readable for inspection and cleanup, but cannot authorize execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Startup {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preparation: Option<crate::prepare::disposition::Binding>,
    #[serde(flatten)]
    pub scope: ExecutionScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum ExecutionScope {
    Git {
        checkout: crate::cleanup::Tree,
    },
    NonRepository {
        cwd: String,
    },
    #[serde(other)]
    Unsupported,
}

impl Startup {
    pub fn captured(cwd: &str, checkout: Option<&crate::cleanup::Tree>) -> Self {
        Self {
            version: 1,
            preparation: None,
            scope: checkout.map_or_else(
                || ExecutionScope::NonRepository { cwd: cwd.into() },
                |checkout| ExecutionScope::Git {
                    checkout: checkout.clone(),
                },
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub cmd: Vec<String>,
    pub cwd: String,
    pub host: String,
    pub owner: String,
    pub pane: String,
    pub state: String,
    pub exit: Option<i32>,
    pub signal: Option<i32>,
    pub reason: Option<String>,
    pub started_ms: Option<u64>,
    pub ended_ms: Option<u64>,
    pub timeout_ms: Option<u64>,
    pub idle_timeout_ms: Option<u64>,
    /// The request's own creation time, copied from the request. A record without it
    /// proves no age.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validity: Option<Validity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup: Option<Startup>,
    /// The job's output was deleted after its retention window. A reader sees this
    /// instead of a missing log.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub output_expired: bool,
}

pub fn duration(text: &str) -> std::result::Result<u64, String> {
    let (number, factor) = if let Some(n) = text.strip_suffix("ms") {
        (n, 1)
    } else if let Some(n) = text.strip_suffix('s') {
        (n, 1_000)
    } else if let Some(n) = text.strip_suffix('m') {
        (n, 60_000)
    } else if let Some(n) = text.strip_suffix('h') {
        (n, 3_600_000)
    } else {
        return Err("duration needs ms, s, m, or h".into());
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(factor))
        .ok_or_else(|| "invalid duration".into())
}

pub fn store(ledger: &Ledger) -> Ledger {
    // This directory holds output files and locks for the external process/pane only.
    ledger.child("jobs")
}

pub fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(Error::Invalid("invalid job id".into()));
    }
    Ok(())
}

pub fn get(ledger: &Ledger, id: &str) -> Result<Job> {
    validate_id(id)?;
    crate::job_store::get(ledger, id)?.ok_or_else(|| Error::Refused {
        code: crate::error::Refusal::UnknownJob,
        detail: RESULT_UNAVAILABLE.into(),
    })
}

pub fn save(ledger: &Ledger, job: &Job) -> Result<()> {
    validate_id(&job.id)?;
    crate::job_store::put(ledger, job)
}

pub fn list(ledger: &Ledger) -> Result<Vec<Job>> {
    crate::job_store::all(ledger)
}

pub fn stop(ledger: &Ledger, id: &str) -> Result<()> {
    validate_id(id)?;
    store(ledger).with_lock(id, || {
        let job = get(ledger, id)?;
        if job.ended_ms.is_none() {
            if let Ownership::Pending(mut pending) = ownership(ledger, id)? {
                pending.state = "blocked".into();
                pending.reason = Some("stopped".into());
                pending.exit = None;
                pending.signal = None;
                pending.ended_ms = Some(now_ms());
                save(ledger, &pending)?;
            }
            crate::job_store::request_stop(ledger, id, true)?;
        }
        Ok(())
    })
}

/// The states a job holds when it finished. A job in any other state keeps its record and
/// its output: age never ends ownership.
const TERMINAL_STATES: [&str; 2] = ["idle", "blocked"];

/// What a job's own record says about its retention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Expiry {
    /// The job is live, unproved, or young: it keeps everything.
    Keep,
    /// The job's output is older than the log window.
    Log,
    /// The job's record is older than the record window.
    Record,
}

/// What the record says about a job's age. Only a conclusively terminal job expires: a
/// running, pending, claiming, or uncertain job keeps everything, and so does a job whose
/// state this build does not know.
pub(crate) fn terminal(job: &Job) -> bool {
    TERMINAL_STATES.contains(&job.state.as_str())
        && job
            .ended_ms
            .is_some_and(|ended| job.started_ms.is_none_or(|started| started <= ended))
}

/// Every record without conclusive terminal evidence independently owns its checkout.
/// Historical or unsupported evidence holds cwd descendants instead of inventing identity.
pub(crate) fn holds(job: &Job, tree: &crate::cleanup::Tree) -> bool {
    if terminal(job) {
        return false;
    }
    if let Some(Startup {
        version: 1,
        scope: ExecutionScope::Git { checkout },
        ..
    }) = &job.startup
        && let Some(identity) = &checkout.identity
    {
        return tree
            .identity
            .as_ref()
            .is_some_and(|current| current == identity);
    }
    crate::worktree::resolved(std::path::Path::new(&job.cwd))
        .starts_with(crate::worktree::resolved(std::path::Path::new(&tree.path)))
}

pub(crate) fn expiry(job: &Job, now: u64) -> Expiry {
    if !terminal(job) {
        return Expiry::Keep;
    }
    let age = now.saturating_sub(job.ended_ms.unwrap_or(now));
    if age > RECORD_KEEP_MS {
        Expiry::Record
    } else if age > LOG_KEEP_MS {
        Expiry::Log
    } else {
        Expiry::Keep
    }
}

/// Delete the output and the records of the jobs whose windows passed.
///
/// The pass runs on the service tick: it needs no Stop event and no doctor run. It
/// repeats safely, because a job it already expired is not expired again, and one
/// unreadable record never stops the others.
pub fn expire(ledger: &Ledger, now: u64) -> Result<()> {
    // Persist the validity fence before any deletion, even when the wall clock rolled back.
    let now = advance_clock(ledger, now)?;
    crate::work::expire_requests(ledger, now)?;
    for id in crate::job_store::expiry_ids(ledger, now)? {
        if let Err(error) = expire_one(ledger, &id, now) {
            // One unreadable record does not stop the others, and an unreadable record is
            // kept: an unknown state never expires.
            eprintln!("dogsled job expiry: {id}: {error}");
        }
    }
    crate::done::expire(ledger, now)?;
    crate::items::expire(ledger, now)?;
    crate::launch::expire(ledger, now)?;
    crate::node::expire_grants(ledger, now)?;
    crate::work::expire_requests(ledger, now)?;
    Ok(())
}

/// Expire one job under its own record lock, so a job that starts or ends meanwhile is
/// never half expired.
pub(crate) fn expire_one(ledger: &Ledger, id: &str, now: u64) -> Result<Expiry> {
    validate_id(id)?;
    let jobs = store(ledger);
    ledger.child("launches").with_lock(id, || {
        jobs.with_lock(id, || {
            let Some(job) = record(ledger, id)? else {
                return Ok(Expiry::Record);
            };
            match expiry(&job, now) {
                Expiry::Keep => Ok(Expiry::Keep),
                Expiry::Log => {
                    expire_log(ledger, &jobs, &job)?;
                    Ok(Expiry::Log)
                }
                Expiry::Record => {
                    expire_log(ledger, &jobs, &job)?;
                    crate::items::job_finished(ledger, &job)?;
                    if let Err(error) = crate::work::job_finished(ledger, &job) {
                        eprintln!("dogsled job terminal evidence {}: {error}", job.id);
                    }
                    // The record goes last: a reader that still sees the record finds the
                    // expiry marker on it.
                    crate::job_store::remove(ledger, id)?;
                    // The marker went with the record: nothing names this job any more.
                    remove(&jobs.dir.join(format!("{id}.log")))?;
                    Ok(Expiry::Record)
                }
            }
        })
    })
}

/// The output of a job goes after a day. The record says so before the log changes, so a
/// reader tells an expired output from a missing one. The log keeps only the marker, so
/// every reader of the log reports the expiry instead of a missing file.
fn expire_log(ledger: &Ledger, jobs: &Ledger, job: &Job) -> Result<()> {
    fs::create_dir_all(&jobs.dir)?;
    if !job.output_expired {
        let mut marked = job.clone();
        marked.output_expired = true;
        save(ledger, &marked)?;
    }
    let log = jobs.dir.join(format!("{}.log", job.id));
    let marker = format!("{OUTPUT_EXPIRED}\n");
    let is_marker = fs::metadata(&log).is_ok_and(|metadata| metadata.len() == marker.len() as u64)
        && fs::read_to_string(&log).is_ok_and(|text| text == marker);
    if !is_marker {
        // The rename is atomic, so a reader sees the old output or the marker, and never
        // a half-written file.
        let tmp = jobs.dir.join(format!("{}.log.new", job.id));
        fs::write(&tmp, &marker)?;
        fs::rename(&tmp, &log)?;
    }
    // The stop marker belongs to the output: it goes with it.
    crate::job_store::request_stop(ledger, &job.id, false)?;
    Ok(())
}

/// Delete a file that may already be gone. A missing file is the wanted end state.
pub(crate) fn remove(path: &std::path::Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Whether output expired, including a deleted record still named by a job item.
pub fn expired(ledger: &Ledger, id: &str) -> Result<bool> {
    match record(ledger, id)? {
        Some(job) => Ok(job.output_expired),
        None => record_expired(ledger, id),
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LogChunk {
    pub bytes: Vec<u8>,
    pub next: u64,
    pub done: bool,
    /// The job's output expired: the reader reports expiry and ignores the bytes.
    #[serde(default)]
    pub expired: bool,
}

/// Expiry is a terminal transition, independent of the output cursor.
pub fn expired_chunk(offset: u64) -> LogChunk {
    LogChunk {
        bytes: Vec::new(),
        next: offset,
        done: true,
        expired: true,
    }
}

/// Whether the job's record expired: it is gone, and its job item still names it.
pub fn record_expired(ledger: &Ledger, id: &str) -> Result<bool> {
    validate_id(id)?;
    if record(ledger, id)?.is_some() {
        return Ok(false);
    }
    Ok(crate::items::list(ledger)?
        .iter()
        .any(|item| item.kind == "job" && item.id == id)
        || crate::launch::retained_job(ledger, id)?.is_some())
}

pub fn logs(ledger: &Ledger, id: &str, offset: u64) -> Result<LogChunk> {
    use std::io::{Seek, SeekFrom};
    let job = match record(ledger, id)? {
        Some(job) => job,
        None if record_expired(ledger, id)? => return Ok(expired_chunk(offset)),
        None => get(ledger, id)?,
    };
    if job.output_expired {
        // The output expired: the read reports it and never fails.
        return Ok(expired_chunk(offset));
    }
    let mut bytes = Vec::new();
    match fs::File::open(store(ledger).dir.join(format!("{id}.log"))) {
        Ok(mut file) => {
            file.seek(SeekFrom::Start(offset))?;
            file.take(64 * 1024).read_to_end(&mut bytes)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && job.ended_ms.is_none() => (),
        Err(e) => return Err(e.into()),
    }
    Ok(LogChunk {
        next: offset + bytes.len() as u64,
        bytes,
        done: job.ended_ms.is_some(),
        expired: false,
    })
}

fn report(shell: &dyn Shell, job: &Job, state: &str, token: Option<&str>) -> Result<()> {
    if job.pane.is_empty() {
        return Ok(());
    }
    shell.run_ok(
        "herdr",
        &[
            "pane".into(),
            "report-agent".into(),
            job.pane.clone(),
            "--source".into(),
            "custom:dogsled".into(),
            "--agent".into(),
            "dogsled-job".into(),
            "--state".into(),
            state.into(),
        ],
    )?;
    let mut args = vec![
        "pane".into(),
        "report-metadata".into(),
        job.pane.clone(),
        "--source".into(),
        "custom:dogsled".into(),
        "--display-agent".into(),
        job.cmd.join(" ").chars().take(60).collect(),
    ];
    if let Some(token) = token {
        args.extend(["--token".into(), format!("dogsled={token}")]);
    }
    shell.run_ok("herdr", &args)?;
    Ok(())
}

fn reader(mut input: impl Read + Send + 'static, send: mpsc::Sender<std::io::Result<Vec<u8>>>) {
    std::thread::spawn(move || {
        let mut buffer = [0; 8192];
        loop {
            match input.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    if send.send(Ok(buffer[..n].to_vec())).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = send.send(Err(e));
                    break;
                }
            }
        }
    });
}

fn pane_write(bytes: &[u8]) -> bool {
    std::io::stdout()
        .write_all(bytes)
        .and_then(|()| std::io::stdout().flush())
        .is_ok()
}

struct Capture {
    signals: signal_hook::iterator::Signals,
    log: std::fs::File,
    child: std::process::Child,
}

fn spawn(ledger: &Ledger, job: &Job) -> Result<Capture> {
    let signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ])?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(store(ledger).dir.join(format!("{}.log", job.id)))?;
    let child = Command::new(&job.cmd[0])
        .args(&job.cmd[1..])
        .current_dir(&job.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    Ok(Capture {
        signals,
        log,
        child,
    })
}

fn capture(
    ledger: &Ledger,
    job: &Job,
    capture: Capture,
) -> Result<(std::process::ExitStatus, Option<String>)> {
    let Capture {
        mut signals,
        mut log,
        mut child,
    } = capture;
    let result = (|| {
        let (send, recv) = mpsc::channel();
        reader(
            child
                .stdout
                .take()
                .ok_or_else(|| Error::Invalid("missing stdout".into()))?,
            send.clone(),
        );
        reader(
            child
                .stderr
                .take()
                .ok_or_else(|| Error::Invalid("missing stderr".into()))?,
            send,
        );
        let start = Instant::now();
        let mut last = start;
        let mut reason = None;
        let mut status = None;
        let mut disconnected = false;
        let mut pane_output = true;
        loop {
            if status.is_none() {
                status = child.try_wait()?;
                if status.is_some() {
                    kill_group(child.id())?;
                }
            }
            if status.is_some() && disconnected {
                break;
            }
            if status.is_none() && reason.is_none() {
                reason = if let Some(signal) = signals.pending().next() {
                    Some(format!("signal {signal}"))
                } else if crate::job_store::stop_requested(ledger, &job.id)? {
                    Some("stopped".into())
                } else if job
                    .timeout_ms
                    .is_some_and(|ms| start.elapsed() >= Duration::from_millis(ms))
                {
                    Some("timeout".into())
                } else if job
                    .idle_timeout_ms
                    .is_some_and(|ms| last.elapsed() >= Duration::from_millis(ms))
                {
                    Some("idle-timeout".into())
                } else {
                    None
                };
                if reason.is_some() {
                    kill_group(child.id())?;
                }
            }
            match recv.recv_timeout(Duration::from_millis(10)) {
                Ok(bytes) => {
                    let bytes = bytes?;
                    log.write_all(&bytes)?;
                    if pane_output {
                        pane_output = pane_write(&bytes);
                    }
                    last = Instant::now();
                }
                Err(mpsc::RecvTimeoutError::Timeout) => (),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    disconnected = true;
                    if status.is_none() {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
            }
        }
        log.sync_all()?;
        Ok((
            status.ok_or_else(|| Error::Invalid("missing child status".into()))?,
            reason,
        ))
    })();
    if result.is_err() {
        kill_group(child.id())?;
        child.wait()?;
    }
    result
}

/// A job record that has not started: the intent published before its pane is known. It is
/// not executable, and no wrapper may run a job from it.
pub const CLAIMING: &str = "claiming";
/// An executable claim: the pane is known, and the command may run.
pub const PENDING: &str = "pending";
/// The wrapper owns the job and runs it.
pub const WORKING: &str = "working";
/// A claim whose dispatch outcome could not be proved.
pub const UNCERTAIN: &str = "uncertain";

/// What a job's durable record says about who may run it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ownership {
    /// No record: nobody claimed this job.
    Unclaimed,
    /// An executable claim. The wrapper may run it.
    Pending(Job),
    /// The wrapper owns it and runs it.
    Started(Job),
    /// It ended.
    Terminal(Job),
    /// It holds an ownership that no execution may take over: an allocation that stopped
    /// before its pane was known, or a dispatch whose outcome was never proved.
    Uncertain(Job),
}

/// The durable record of a job. A missing record is `None`; an unreadable one is an error,
/// so a caller never mistakes a corrupt record for an absent one.
pub fn record(ledger: &Ledger, id: &str) -> Result<Option<Job>> {
    validate_id(id)?;
    crate::job_store::get(ledger, id)
}

/// What the record says about who may run the job.
pub fn ownership(ledger: &Ledger, id: &str) -> Result<Ownership> {
    ownership_record(id, record(ledger, id)?)
}

fn ownership_record(id: &str, job: Option<Job>) -> Result<Ownership> {
    let Some(job) = job else {
        return Ok(Ownership::Unclaimed);
    };
    // The record's own identity must be the job that was asked for. A record stored under
    // this name for another id proves nothing about this job.
    if job.id != id {
        return Err(Error::Invalid(format!(
            "job {id} claim belongs to job {}",
            job.id
        )));
    }
    Ok(if job.started_ms.is_some() {
        Ownership::Started(job)
    } else if job.ended_ms.is_some() {
        Ownership::Terminal(job)
    } else if job.state == PENDING {
        Ownership::Pending(job)
    } else {
        // Not executable: an intent without a pane, or a dispatch that was never proved.
        Ownership::Uncertain(job)
    })
}

/// Publish the durable intent for a launch that is about to allocate a pane. The intent is
/// not executable. It is written before the pane side effect, so an allocation that stops
/// here stays uncertain and cannot authorize another allocation.
pub fn publish_intent(ledger: &Ledger, job: &Job) -> Result<()> {
    validate_id(&job.id)?;
    crate::store::write(ledger, |tx| {
        if !matches!(
            ownership_record(&job.id, crate::job_store::load(tx, &job.id)?)?,
            Ownership::Unclaimed
        ) {
            return Err(Error::Invalid(format!(
                "job {} already has an owner",
                job.id
            )));
        }
        let mut intent = job.clone();
        intent.state = CLAIMING.into();
        intent.pane = String::new();
        intent.exit = None;
        intent.signal = None;
        intent.reason = None;
        intent.started_ms = None;
        intent.ended_ms = None;
        crate::job_store::save(tx, &intent)
    })
}

/// Publish the executable claim for `job`, once its pane is known. The intent must still be
/// the record's own: another owner never takes a job over.
pub fn publish_claim(ledger: &Ledger, job: &Job) -> Result<()> {
    validate_id(&job.id)?;
    crate::store::write(ledger, |tx| {
        match ownership_record(&job.id, crate::job_store::load(tx, &job.id)?)? {
            Ownership::Uncertain(existing) if existing.state == CLAIMING => {
                // The intent must be this request's intent. Only the pane may change: the
                // command, directory, host, owner, and options are the request's own.
                if existing.id != job.id
                    || existing.cmd != job.cmd
                    || existing.cwd != job.cwd
                    || existing.host != job.host
                    || existing.owner != job.owner
                    || existing.timeout_ms != job.timeout_ms
                    || existing.idle_timeout_ms != job.idle_timeout_ms
                    || existing.validity != job.validity
                    || existing.startup != job.startup
                {
                    return Err(Error::Invalid(format!(
                        "job {} claim changed after its intent was published",
                        job.id
                    )));
                }
            }
            _ => {
                return Err(Error::Invalid(format!(
                    "job {} already has an owner",
                    job.id
                )));
            }
        }
        let mut claim = job.clone();
        claim.state = PENDING.into();
        claim.exit = None;
        claim.signal = None;
        claim.reason = None;
        claim.started_ms = None;
        claim.ended_ms = None;
        crate::job_store::save(tx, &claim)
    })
}

/// Only the allocating producer can settle its exact non-executable intent.
pub(crate) fn refuse_intent(ledger: &Ledger, expected: &Job, reason: &str) -> Result<u64> {
    let mut finished = None;
    finish_once(
        ledger,
        &mut finished,
        expected.validity.map(|v| v.created_ms),
    )?;
    let ended = finished.ok_or_else(|| Error::Invalid("job refusal completion missing".into()))?;
    crate::store::write(ledger, |tx| {
        let mut job = crate::job_store::load(tx, &expected.id)?
            .ok_or_else(|| Error::Invalid("job intent disappeared".into()))?;
        if job != *expected
            || job.state != CLAIMING
            || job.started_ms.is_some()
            || !job.pane.is_empty()
        {
            return Err(Error::Invalid("job intent changed before refusal".into()));
        }
        job.state = "blocked".into();
        job.reason = Some(reason.into());
        job.ended_ms = Some(ended);
        crate::job_store::save(tx, &job)?;
        Ok(ended)
    })
}

/// Keep the uncertainty of a dispatch that was not proved. A record the wrapper started or
/// ended is its evidence, and is never overwritten. Nothing here invents an end or a safe
/// retry: the job stays unproved until the host says otherwise.
pub fn uncertain(ledger: &Ledger, job: &Job, reason: String) -> Result<()> {
    validate_id(&job.id)?;
    crate::store::write(ledger, |tx| {
        let Some(current) = crate::job_store::load(tx, &job.id)? else {
            return Ok(());
        };
        if current.started_ms.is_some() || current.ended_ms.is_some() {
            return Ok(());
        }
        if current.state != PENDING {
            return Ok(());
        }
        let mut held = current;
        held.state = UNCERTAIN.into();
        held.reason = Some(reason);
        held.exit = None;
        held.signal = None;
        held.ended_ms = None;
        crate::job_store::save(tx, &held)
    })
}

/// The exact claim that authorizes this wrapper to run `id`. A record that is missing,
/// unreadable, for another job, incomplete, for another command, already started, ended, or
/// uncertain is refused here, before any process, log, or pane exists.
///
/// `initial` is the record the caller read for the requested job. The reload must still be
/// that same job: a record whose execution-affecting fields changed is refused, never
/// silently adopted.
fn pending_claim(claimed: Option<Job>, id: &str, initial: &Job) -> Result<Job> {
    let Some(claimed) = claimed else {
        return Err(Error::Invalid(format!("job {id} has no claim to run")));
    };
    // The record's own identity must be the requested job. A record stored under this name
    // for another id never runs here.
    if claimed.id != id {
        return Err(Error::Invalid(format!(
            "job {id} claim belongs to job {}",
            claimed.id
        )));
    }
    // The claim is complete: a command, a directory, a host, an owner, and the pane it was
    // allocated. An incomplete claim proves nothing.
    if claimed.cmd.is_empty()
        || claimed.cwd.is_empty()
        || claimed.host.is_empty()
        || claimed.owner.is_empty()
    {
        return Err(Error::Invalid(format!("job {id} claim is incomplete")));
    }
    if claimed.pane.is_empty() {
        return Err(Error::Invalid(format!(
            "job {id} claim has no allocated pane"
        )));
    }
    if claimed.started_ms.is_some() {
        return Err(Error::Invalid("job already started".into()));
    }
    if claimed.ended_ms.is_some() {
        return Err(Error::Invalid("job already ended".into()));
    }
    if claimed.state != PENDING {
        return Err(Error::Invalid(format!("job {id} claim is not executable")));
    }
    if claimed.exit.is_some() || claimed.signal.is_some() || claimed.reason.is_some() {
        return Err(Error::Invalid(format!(
            "job {id} claim already carries a result"
        )));
    }
    if claimed.cmd != initial.cmd {
        return Err(Error::Invalid(
            "exec command differs from the claimed command".into(),
        ));
    }
    // The execution-affecting inputs are the ones that were requested. A record that changed
    // its directory, host, owner, pane, or options since it was read asks for another job.
    if claimed.cwd != initial.cwd
        || claimed.host != initial.host
        || claimed.owner != initial.owner
        || claimed.pane != initial.pane
        || claimed.timeout_ms != initial.timeout_ms
        || claimed.idle_timeout_ms != initial.idle_timeout_ms
        || claimed.validity != initial.validity
        || claimed.startup != initial.startup
    {
        return Err(Error::Invalid(format!(
            "job {id} claim changed since it was requested"
        )));
    }
    Ok(claimed)
}

/// Mark the claim as started and return the running record. The short per-job record lock
/// holds this publication, and it is released before any capture.
fn start(ledger: &Ledger, id: &str, initial: &Job) -> Result<Job> {
    check_validity(ledger, initial.validity.as_ref())?;
    crate::store::write(ledger, |tx| {
        let mut running = pending_claim(crate::job_store::load(tx, id)?, id, initial)?;
        // The wrapper checks the request's age before it publishes the startup. `exec`
        // checks again right before the process starts, because the pane report blocks.
        // An expired or unprovable request never starts a process.
        running.started_ms = Some(now_ms());
        running.state = WORKING.into();
        crate::job_store::save(tx, &running)?;
        Ok(running)
    })
}

fn validate_startup(
    shell: &dyn Shell,
    job: &Job,
    guard: Option<&crate::worktree::Guard>,
) -> Result<()> {
    let Some(Startup {
        version: 1, scope, ..
    }) = &job.startup
    else {
        return Err(Error::Invalid(format!(
            "job {} has no supported startup ownership",
            job.id
        )));
    };
    crate::worktree::require_execution_directory(&job.cwd)?;
    match scope {
        ExecutionScope::Git { checkout } => {
            let identity = checkout
                .identity
                .as_ref()
                .ok_or_else(|| crate::worktree::refuse("job checkout has no identity"))?;
            let guard =
                guard.ok_or_else(|| crate::worktree::refuse("job repository is unguarded"))?;
            let live = guard
                .observe(shell, &job.cwd)?
                .ok_or_else(|| crate::worktree::refuse("job checkout disappeared"))?;
            if live.identity.as_ref() != Some(identity)
                || !crate::worktree::same(&live.path, &checkout.path)
                || !crate::worktree::same(&live.checkout.display().to_string(), &checkout.repo)
                || live.branch != checkout.branch
            {
                return Err(crate::worktree::refuse("job checkout identity changed"));
            }
        }
        ExecutionScope::NonRepository { cwd } => {
            if cwd != &job.cwd || crate::worktree::snapshot(shell, cwd)?.is_some() {
                return Err(crate::worktree::refuse(
                    "non-repository execution scope changed",
                ));
            }
        }
        ExecutionScope::Unsupported => {
            return Err(crate::worktree::refuse("unsupported job execution scope"));
        }
    }
    Ok(())
}

/// The lifetime lock crosses this boundary, but preparation/repository gates never do.
fn startup(
    ledger: &Ledger,
    shell: &dyn Shell,
    id: &str,
    initial: &Job,
    guard: Option<&crate::worktree::Guard>,
) -> Result<(crate::ledger::Lock, Job, Result<Capture>)> {
    let execution = store(ledger)
        .try_lock(&format!("{id}-exec"))?
        .ok_or_else(|| Error::Invalid(format!("job {id} already has an execution wrapper")))?;
    crate::prepare::disposition::allow_startup(ledger, initial)?;
    validate_startup(shell, initial, guard)?;
    let job = start(ledger, id, initial)?;
    if let Err(error) = report(shell, &job, WORKING, None) {
        eprintln!("dogsled pane report: {error}");
    }
    // Reporting may block. Recheck authority and checkout immediately before output/spawn.
    // Validation failure leaves the started evidence uncertain, without a terminal result.
    let captured = store(ledger).with_lock(id, || {
        if get(ledger, id)? != job {
            return Err(Error::Invalid(format!(
                "job {id} claim changed during startup"
            )));
        }
        check_validity(ledger, job.validity.as_ref())?;
        crate::prepare::disposition::allow_startup(ledger, &job)?;
        validate_startup(shell, &job, guard)?;
        Ok(spawn(ledger, &job))
    })?;
    Ok((execution, job, captured))
}

/// Run the job from its captured scope. Startup takes preparation, repository, nonblocking
/// lifetime exec, short record, and validity locks in that order. Monitoring retains only exec.
pub fn exec(ledger: &Ledger, shell: &dyn Shell, id: &str, initial: &Job) -> Result<i32> {
    validate_id(id)?;
    if initial.cmd.is_empty() {
        return Err(Error::Invalid("empty command".into()));
    }
    if initial.id != id {
        return Err(Error::Invalid(format!(
            "job {id} claim belongs to job {}",
            initial.id
        )));
    }
    let (_execution, mut job, captured) =
        ledger
            .child("preparation-receipts")
            .with_lock(id, || match &initial.startup {
                Some(Startup {
                    version: 1,
                    scope: ExecutionScope::Git { checkout },
                    ..
                }) => {
                    if checkout.identity.is_none() {
                        return Err(crate::worktree::refuse("job checkout has no identity"));
                    }
                    crate::worktree::with_lifecycle(
                        shell,
                        ledger,
                        std::path::Path::new(&checkout.repo),
                        |guard| startup(ledger, shell, id, initial, Some(guard)),
                    )
                }
                _ => startup(ledger, shell, id, initial, None),
            })?;
    // No outer gate is ever reacquired while exec remains held.
    let outcome = captured.and_then(|captured| capture(ledger, &job, captured));
    match &outcome {
        Ok((status, reason)) => {
            job.signal = status.signal();
            job.exit = Some(
                if reason.as_deref().is_some_and(|r| r.ends_with("timeout")) {
                    124
                } else {
                    status.code().unwrap_or(128 + status.signal().unwrap_or(0))
                },
            );
            job.reason.clone_from(reason);
        }
        Err(e) => {
            job.exit = Some(2);
            job.reason = Some(e.to_string());
        }
    }
    let code = job.exit.unwrap_or(2);
    job.state = if code == 0 { "idle" } else { "blocked" }.into();
    job.ended_ms = Some(now_ms());
    save(ledger, &job)?;
    let elapsed = job
        .ended_ms
        .unwrap_or(0)
        .saturating_sub(job.started_ms.unwrap_or(0));
    let age = if elapsed >= 3_600_000 {
        format!("{}h", elapsed / 3_600_000)
    } else if elapsed >= 60_000 {
        format!("{}m", elapsed / 60_000)
    } else {
        format!("{}s", elapsed / 1_000)
    };
    let token = format!(
        "{} · {}",
        job.reason
            .as_ref()
            .map_or_else(|| format!("exit {code}"), Clone::clone),
        age
    );
    if let Err(error) = report(shell, &job, &job.state, Some(&token)) {
        eprintln!("dogsled pane report: {error}");
    }
    // Closing our own foreground pane can terminate this wrapper. Keep all
    // durable writes and completion reports above this final best-effort call.
    if !job.pane.is_empty()
        && let Err(error) =
            shell.run_ok("herdr", &["pane".into(), "close".into(), job.pane.clone()])
    {
        eprintln!("dogsled pane close: {error}");
    }
    outcome?;
    Ok(code)
}
