//! User-facing status formatting and placement. No agent spawning or queue.

use crate::error::Result;
use crate::hosts::{Fleet, Host, HostReport};
use crate::ledger::new_id;
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
}

pub fn status(fleet: &Fleet<'_>) -> Result<Status> {
    Ok(Status {
        hosts: fleet.status(crate::ledger::now_ms())?,
        items: crate::items::list(fleet.ledger)?,
        queue: crate::work::queue(fleet.ledger)?,
        orphans: crate::sweep::list(fleet.ledger)?,
        sweep: crate::sweep::latest(fleet.ledger)?,
    })
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

pub fn place(fleet: &Fleet, host: Option<&str>, now: u64) -> Result<Placement> {
    let pinned = host.map(|h| fleet.find(h)).transpose()?;
    let mut reports = fleet.status(now)?;
    let mut candidates: Vec<_> = reports
        .iter()
        .filter(|r| pinned.is_none_or(|h| h.id == r.host.id))
        .filter_map(|r| r.headroom.clone())
        .collect();
    while let Some(choice) = plan::choose(&candidates) {
        let host = fleet.find(&choice.host)?;
        let id = new_id();
        let outcome = fleet.reserve(host, &id, now)?;
        if outcome.reserved {
            let prepared = Prepared::default();
            return Ok(Placement::Placed {
                host: host.clone(),
                reservation: id,
                prepared,
            });
        }
        if let Some(report) = reports.iter_mut().find(|r| r.host.id == host.id) {
            report.headroom = outcome.headroom;
        }
        candidates.retain(|c| c.host != host.id);
    }
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

fn marker(host: &HostReport) -> &'static str {
    if !host.reachable || host.service == "down" {
        " ?"
    } else if !host.headroom.as_ref().is_some_and(|h| h.eligible) {
        " ▲"
    } else {
        ""
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
                if host.probe.is_none() {
                    format!("{} ?", one_line(&host.host.label))
                } else {
                    format!(
                        "{} {}{}",
                        one_line(&host.host.label),
                        load(host),
                        marker(host)
                    )
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
            format!(
                "mem {:.1}/{:.1}GiB disk {:.0}% free agents {} ({})",
                p.memory.used_bytes as f64 / gib,
                p.memory.total_bytes as f64 / gib,
                p.disk_free_percent,
                p.agents.total,
                p.agents
                    .by_status
                    .iter()
                    .map(|(state, n)| format!("{} {n}", one_line(state)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        },
    );
    format!(
        "{} {}{} {details} service: {}",
        one_line(&host.host.label),
        load(host),
        marker(host),
        host.service
    )
}

pub fn brief(status: &Status) -> String {
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
        format!(" · {} orphaned", status.orphans.len())
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
    if let Some(sweep) = &status.sweep
        && (!sweep.applied.failures.is_empty() || !sweep.applied.repositories.is_empty())
    {
        rows.push(format!("Cleanup at {} ms: {} failures, {} repository warnings. Run `dogsled status` for details.", sweep.finished_ms, sweep.applied.failures.len(), sweep.applied.repositories.len()));
    }
    format!("{}\n", rows.join("\n"))
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
                lines.push(format!(
                    "  {} {} {}{}",
                    agent.pane,
                    agent.state,
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
    for item in status.items.iter().filter(|i| i.closed_ms.is_none()) {
        lines.push(format!(
            "owner {}: {} {} on {} ({}){}",
            item.owner_pane,
            item.kind,
            item.id,
            item.host,
            item.pane,
            if let Some(orphan) = status.orphans.iter().find(|o| o.item == item.id) {
                format!(" orphaned: {}", one_line(&orphan.reason))
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
    for ticket in status.queue.iter().filter(|t| t.state == "pending") {
        lines.push(format!(
            "queued {} owner {}",
            ticket.id, ticket.request.owner
        ));
    }
    format!("{}\n", lines.join("\n"))
}
