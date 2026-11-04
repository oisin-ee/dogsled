//! Service-lifetime peer workers. A peer has at most one bounded request outstanding.

use super::peers::{self, Ingestion};
use super::{Availability, Gap, PeerSource, Publication};
use crate::config::Policy;
use crate::error::{Error, Result};
use crate::hosts::{Fleet, Host, HostReport};
use crate::ledger::{Ledger, now_ms};
use crate::service::wake::{Dirty, PEERS};
use crate::shell::System;
use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
struct State {
    stopped: bool,
    retry: bool,
    sources: BTreeMap<String, PeerSource>,
    gaps: Vec<Gap>,
    cached: Option<crate::hosts::Cached>,
}

type Shared = Arc<(Mutex<State>, Condvar)>;

struct Worker {
    host: Host,
    state: Shared,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        let (mutex, wake) = &*self.state;
        mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stopped = true;
        wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Default)]
pub(crate) struct Observers {
    hosts: Vec<Host>,
    workers: BTreeMap<String, Worker>,
    discovered: Option<Instant>,
}

impl Observers {
    /// Discovery belongs to the peer cadence, never to local evidence invalidation.
    pub fn discover(&mut self, fleet: &Fleet<'_>, dirty: &Dirty, once: bool) -> Result<()> {
        if self
            .discovered
            .is_some_and(|at| at.elapsed() < Duration::from_secs(10))
        {
            return Ok(());
        }
        self.discovered = Some(Instant::now());
        let discovered =
            Fleet::discover(fleet.shell, fleet.ledger, fleet.policy, fleet.home.clone())?;
        self.hosts = discovered.hosts;
        self.workers.retain(|id, worker| {
            self.hosts
                .iter()
                .any(|host| &host.id == id && host == &worker.host)
        });
        for host in self.hosts.iter().filter(|host| !host.is_local()) {
            if self.workers.contains_key(&host.id) {
                continue;
            }
            let host = host.clone();
            let ledger = fleet.ledger.clone();
            let policy = fleet.policy.clone();
            let home = fleet.home.clone();
            let dirty = dirty.clone();
            let state = Arc::new((Mutex::new(State::default()), Condvar::new()));
            let shared = Arc::clone(&state);
            let target = host.clone();
            let thread = std::thread::spawn(move || {
                poll(&target, ledger, &policy, home, &shared, &dirty, once);
            });
            self.workers.insert(
                host.id.clone(),
                Worker {
                    host,
                    state,
                    thread: Some(thread),
                },
            );
        }
        if once {
            for worker in self.workers.values_mut() {
                if let Some(thread) = worker.thread.take() {
                    thread
                        .join()
                        .map_err(|_| Error::Invalid("peer observation worker failed".into()))?;
                }
            }
        }
        Ok(())
    }

    pub fn retry(&self, hosts: std::collections::BTreeSet<String>) {
        for host in hosts {
            if let Some(worker) = self.workers.get(&host) {
                worker
                    .state
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .retry = true;
                worker.state.1.notify_all();
            }
        }
    }

    pub fn compose(
        &self,
        fleet: &Fleet<'_>,
        input: &mut Publication,
        now: u64,
        mut reports: Vec<HostReport>,
    ) -> Result<()> {
        for host in self.hosts.iter().filter(|host| !host.is_local()) {
            let state = self
                .workers
                .get(&host.id)
                .map(|worker| {
                    worker
                        .state
                        .0
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone()
                })
                .unwrap_or_default();
            let report = state.cached.map_or_else(
                || fleet.read_host(host, now),
                |cached| fleet.cached_report(host, cached, now),
            );
            let mut source = state.sources.get(&host.id).cloned().unwrap_or(PeerSource {
                epoch: String::new(),
                local_revision: 0,
                received_ms: 0,
                availability: Availability::Unavailable,
            });
            if source.availability == Availability::Available && !report.fresh(now) {
                source.availability = Availability::Stale;
            }
            if source.availability != Availability::Available {
                input.gaps.push(Gap {
                    domain: format!("peer:{}", host.id),
                    availability: source.availability.clone(),
                    detail: report.condition(),
                });
            }
            input.gaps.extend(state.gaps.iter().map(|gap| Gap {
                domain: format!("peer:{}:branches", host.id),
                availability: gap.availability.clone(),
                detail: "peer branch observation failed".into(),
            }));
            input.sources.peers.insert(host.id.clone(), source);
            reports.push(report);
        }
        input
            .views
            .insert("hosts".into(), serde_json::to_value(&reports)?);
        input.views.insert(
            "status".into(),
            serde_json::to_value(super::collect::status(fleet.ledger, reports, now)?)?,
        );
        Ok(())
    }
}

fn poll(
    host: &Host,
    mut ledger: Ledger,
    policy: &Policy,
    home: String,
    shared: &Shared,
    dirty: &Dirty,
    once: bool,
) {
    ledger.budget = crate::budget::Budget::default();
    let shell = System::default();
    let fleet = Fleet {
        shell: &shell,
        ledger: &ledger,
        policy,
        home,
        hosts: vec![host.clone()],
    };
    let mut ingestion = Ingestion::default();
    loop {
        if shared
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stopped
        {
            break;
        }
        let retry = {
            let mut state = shared
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut state.retry)
        };
        if retry {
            ingestion.reset();
        }
        let result = crate::store::observing(|| {
            let now = now_ms();
            let interests = peers::interests(&ledger, &host.id, now)?;
            ingestion.refresh(&fleet, host, now, interests, !once && !retry)
        });
        let mut state = shared
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = state.sources.get(&host.id).cloned();
        state.cached.clone_from(&ingestion.cached);
        let gaps = ingestion.gaps();
        let gaps_changed = state.gaps != gaps;
        state.gaps = gaps;
        if let Some(source) = &ingestion.source {
            state.sources.insert(host.id.clone(), source.clone());
        }
        if result.as_ref().is_ok_and(|changed| *changed)
            || previous.as_ref() != ingestion.source.as_ref()
            || gaps_changed
        {
            dirty.mark(PEERS);
        }
        if let Err(error) = result {
            eprintln!("dogsled peer {}: {error}", host.label);
        }
        if once || state.stopped {
            break;
        }
        // A successful native long-poll already waited. Legacy or failed peers follow
        // the cache/backoff cadence without busy-spinning or accumulating work.
        let delay = if ingestion.source.as_ref().is_some_and(|source| {
            source.availability == Availability::Available && source.epoch != "legacy"
        }) {
            Duration::from_millis(50)
        } else {
            Duration::from_secs(10)
        };
        let (state, _) = shared
            .1
            .wait_timeout_while(state, delay, |state| !state.stopped && !state.retry)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.stopped {
            break;
        }
    }
}
