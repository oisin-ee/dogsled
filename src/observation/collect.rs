//! Service-owned local evidence capture. Legacy readers share these capture functions until
//! their reader cuts land; observation clients never call them or reconstruct a publication.

#[path = "catalogue.rs"]
mod catalogue;
#[path = "models.rs"]
mod models;

use super::timing::{Phases, measure};
use super::{Availability, Domains, Gap, Publication, Sources};
use crate::commands::Status;
use crate::error::Result;
use crate::herdr_socket::Client;
use crate::hosts::{Host, HostReport};
use crate::ledger::Ledger;
use crate::node::{Node, Snapshot, SnapshotReport};
use crate::probe::{self, CpuSampler, Terminals};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

/// Capture host-owned facts without probing liveness or rereading the capacity sample.
/// The service supplies its exact new sample; the legacy snapshot adapter supplies its cache.
pub(crate) fn host(
    node: &Node<'_>,
    snapshot: Option<Snapshot>,
    running: bool,
    now: u64,
) -> Result<SnapshotReport> {
    timed_host(node, snapshot, running, now, &mut Vec::new())
}

fn timed_host(
    node: &Node<'_>,
    snapshot: Option<Snapshot>,
    running: bool,
    now: u64,
    timings: &mut Phases,
) -> Result<SnapshotReport> {
    Ok(SnapshotReport {
        service_lanes: measure(timings, "collect.host.service_lanes", || {
            crate::store::read(node.ledger, crate::tables::failing_lanes)
        })?,
        snapshot,
        running: Some(running),
        handoffs: measure(timings, "collect.host.handoffs", || {
            crate::done::list(node.ledger)
        })?,
        job_validity: Some(crate::jobs::VALIDITY_PROTOCOL_VERSION),
        repo_open: Some(crate::checkout::OPEN_VERSION),
        request_validity: Some(crate::jobs::REQUEST_VALIDITY_PROTOCOL_VERSION),
        placement_replay: Some(crate::node::PLACEMENT_VERSION),
        transfer: Some(crate::node::TRANSFER_VERSION),
        holds: measure(timings, "collect.host.holds", || {
            crate::holds::local(
                node.ledger,
                crate::hosts::LOCAL,
                now,
                node.thresholds.reservation_ttl_ms,
            )
        })?,
    })
}

/// Capture and assemble from an already observed host set. No fleet discovery or probing.
/// All store operations end before any publication is serialized or replaced.
pub(crate) fn status(ledger: &Ledger, hosts: Vec<HostReport>, now: u64) -> Result<Status> {
    let items = crate::items::list(ledger)?;
    let status = Status {
        hosts,
        items,
        queue: crate::work::queue(ledger)?,
        orphans: crate::sweep::list(ledger)?,
        sweep: crate::sweep::latest(ledger)?,
        stopped_activity: BTreeMap::new(),
        holds: Vec::new(),
    };
    let mut status = assemble(status);
    status.stopped_activity = crate::watchdog::stopped_activity(ledger, &status, now)?;
    Ok(status)
}

/// Recompute fleet-dependent state from frozen inventory and host-owned facts.
pub(crate) fn assemble(mut status: Status) -> Status {
    crate::items::merge_handoffs(&mut status.items, &status.hosts);
    status.holds = status
        .hosts
        .iter()
        .flat_map(|host| host.holds.clone())
        .collect();
    status.holds.extend(crate::holds::owner(&status));
    // Prefer owner-side evidence when the execution host reports the same branch.
    status.holds.retain(|hold| {
        hold.code != "retained_branch"
            || !status.items.iter().any(|item| {
                hold.request_id.as_deref() == Some(item.id.as_str())
                    && (item.dropped_ms.is_some()
                        || item.branch.as_ref().is_some_and(|branch| branch.landed))
            })
    });
    status.holds.reverse();
    let mut resources = BTreeSet::new();
    status.holds.retain(|hold| {
        resources.insert((hold.host.clone(), hold.code.clone(), hold.resource.clone()))
    });
    status.holds.reverse();
    status
}

fn todos(ledger: &Ledger) -> Result<Vec<crate::board::TodoSections>> {
    crate::board::collect_todos(ledger)?;
    crate::board::cached_todos(ledger)
}

/// Sampling errors retain their original type for the one-shot service's exit contract.
/// Failures in display derivation remain publication gaps, not failed capacity samples.
#[derive(Debug)]
pub(crate) struct Collection {
    pub input: Publication,
    pub sample: Result<()>,
    pub timings: Phases,
}

/// Retained values keep their original observation time when a source fails. Gaps describe
/// the current failure, including a cold failure where there is no value to retain.
#[derive(Default)]
pub(crate) struct Collector {
    terminal_source: TerminalSource,
    cpu: CpuSampler,
    local: Domains,
    measured: BTreeMap<String, u64>,
    gaps: BTreeMap<String, Gap>,
    snapshot: Option<Snapshot>,
    sample_error: Option<String>,
    report: Option<HostReport>,
    worker_clocks: super::owned_work::WorkerClocks,
    catalogue: catalogue::Catalogue,
    models: models::Catalogue,
}

enum TerminalSource {
    Socket(Option<Client>),
    // Existing unit fixtures exercise collection/retention using a scripted Shell.
    #[cfg(test)]
    ScriptedShell,
}

impl Default for TerminalSource {
    fn default() -> Self {
        Self::Socket(None)
    }
}

impl TerminalSource {
    fn capture(
        &mut self,
        shell: &dyn crate::shell::Shell,
        timings: &mut Phases,
    ) -> Result<Terminals> {
        match self {
            Self::Socket(socket) => {
                let client = match socket.take() {
                    Some(client) => client,
                    None => measure(timings, "collect.terminals.discover", || {
                        Client::discover_with(shell, std::env::var_os("HERDR_SOCKET_PATH"))
                    })?,
                };
                let result = probe::socket_terminals(&client, timings);
                // A restarted server can move its socket; resolve the path on the next sample.
                if result.is_ok() {
                    *socket = Some(client);
                }
                result
            }
            #[cfg(test)]
            Self::ScriptedShell => probe::terminals(shell),
        }
    }
}

impl Collector {
    #[cfg(test)]
    pub(super) fn with_scripted_shell() -> Self {
        Self {
            terminal_source: TerminalSource::ScriptedShell,
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(super) fn with_socket(socket: Client) -> Self {
        Self {
            terminal_source: TerminalSource::Socket(Some(socket)),
            ..Self::default()
        }
    }

    pub(crate) fn with_activity(feed: super::live_activity::ActivityFeed) -> Self {
        Self {
            worker_clocks: super::owned_work::WorkerClocks::with_feed(feed),
            ..Self::default()
        }
    }

    pub(super) fn record<T: Serialize>(&mut self, domain: &str, now: u64, value: Result<T>) {
        self.record_value(
            domain,
            now,
            value.and_then(|value| serde_json::to_value(value).map_err(Into::into)),
        );
    }

    fn record_value(&mut self, domain: &str, now: u64, value: Result<serde_json::Value>) {
        match value {
            Ok(value) => {
                self.local.insert(domain.into(), value);
                self.measured.insert(domain.into(), now);
                self.gaps.remove(domain);
            }
            Err(error) => {
                self.gaps.insert(
                    domain.into(),
                    Gap {
                        domain: domain.into(),
                        availability: Availability::Failed,
                        detail: error.to_string(),
                    },
                );
            }
        }
    }

    fn record_holds(&mut self, holds: &[crate::holds::Hold]) {
        self.gaps.retain(|domain, _| !domain.starts_with("holds/"));
        for hold in holds {
            if hold.code == "unreadable"
                || (hold.owner_pane.is_none() && hold.owner == "unrecorded")
            {
                let domain = format!("holds/{}", hold.resource);
                self.gaps.insert(
                    domain.clone(),
                    Gap {
                        domain,
                        availability: Availability::Failed,
                        detail: format!("{}: {}", hold.resource, hold.reason),
                    },
                );
            }
        }
    }

    /// Capture first, then freeze and publish. `host` includes jobs, handoffs, holds,
    /// capabilities, lane diagnostics, agent metadata and checkout/retained-branch facts.
    /// `status` is local-only inventory, not an aggregated fleet projection.
    pub fn collect(&mut self, node: &Node<'_>, now: u64) -> Collection {
        self.refresh(node, now, true)
    }

    /// Peer-only wakes reuse local inputs without touching Git, terminals or the ledger.
    pub fn publication(&self, now: u64) -> Publication {
        Publication {
            published_ms: now,
            sources: Sources {
                local: self.measured.clone(),
                peers: BTreeMap::new(),
            },
            local: self.local.clone(),
            views: Domains::new(),
            gaps: self.gaps.values().cloned().collect(),
        }
    }

    pub fn reports(&self) -> Vec<HostReport> {
        self.report.iter().cloned().collect()
    }

    pub fn panes(&self) -> BTreeSet<String> {
        self.snapshot
            .as_ref()
            .map_or_else(BTreeSet::new, |snapshot| {
                snapshot
                    .probe
                    .panes
                    .iter()
                    .map(|pane| pane.pane_id.clone())
                    .collect()
            })
    }

    fn retained_terminals(&self) -> Result<crate::probe::Terminals> {
        self.local
            .get("terminals")
            .ok_or_else(|| crate::error::Error::Invalid("terminal observation unavailable".into()))
            .and_then(|value| serde_json::from_value(value.clone()).map_err(Into::into))
    }

    fn capture_snapshot(
        &mut self,
        node: &Node<'_>,
        now: u64,
        sample: bool,
        mut terminals: Result<Terminals>,
        timings: &mut Phases,
    ) -> (Result<Snapshot>, Result<Terminals>) {
        let snapshot = if let Some(error) = self.sample_error.as_ref().filter(|_| !sample) {
            Err(crate::error::Error::Invalid(error.clone()))
        } else if let Some(snapshot) = self.snapshot.as_ref().filter(|_| !sample) {
            let mut snapshot = snapshot.clone();
            (|| {
                snapshot.jobs = measure(timings, "collect.snapshot.jobs", || {
                    crate::jobs::list(node.ledger)
                })?;
                snapshot.probe.reservations =
                    measure(timings, "collect.snapshot.reservations", || {
                        node.ledger
                            .reservations(now, node.thresholds.reservation_ttl_ms)
                    })?;
                Ok(snapshot)
            })()
        } else {
            let captured = match terminals {
                Ok(ref terminals) => {
                    node.tick_with_terminals(now, terminals.clone(), &mut self.cpu, timings)
                }
                Err(error) => {
                    terminals = Err(crate::error::Error::Invalid(error.to_string()));
                    Err(error)
                }
            };
            match captured {
                Ok(snapshot) => {
                    self.snapshot = Some(snapshot.clone());
                    self.sample_error = None;
                    Ok(snapshot)
                }
                Err(error) => {
                    self.sample_error = Some(error.to_string());
                    Err(error)
                }
            }
        };
        (snapshot, terminals)
    }

    /// Ledger changes reuse the last terminal/resource sample. The source's `sampled_ms`
    /// stays unchanged; newly persisted jobs, holds, inventory and todos are recaptured.
    pub fn refresh(&mut self, node: &Node<'_>, now: u64, sample: bool) -> Collection {
        let mut timings = Vec::with_capacity(28);
        let started = Instant::now();
        if let Some(catalogue) = self.catalogue.refresh(node.shell, now) {
            self.record("repos", now, Ok(catalogue));
        }
        timings.push(("collect.catalogue", started.elapsed()));
        let started = Instant::now();
        if let Some(models) = self.models.refresh(node.shell, now) {
            self.record("models", now, Ok(models));
        }
        timings.push(("collect.models", started.elapsed()));
        let started = Instant::now();
        let capture_terminals = sample || (self.snapshot.is_none() && self.sample_error.is_none());
        let terminals = if capture_terminals {
            self.terminal_source.capture(node.shell, &mut timings)
        } else {
            self.retained_terminals()
        };
        timings.push(("collect.terminals", started.elapsed()));
        let started = Instant::now();
        let (snapshot, mut terminals) =
            self.capture_snapshot(node, now, sample, terminals, &mut timings);
        timings.push(("collect.snapshot", started.elapsed()));
        let started = Instant::now();
        let (host, sample) = match snapshot {
            Ok(snapshot) => {
                terminals = Ok(crate::probe::Terminals {
                    agents: snapshot.probe.agent_panes.clone(),
                    panes: snapshot.probe.panes.clone(),
                });
                (
                    timed_host(node, Some(snapshot), true, now, &mut timings),
                    Ok(()),
                )
            }
            Err(error) => (
                Err(crate::error::Error::Invalid(error.to_string())),
                Err(error),
            ),
        };
        timings.push(("collect.host", started.elapsed()));
        let started = Instant::now();
        let (headroom, status) = match &host {
            Ok(host) => {
                self.record_holds(&host.holds);
                let headroom = host
                    .snapshot
                    .as_ref()
                    .map(|snapshot| crate::plan::evaluate(&snapshot.probe, &node.thresholds));
                let mut report = crate::hosts::cached_report(
                    &Host::local(),
                    host.clone().into(),
                    now,
                    &node.thresholds,
                );
                // Local refresh preserves the service-owned sample between native ticks.
                report.headroom.clone_from(&headroom);
                self.report = Some(report.clone());
                (Ok(headroom), status(node.ledger, vec![report], now))
            }
            Err(error) => {
                self.unavailable(error);
                let dependency =
                    || crate::error::Error::Invalid(format!("host collection failed: {error}"));
                (Err(dependency()), Err(dependency()))
            }
        };
        timings.push(("collect.status", started.elapsed()));
        let started = Instant::now();
        let todos = todos(node.ledger);
        timings.push(("collect.todos", started.elapsed()));
        let started = Instant::now();
        if capture_terminals {
            self.record("terminals", now, terminals);
        }
        let host = host.and_then(|report| {
            super::owned_work::serialize_host(
                node.ledger,
                &report,
                status.as_ref().ok(),
                todos.as_deref().ok(),
                &mut self.worker_clocks,
            )
        });
        self.record_value("host", now, host);
        self.record("headroom", now, headroom);
        self.record("status", now, status);
        self.record("todos", now, todos);
        self.record(
            "sidebar_preferences",
            now,
            super::sidebar::preferences(node.ledger),
        );
        let input = self.publication(now);
        timings.push(("collect.publication", started.elapsed()));
        Collection {
            input,
            sample,
            timings,
        }
    }

    fn unavailable(&mut self, error: &crate::error::Error) {
        let mut report = self.report.clone().unwrap_or_else(|| HostReport {
            service_lanes: Vec::new(),
            host: Host::local(),
            service: "unavailable".into(),
            reachable: true,
            sampled_ms: None,
            probe: None,
            headroom: None,
            error: None,
            failure: None,
            jobs: Vec::new(),
            handoffs: Vec::new(),
            holds: Vec::new(),
            retry_ms: None,
            job_validity: None,
        });
        report.service = "unavailable".into();
        report.error = Some(format!("host observation unavailable: {error}"));
        if let Some(headroom) = &mut report.headroom {
            headroom.eligible = false;
            headroom.reason.clone_from(&report.error);
        }
        self.report = Some(report);
    }
}
