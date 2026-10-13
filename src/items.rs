//! Owner-host item records and completion reconciliation.

use crate::error::Result;
use crate::hosts::HostReport;
use crate::ledger::Ledger;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub kind: String,
    pub host: String,
    pub pane: String,
    pub owner_pane: String,
    pub name: Option<String>,
    pub created_ms: u64,
    pub closed_ms: Option<u64>,
}

pub fn list(ledger: &Ledger) -> Result<Vec<Item>> {
    Ok(ledger.read("items.json")?.unwrap_or_default())
}

pub fn record(ledger: &Ledger, item: Item) -> Result<()> {
    ledger.with_lock("items", || {
        let mut items = list(ledger)?;
        if !items.iter().any(|i| {
            i.id == item.id || (i.closed_ms.is_none() && i.host == item.host && i.pane == item.pane)
        }) {
            items.push(item);
            ledger.write("items.json", &items)?;
        }
        Ok(())
    })
}

pub fn reconcile(ledger: &Ledger, hosts: &[HostReport], now: u64) -> Result<()> {
    ledger.with_lock("items", || {
        let mut items = list(ledger)?;
        for item in items.iter_mut().filter(|i| i.closed_ms.is_none()) {
            let Some(host) = hosts
                .iter()
                .find(|h| h.host.id == item.host && h.service == "up")
            else {
                continue;
            };
            let Some(probe) = &host.probe else {
                continue;
            };
            // Ignore a snapshot taken before creation: a newly launched pane is not missing.
            if host.sampled_ms.is_none_or(|t| t < item.created_ms) {
                continue;
            }
            let ended = if item.kind == "job" {
                host.jobs
                    .iter()
                    .any(|j| j.id == item.id && j.ended_ms.is_some())
            } else {
                probe
                    .agent_panes
                    .iter()
                    .any(|a| a.pane == item.pane && matches!(a.state.as_str(), "idle" | "done"))
            };
            let gone = !probe.panes.iter().any(|p| p.pane_id == item.pane);
            if ended || gone {
                item.closed_ms = Some(now);
            }
        }
        ledger.write("items.json", &items)
    })
}
