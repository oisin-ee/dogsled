//! User-facing status formatting and placement. No agent spawning or queue.

use crate::error::{Error, Result};
use crate::hosts::{Fleet, Host, HostReport};
use crate::plan;
use crate::prepare::Prepared;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Status {
    pub hosts: Vec<HostReport>,
    pub items: Vec<crate::items::Item>,
    pub queue: Vec<crate::work::Ticket>,
    /// Open children whose owner is gone and that the sweep keeps, with the reason.
    pub orphans: Vec<crate::sweep::Orphan>,
    pub sweep: Option<crate::sweep::Report>,
    #[serde(default)]
    pub stopped_activity: std::collections::BTreeMap<String, String>,
}

pub fn status(fleet: &Fleet<'_>) -> Result<Status> {
    let hosts = fleet.status(crate::ledger::now_ms())?;
    let mut items = crate::items::list(fleet.ledger)?;
    crate::items::merge_handoffs(&mut items, &hosts);
    let mut status = Status {
        hosts,
        items,
        queue: crate::work::queue(fleet.ledger)?,
        orphans: crate::sweep::list(fleet.ledger)?,
        sweep: crate::sweep::latest(fleet.ledger)?,
        stopped_activity: std::collections::BTreeMap::new(),
    };
    status.stopped_activity =
        crate::watchdog::stopped_activity(fleet.ledger, &status, crate::ledger::now_ms())?;
    Ok(status)
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

/// Persist selection before reservation so an ambiguous SSH reply repeats the same host and
/// reservation ID. The host owns same-ID reservation replay under its admission lock.
#[derive(Debug, serde::Deserialize, Serialize)]
struct ReservationAttempt {
    host: String,
}

/// A placement for a job request. The request's own creation time rides with the
/// reservation, so the host checks the age before it holds capacity.
pub fn place_with(
    fleet: &Fleet,
    host: Option<&str>,
    id: &str,
    now: u64,
    validity: Option<crate::jobs::Validity>,
    placement: Option<&crate::work::Request>,
) -> Result<Placement> {
    crate::jobs::validate_id(id)?;
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
        place_locked(fleet, host, id, now, &selections, validity, nonjob)
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

fn place_recorded(fleet: &Fleet, request: &crate::work::Request, now: u64) -> Result<Placement> {
    use crate::work::{Attempt, PlacementPhase};
    let attempts = fleet.ledger.child("admissions");
    let file = format!("{}.json", request.id);
    let mut attempt = attempts
        .read::<Attempt>(&file)?
        .ok_or_else(|| crate::unit_transport::conflict("placement admission is missing"))?;
    if attempt.request != serde_json::to_value(request)? {
        return Err(crate::unit_transport::conflict("placement request changed"));
    }
    if fleet
        .ledger
        .child("placement-attempts")
        .read::<serde_json::Value>(&file)?
        .is_some_and(|v| !v.is_null())
    {
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
    let mut reports = fleet.status(now)?;
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
            attempts.write(&file, &attempt)?;
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
            attempts.write(&file, &attempt)?;
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
        attempts.write(&file, &attempt)?;
        if let Some(report) = reports.iter_mut().find(|r| r.host.id == envelope.host.id) {
            report.headroom = outcome.headroom;
        }
        candidates.retain(|c| c.host != envelope.host.id);
    }
    full(reports, pinned)
}

fn place_locked(
    fleet: &Fleet,
    host: Option<&str>,
    id: &str,
    now: u64,
    selections: &crate::ledger::Ledger,
    validity: Option<crate::jobs::Validity>,
    nonjob: bool,
) -> Result<Placement> {
    let pinned = host.map(|h| fleet.find(h)).transpose()?;
    let file = format!("{id}.json");
    let saved = selections
        .read::<Option<ReservationAttempt>>(&file)?
        .flatten();
    if let Some(saved) = &saved {
        let selected = fleet.find(&saved.host)?;
        if pinned.is_some_and(|h| h.id != selected.id) {
            return Err(crate::unit_transport::conflict("reservation host changed"));
        }
    }
    // A saved selection binds host identity, not the lifetime of its hold. Only the host's
    // admission lock can replay a live hold or admit the same ID again after it expires.
    let pinned = saved
        .as_ref()
        .map(|s| fleet.find(&s.host))
        .transpose()?
        .or(pinned);
    let mut reports = fleet.status(now)?;
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
    let mut retry_host = saved.map(|s| s.host);
    while let Some(selected) = retry_host
        .take()
        .or_else(|| plan::choose(&candidates).map(|c| c.host.clone()))
    {
        let host = fleet.find(&selected)?;
        selections.write(
            &file,
            &Some(ReservationAttempt {
                host: host.id.clone(),
            }),
        )?;
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
        selections.write(&file, &Option::<ReservationAttempt>::None)?;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostState {
    Eligible,
    /// Reachable, but admission would refuse new work (` ▲`).
    NoHeadroom,
    /// Not reachable, or its service is down (` ?`).
    Unreachable,
}

impl HostState {
    pub fn of(host: &HostReport) -> Self {
        if !host.reachable || host.service == "down" {
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
#[derive(Debug, Clone, PartialEq, Eq)]
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

pub fn line(status: &Status) -> String {
    format!(
        "{}\n",
        status
            .hosts
            .iter()
            .map(|host| {
                let brief = HostBrief::of(host);
                if brief.measured {
                    format!("{} {}{}", brief.label, brief.load, brief.state.marker())
                } else {
                    format!("{} ?", brief.label)
                }
            })
            .collect::<Vec<_>>()
            .join(" · ")
    )
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn summary(host: &HostReport) -> String {
    let details = host.probe.as_ref().map_or_else(
        || "mem ? disk ? agents ?".into(),
        |p| {
            let gib = 1024.0 * 1024.0 * 1024.0;
            let mut states = std::collections::BTreeMap::new();
            for agent in &p.agent_panes {
                let state = crate::lifecycle::of_agent(
                    agent,
                    if host.reachable && host.service == "up" {
                        crate::lifecycle::Host::Up
                    } else {
                        crate::lifecycle::Host::Unavailable
                    },
                )
                .state;
                *states.entry(state.as_str()).or_insert(0) += 1;
            }
            format!(
                "mem {:.1}/{:.1}GiB disk {:.0}% free agents {} ({})",
                p.memory.used_bytes as f64 / gib,
                p.memory.total_bytes as f64 / gib,
                p.disk_free_percent,
                p.agents.total,
                states
                    .iter()
                    .map(|(state, n)| format!("{} {n}", one_line(state)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        },
    );
    let brief = HostBrief::of(host);
    format!(
        "{} {}{} {details} {}{}",
        brief.label,
        brief.load,
        brief.state.marker(),
        host.condition(),
        host.sampled_ms.map_or_else(String::new, |sampled| format!(
            " (snapshot {} ago)",
            crate::watchdog::age(crate::ledger::now_ms().saturating_sub(sampled))
        ))
    )
}

/// The fleet brief for `status --brief`: every host, the counts, and the last pass's findings.
pub fn brief(status: &Status) -> String {
    let mut rows = fleet(status);
    if let Some(sweep) = &status.sweep
        && (!sweep.applied.failures.is_empty()
            || !sweep.applied.repositories.is_empty()
            || !sweep.applied.unlinked.is_empty())
    {
        rows.push(format!("Cleanup at {} ms: {} failures, {} repository warnings, {} held worktrees. Run `dogsled status` for details.", sweep.finished_ms, sweep.applied.failures.len(), sweep.applied.repositories.len(), sweep.applied.unlinked.len()));
    }
    format!("{}\n", rows.join("\n"))
}

/// The host rows and the work counts, without cleanup findings.
pub fn fleet(status: &Status) -> Vec<String> {
    // Keep every host even when the fleet exceeds the prompt's line budget.
    // Larger fleets share rows; no host disappears into an omitted count.
    let mut rows = vec![String::new(); status.hosts.len().min(5)];
    let count = rows.len();
    for (i, host) in status.hosts.iter().enumerate() {
        let row = &mut rows[i % count];
        if !row.is_empty() {
            row.push_str(" · ");
        }
        row.push_str(&summary(host));
    }
    let finished = status
        .items
        .iter()
        .filter(|i| crate::items::finished(i, &status.hosts))
        .count();
    let orphans = if status.orphans.is_empty() {
        String::new()
    } else {
        format!(
            " · {} orphaned",
            status
                .orphans
                .iter()
                .filter(|orphan| orphan.status == crate::sweep::CustodyState::Orphaned)
                .count()
        )
    };
    rows.push(format!(
        "{} open items{}{orphans} · {} queued · {} stalls. Run `dogsled status` for details.",
        status
            .items
            .iter()
            .filter(|i| i.closed_ms.is_none())
            .count(),
        if finished == 0 {
            String::new()
        } else {
            format!(" ({finished} finished)")
        },
        status.queue.iter().filter(|t| t.state == "pending").count(),
        status
            .hosts
            .iter()
            .filter_map(|h| h.probe.as_ref())
            .flat_map(|p| &p.agent_panes)
            .filter(|a| a.stalled_since_ms.is_some())
            .count()
    ));
    rows
}

/// What the status says about a child the sweep keeps: it is orphaned and unclaimed, or its
/// custody cannot be proven.
fn orphan_state(orphan: &crate::sweep::Orphan) -> &'static str {
    match orphan.status {
        crate::sweep::CustodyState::Orphaned if orphan.claimed => "orphaned (claimed)",
        crate::sweep::CustodyState::Orphaned => "orphaned (unclaimed)",
        crate::sweep::CustodyState::Unknown => "unknown",
    }
}

fn item_lines(status: &Status, lines: &mut Vec<String>) {
    for item in &status.items {
        if let Some(branch) = &item.branch
            && !branch.landed
        {
            lines.push(format!(
                "{}: branch {} +{} to land",
                item.name.as_deref().unwrap_or(&item.pane),
                branch.name,
                branch.ahead
            ));
        }
        if item.closed_ms.is_some() {
            continue;
        }
        lines.push(format!(
            "owner {}: {} {} on {} ({}){}",
            item.owner_pane,
            item.kind,
            item.id,
            item.host,
            item.pane,
            if let Some(orphan) = status.orphans.iter().find(|o| o.item == item.id) {
                format!(" {}: {}", orphan_state(orphan), one_line(&orphan.reason))
            } else if crate::items::finished(item, &status.hosts) {
                format!(
                    " finished; run {}",
                    crate::items::close_command(item, &status.hosts)
                )
            } else {
                String::new()
            }
        ));
    }
}

pub fn render(status: &Status) -> String {
    let mut lines = Vec::new();
    if let Some(sweep) = &status.sweep {
        lines.push(format!(
            "Last cleanup: {} ms (started {} ms)",
            sweep.finished_ms, sweep.started_ms
        ));
        lines.extend(sweep.applied.results.iter().map(|r| format!("  {r}")));
        lines.extend(
            sweep
                .applied
                .skipped
                .iter()
                .map(|s| format!("  skipped: {s}")),
        );
        lines.extend(
            sweep
                .applied
                .failures
                .iter()
                .map(|f| format!("  failed: {f}")),
        );
        lines.extend(
            sweep
                .applied
                .repositories
                .iter()
                .map(|w| format!("  warning: {}: {}", w.id, w.detail)),
        );
        lines.extend(
            sweep
                .applied
                .unlinked
                .iter()
                .map(|h| format!("  hold: {}: {}", h.id, h.detail)),
        );
    } else {
        lines.push("Last cleanup: none".into());
    }
    for host in &status.hosts {
        lines.push(summary(host));
        if let Some(reason) = host
            .headroom
            .as_ref()
            .and_then(|headroom| headroom.reason.as_deref())
        {
            lines.push(format!("  {reason}"));
        }
        if let Some(error) = &host.error {
            lines.push(format!("  {error}"));
        }
        for job in &host.jobs {
            lines.push(format!(
                "  job {} {} exit={} {}",
                job.id,
                job.state,
                job.exit.map_or_else(|| "-".into(), |code| code.to_string()),
                one_line(&job.cmd.join(" "))
            ));
        }
        if let Some(probe) = &host.probe {
            for agent in &probe.agent_panes {
                // The lifecycle owner publishes the state beside the raw terminal state.
                // The status row shows the derived state, so a Herdr `done` never reads
                // as a finished handoff.
                let state = crate::lifecycle::of_agent(
                    agent,
                    if host.reachable && host.service == "up" {
                        crate::lifecycle::Host::Up
                    } else {
                        crate::lifecycle::Host::Unavailable
                    },
                )
                .state
                .as_str();
                lines.push(format!(
                    "  {} {} {}{}",
                    agent.pane,
                    state,
                    agent.name.as_deref().unwrap_or(""),
                    agent.stalled_since_ms.map_or_else(String::new, |_| format!(
                        " stalled {}",
                        crate::watchdog::age(
                            host.sampled_ms
                                .unwrap_or(0)
                                .saturating_sub(agent.transcript_modified.unwrap_or(0))
                        )
                    ))
                ));
            }
            if !probe.reservations.is_empty() {
                lines.push(format!("  {} reservations", probe.reservations.len()));
            }
        }
    }
    item_lines(status, &mut lines);
    for ticket in status.queue.iter().filter(|t| t.state == "pending") {
        lines.push(format!(
            "queued {} owner {}",
            ticket.id, ticket.request.owner
        ));
    }
    format!("{}\n", lines.join("\n"))
}
