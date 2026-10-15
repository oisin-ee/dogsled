//! The last-resort sweep in `serve`. The closed loop ends most work: an owner closes its
//! children, and a job closes its own tab. The sweep catches what a broken loop leaves behind. It
//! applies only doctor entries whose removal loses nothing, and closes an orphaned child only when
//! it is idle or done, clean, and pushed. Each removal is logged to stderr, the service journal.

use crate::error::Result;
use crate::hosts::{Fleet, HostReport};
use crate::items::Item;
use crate::ledger::Ledger;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// An open child whose owner pane no longer exists, which the sweep keeps. Status and check-ins
/// report it, because nothing else will.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Orphan {
    pub item: String,
    pub host: String,
    pub pane: String,
    pub owner_pane: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Why the sweep keeps it: dirty, unpushed, working, blocked, or unreachable.
    pub reason: String,
    pub since_ms: u64,
}

const FILE: &str = "orphans.json";

/// The orphans the last sweep kept, for status and check-ins. A closed item is no longer one.
pub fn list(ledger: &Ledger) -> Result<Vec<Orphan>> {
    let orphans: Vec<Orphan> = ledger.read(FILE)?.unwrap_or_default();
    let open: BTreeSet<String> = crate::items::list(ledger)?
        .into_iter()
        .filter(|i| i.closed_ms.is_none())
        .map(|i| i.id)
        .collect();
    Ok(orphans
        .into_iter()
        .filter(|o| open.contains(&o.item))
        .collect())
}

/// One sweep pass. The doctor entries and the orphans are independent: a failure in one still
/// sweeps the other.
pub fn pass(fleet: &Fleet, now: u64) -> Result<()> {
    let doctor = crate::cleanup::sweep(&fleet.node(), now);
    if let Ok(applied) = &doctor {
        for result in &applied.results {
            eprintln!("wrangle sweep: {result}");
        }
        for failure in &applied.failures {
            eprintln!("wrangle sweep: failed: {failure}");
        }
    }
    let orphans = orphans(fleet, now);
    doctor.and(orphans)
}

/// Close each orphaned child that is idle or done, clean, and pushed, exactly as `wrangle close`
/// would; record the others for status and check-ins.
fn orphans(fleet: &Fleet, now: u64) -> Result<()> {
    let items: Vec<Item> = crate::items::list(fleet.ledger)?
        .into_iter()
        .filter(|i| i.closed_ms.is_none() && i.kind == "agent")
        .collect();
    let previous: BTreeMap<String, Orphan> = fleet
        .ledger
        .read::<Vec<Orphan>>(FILE)?
        .unwrap_or_default()
        .into_iter()
        .map(|o| (o.item.clone(), o))
        .collect();
    let mut kept = Vec::new();
    if !items.is_empty() {
        // An owner is a pane on this host, where its items are recorded. A failed pane list
        // proves nothing is gone, so it stops the pass.
        let live: BTreeSet<String> = crate::cleanup::current_panes(fleet.shell)?
            .into_iter()
            .map(|p| p.pane_id)
            .collect();
        let orphaned: Vec<&Item> = items
            .iter()
            .filter(|i| !live.contains(&i.owner_pane))
            .collect();
        let reports = if orphaned.is_empty() {
            Vec::new()
        } else {
            fleet.status(now)?
        };
        for item in orphaned {
            let Some(reason) = settle(fleet, item, &reports, now) else {
                continue;
            };
            let since_ms = previous.get(&item.id).map_or(now, |o| o.since_ms);
            if previous.get(&item.id).is_none_or(|o| o.reason != reason) {
                eprintln!(
                    "wrangle sweep: keep orphan {} on {}: {reason}",
                    item.pane, item.host
                );
            }
            kept.push(Orphan {
                item: item.id.clone(),
                host: item.host.clone(),
                pane: item.pane.clone(),
                owner_pane: item.owner_pane.clone(),
                name: item.name.clone(),
                reason,
                since_ms,
            });
        }
    }
    if kept.is_empty() && previous.is_empty() {
        return Ok(());
    }
    fleet.ledger.write(FILE, &kept)
}

/// Close `item` when that loses nothing, or say why it stays. `None` when it was closed, or when
/// its pane is gone and item reconciliation ends it.
fn settle(fleet: &Fleet, item: &Item, reports: &[HostReport], now: u64) -> Option<String> {
    let owner = format!("owner {} is gone", item.owner_pane);
    let Some(report) = reports
        .iter()
        .find(|h| h.host.id == item.host && h.service == "up")
    else {
        return Some(format!("{owner}; host {} is unavailable", item.host));
    };
    let probe = report.probe.as_ref()?;
    let Some(agent) = probe.agent_panes.iter().find(|a| a.pane == item.pane) else {
        return probe
            .panes
            .iter()
            .any(|p| p.pane_id == item.pane)
            .then(|| format!("{owner}; its pane runs no agent"));
    };
    if !matches!(agent.state.as_str(), "idle" | "done") {
        return Some(format!("{owner}; the child is {}", agent.state));
    }
    match close(fleet, item, &report.host) {
        Ok(closed) => {
            if let Err(error) = crate::items::close(fleet.ledger, &item.host, &item.pane, now) {
                eprintln!("wrangle sweep: failed: close item {}: {error}", item.id);
            }
            eprintln!(
                "wrangle sweep: closed orphan {} on {} (owner {} is gone){}",
                item.pane,
                report.host.label,
                item.owner_pane,
                closed.worktree.map_or_else(String::new, |tree| format!(
                    "; removed {} ({})",
                    tree.path, tree.branch
                ))
            );
            None
        }
        Err(error) => Some(format!(
            "{owner}; {}",
            error
                .to_string()
                .trim_start_matches("refuse: ")
                .trim_end_matches("; nothing was closed")
        )),
    }
}

fn close(fleet: &Fleet, item: &Item, host: &crate::hosts::Host) -> Result<crate::close::Closed> {
    if host.is_local() {
        crate::close::local(
            fleet.shell,
            fleet.ledger,
            &item.pane,
            &crate::close::Caller::from_env(),
            true,
        )
    } else {
        fleet.remote_setup(
            host,
            &["close".into(), item.pane.clone(), "--pushed".into()],
        )
    }
}
