//! Local service steps and reservation ownership. No SSH enters this module.

use crate::config::{Policy, Thresholds};
use crate::error::Result;
use crate::ledger::{Ledger, Reservation};
use crate::plan::{Headroom, evaluate};
use crate::probe::{self, Probe};
use crate::shell::Shell;
use serde::{Deserialize, Serialize};

pub const STALE_MS: u64 = 30_000;
/// Probe samples older than this are dropped from the `samples` table.
pub const SAMPLE_KEEP_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// One row of the `samples` table: what admission saw at each tick, so the
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

pub(crate) const SAMPLES_SCHEMA: &str = r"CREATE TABLE samples (
 ts_ms INTEGER NOT NULL CHECK (ts_ms >= 0),
 cores INTEGER NOT NULL CHECK (cores >= 0),
 busy REAL,
 load1 REAL NOT NULL,
 memory_used_bytes INTEGER NOT NULL CHECK (memory_used_bytes >= 0),
 memory_total_bytes INTEGER NOT NULL CHECK (memory_total_bytes >= 0),
 pressure_level INTEGER CHECK (pressure_level BETWEEN 0 AND 255),
 disk_free_percent REAL NOT NULL,
 reservations INTEGER NOT NULL CHECK (reservations >= 0),
 live_agents INTEGER NOT NULL CHECK (live_agents >= 0),
 free_cores REAL NOT NULL,
 eligible INTEGER NOT NULL CHECK (eligible IN (0,1))
) STRICT;
CREATE INDEX samples_ts ON samples(ts_ms);
";

fn sql_count(value: impl TryInto<i64>) -> Result<i64> {
    value
        .try_into()
        .map_err(|_| crate::error::Error::Invalid("sample value out of range".into()))
}

fn stored<T: TryFrom<i64>>(value: i64) -> rusqlite::Result<T> {
    T::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, value))
}

/// Append one sample and drop samples older than `keep_from_ms`.
fn record_sample(ledger: &Ledger, sample: &Sample, keep_from_ms: u64) -> Result<()> {
    let row = (
        sql_count(sample.ts_ms)?,
        sql_count(sample.cores)?,
        sql_count(sample.memory_used_bytes)?,
        sql_count(sample.memory_total_bytes)?,
        sql_count(sample.reservations)?,
        sql_count(sample.live_agents)?,
    );
    let keep_from = sql_count(keep_from_ms)?;
    crate::store::write(ledger, |tx| {
        tx.execute(
            "INSERT INTO samples (ts_ms, cores, busy, load1, memory_used_bytes,
             memory_total_bytes, pressure_level, disk_free_percent, reservations, live_agents,
             free_cores, eligible) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            rusqlite::params![
                row.0,
                row.1,
                sample.busy,
                sample.load1,
                row.2,
                row.3,
                sample.pressure_level,
                sample.disk_free_percent,
                row.4,
                row.5,
                sample.free_cores,
                sample.eligible,
            ],
        )?;
        tx.execute("DELETE FROM samples WHERE ts_ms < ?1", [keep_from])?;
        Ok(())
    })
}

/// Every retained sample, oldest first.
pub(crate) fn samples(ledger: &Ledger) -> Result<Vec<Sample>> {
    crate::store::read(ledger, |tx| {
        Ok(tx
            .prepare("SELECT * FROM samples ORDER BY ts_ms, rowid")?
            .query_map([], |row| {
                Ok(Sample {
                    ts_ms: stored(row.get("ts_ms")?)?,
                    cores: stored(row.get("cores")?)?,
                    busy: row.get("busy")?,
                    load1: row.get("load1")?,
                    memory_used_bytes: stored(row.get("memory_used_bytes")?)?,
                    memory_total_bytes: stored(row.get("memory_total_bytes")?)?,
                    pressure_level: row.get("pressure_level")?,
                    disk_free_percent: row.get("disk_free_percent")?,
                    reservations: stored(row.get("reservations")?)?,
                    live_agents: stored(row.get("live_agents")?)?,
                    free_cores: row.get("free_cores")?,
                    eligible: row.get("eligible")?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    })
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
    /// Current failures, read from the host-owned typed lane table.
    #[serde(default)]
    pub service_lanes: Vec<crate::tables::ServiceLane>,
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
    #[serde(default)]
    pub request_validity: Option<u32>,
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

pub const PLACEMENT_VERSION: u32 = 3;

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

/// Durable origin-terminal proof for one exact grant; never part of grant identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrantCompletion {
    pub version: u32,
    pub grant: PlacementGrant,
    pub admission: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct GrantLifecycle {
    pub(crate) proof: GrantCompletion,
    pub(crate) finished_ms: Option<u64>,
}

/// One complete origin expectation. Neither transport nor recovery may reconstruct it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementReservation {
    pub version: u32,
    #[serde(deserialize_with = "placement_request")]
    pub request: serde_json::Value,
    pub host: crate::hosts::Host,
    pub reservation: String,
}

fn placement_request<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> std::result::Result<serde_json::Value, D::Error> {
    let request = crate::work::Request::deserialize(decoder)?;
    serde_json::to_value(request).map_err(serde::de::Error::custom)
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

fn validate_expected_grant(replay: &PlacementReplay, saved: Option<&PlacementGrant>) -> Result<()> {
    if let Some(expected) = &replay.expected_grant {
        let grant = saved.ok_or_else(|| {
            crate::unit_transport::conflict("placement grant missing; no new capacity")
        })?;
        if grant != expected {
            return Err(crate::unit_transport::conflict(
                "placement grant identity changed",
            ));
        }
    }
    Ok(())
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
    pub repos: &'a [crate::config::Repo],
}

impl<'a> Node<'a> {
    pub fn new(
        shell: &'a dyn Shell,
        ledger: &'a Ledger,
        policy: &'a Policy,
        home: &'a str,
    ) -> Self {
        Self {
            shell,
            ledger,
            thresholds: Thresholds::from(policy),
            home,
            cleanup: &policy.cleanup,
            repos: &policy.repos,
        }
    }

    pub fn release_references(
        &self,
        certificate: &crate::launch::ReferenceRelease,
    ) -> Result<crate::launch::ReferenceReleaseAck> {
        crate::launch::release_references(self.ledger, self.shell, certificate)
    }

    pub fn snapshot(&self) -> Result<SnapshotReport> {
        crate::observation::read::local(
            &crate::observation::ipc::Client::at(&self.ledger.dir).current()?,
            "host",
        )
    }

    /// Collection never waits for prompts, notifications, or cleanup. Return the exact
    /// persisted sample so publication cannot accidentally reread a newer reservation write.
    #[cfg(test)]
    pub(crate) fn tick(&self, now: u64) -> Result<Snapshot> {
        self.tick_with_terminals(now, probe::terminals(self.shell)?)
    }

    pub(crate) fn tick_with_terminals(
        &self,
        now: u64,
        terminals: probe::Terminals,
    ) -> Result<Snapshot> {
        let probe = probe::local_with_terminals(self.shell, self.home, terminals)?;
        let mut probe = observe_probe(self.ledger, self.shell, probe)?;
        self.ledger.with_lock("host", || {
            let previous: Option<Snapshot> = crate::host_store::snapshot(self.ledger)?;
            crate::watchdog::observe(&mut probe, previous.as_ref().map(|s| &s.probe), now);
            if probe.busy.is_none() {
                probe.busy = busy_since(&probe, previous.as_ref().map(|s| &s.probe));
                if probe.busy.is_none()
                    && let Some(previous) = &previous
                    && probe
                        .cpu_ticks
                        .zip(previous.probe.cpu_ticks)
                        .is_some_and(|(current, baseline)| current.total == baseline.total)
                {
                    probe.busy = previous.probe.busy;
                    probe.cpu_ticks = previous.probe.cpu_ticks;
                }
            }
            probe.reservations = self.live_reservations(now)?;
            let headroom = evaluate(&probe, &self.thresholds);
            self.append_sample(&probe, &headroom, now)?;
            let snapshot = Snapshot {
                sampled_ms: now,
                probe,
                jobs: crate::jobs::list(self.ledger)?,
            };
            crate::host_store::put_snapshot(self.ledger, &snapshot)?;
            Ok(snapshot)
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
        record_sample(self.ledger, &sample, now.saturating_sub(SAMPLE_KEEP_MS))
    }

    fn live_reservations(&self, now: u64) -> Result<Vec<Reservation>> {
        let reservations = self
            .ledger
            .reservations(now, self.thresholds.reservation_ttl_ms)?;
        crate::job_store::put_reservations(self.ledger, &reservations)?;
        Ok(reservations)
    }

    /// Preparation fencing precedes the short capacity transaction. No lifecycle work
    /// runs while capacity is locked, and a retired request cannot repair a delayed grant.
    pub fn grant_completion(&self, id: &str) -> Result<Option<u64>> {
        crate::jobs::validate_id(id)?;
        let Some(lifecycle) = crate::preparation_store::lifecycles::get(self.ledger, id)? else {
            return Ok(None);
        };
        if crate::preparation_store::grants::get(self.ledger, id)?.as_ref()
            != Some(&lifecycle.proof.grant)
        {
            return Err(crate::unit_transport::conflict(
                "grant completion identity changed",
            ));
        }
        crate::work::validate_placement_completion(&lifecycle.proof)?;
        crate::jobs::completion_time(self.ledger, lifecycle.finished_ms, None, true)
    }

    /// Only exact durable origin-terminal proof permits lifecycle completion.
    pub fn finish_placement(&self, proof: &GrantCompletion) -> Result<bool> {
        let id = &proof.grant.envelope.reservation;
        crate::jobs::validate_id(id)?;
        if proof.version != 1 {
            return Err(crate::unit_transport::conflict(
                "unsupported grant completion",
            ));
        }
        crate::work::validate_placement_completion(proof)?;
        self.ledger.child("preparation-receipts").with_lock(id, || {
            self.ledger.with_lock("host", || {
                let saved = crate::preparation_store::grants::get(self.ledger, id)?;
                if saved.as_ref() != Some(&proof.grant) {
                    return Err(crate::unit_transport::conflict(
                        "grant completion identity changed",
                    ));
                }
                if let Some(saved) = crate::preparation_store::lifecycles::get(self.ledger, id)? {
                    if saved.proof != *proof || saved.finished_ms.is_none() {
                        return Err(crate::unit_transport::conflict(
                            "grant completion provenance changed",
                        ));
                    }
                    self.grant_completion(id)?;
                } else {
                    let mut finished_ms = None;
                    crate::jobs::finish_once(self.ledger, &mut finished_ms, None)?;
                    crate::preparation_store::lifecycles::put(
                        self.ledger,
                        id,
                        &GrantLifecycle {
                            proof: proof.clone(),
                            finished_ms,
                        },
                    )?;
                }
                Ok(true)
            })
        })
    }

    pub fn reserve_placement(&self, replay: &PlacementReplay, now: u64) -> Result<ReserveOutcome> {
        let envelope = &replay.envelope;
        replay.validate(&envelope.reservation)?;
        crate::work::launch_test_barrier("PLACEMENT_BEFORE_GATE")?;
        self.ledger
            .child("preparation-receipts")
            .with_lock(&envelope.reservation, || {
                crate::prepare::disposition::allow_preparation(self.ledger, &envelope.reservation)?;
                crate::work::launch_test_barrier("PLACEMENT_ENTERED_GATE")?;
                let request: crate::work::Request =
                    serde_json::from_value(envelope.request.clone())?;
                crate::jobs::receive_request_validity(self.ledger, &request.id, request.validity)?;
                self.ledger
                    .with_lock("host", || self.placement_locked(replay, now))
            })
    }

    fn placement_locked(&self, replay: &PlacementReplay, now: u64) -> Result<ReserveOutcome> {
        let envelope = &replay.envelope;
        let request: crate::work::Request = serde_json::from_value(envelope.request.clone())?;
        crate::work::check_request_validity(self.ledger, &request)?;
        let saved = crate::preparation_store::grants::get(self.ledger, &envelope.reservation)?;
        validate_expected_grant(replay, saved.as_ref())?;
        let raw = crate::job_store::reservations(self.ledger)?;
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
            let mut snapshot = crate::host_store::snapshot(self.ledger)?.filter(|s| s.fresh(now));
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
            crate::preparation_store::grants::put(self.ledger, &envelope.reservation, &grant)?;
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
            crate::job_store::put_reservations(self.ledger, &holds)?;
            crate::unit_request::crash_point("placement-hold-published");
            grant.materialized = true;
            crate::preparation_store::grants::put(self.ledger, &envelope.reservation, &grant)?;
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
        owner: &str,
        owner_host: &str,
    ) -> Result<ReserveOutcome> {
        crate::jobs::validate_id(id)?;
        if owner.is_empty() || owner_host.is_empty() {
            return Err(crate::error::Error::Invalid(
                "reservation requires owner and owner host".into(),
            ));
        }
        self.ledger.with_lock("host", || {
            crate::jobs::reject_unstarted(self.ledger, id)?;
            // Publish request authority before the hold, for jobs too: a later checkout or
            // launch for this id matches it, and never sees a hold without authority.
            crate::jobs::receive_request_validity(self.ledger, id, job_validity)?;
            crate::job_store::remember_owner(self.ledger, id, owner, Some(owner_host))?;
            if crate::preparation_store::grants::get(self.ledger, id)?.is_some() {
                return Err(crate::unit_transport::conflict(
                    "placement reservation requires its replay envelope",
                ));
            }
            // The host checks the request's age before it reads or rewrites any capacity
            // record, and before it replays a hold. A request that may not start new
            // execution never takes, keeps, or confirms a reservation.
            crate::jobs::check_validity(self.ledger, job_validity.as_ref())?;
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
            let Some(mut snapshot) =
                crate::host_store::snapshot(self.ledger)?.filter(|s| s.fresh(now))
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
                crate::job_store::put_reservations(self.ledger, &snapshot.probe.reservations)?;
                crate::host_store::put_snapshot(self.ledger, &snapshot)?;
            }
            Ok(ReserveOutcome {
                reserved: headroom.eligible,
                headroom: Some(headroom),
                placement: None,
            })
        })
    }
}

/// The caller holds the preparation gate. The host store validates the exact
/// durable origin-terminal proof before forgetting a placement grant.
pub(crate) fn expire_grant(ledger: &Ledger, id: &str, now: u64) -> Result<bool> {
    ledger.with_lock("host", || {
        let Some(grant) = crate::preparation_store::grants::get(ledger, id)? else {
            return Ok(true);
        };
        let Some(lifecycle) = crate::preparation_store::lifecycles::get(ledger, id)? else {
            return Ok(false);
        };
        if lifecycle.proof.grant != grant {
            return Err(crate::unit_transport::conflict(
                "grant completion identity changed",
            ));
        }
        crate::work::validate_placement_completion(&lifecycle.proof)?;
        if crate::jobs::completion_time(ledger, lifecycle.finished_ms, None, true)?
            .is_none_or(|finished| now.saturating_sub(finished) <= crate::jobs::RECORD_KEEP_MS)
        {
            return Ok(false);
        }
        if !crate::work::references_released(ledger, id)?
            || !crate::prepare::retention_ready(ledger, id, now)?
        {
            return Ok(false);
        }
        crate::preparation_store::grants::remove(ledger, id)?;
        crate::preparation_store::lifecycles::remove(ledger, id)?;
        Ok(true)
    })
}

pub(crate) fn expire_grants(ledger: &Ledger, now: u64) -> Result<()> {
    for id in crate::preparation_store::lifecycles::ids(ledger)? {
        let id = id.as_str();
        let result = ledger.child("preparation-receipts").with_lock(id, || {
            if expire_grant(ledger, id, now)? {
                crate::prepare::expire_record(ledger, id, now)?;
            }
            Ok(())
        });
        if let Err(error) = result {
            eprintln!("dogsled placement expiry: {id}: {error}");
        }
    }
    Ok(())
}

/// Observe the same lifecycle facts for publication and host-side close verification.
pub fn observed_probe(ledger: &Ledger, shell: &dyn Shell, home: &str) -> Result<Probe> {
    observe_probe(ledger, shell, probe::local(shell, home)?)
}

fn observe_probe(ledger: &Ledger, shell: &dyn Shell, mut probe: Probe) -> Result<Probe> {
    // Transcript and Git reads stay outside the lock, so a reservation never waits on them.
    // Reading the last snapshot without the lock is safe: writes replace it atomically.
    let last: Option<Snapshot> = crate::host_store::snapshot(ledger)?;
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
    let leads = crate::lead_store::list(ledger)?;
    probe.agents = probe::AgentCounts::from_agents(probe.agent_panes.iter().filter(|agent| {
        !leads
            .iter()
            .any(|lead| crate::leads::matches_agent(lead, agent))
    }));
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

    fn sample(ts_ms: u64) -> Sample {
        Sample {
            ts_ms,
            cores: 4,
            busy: Some(0.25),
            load1: 1.5,
            memory_used_bytes: 3,
            memory_total_bytes: 8,
            pressure_level: Some(1),
            disk_free_percent: 50.0,
            reservations: 2,
            live_agents: 1,
            free_cores: 2.5,
            eligible: true,
        }
    }

    #[test]
    fn zero_delta_capture_keeps_busy_and_cpu_baseline() {
        use crate::config::Policy;
        use crate::shell::Fake;
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let shell = Fake::new();
        shell
            .on("herdr pane list", r#"{"result":{"panes":[]}}"#)
            .on("herdr agent list", r#"{"result":{"agents":[]}}"#)
            .on("read /proc/stat", "cpu 20 0 0 80 0 0 0 0 0 0")
            .on("read /proc/loadavg", "0.5 0.2 0.1 1/10 42")
            .on(
                "read /proc/meminfo",
                "MemTotal: 100 kB\nMemAvailable: 75 kB",
            )
            .on("df -Pk /home", "FS 1000 500 400 60% /");
        let policy = Policy::default();
        let node = Node::new(&shell, &ledger, &policy, "/home");
        assert_eq!(node.tick(100).unwrap().probe.busy, None);
        shell.on("read /proc/stat", "cpu 30 0 0 90 0 0 0 0 0 0");
        let measured = node.tick(200).unwrap();
        assert_eq!(measured.probe.busy, Some(0.5));
        // Total ticks have not advanced, even though individual counters moved.
        shell.on("read /proc/stat", "cpu 31 0 0 89 0 0 0 0 0 0");
        let retained = node.tick(250).unwrap();
        assert_eq!(retained.probe.busy, measured.probe.busy);
        assert_eq!(retained.sampled_ms, 250);
        assert_eq!(retained.probe.cpu_ticks, measured.probe.cpu_ticks);
        shell.on("read /proc/stat", "cpu 40 0 0 100 0 0 0 0 0 0");
        let next = node.tick(300).unwrap();
        assert_eq!(next.probe.busy, Some(0.5));
        assert_eq!(next.sampled_ms, 300);
        std::fs::remove_dir_all(&ledger.dir).unwrap();
    }

    #[test]
    fn samples_keep_a_bounded_window_of_typed_rows() {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        for ts in [1_000, 2_000, 3_000] {
            record_sample(&ledger, &sample(ts), 0).unwrap();
        }
        record_sample(&ledger, &sample(4_000), 2_000).unwrap();
        let kept = samples(&ledger).unwrap();
        assert_eq!(kept, [sample(2_000), sample(3_000), sample(4_000)]);
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn same_id_reservation_is_atomic_and_does_not_extend_or_duplicate_the_hold() {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let shell = crate::shell::Fake::new();
        let policy = crate::config::Policy {
            memory_reserve_bytes: 0,
            ..crate::config::Policy::default()
        };
        let node = Node::new(&shell, &ledger, &policy, "/");
        crate::host_store::put_snapshot(
            &ledger,
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
        // One request has one creation time; every replay carries the same stamp.
        let validity = Some(crate::jobs::Validity::new(crate::ledger::now_ms()));
        std::thread::scope(|scope| {
            let calls: Vec<_> = (0..4)
                .map(|_| scope.spawn(|| node.reserve("same", 100, validity, "w1:p1", "origin")))
                .collect();
            for call in calls {
                assert!(call.join().unwrap().unwrap().reserved);
            }
        });
        let holds = crate::job_store::reservations(&ledger).unwrap();
        assert_eq!(holds.len(), 1);
        assert_eq!(holds[0].id, "same");
        assert_eq!(holds[0].created_ms, 100);
        // An existing hold is not new admission and does not need a fresh probe.
        assert!(
            node.reserve("same", 100 + STALE_MS + 1, validity, "w1:p1", "origin")
                .unwrap()
                .reserved
        );
        let holds = crate::job_store::reservations(&ledger).unwrap();
        assert_eq!(holds.len(), 1);
        assert_eq!(holds[0].created_ms, 100);
        // Another request carries its own stamp.
        let another = Some(crate::jobs::Validity::new(crate::ledger::now_ms()));
        assert!(
            !node
                .reserve("another", 100 + STALE_MS + 1, another, "w1:p1", "origin")
                .unwrap()
                .reserved
        );
        assert!(
            !node
                .reserve(
                    "same",
                    100 + policy.reservation_ttl_ms,
                    validity,
                    "w1:p1",
                    "origin"
                )
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
