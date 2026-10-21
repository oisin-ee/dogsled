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
    pub fn snapshot(&self) -> Result<SnapshotReport> {
        Ok(SnapshotReport {
            snapshot: self.ledger.read("host.json")?,
            running: Some(self.ledger.locked("service")?),
        })
    }

    /// Publication never waits for prompts, notifications, or cleanup.
    pub fn tick(&self, now: u64) -> Result<()> {
        let mut probe = probe::local(self.shell, self.home)?;
        // Transcript and Git reads stay outside the lock, so a reservation never waits on them.
        // Reading the last snapshot without the lock is safe: writes replace it atomically.
        let last: Option<Snapshot> = self.ledger.read("host.json")?;
        crate::facts::observe(self.shell, &mut probe, last.as_ref().map(|s| &s.probe));
        crate::done::observe(self.ledger, &mut probe.agent_panes)?;
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

    pub fn reserve(&self, id: &str, now: u64) -> Result<ReserveOutcome> {
        self.ledger.with_lock("host", || {
            let Some(mut snapshot) = self
                .ledger
                .read::<Snapshot>("host.json")?
                .filter(|s| s.fresh(now))
            else {
                return Ok(ReserveOutcome {
                    reserved: false,
                    headroom: None,
                });
            };
            // Agent discovery cannot retire reservations: a new agent's first
            // build is not in the busy fraction until it runs.
            snapshot.probe.reservations = self.live_reservations(now)?;
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
            })
        })
    }
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
