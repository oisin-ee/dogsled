//! Service-owned local evidence capture. Legacy readers share these capture functions until
//! their reader cuts land; observation clients never call them or reconstruct a publication.

use super::{Availability, Domains, Gap, Publication, Sources};
use crate::commands::Status;
use crate::error::Result;
use crate::hosts::{Host, HostReport};
use crate::ledger::Ledger;
use crate::node::{Node, Snapshot, SnapshotReport};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Capture host-owned facts without probing liveness or rereading the capacity sample.
/// The service supplies its exact new sample; the legacy snapshot adapter supplies its cache.
pub(crate) fn host(
    node: &Node<'_>,
    snapshot: Option<Snapshot>,
    running: bool,
    now: u64,
) -> Result<SnapshotReport> {
    Ok(SnapshotReport {
        service_lanes: crate::store::read(node.ledger, crate::tables::failing_lanes)?,
        snapshot,
        running: Some(running),
        handoffs: crate::done::list(node.ledger)?,
        job_validity: Some(crate::jobs::VALIDITY_PROTOCOL_VERSION),
        request_validity: Some(crate::jobs::REQUEST_VALIDITY_PROTOCOL_VERSION),
        placement_replay: Some(crate::node::PLACEMENT_VERSION),
        holds: crate::holds::local(
            node.ledger,
            crate::hosts::LOCAL,
            now,
            node.thresholds.reservation_ttl_ms,
        )?,
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
}

/// Retained values keep their original observation time when a source fails. Gaps describe
/// the current failure, including a cold failure where there is no value to retain.
#[derive(Debug, Default)]
pub(crate) struct Collector {
    local: Domains,
    measured: BTreeMap<String, u64>,
    gaps: BTreeMap<String, Gap>,
    snapshot: Option<Snapshot>,
    sample_error: Option<String>,
    report: Option<HostReport>,
}

impl Collector {
    fn record<T: Serialize>(&mut self, domain: &str, now: u64, value: Result<T>) {
        match value.and_then(|value| serde_json::to_value(value).map_err(Into::into)) {
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

    /// Ledger changes reuse the last terminal/resource sample. The source's `sampled_ms`
    /// stays unchanged; newly persisted jobs, holds, inventory and todos are recaptured.
    pub fn refresh(&mut self, node: &Node<'_>, now: u64, sample: bool) -> Collection {
        let snapshot = if let Some(error) = self.sample_error.as_ref().filter(|_| !sample) {
            Err(crate::error::Error::Invalid(error.clone()))
        } else if let Some(snapshot) = self.snapshot.as_ref().filter(|_| !sample) {
            let mut snapshot = snapshot.clone();
            (|| {
                snapshot.jobs = crate::jobs::list(node.ledger)?;
                snapshot.probe.reservations = node
                    .ledger
                    .reservations(now, node.thresholds.reservation_ttl_ms)?;
                Ok(snapshot)
            })()
        } else {
            match node.tick(now) {
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
        let (host, sample) = match snapshot {
            Ok(snapshot) => (host(node, Some(snapshot), true, now), Ok(())),
            Err(error) => (
                Err(crate::error::Error::Invalid(error.to_string())),
                Err(error),
            ),
        };
        let (headroom, status) = match &host {
            Ok(host) => {
                let headroom = host
                    .snapshot
                    .as_ref()
                    .map(|snapshot| crate::plan::evaluate(&snapshot.probe, &node.thresholds));
                let report = HostReport {
                    service_lanes: host.service_lanes.clone(),
                    host: Host::local(),
                    service: "up".into(),
                    reachable: true,
                    sampled_ms: host.snapshot.as_ref().map(|snapshot| snapshot.sampled_ms),
                    probe: host
                        .snapshot
                        .as_ref()
                        .map(|snapshot| snapshot.probe.clone()),
                    headroom: headroom.clone(),
                    error: None,
                    failure: None,
                    jobs: host
                        .snapshot
                        .as_ref()
                        .map_or_else(Vec::new, |s| s.jobs.clone()),
                    handoffs: host.handoffs.clone(),
                    holds: host.holds.clone(),
                    retry_ms: None,
                    job_validity: host.job_validity,
                };
                self.report = Some(report.clone());
                (Ok(headroom), status(node.ledger, vec![report], now))
            }
            Err(error) => {
                let dependency =
                    || crate::error::Error::Invalid(format!("host collection failed: {error}"));
                (Err(dependency()), Err(dependency()))
            }
        };
        let todos = todos(node.ledger);
        self.record("host", now, host);
        self.record("headroom", now, headroom);
        self.record("status", now, status);
        self.record("todos", now, todos);
        Collection {
            input: Publication {
                published_ms: now,
                sources: Sources {
                    local: self.measured.clone(),
                    peers: BTreeMap::new(),
                },
                local: self.local.clone(),
                views: Domains::new(),
                gaps: self.gaps.values().cloned().collect(),
            },
            sample,
        }
    }
}
