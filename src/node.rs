//! Local service steps and reservation ownership. No SSH enters this module.

use crate::config::Thresholds;
use crate::error::Result;
use crate::ledger::{Ledger, Reservation};
use crate::plan::{Headroom, evaluate};
use crate::probe::{self, Probe};
use crate::shell::Shell;
use serde::{Deserialize, Serialize};

pub const STALE_MS: u64 = 30_000;
/// Probe samples older than this are dropped from `samples.jsonl`.
pub const SAMPLE_KEEP_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// One line of `samples.jsonl`: what admission saw at each tick, so the
/// thresholds can be re-scored on weeks of data instead of minutes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub ts_ms: u64,
    pub cores: u32,
    pub busy: Option<f64>,
    pub load1: f64,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub pressure_level: Option<u8>,
    pub disk_free_percent: f64,
    pub reservations: usize,
    pub live_agents: usize,
    pub free_cores: f64,
    pub eligible: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub sampled_ms: u64,
    pub probe: Probe,
    #[serde(default)]
    pub jobs: Vec<crate::jobs::Job>,
}

/// Live host response. Snapshot fields stay at the top level for older readers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotReport {
    #[serde(flatten)]
    pub snapshot: Option<Snapshot>,
    /// Absent on peers predating lock-based liveness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<bool>,
    /// Host-owned results survive the child pane and checkout.
    #[serde(default)]
    pub handoffs: Vec<crate::done::Mark>,
    /// The job validity protocol this host speaks. A peer without it cannot run a job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_validity: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement_replay: Option<u32>,
    /// Read-only ownership reports from this host's stores.
    #[serde(default)]
    pub holds: Vec<crate::holds::Hold>,
}

impl Snapshot {
    pub fn fresh(&self, now: u64) -> bool {
        now.saturating_sub(self.sampled_ms) <= STALE_MS
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReserveOutcome {
    pub reserved: bool,
    pub headroom: Option<Headroom>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<PlacementReply>,
}

pub const PLACEMENT_VERSION: u32 = 2;

/// Acknowledged replay carries the origin's immutable grant, not a fresh capacity request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacementReplay {
    #[serde(flatten)]
    pub envelope: PlacementReservation,
    pub expected_grant: Option<PlacementGrant>,
}

impl PlacementReplay {
    pub fn validate(&self, id: &str) -> Result<()> {
        self.envelope.validate(id)?;
        if let Some(grant) = &self.expected_grant {
            PlacementReply {
                envelope: self.envelope.clone(),
                grant: Some(grant.clone()),
            }
            .validate(&self.envelope, true)?;
        }
        Ok(())
    }
}

/// One complete origin expectation. Neither transport nor recovery may reconstruct it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementReservation {
    pub version: u32,
    pub request: serde_json::Value,
    pub host: crate::hosts::Host,
    pub reservation: String,
}

impl PlacementReservation {
    pub fn validate(&self, id: &str) -> Result<()> {
        let request: crate::work::Request = serde_json::from_value(self.request.clone())?;
        crate::jobs::validate_id(id)?;
        if self.version != PLACEMENT_VERSION
            || self.reservation != id
            || request.id != id
            || !matches!(request.action, crate::work::Action::Place)
            || self.host.id.is_empty()
            || self.host.target.as_deref() == Some("")
        {
            return Err(crate::unit_transport::conflict(
                "placement reservation identity or protocol changed",
            ));
        }
        Ok(())
    }
}

/// Retained until R1 can expire it safely. Existence proves the original capacity decision;
/// materialization never permits refreshing or recreating an expired hold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementGrant {
    pub version: u32,
    pub envelope: PlacementReservation,
    pub reservation: Reservation,
    pub headroom: Headroom,
    pub materialized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacementReply {
    pub envelope: PlacementReservation,
    pub grant: Option<PlacementGrant>,
}

impl PlacementReply {
    pub fn validate(&self, envelope: &PlacementReservation, reserved: bool) -> Result<()> {
        envelope.validate(&envelope.reservation)?;
        if self.envelope != *envelope
            || self.grant.is_some() != reserved
            || self.grant.as_ref().is_some_and(|grant| {
                grant.version != PLACEMENT_VERSION
                    || grant.envelope != *envelope
                    || grant.reservation.id != envelope.reservation
                    || !grant.materialized
                    || !grant.headroom.eligible
            })
        {
            return Err(crate::unit_transport::conflict(
                "placement grant does not match admission",
            ));
        }
        Ok(())
    }
}

/// Busy fraction between two Linux `/proc/stat` readings. `None` without a
/// previous reading, or when no tick passed (same reading twice).
#[must_use]
pub fn busy_since(probe: &Probe, previous: Option<&Probe>) -> Option<f64> {
    let now = probe.cpu_ticks?;
    let then = previous?.cpu_ticks?;
    let total = now.total.checked_sub(then.total).filter(|t| *t > 0)?;
    let busy = now.busy.checked_sub(then.busy)?;
    Some((busy as f64 / total as f64).clamp(0.0, 1.0))
}

#[derive(Debug)]
pub struct Node<'a> {
    pub shell: &'a dyn Shell,
    pub ledger: &'a Ledger,
    pub thresholds: Thresholds,
    pub home: &'a str,
    pub cleanup: &'a crate::config::Cleanup,
}

impl Node<'_> {
    pub fn release_references(
        &self,
        certificate: &crate::launch::ReferenceRelease,
    ) -> Result<crate::launch::ReferenceReleaseAck> {
        crate::launch::release_references(self.ledger, self.shell, certificate)
    }

    pub fn snapshot(&self) -> Result<SnapshotReport> {
        Ok(SnapshotReport {
            snapshot: self.ledger.read("host.json")?,
            running: Some(self.ledger.locked("service")?),
            handoffs: crate::done::list(self.ledger)?,
            job_validity: Some(crate::jobs::VALIDITY_PROTOCOL_VERSION),
            placement_replay: Some(PLACEMENT_VERSION),
            holds: crate::holds::local(
                self.ledger,
                crate::hosts::LOCAL,
                crate::ledger::now_ms(),
                self.thresholds.reservation_ttl_ms,
            )?,
        })
    }

    /// Publication never waits for prompts, notifications, or cleanup.
    pub fn tick(&self, now: u64) -> Result<()> {
        let mut probe = observed_probe(self.ledger, self.shell, self.home)?;
        self.ledger.with_lock("host", || {
            let previous: Option<Snapshot> = self.ledger.read("host.json")?;
            crate::watchdog::observe(&mut probe, previous.as_ref().map(|s| &s.probe), now);
            if probe.busy.is_none() {
                probe.busy = busy_since(&probe, previous.as_ref().map(|s| &s.probe));
            }
            probe.reservations = self.live_reservations(now)?;
            let headroom = evaluate(&probe, &self.thresholds);
            self.append_sample(&probe, &headroom, now)?;
            self.ledger.write(
                "host.json",
                &Snapshot {
                    sampled_ms: now,
                    probe,
                    jobs: crate::jobs::list(self.ledger)?,
                },
            )
        })
    }

    fn append_sample(&self, probe: &Probe, headroom: &Headroom, now: u64) -> Result<()> {
        let sample = Sample {
            ts_ms: now,
            cores: probe.cores,
            busy: probe.busy,
            load1: probe.load1,
            memory_used_bytes: probe.memory.used_bytes,
            memory_total_bytes: probe.memory.total_bytes,
            pressure_level: probe.memory.pressure_level,
            disk_free_percent: probe.disk_free_percent,
            reservations: probe.reservations.len(),
            live_agents: headroom.live_agents,
            free_cores: headroom.headroom,
            eligible: headroom.eligible,
        };
        self.ledger
            .append_line("samples.jsonl", &sample, now.saturating_sub(SAMPLE_KEEP_MS))
    }

    fn live_reservations(&self, now: u64) -> Result<Vec<Reservation>> {
        let reservations = self
            .ledger
            .reservations(now, self.thresholds.reservation_ttl_ms)?;
        self.ledger.write("placements.json", &reservations)?;
        Ok(reservations)
    }

    /// Preparation fencing precedes the short capacity transaction. No lifecycle work
    /// runs while capacity is locked, and a retired request cannot repair a delayed grant.
    pub fn reserve_placement(&self, replay: &PlacementReplay, now: u64) -> Result<ReserveOutcome> {
        let envelope = &replay.envelope;
        replay.validate(&envelope.reservation)?;
        crate::work::launch_test_barrier("PLACEMENT_BEFORE_GATE")?;
        self.ledger
            .child("preparation-receipts")
            .with_lock(&envelope.reservation, || {
                crate::prepare::disposition::allow_preparation(self.ledger, &envelope.reservation)?;
                crate::work::launch_test_barrier("PLACEMENT_ENTERED_GATE")?;
                self.ledger
                    .with_lock("host", || self.placement_locked(replay, now))
            })
    }

    fn placement_locked(&self, replay: &PlacementReplay, now: u64) -> Result<ReserveOutcome> {
        let envelope = &replay.envelope;
        let grants = self.ledger.child("placement-grants");
        let file = format!("{}.json", envelope.reservation);
        let saved = grants.read::<PlacementGrant>(&file)?;
        if let Some(expected) = &replay.expected_grant {
            let grant = saved.as_ref().ok_or_else(|| {
                crate::unit_transport::conflict("placement grant missing; no new capacity")
            })?;
            if grant != expected {
                return Err(crate::unit_transport::conflict(
                    "placement grant identity changed",
                ));
            }
        }
        let raw: Vec<Reservation> = self.ledger.read("placements.json")?.unwrap_or_default();
        let mut grant = if let Some(grant) = saved {
            if grant.version != PLACEMENT_VERSION
                || grant.envelope != *envelope
                || grant.reservation.id != envelope.reservation
                || !grant.headroom.eligible
                || raw
                    .iter()
                    .filter(|hold| hold.id == envelope.reservation)
                    .count()
                    > 1
                || raw
                    .iter()
                    .any(|hold| hold.id == envelope.reservation && *hold != grant.reservation)
            {
                return Err(crate::unit_transport::conflict(
                    "placement grant identity changed",
                ));
            }
            grant
        } else {
            // A recordless same-ID hold is not authority to adopt somebody else's grant.
            if raw.iter().any(|hold| hold.id == envelope.reservation) {
                return Err(crate::unit_transport::conflict(
                    "placement hold has no grant evidence",
                ));
            }
            let reservations = self.live_reservations(now)?;
            let mut snapshot = self
                .ledger
                .read::<Snapshot>("host.json")?
                .filter(|s| s.fresh(now));
            let headroom = snapshot.as_mut().map(|s| {
                s.probe.reservations = reservations;
                evaluate(&s.probe, &self.thresholds)
            });
            let headroom = match headroom {
                Some(headroom) if headroom.eligible => headroom,
                headroom => {
                    return Ok(ReserveOutcome {
                        reserved: false,
                        headroom,
                        placement: Some(PlacementReply {
                            envelope: envelope.clone(),
                            grant: None,
                        }),
                    });
                }
            };
            let grant = PlacementGrant {
                version: PLACEMENT_VERSION,
                envelope: envelope.clone(),
                reservation: Reservation {
                    id: envelope.reservation.clone(),
                    created_ms: now,
                },
                headroom,
                materialized: false,
            };
            grants.write(&file, &grant)?;
            crate::unit_request::crash_point("placement-grant-intent");
            grant
        };
        if grant.materialized {
            self.live_reservations(now)?;
        } else {
            let mut holds = self
                .ledger
                .reservations(now, self.thresholds.reservation_ttl_ms)?;
            // Preserve the original timestamp even if publication was interrupted before
            // acknowledgement. An expired original hold cannot become live again.
            if now.saturating_sub(grant.reservation.created_ms) < self.thresholds.reservation_ttl_ms
                && !holds.iter().any(|hold| hold.id == grant.reservation.id)
            {
                holds.push(grant.reservation.clone());
            }
            self.ledger.write("placements.json", &holds)?;
            crate::unit_request::crash_point("placement-hold-published");
            grant.materialized = true;
            grants.write(&file, &grant)?;
        }
        Ok(ReserveOutcome {
            reserved: true,
            headroom: Some(grant.headroom.clone()),
            placement: Some(PlacementReply {
                envelope: envelope.clone(),
                grant: Some(grant),
            }),
        })
    }

    pub fn reserve(
        &self,
        id: &str,
        now: u64,
        job_validity: Option<crate::jobs::Validity>,
    ) -> Result<ReserveOutcome> {
        crate::jobs::validate_id(id)?;
        self.ledger.with_lock("host", || {
            if self
                .ledger
                .child("placement-grants")
                .read::<serde_json::Value>(&format!("{id}.json"))?
                .is_some()
            {
                return Err(crate::unit_transport::conflict(
                    "placement reservation requires its replay envelope",
                ));
            }
            // The host checks the request's age before it reads or rewrites any capacity
            // record, and before it replays a hold. A request that may not start new
            // execution never takes, keeps, or confirms a reservation.
            if let Some(validity) = &job_validity {
                crate::jobs::check_validity(self.ledger, Some(validity))?;
            }
            let reservations = self.live_reservations(now)?;
            // The caller may have lost a successful SSH reply. Replaying a live hold is
            // not new admission: do not append it, refresh its TTL, or require a fresh probe.
            if reservations.iter().any(|r| r.id == id) {
                return Ok(ReserveOutcome {
                    reserved: true,
                    headroom: None,
                    placement: None,
                });
            }
            let Some(mut snapshot) = self
                .ledger
                .read::<Snapshot>("host.json")?
                .filter(|s| s.fresh(now))
            else {
                return Ok(ReserveOutcome {
                    reserved: false,
                    headroom: None,
                    placement: None,
                });
            };
            // Agent discovery cannot retire reservations: a new agent's first
            // build is not in the busy fraction until it runs.
            snapshot.probe.reservations = reservations;
            let headroom = evaluate(&snapshot.probe, &self.thresholds);
            if headroom.eligible {
                snapshot.probe.reservations.push(Reservation {
                    id: id.into(),
                    created_ms: now,
                });
                self.ledger
                    .write("placements.json", &snapshot.probe.reservations)?;
                self.ledger.write("host.json", &snapshot)?;
            }
            Ok(ReserveOutcome {
                reserved: headroom.eligible,
                headroom: Some(headroom),
                placement: None,
            })
        })
    }
}

/// Observe the same lifecycle facts for publication and host-side close verification.
pub fn observed_probe(ledger: &Ledger, shell: &dyn Shell, home: &str) -> Result<Probe> {
    let mut probe = probe::local(shell, home)?;
    // Transcript and Git reads stay outside the lock, so a reservation never waits on them.
    // Reading the last snapshot without the lock is safe: writes replace it atomically.
    let last: Option<Snapshot> = ledger.read("host.json")?;
    crate::facts::observe(shell, &mut probe, last.as_ref().map(|s| &s.probe));
    // Publish launch progress before deriving states, including pending and contended launches.
    for agent in &mut probe.agent_panes {
        let progress = crate::launch::progress(ledger, &agent.pane)?;
        agent.launch_progress = match progress {
            crate::launch::LaunchProgress::None => None,
            progress => Some(progress.as_str().to_owned()),
        };
    }
    crate::done::observe(ledger, &mut probe.agent_panes)?;
    Ok(probe)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{AgentCounts, CpuTicks, Memory};

    fn probe(ticks: Option<CpuTicks>) -> Probe {
        Probe {
            host: "local".into(),
            label: "local".into(),
            load1: 0.0,
            cores: 4,
            cpu_ticks: ticks,
            busy: None,
            memory: Memory::default(),
            disk_free_percent: 50.0,
            agents: AgentCounts::default(),
            agent_panes: Vec::new(),
            reservations: Vec::new(),
            panes: Vec::new(),
        }
    }

    #[test]
    fn same_id_reservation_is_atomic_and_does_not_extend_or_duplicate_the_hold() {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let shell = crate::shell::Fake::new();
        let policy = crate::config::Policy {
            memory_reserve_bytes: 0,
            ..crate::config::Policy::default()
        };
        let node = Node {
            shell: &shell,
            ledger: &ledger,
            thresholds: Thresholds::from(&policy),
            home: "/",
            cleanup: &policy.cleanup,
        };
        ledger
            .write(
                "host.json",
                &Snapshot {
                    sampled_ms: 100,
                    probe: Probe {
                        busy: Some(0.0),
                        ..probe(None)
                    },
                    jobs: Vec::new(),
                },
            )
            .unwrap();
        std::thread::scope(|scope| {
            let calls: Vec<_> = (0..4)
                .map(|_| scope.spawn(|| node.reserve("same", 100, None)))
                .collect();
            for call in calls {
                assert!(call.join().unwrap().unwrap().reserved);
            }
        });
        let holds: Vec<Reservation> = ledger.read("placements.json").unwrap().unwrap();
        assert_eq!(holds.len(), 1);
        assert_eq!(holds[0].id, "same");
        assert_eq!(holds[0].created_ms, 100);
        // An existing hold is not new admission and does not need a fresh probe.
        assert!(
            node.reserve("same", 100 + STALE_MS + 1, None)
                .unwrap()
                .reserved
        );
        let holds: Vec<Reservation> = ledger.read("placements.json").unwrap().unwrap();
        assert_eq!(holds.len(), 1);
        assert_eq!(holds[0].created_ms, 100);
        assert!(
            !node
                .reserve("another", 100 + STALE_MS + 1, None)
                .unwrap()
                .reserved
        );
        assert!(
            !node
                .reserve("same", 100 + policy.reservation_ttl_ms, None)
                .unwrap()
                .reserved
        );
        std::fs::remove_dir_all(&ledger.dir).unwrap();
    }

    #[test]
    fn busy_since_is_the_tick_delta_ratio() {
        let then = probe(Some(CpuTicks {
            busy: 100,
            total: 400,
        }));
        let now = probe(Some(CpuTicks {
            busy: 160,
            total: 480,
        }));
        assert_eq!(busy_since(&now, Some(&then)), Some(0.75));
        assert_eq!(busy_since(&now, None), None);
        assert_eq!(busy_since(&now, Some(&now)), None);
        assert_eq!(busy_since(&probe(None), Some(&then)), None);
        // A counter that went backwards (reboot) yields no sample rather than a wrong one.
        assert_eq!(busy_since(&then, Some(&now)), None);
    }
}
