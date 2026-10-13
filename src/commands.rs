//! User-facing status formatting and placement. No agent spawning or queue.

use crate::error::Result;
use crate::hook::{self, Prepared};
use crate::hosts::{Fleet, Host, HostReport};
use crate::ledger::new_id;
use crate::plan;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Status {
    pub hosts: Vec<HostReport>,
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

pub fn place(
    fleet: &Fleet,
    host: Option<&str>,
    branch: Option<&str>,
    base: Option<&str>,
    repo: &str,
    now: u64,
) -> Result<Placement> {
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
            let prepared = if let Some(branch) = branch {
                hook::run(fleet, host, repo, branch, base)?
            } else {
                Prepared::default()
            };
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
    host.probe
        .as_ref()
        .map_or_else(|| "-/−".into(), |p| format!("{:.0}/{}", p.load1, p.cores))
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
    rows.push("Run `wrangle status` for details.".into());
    format!("{}\n", rows.join("\n"))
}

pub fn render(status: &Status) -> String {
    let mut lines = Vec::new();
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
        if let Some(probe) = &host.probe {
            for agent in &probe.agent_panes {
                lines.push(format!(
                    "  {} {} {}",
                    agent.pane,
                    agent.state,
                    agent.name.as_deref().unwrap_or("")
                ));
            }
            if !probe.reservations.is_empty() {
                lines.push(format!("  {} reservations", probe.reservations.len()));
            }
        }
    }
    format!("{}\n", lines.join("\n"))
}

pub fn render_place(placement: &Placement) -> String {
    match placement {
        Placement::Placed { host, prepared, .. } => {
            let mut text = format!("{} ({})", host.id, host.label);
            if let Some(path) = &prepared.path {
                text.push('\n');
                text.push_str(path);
            }
            if let Some(pane) = &prepared.pane_id {
                text.push('\n');
                text.push_str(pane);
            }
            format!("{text}\n")
        }
        Placement::Full { .. } => "every eligible host is full\n".into(),
    }
}
