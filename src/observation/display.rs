//! Status display facts are classified once by the producer. Formatting uses only these
//! facts and the reader's clock; it never calls lifecycle, settlement or eligibility rules.

use crate::commands::{HostBrief, Status};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Display {
    hosts: Vec<Host>,
    counts: Counts,
    holds: Vec<crate::holds::Hold>,
    sweep: Option<Sweep>,
    items: Vec<Item>,
    queue: Vec<Queued>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Host {
    id: String,
    brief: HostBrief,
    condition: String,
    sampled_ms: Option<u64>,
    metrics: Option<Metrics>,
    states: BTreeMap<String, usize>,
    agents: Vec<Agent>,
    jobs: Vec<crate::jobs::Job>,
    reservations: usize,
    lanes: Vec<crate::tables::ServiceLane>,
    reason: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Metrics {
    memory: crate::probe::Memory,
    disk_free_percent: f64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Sweep {
    started_ms: u64,
    finished_ms: u64,
    applied: SweepRows,
    notice: Option<[usize; 3]>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SweepRows {
    results: Vec<String>,
    skipped: Vec<String>,
    failures: Vec<String>,
    repositories: Vec<crate::cleanup::Entry>,
    unlinked: Vec<crate::cleanup::Entry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Agent {
    pane: String,
    state: String,
    name: String,
    stalled_ms: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Counts {
    open: usize,
    finished: usize,
    orphans: Option<usize>,
    queued: usize,
    stalls: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct Item {
    branch: Option<Branch>,
    owner: Option<Owner>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Branch {
    name: String,
    branch: String,
    ahead: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Owner {
    owner: String,
    kind: String,
    id: String,
    host: String,
    pane: String,
    orphan: Option<(String, String)>,
    close_command: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Queued {
    id: String,
    owner: String,
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Display {
    /// Producer-only capture: all classifications and row selection happen here.
    pub fn capture(status: &Status) -> Self {
        let hosts = status.hosts.iter().map(Host::capture).collect();
        let counts = Counts::capture(status);
        let items = status
            .items
            .iter()
            .map(|item| Item {
                branch: item
                    .branch
                    .as_ref()
                    .filter(|branch| !branch.landed)
                    .map(|branch| Branch {
                        name: item.name.clone().unwrap_or_else(|| item.pane.clone()),
                        branch: branch.name.clone(),
                        ahead: branch.ahead,
                    }),
                owner: item.closed_ms.is_none().then(|| {
                    let orphan = status
                        .orphans
                        .iter()
                        .find(|orphan| orphan.item == item.id)
                        .map(|orphan| {
                            let state = match orphan.status {
                                crate::sweep::CustodyState::Orphaned if orphan.claimed => {
                                    "orphaned (claimed)"
                                }
                                crate::sweep::CustodyState::Orphaned => "orphaned (unclaimed)",
                                crate::sweep::CustodyState::Unknown => "unknown",
                            };
                            (state.to_owned(), orphan.reason.clone())
                        });
                    let close_command = (orphan.is_none()
                        && crate::items::finished(item, &status.hosts))
                    .then(|| crate::items::close_command(item, &status.hosts));
                    Owner {
                        owner: item.owner_pane.clone(),
                        kind: item.kind.clone(),
                        id: item.id.clone(),
                        host: item.host.clone(),
                        pane: item.pane.clone(),
                        orphan,
                        close_command,
                    }
                }),
            })
            .collect();
        let sweep = status.sweep.as_ref().map(|sweep| Sweep {
            started_ms: sweep.started_ms,
            finished_ms: sweep.finished_ms,
            notice: (!sweep.applied.failures.is_empty()
                || !sweep.applied.repositories.is_empty()
                || !sweep.applied.unlinked.is_empty())
            .then_some([
                sweep.applied.failures.len(),
                sweep.applied.repositories.len(),
                sweep.applied.unlinked.len(),
            ]),
            applied: SweepRows {
                results: sweep.applied.results.clone(),
                skipped: sweep.applied.skipped.clone(),
                failures: sweep.applied.failures.clone(),
                repositories: sweep.applied.repositories.clone(),
                unlinked: sweep.applied.unlinked.clone(),
            },
        });
        Self {
            hosts,
            counts,
            holds: status.holds.clone(),
            sweep,
            items,
            queue: status
                .queue
                .iter()
                .filter(|ticket| ticket.state == "pending")
                .map(|ticket| Queued {
                    id: ticket.id.clone(),
                    owner: ticket.request.owner.clone(),
                })
                .collect(),
        }
    }

    pub fn line(&self) -> String {
        format!(
            "{}\n",
            self.hosts
                .iter()
                .map(|host| {
                    let brief = &host.brief;
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

    pub(crate) fn fleet(&self, now: u64) -> Vec<String> {
        let mut rows = vec![String::new(); self.hosts.len().min(5)];
        let count = rows.len();
        for (i, host) in self.hosts.iter().enumerate() {
            let row = &mut rows[i % count];
            if !row.is_empty() {
                row.push_str(" · ");
            }
            row.push_str(&host.summary(now));
        }
        rows.extend(self.hosts.iter().flat_map(Host::lane_failures));
        let c = &self.counts;
        let finished = if c.finished == 0 {
            String::new()
        } else {
            format!(" ({} finished)", c.finished)
        };
        let orphans = c
            .orphans
            .map_or_else(String::new, |count| format!(" · {count} orphaned"));
        rows.push(format!("{} open items{finished}{orphans} · {} queued · {} stalls. Run `dogsled status` for details.", c.open, c.queued, c.stalls));
        rows
    }

    pub fn brief(&self, now: u64) -> String {
        let mut rows = self.fleet(now);
        if let Some(sweep) = &self.sweep
            && let Some([failures, repositories, unlinked]) = sweep.notice
        {
            rows.push(format!("Cleanup at {} ms: {} failures, {} repository warnings, {} held worktrees. Run `dogsled status` for details.", sweep.finished_ms, failures, repositories, unlinked));
        }
        format!("{}\n", rows.join("\n"))
    }

    pub fn render(&self, now: u64) -> String {
        let mut lines = self.cleanup();
        for host in &self.hosts {
            lines.push(host.summary(now));
            lines.extend(host.lane_failures());
            if let Some(reason) = &host.reason {
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
            for agent in &host.agents {
                lines.push(format!(
                    "  {} {} {}{}",
                    agent.pane,
                    agent.state,
                    agent.name,
                    agent.stalled_ms.map_or_else(String::new, |ms| format!(
                        " stalled {}",
                        crate::watchdog::age(ms)
                    ))
                ));
            }
            if host.reservations != 0 {
                lines.push(format!("  {} reservations", host.reservations));
            }
        }
        for item in &self.items {
            if let Some(branch) = &item.branch {
                lines.push(format!(
                    "{}: branch {} +{} to land",
                    branch.name, branch.branch, branch.ahead
                ));
            }
            if let Some(owner) = &item.owner {
                let suffix = if let Some((state, reason)) = &owner.orphan {
                    format!(" {state}: {}", one_line(reason))
                } else {
                    owner
                        .close_command
                        .as_ref()
                        .map_or_else(String::new, |command| format!(" finished; run {command}"))
                };
                lines.push(format!(
                    "owner {}: {} {} on {} ({}){suffix}",
                    owner.owner, owner.kind, owner.id, owner.host, owner.pane
                ));
            }
        }
        for ticket in &self.queue {
            lines.push(format!("queued {} owner {}", ticket.id, ticket.owner));
        }
        format!("{}\n", lines.join("\n"))
    }
}

impl Counts {
    fn capture(status: &Status) -> Self {
        Self {
            open: status
                .items
                .iter()
                .filter(|item| item.closed_ms.is_none())
                .count(),
            finished: status
                .items
                .iter()
                .filter(|item| crate::items::finished(item, &status.hosts))
                .count(),
            orphans: (!status.orphans.is_empty()).then(|| {
                status
                    .orphans
                    .iter()
                    .filter(|orphan| orphan.status == crate::sweep::CustodyState::Orphaned)
                    .count()
            }),
            queued: status
                .queue
                .iter()
                .filter(|ticket| ticket.state == "pending")
                .count(),
            stalls: status
                .hosts
                .iter()
                .filter_map(|host| host.probe.as_ref())
                .flat_map(|probe| &probe.agent_panes)
                .filter(|agent| agent.stalled_since_ms.is_some())
                .count(),
        }
    }
}

impl Display {
    fn cleanup(&self) -> Vec<String> {
        let mut lines = crate::holds::render(&self.holds);
        if let Some(sweep) = &self.sweep {
            lines.push(format!(
                "Last cleanup: {} ms (started {} ms)",
                sweep.finished_ms, sweep.started_ms
            ));
            lines.extend(
                sweep
                    .applied
                    .results
                    .iter()
                    .map(|result| format!("  {result}")),
            );
            lines.extend(
                sweep
                    .applied
                    .skipped
                    .iter()
                    .map(|skipped| format!("  skipped: {skipped}")),
            );
            lines.extend(
                sweep
                    .applied
                    .failures
                    .iter()
                    .map(|failure| format!("  failed: {failure}")),
            );
            lines.extend(
                sweep
                    .applied
                    .repositories
                    .iter()
                    .map(|warning| format!("  warning: {}: {}", warning.id, warning.detail)),
            );
            lines.extend(
                sweep
                    .applied
                    .unlinked
                    .iter()
                    .map(|hold| format!("  hold: {}: {}", hold.id, hold.detail)),
            );
        } else {
            lines.push("Last cleanup: none".into());
        }
        lines
    }
}

impl Host {
    fn capture(host: &crate::hosts::HostReport) -> Self {
        let mut states = BTreeMap::new();
        let agents = host.probe.as_ref().map_or_else(Vec::new, |probe| {
            probe
                .agent_panes
                .iter()
                .map(|agent| {
                    let state = crate::lifecycle::of_agent(
                        agent,
                        if host.reachable && host.service == "up" {
                            crate::lifecycle::Host::Up
                        } else {
                            crate::lifecycle::Host::Unavailable
                        },
                    )
                    .state
                    .as_str()
                    .to_owned();
                    *states.entry(state.clone()).or_insert(0) += 1;
                    Agent {
                        pane: agent.pane.clone(),
                        state,
                        name: agent.name.clone().unwrap_or_default(),
                        stalled_ms: agent.stalled_since_ms.map(|_| {
                            host.sampled_ms
                                .unwrap_or(0)
                                .saturating_sub(agent.transcript_modified.unwrap_or(0))
                        }),
                    }
                })
                .collect()
        });
        Host {
            id: host.host.id.clone(),
            brief: HostBrief::of(host),
            condition: host.condition(),
            sampled_ms: host.sampled_ms,
            metrics: host.probe.as_ref().map(|probe| Metrics {
                memory: probe.memory.clone(),
                disk_free_percent: probe.disk_free_percent,
            }),
            agents,
            states,
            jobs: host.jobs.clone(),
            reservations: host
                .probe
                .as_ref()
                .map_or(0, |probe| probe.reservations.len()),
            lanes: host
                .service_lanes
                .iter()
                .filter(|lane| lane.error.is_some())
                .cloned()
                .collect(),
            reason: host
                .headroom
                .as_ref()
                .and_then(|headroom| headroom.reason.clone()),
            error: host.error.clone(),
        }
    }

    fn summary(&self, now: u64) -> String {
        let details = self.metrics.as_ref().map_or_else(
            || "mem ? disk ? agents ?".into(),
            |metrics| {
                let gib = 1024.0 * 1024.0 * 1024.0;
                format!(
                    "mem {:.1}/{:.1}GiB disk {:.0}% free agents {} ({})",
                    metrics.memory.used_bytes as f64 / gib,
                    metrics.memory.total_bytes as f64 / gib,
                    metrics.disk_free_percent,
                    self.agents.len(),
                    self.states
                        .iter()
                        .map(|(state, count)| format!("{} {count}", one_line(state)))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
        );
        format!(
            "{} {}{} {details} {}{}",
            self.brief.label,
            self.brief.load,
            self.brief.state.marker(),
            self.condition,
            self.sampled_ms.map_or_else(String::new, |sampled| format!(
                " (snapshot {} ago)",
                crate::watchdog::age(now.saturating_sub(sampled))
            ))
        )
    }

    fn lane_failures(&self) -> impl Iterator<Item = String> + '_ {
        self.lanes.iter().map(|lane| {
            let since = lane
                .since_ms
                .and_then(|ms| chrono::DateTime::from_timestamp_millis(i64::try_from(ms).ok()?))
                .map_or_else(
                    || "unknown".into(),
                    |time| time.format("%H:%M UTC").to_string(),
                );
            format!(
                "{} service: {} lane failing since {since} (x{}): {}",
                one_line(&self.brief.label),
                one_line(&lane.lane),
                lane.repeats,
                one_line(lane.error.as_deref().unwrap_or_default())
            )
        })
    }
}
