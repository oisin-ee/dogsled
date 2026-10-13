//! Local service steps and reservation ownership. No SSH enters this module.

use crate::config::Thresholds;
use crate::error::Result;
use crate::ledger::{Ledger, Reservation};
use crate::plan::{Headroom, evaluate};
use crate::probe::{self, Probe};
use crate::shell::Shell;
use serde::{Deserialize, Serialize};

pub const STALE_MS: u64 = 30_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub sampled_ms: u64,
    pub probe: Probe,
    #[serde(default)]
    pub jobs: Vec<crate::jobs::Job>,
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

#[derive(Debug)]
pub struct Node<'a> {
    pub shell: &'a dyn Shell,
    pub ledger: &'a Ledger,
    pub thresholds: Thresholds,
    pub home: &'a str,
}

impl Node<'_> {
    /// One service cycle. Future watchdog and admission steps belong after
    /// publication, rather than in fleet reads or transport.
    pub fn tick(&self, now: u64) -> Result<()> {
        let mut probe = probe::local(self.shell, self.home)?;
        self.ledger.with_lock("host", || {
            probe.reservations = self.live_reservations(now)?;
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
            // Agent discovery cannot retire reservations: load1 lags new work.
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
