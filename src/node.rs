//! Local service steps and reservation ownership. No SSH enters this module.

use crate::config::Thresholds;
use crate::error::{Error, Result};
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
            probe.reservations = self.live_reservations(now, &probe.agent_panes)?;
            self.ledger.write(
                "host.json",
                &Snapshot {
                    sampled_ms: now,
                    probe,
                },
            )
        })
    }

    fn live_reservations(&self, now: u64, agents: &[probe::Agent]) -> Result<Vec<Reservation>> {
        let mut reservations = self
            .ledger
            .reservations(now, self.thresholds.reservation_ttl_ms)?;
        reservations.retain(|r| {
            !r.pane
                .as_deref()
                .is_some_and(|pane| agents.iter().any(|a| a.pane == pane))
        });
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
            let agents = probe::agent_list(self.shell)?;
            snapshot.probe.reservations = self.live_reservations(now, &agents)?;
            let headroom = evaluate(&snapshot.probe, &self.thresholds);
            if headroom.eligible {
                snapshot.probe.reservations.push(Reservation {
                    id: id.into(),
                    created_ms: now,
                    pane: None,
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

    pub fn bind(&self, id: &str, pane: &str, now: u64) -> Result<()> {
        self.change(id, now, |reservations| {
            let reservation = reservations
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or_else(|| Error::Invalid(format!("reservation {id} expired")))?;
            reservation.pane = Some(pane.into());
            Ok(())
        })
    }

    pub fn release(&self, id: &str, now: u64) -> Result<()> {
        self.change(id, now, |reservations| {
            reservations.retain(|r| r.id != id);
            Ok(())
        })
    }

    fn change(
        &self,
        _id: &str,
        now: u64,
        f: impl FnOnce(&mut Vec<Reservation>) -> Result<()>,
    ) -> Result<()> {
        self.ledger.with_lock("host", || {
            let mut reservations = self
                .ledger
                .reservations(now, self.thresholds.reservation_ttl_ms)?;
            f(&mut reservations)?;
            self.ledger.write("placements.json", &reservations)?;
            if let Some(mut snapshot) = self.ledger.read::<Snapshot>("host.json")? {
                snapshot.probe.reservations = reservations;
                self.ledger.write("host.json", &snapshot)?;
            }
            Ok(())
        })
    }
}
