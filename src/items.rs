//! Owner-host item records and completion reconciliation.

use crate::error::Result;
use crate::hosts::HostReport;
use crate::ledger::Ledger;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub kind: String,
    pub host: String,
    pub pane: String,
    pub owner_pane: String,
    pub name: Option<String>,
    pub created_ms: u64,
    pub closed_ms: Option<u64>,
    /// What was asked for. Records from before these fields load with `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// `reader` or `writer`: whether the child is expected to commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// The child's final reply, captured before the sweep closes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<Reply>,
}

/// A finished child's final reply, for the completion Herdr numbered `seq`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub seq: u64,
    /// Immutable execution evidence uses a separate identity from turn notifications.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_id: Option<String>,
    pub text: String,
    pub captured_ms: u64,
    /// When the owner was told. A closed item with an untold reply still reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_ms: Option<u64>,
}

impl Item {
    /// A captured reply the owner has not been told yet.
    pub fn untold(&self) -> Option<&Reply> {
        self.reply.as_ref().filter(|r| r.reported_ms.is_none())
    }
}

fn change(ledger: &Ledger, f: impl FnOnce(&mut Vec<Item>)) -> Result<()> {
    ledger.with_lock("items", || {
        let mut items = list(ledger)?;
        f(&mut items);
        ledger.write("items.json", &items)
    })
}

/// Keep `reply` as `id`'s final reply.
pub fn set_reply(ledger: &Ledger, id: &str, reply: Reply) -> Result<()> {
    ledger.with_lock("items", || {
        let mut items = list(ledger)?;
        let Some(item) = items.iter_mut().find(|i| i.id == id) else {
            return Ok(());
        };
        if let Some(saved) = &item.reply
            && saved.result_id.is_some()
        {
            if saved.result_id != reply.result_id || saved.text != reply.text {
                return Err(crate::unit_transport::conflict(
                    "correlated item reply changed",
                ));
            }
            // Keep captured_ms and reported_ms: an identical replay is not new delivery.
            return Ok(());
        }
        item.reply = Some(reply);
        ledger.write("items.json", &items)
    })
}

/// Mark the replies of `ids` told.
pub fn told(ledger: &Ledger, ids: &[String], now: u64) -> Result<()> {
    change(ledger, |items| {
        for reply in items
            .iter_mut()
            .filter(|i| ids.contains(&i.id))
            .filter_map(|i| i.reply.as_mut())
        {
            reply.reported_ms.get_or_insert(now);
        }
    })
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

/// End every open item on `host`'s `pane`, after `dogsled close` removed it.
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
/// pane and worktree wait for `dogsled close`.
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
        "`dogsled close {} --host {}`",
        crate::shell::quote(&item.pane),
        crate::shell::quote(host)
    )
}

/// A job item ends with its job. An agent item stays open while its pane exists, so the
/// owner keeps hearing about a finished child until `dogsled close` removes it.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_from_before_the_request_fields_still_load() {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        std::fs::create_dir_all(&ledger.dir).unwrap();
        std::fs::write(
            ledger.dir.join("items.json"),
            r#"[{"id":"a","kind":"agent","host":"local","pane":"w1:p1","owner_pane":"w1:p2","name":null,"created_ms":1,"closed_ms":null}]"#,
        )
        .unwrap();
        let items = list(&ledger).unwrap();
        assert_eq!(items[0].pane, "w1:p1");
        assert_eq!(items[0].harness, None);
        assert_eq!(items[0].role, None);
        let text = serde_json::to_string(&items[0]).unwrap();
        assert!(!text.contains("harness"), "{text}");
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
}
