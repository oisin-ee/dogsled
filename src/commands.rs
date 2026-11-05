//! User-facing status formatting and placement. No agent spawning or queue.

use crate::error::{Error, Result};
use crate::hosts::{ActionFleet, Fleet, Host, HostReport};
use crate::plan;
use crate::prepare::Prepared;
use serde::Serialize;

#[derive(Debug, Serialize, serde::Deserialize)]
pub struct Status {
    pub hosts: Vec<HostReport>,
    pub items: Vec<crate::items::Item>,
    pub queue: Vec<crate::work::Ticket>,
    /// Open children whose owner is gone and that the sweep keeps, with the reason.
    pub orphans: Vec<crate::sweep::Orphan>,
    pub sweep: Option<crate::sweep::Report>,
    #[serde(default)]
    pub stopped_activity: std::collections::BTreeMap<String, String>,
    /// Resource ownership is independent of task state and completion.
    #[serde(default)]
    pub holds: Vec<crate::holds::Hold>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Placement {
    Placed {
        host: Host,
        reservation: String,
        #[serde(flatten)]
        prepared: Prepared,
    },
    Full {
        full: bool,
        hosts: Vec<HostReport>,
    },
}

/// Resource pruning retries the lifecycle owner; it never deletes unproved evidence.
pub(crate) fn prune(
    fleet: &Fleet,
    host: &Host,
    plan: &crate::cleanup::Plan,
    now: u64,
) -> Result<crate::cleanup::Applied> {
    let mut ordinary = crate::cleanup::Plan::default();
    ordinary.entries.extend(
        plan.entries
            .iter()
            .filter(|entry| entry.class != "resources")
            .cloned(),
    );
    let resources: Vec<_> = plan
        .entries
        .iter()
        .filter(|entry| entry.class == "resources")
        .collect();
    if !resources.is_empty() {
        crate::work::settle_failed_jobs(fleet.ledger)?;
        crate::work::repair_cancellations(fleet)?;
        crate::launch::release_owned(fleet)?;
        crate::jobs::expire(fleet.ledger, now)?;
    }
    let mut applied: crate::cleanup::Applied = if host.is_local() {
        crate::cleanup::apply(&fleet.node(), &ordinary, now)?
    } else {
        fleet.remote_setup(
            host,
            &[
                "prune".into(),
                "--plan".into(),
                serde_json::to_string(&ordinary)?,
            ],
        )?
    };
    if !resources.is_empty() {
        let held = if host.is_local() {
            crate::holds::local(fleet.ledger, &host.id, now, fleet.policy.reservation_ttl_ms)?
        } else {
            fleet
                .remote::<crate::node::SnapshotReport>(host, &["snapshot".into()])?
                .holds
        };
        for resource in resources {
            if let Some(hold) = held.iter().find(|hold| hold.resource == resource.id) {
                let detail = format!("{} remains held: {}", resource.id, hold.display());
                applied.failures.push(detail.clone());
                applied.failure_entries.push(crate::cleanup::Failure {
                    entry: resource.clone(),
                    detail,
                });
            } else {
                applied.results.push(format!(
                    "{}: ownership released; replay evidence follows normal retention",
                    resource.id
                ));
            }
        }
    }
    Ok(applied)
}

/// Admission expiry has already proved completion and released every dependant.
pub(crate) fn expire_selection(ledger: &crate::ledger::Ledger, id: &str) -> Result<()> {
    crate::job_store::put_selection(ledger, id, None)
}

/// A placement for a job request. The request's own creation time rides with the
/// reservation, so the host checks the age before it holds capacity.
pub fn place_with(
    fleet: &ActionFleet,
    host: Option<&str>,
    id: &str,
    now: u64,
    validity: Option<crate::jobs::Validity>,
    placement: Option<&crate::work::Request>,
) -> Result<Placement> {
    crate::jobs::validate_id(id)?;
    // Selection spans an external host reservation; its lock is not a file-store lock.
    let selections = fleet.ledger.child("placement-attempts");
    selections.with_lock(id, || {
        if let Some(request) = placement
            && matches!(request.action, crate::work::Action::Place)
        {
            return place_recorded(fleet, request, now);
        }
        let nonjob = placement
            .is_some_and(|request| !matches!(request.action, crate::work::Action::Job { .. }));
        crate::jobs::check_validity(fleet.ledger, validity.as_ref())?;
        place_locked(fleet, host, id, now, validity, nonjob)
    })
}

/// The caller holds admission before entering selection. Only a correlated definitive
/// refusal permits another candidate; an ambiguous reply retains the pending envelope.
fn reserve_recorded(
    fleet: &Fleet,
    request: &crate::work::Request,
    transport: &crate::node::PlacementReplay,
    now: u64,
) -> Result<crate::node::ReserveOutcome> {
    fleet.reserve(
        &transport.envelope.host,
        &request.id,
        now,
        None,
        Some(transport),
        true,
    )
}

fn place_recorded(
    fleet: &ActionFleet,
    request: &crate::work::Request,
    now: u64,
) -> Result<Placement> {
    use crate::work::PlacementPhase;
    let mut attempt = crate::admissions::get(fleet.ledger, &request.id)?
        .ok_or_else(|| crate::unit_transport::conflict("placement admission is missing"))?;
    if attempt.request != serde_json::to_value(request)? {
        return Err(crate::unit_transport::conflict("placement request changed"));
    }
    if crate::job_store::selection(fleet.ledger, &request.id)?.is_some() {
        return Err(crate::unit_transport::conflict(
            "historical selection has no placement replay evidence",
        ));
    }
    let mut pending = attempt
        .placement
        .as_ref()
        .ok_or_else(|| crate::unit_transport::conflict("placement progress is missing"))?
        .expectation(request)?;
    if let Some(transport) = &pending
        && *fleet.find(&transport.envelope.host.id)? != transport.envelope.host
    {
        return Err(crate::unit_transport::conflict(
            "selected placement host or target changed",
        ));
    }
    let pinned = request.host.as_deref().map(|h| fleet.find(h)).transpose()?;
    let mut reports = fleet.reports()?;
    let mut candidates: Vec<_> = reports
        .iter()
        .filter(|r| pinned.is_none_or(|h| h.id == r.host.id))
        .filter_map(|r| r.headroom.clone())
        .collect();
    loop {
        let transport = if let Some(transport) = pending.take() {
            transport
        } else {
            let Some(selected) = plan::choose(&candidates) else {
                break;
            };
            let envelope = crate::node::PlacementReservation {
                version: crate::node::PLACEMENT_VERSION,
                request: attempt.request.clone(),
                host: fleet.find(&selected.host)?.clone(),
                reservation: request.id.clone(),
            };
            attempt
                .placement
                .as_mut()
                .ok_or_else(|| crate::unit_transport::conflict("missing placement progress"))?
                .phase = PlacementPhase::SelectionPending {
                envelope: envelope.clone(),
            };
            crate::admissions::put(fleet.ledger, &request.id, &attempt)?;
            crate::node::PlacementReplay {
                envelope,
                expected_grant: None,
            }
        };
        let envelope = &transport.envelope;
        let outcome = reserve_recorded(fleet, request, &transport, now)?;
        let reply = outcome.placement.ok_or_else(|| {
            crate::unit_transport::conflict(
                "peer lacks placement replay evidence; no preparation authorized",
            )
        })?;
        let progress = attempt
            .placement
            .as_mut()
            .ok_or_else(|| crate::unit_transport::conflict("missing placement progress"))?;
        if let Some(grant) = reply.grant {
            progress.phase = PlacementPhase::HostSelected { grant };
            attempt.host = Some(envelope.host.clone());
            crate::admissions::put(fleet.ledger, &request.id, &attempt)?;
            return Ok(Placement::Placed {
                host: envelope.host.clone(),
                reservation: envelope.reservation.clone(),
                prepared: Prepared::default(),
            });
        }
        // No hold was granted. Keep the immutable negative acknowledgement as proof
        // that this candidate finished, rather than silently replacing an expectation.
        progress.declined.push(reply);
        progress.phase = PlacementPhase::Recorded;
        crate::admissions::put(fleet.ledger, &request.id, &attempt)?;
        if let Some(report) = reports.iter_mut().find(|r| r.host.id == envelope.host.id) {
            report.headroom = outcome.headroom;
        }
        candidates.retain(|c| c.host != envelope.host.id);
    }
    full(reports, pinned)
}

fn place_locked(
    fleet: &ActionFleet,
    host: Option<&str>,
    id: &str,
    now: u64,
    validity: Option<crate::jobs::Validity>,
    nonjob: bool,
) -> Result<Placement> {
    let pinned = host.map(|h| fleet.find(h)).transpose()?;
    let saved = crate::job_store::selection(fleet.ledger, id)?;
    if let Some(saved) = &saved {
        let selected = fleet.find(saved)?;
        if pinned.is_some_and(|h| h.id != selected.id) {
            return Err(crate::unit_transport::conflict("reservation host changed"));
        }
    }
    // A saved selection binds host identity, not the lifetime of its hold. Only the host's
    // admission lock can replay a live hold or admit the same ID again after it expires.
    let pinned = saved
        .as_ref()
        .map(|s| fleet.find(s))
        .transpose()?
        .or(pinned);
    let mut reports = fleet.reports()?;
    if saved.is_some() {
        reports.retain(|r| pinned.is_some_and(|h| h.id == r.host.id));
    }
    // A job request needs a host that speaks the validity protocol. A peer that does not
    // report it is not a candidate: a job never falls back to an older protocol. The
    // refusal comes before any reservation, so no capacity is held for it.
    if validity.is_some() {
        let speaks = |r: &HostReport| {
            nonjob
                || r.host.is_local()
                || r.job_validity == Some(crate::jobs::VALIDITY_PROTOCOL_VERSION)
        };
        // A request pinned to a peer without the protocol is refused there and then. It
        // never moves to another host.
        if let Some(pinned) = pinned
            && reports
                .iter()
                .find(|r| r.host.id == pinned.id)
                .is_some_and(|r| !speaks(r))
        {
            return Err(Error::Refused {
                code: crate::error::Refusal::JobValidityUnsupported,
                detail: crate::error::Refusal::JobValidityUnsupported
                    .message()
                    .to_owned(),
            });
        }
        reports.retain(speaks);
        if reports.is_empty() {
            return Err(Error::Refused {
                code: crate::error::Refusal::JobValidityUnsupported,
                detail: crate::error::Refusal::JobValidityUnsupported
                    .message()
                    .to_owned(),
            });
        }
    }
    let mut candidates: Vec<_> = reports
        .iter()
        .filter(|r| pinned.is_none_or(|h| h.id == r.host.id))
        .filter_map(|r| r.headroom.clone())
        .collect();
    let replay = saved.is_some();
    let mut retry_host = saved;
    while let Some(selected) = retry_host
        .take()
        .or_else(|| plan::choose(&candidates).map(|c| c.host.clone()))
    {
        let host = fleet.find(&selected)?;
        crate::job_store::put_selection(fleet.ledger, id, Some(&host.id))?;
        let outcome = fleet.reserve(host, id, now, validity, None, nonjob)?;
        if outcome.reserved {
            let prepared = Prepared::default();
            return Ok(Placement::Placed {
                host: host.clone(),
                reservation: id.into(),
                prepared,
            });
        }
        if let Some(report) = reports.iter_mut().find(|r| r.host.id == host.id) {
            report.headroom = outcome.headroom;
        }
        if replay {
            // Keep the original host through a new admission refusal. A queued retry must
            // not migrate a checkout whose preparation may already have reached that host.
            break;
        }
        candidates.retain(|c| c.host != host.id);
        crate::job_store::put_selection(fleet.ledger, id, None)?;
    }
    full(reports, pinned)
}

/// A timed-out report is not evidence that capacity is full. Preserve the original
/// terminal deadline behavior for placement as well as Agent and Job selection.
fn full(reports: Vec<HostReport>, pinned: Option<&Host>) -> Result<Placement> {
    if let Some(error) = reports
        .iter()
        .filter(|report| pinned.is_none_or(|host| host.id == report.host.id))
        .filter_map(|report| report.error.as_ref())
        .find(|error| error.contains("deadline reached"))
    {
        return Err(crate::error::Error::Deadline(format!("placement: {error}")));
    }
    Ok(Placement::Full {
        full: true,
        hosts: reports,
    })
}

/// Whether a host can take work, as `status --line` marks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostState {
    Eligible,
    /// Reachable, but admission would refuse new work (` ▲`).
    NoHeadroom,
    /// Not reachable, or its service is down (` ?`).
    Unreachable,
}

impl HostState {
    pub fn of(host: &HostReport) -> Self {
        if !host.reachable || host.service != "up" {
            Self::Unreachable
        } else if !host.headroom.as_ref().is_some_and(|h| h.eligible) {
            Self::NoHeadroom
        } else {
            Self::Eligible
        }
    }

    /// The text `status --line` appends after the load.
    pub fn marker(self) -> &'static str {
        match self {
            Self::Unreachable => " ?",
            Self::NoHeadroom => " ▲",
            Self::Eligible => "",
        }
    }
}

/// The first line of a host, as typed values. `status --line` and the sidebar both read it,
/// so they never disagree and nothing parses the rendered text.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HostBrief {
    pub label: String,
    /// Busy cores over cores (`11/12`), `load N/cores` before the first sample, or `-`.
    pub load: String,
    pub state: HostState,
    /// False until the host has reported a probe; its line is then just `?`.
    pub measured: bool,
}

impl HostBrief {
    pub fn of(host: &HostReport) -> Self {
        Self {
            label: one_line(&host.host.label),
            load: load(host),
            state: HostState::of(host),
            measured: host.probe.is_some(),
        }
    }
}

fn load(host: &HostReport) -> String {
    // Busy cores over the last interval; load1 only while no sample exists.
    host.probe.as_ref().map_or_else(
        || "-".into(),
        |p| match p.busy {
            Some(busy) => format!("{:.0}/{}", busy * f64::from(p.cores), p.cores),
            None => format!("load {:.0}/{}", p.load1, p.cores),
        },
    )
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
pub fn brief(status: &Status) -> String {
    crate::observation::display::Display::capture(status).brief(crate::ledger::now_ms())
}
