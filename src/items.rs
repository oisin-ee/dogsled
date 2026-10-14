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

/// End every open item on `host`'s `pane`, after `wrangle close` removed it.
pub fn close(ledger: &Ledger, host: &str, pane: &str, now: u64) -> Result<usize> {
    ledger.with_lock("items", || {
        let mut items = list(ledger)?;
        let mut count = 0;
        for item in items
            .iter_mut()
            .filter(|i| i.closed_ms.is_none() && i.host == host && i.pane == pane)
        {
            item.closed_ms = Some(now);
            count += 1;
        }
        ledger.write("items.json", &items)?;
        Ok(count)
    })
}

/// An open agent item whose agent is idle or done: its result is ready, and its
/// pane and worktree wait for `wrangle close`.
pub fn finished(item: &Item, hosts: &[HostReport]) -> bool {
    item.closed_ms.is_none()
        && item.kind == "agent"
        && hosts
            .iter()
            .filter(|h| h.host.id == item.host)
            .filter_map(|h| h.probe.as_ref())
            .flat_map(|p| &p.agent_panes)
            .any(|a| a.pane == item.pane && matches!(a.state.as_str(), "idle" | "done"))
}

/// The exact command that closes `item`, naming its host by label.
pub fn close_command(item: &Item, hosts: &[HostReport]) -> String {
    let host = hosts
        .iter()
        .find(|h| h.host.id == item.host)
        .map_or(item.host.as_str(), |h| h.host.label.as_str());
    format!(
        "`wrangle close {} --host {}`",
        crate::shell::quote(&item.pane),
        crate::shell::quote(host)
    )
}

/// A job item ends with its job. An agent item stays open while its pane exists, so the
/// owner keeps hearing about a finished child until `wrangle close` removes it.
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
            let ended = item.kind == "job"
                && host
                    .jobs
                    .iter()
                    .any(|j| j.id == item.id && j.ended_ms.is_some());
            let gone = !probe.panes.iter().any(|p| p.pane_id == item.pane);
            if ended || gone {
                item.closed_ms = Some(now);
            }
        }
        ledger.write("items.json", &items)
    })
}
