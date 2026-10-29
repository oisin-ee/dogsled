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
    /// The owner's harness session when the item was created. Records from before this field
    /// load with `None`, and an unrecorded session never proves the owner's session ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_session: Option<String>,
    pub name: Option<String>,
    pub created_ms: u64,
    /// Successful launch confirmation, measured on `host`, not the owner's clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_confirmed_ms: Option<u64>,
    pub closed_ms: Option<u64>,
    /// Execution-host terminal proof copied before bounded Job metadata is removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_ended_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped_ms: Option<u64>,
    /// Last successful missing-mark branch check on the origin clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_checked_ms: Option<u64>,
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
    /// Last observed reply preserved when a stopped orphan loses its resources, not a result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orphan_reply: Option<Reply>,
    /// Resource-only closure leaves a managed unit unresolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_closed_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_close_reason: Option<String>,
    /// A turn signal is not a final reply and never proves completion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<ItemSignal>,
    /// The item's final reply came from the native `done` handoff, so it proves the child
    /// finished. A sealed terminal receipt is a result, not a native `done`, so it never
    /// proves a handoff. Records from before this field load with `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub native_done: bool,
    /// Retained branch on the child's host, observed without a live pane or checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<crate::done::Branch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<crate::preservation::Recovery>,
}

/// The owner-side projection of a child-host turn signal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemSignal {
    pub kind: crate::lifecycle::Signal,
    pub text: String,
    pub event_id: String,
    pub recorded_ms: u64,
    /// Session that recorded this event; absent on older owner-side projections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_ms: Option<u64>,
}

impl From<&crate::done::Signal> for ItemSignal {
    fn from(signal: &crate::done::Signal) -> Self {
        Self {
            kind: signal.kind,
            text: signal.text.clone(),
            event_id: signal.event_id.clone(),
            recorded_ms: signal.recorded_ms,
            session: Some(signal.session.clone()),
            reported_ms: None,
        }
    }
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
    /// The child called `done` before this turn ended. Otherwise it waits for input.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub done: bool,
}

impl Item {
    /// A captured reply the owner has not been told yet.
    pub fn untold(&self) -> Option<&Reply> {
        self.reply.as_ref().filter(|r| r.reported_ms.is_none())
    }

    /// The item's final reply came from the native `done` handoff. Only that provenance
    /// proves the child finished: a terminal receipt or a legacy reply does not.
    pub fn native_done_reply(&self) -> Option<&Reply> {
        if self.native_done {
            self.reply.as_ref()
        } else {
            None
        }
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
    set_reply_with(ledger, id, reply, false)
}

/// Keep `reply` as `id`'s final reply, and record whether it came from the native `done`
/// handoff. Only that provenance proves the child finished later, when the pane is gone.
pub fn set_reply_with(ledger: &Ledger, id: &str, reply: Reply, native_done: bool) -> Result<()> {
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
            if !native_done || item.native_done {
                return Ok(());
            }
            item.native_done = true;
            return ledger.write("items.json", &items);
        }
        item.reply = Some(reply);
        item.native_done |= native_done;
        ledger.write("items.json", &items)
    })
}

/// Preserve stopped-orphan output without assigning result or native-done provenance.
pub fn orphan_reply(ledger: &Ledger, id: &str, reply: Reply) -> Result<()> {
    change(ledger, |items| {
        if let Some(item) = items.iter_mut().find(|item| item.id == id) {
            item.orphan_reply = Some(reply);
        }
    })
}

/// Record resource-only closure without resolving the unit binding.
pub fn orphan_closed(
    ledger: &Ledger,
    id: &str,
    now: u64,
    resource_only: bool,
    branch: Option<crate::done::Branch>,
) -> Result<()> {
    change(ledger, |items| {
        if let Some(item) = items.iter_mut().find(|item| item.id == id) {
            item.closed_ms.get_or_insert(now);
            item.signal = None;
            if let Some(branch) = branch {
                item.branch = Some(branch);
            }
            if resource_only {
                item.resource_closed_ms.get_or_insert(now);
                item.resource_close_reason = Some("resource-only close; task unresolved".into());
            }
        }
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

pub(crate) fn job_finished(ledger: &Ledger, job: &crate::jobs::Job) -> Result<()> {
    if !crate::jobs::terminal(job) {
        return Ok(());
    }
    change(ledger, |items| {
        for item in items
            .iter_mut()
            .filter(|item| item.kind == "job" && item.id == job.id && item.host == job.host)
        {
            item.job_ended_ms = item.job_ended_ms.or(job.ended_ms);
            item.closed_ms = item.closed_ms.or(job.ended_ms);
        }
    })
}

pub(crate) fn retention_ready(ledger: &Ledger, item: &Item, now: u64) -> Result<bool> {
    let Some(closed) = item.closed_ms else {
        return Ok(false);
    };
    if now.saturating_sub(closed) <= crate::jobs::RECORD_KEEP_MS
        || closed < item.created_ms
        || item.resource_closed_ms.is_some()
        || item.recovery.is_some()
        || (item.dropped_ms.is_none() && item.branch.as_ref().is_some_and(|branch| !branch.landed))
        || !crate::work::references_released(ledger, &item.id)?
    {
        return Ok(false);
    }
    if item.kind == "job" {
        return Ok(item.job_ended_ms.is_some_and(|ended| {
            ended <= closed && now.saturating_sub(ended) > crate::jobs::RECORD_KEEP_MS
        }));
    }
    Ok(item.native_done
        && item
            .reply
            .as_ref()
            .is_some_and(|reply| reply.done && reply.reported_ms.is_some()))
}

pub(crate) fn retained(ledger: &Ledger, id: &str, now: u64) -> Result<bool> {
    for item in list(ledger)?.iter().filter(|item| item.id == id) {
        if !retention_ready(ledger, item, now)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Custody and child notification share this lock order. Re-read after locking.
pub(crate) fn expire(ledger: &Ledger, now: u64) -> Result<()> {
    if !list(ledger)?.iter().any(|item| {
        item.closed_ms
            .is_some_and(|closed| now.saturating_sub(closed) > crate::jobs::RECORD_KEEP_MS)
    }) {
        return Ok(());
    }
    let (panes, _) = crate::cleanup::pane_snapshot(&crate::shell::System::default())?;
    ledger.with_lock("custody", || {
        ledger.with_lock("items", || {
            let items = list(ledger)?;
            let mut kept = Vec::new();
            let mut expired = Vec::new();
            for item in items {
                if (item.kind == "agent"
                    && item.host == crate::hosts::LOCAL
                    && panes.iter().any(|pane| pane.pane_id == item.pane))
                    || crate::done::list(ledger)?
                        .iter()
                        .any(|mark| mark.item.as_deref() == Some(&item.id))
                    || !retention_ready(ledger, &item, now)?
                {
                    kept.push(item);
                } else {
                    expired.push(item.id);
                }
            }
            if expired.is_empty() {
                return Ok(());
            }
            ledger.write("items.json", &kept)?;
            for id in expired {
                crate::custody::forget_released(ledger, &id)?;
            }
            Ok(())
        })
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

/// Persist the saved resource reference before setting any item closure timestamp.
pub fn saved_close(ledger: &Ledger, host: &str, closed: &crate::close::Closed) -> Result<()> {
    change(ledger, |items| {
        for item in items.iter_mut().filter(|item| {
            item.host == host
                && item.pane == closed.pane
                && closed
                    .recovery
                    .as_ref()
                    .and_then(|recovery| recovery.item.as_ref())
                    .map_or(item.closed_ms.is_none(), |id| item.id == *id)
        }) {
            if let Some(recovery) = &closed.recovery {
                item.recovery = Some(recovery.clone());
            }
            item.resource_close_reason = None;
            if let Some(branch) = &closed.retained_branch {
                item.branch = Some(branch.clone());
            }
        }
    })
}

pub fn close_blocker(ledger: &Ledger, id: &str, reason: &str) -> Result<()> {
    change(ledger, |items| {
        if let Some(item) = items
            .iter_mut()
            .find(|item| item.id == id && item.closed_ms.is_none())
        {
            item.resource_close_reason = Some(reason.into());
        }
    })
}

/// A replayed preservation result closes only its captured item, never a replacement.
pub fn close_result(
    ledger: &Ledger,
    host: &str,
    closed: &crate::close::Closed,
    now: u64,
) -> Result<()> {
    saved_close(ledger, host, closed)?;
    if let Some(recovery) = &closed.recovery {
        change(ledger, |items| {
            for item in items.iter_mut().filter(|item| {
                item.host == host
                    && item.pane == closed.pane
                    && recovery.item.as_deref() == Some(item.id.as_str())
            }) {
                item.closed_ms.get_or_insert(now);
                item.signal = None;
            }
        })
    } else {
        close(ledger, host, &closed.pane, now).map(|_| ())
    }
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
            item.signal = None;
            count += 1;
        }
        ledger.write("items.json", &items)?;
        Ok(count)
    })
}

/// Settle an explicitly discarded branch on the origin.
pub fn dropped(ledger: &Ledger, id: &str, now: u64) -> Result<()> {
    change(ledger, |items| {
        if let Some(item) = items.iter_mut().find(|i| i.id == id) {
            item.dropped_ms.get_or_insert(now);
            if let Some(branch) = &mut item.branch {
                branch.landed = true;
                branch.ahead = 0;
            }
        }
    })
}

/// Import one successful batch without replacing a newer or different branch observation.
pub fn checked(
    ledger: &Ledger,
    host: &str,
    checked: &[crate::drop::Checked],
    now: u64,
) -> Result<()> {
    change(ledger, |items| {
        for check in checked {
            if let Some(item) = items
                .iter_mut()
                .find(|i| i.id == check.item && i.host == host)
                && item.closed_ms.is_some()
                && item.dropped_ms.is_none()
                && let Some(branch) = &mut item.branch
                && branch.repo == check.branch.repo
                && branch.name == check.branch.name
                && !branch.landed
                && item.branch_checked_ms.is_none_or(|t| t <= now)
            {
                branch.clone_from(&check.branch);
                item.branch_checked_ms = Some(now);
            }
        }
    })
}

/// Import durable child-host handoffs, including after pane closure or owner downtime.
pub fn handoffs(ledger: &Ledger, hosts: &[HostReport]) -> Result<()> {
    change(ledger, |items| merge_handoffs(items, hosts))
}

pub fn merge_handoffs(items: &mut [Item], hosts: &[HostReport]) {
    for item in items {
        if item.closed_ms.is_some() {
            item.signal = None;
        } else if let Some(host) = hosts.iter().find(|h| h.host.id == item.host)
            && host.reachable
            && host.failure.is_none()
            && host.service == "up"
        {
            let live = host.agent(&item.pane);
            if item.signal.as_ref().is_some_and(|s| {
                s.session.as_ref().is_some_and(|session| {
                    live.and_then(|a| a.agent_session.as_ref())
                        .is_some_and(|current| current != session)
                })
            }) {
                item.signal = None;
            }
            let signal_mark = host
                .handoffs
                .iter()
                .filter(|m| m.pane == item.pane && m.item.as_deref().is_none_or(|id| id == item.id))
                .filter(|m| m.signal.is_some())
                .filter(|m| live.is_none_or(|a| m.session == a.agent_session))
                .max_by_key(|m| m.marked_ms);
            if let Some(mark) = signal_mark {
                let next = mark
                    .signal
                    .as_ref()
                    .filter(|s| s.cleared_ms.is_none())
                    .map(ItemSignal::from);
                if item.signal.as_ref().map(|s| &s.event_id) != next.as_ref().map(|s| &s.event_id) {
                    item.signal = next;
                } else if let (Some(saved), Some(next)) = (&mut item.signal, next) {
                    // Backfill identity without resetting an unchanged event's delivery stamp.
                    if saved.session.is_none() {
                        saved.session = next.session;
                    }
                }
            }
            // Missing marks, including failed reads with cached agents, prove no clearance.
        }
        let Some(mark) = hosts.iter().find(|h| h.host.id == item.host).and_then(|h| {
            h.handoffs
                .iter()
                .find(|m| m.pane == item.pane && m.item.as_deref().is_none_or(|id| id == item.id))
        }) else {
            continue;
        };
        if let Some(reply) = &mark.reply {
            // Preserve the owner's delivery stamp and immutable unit evidence.
            if item
                .reply
                .as_ref()
                .is_none_or(|r| r.result_id.is_none() && (r.seq != reply.seq || !r.done))
            {
                item.reply = Some(reply.clone());
            }
            // The mark knows whether its reply came from the native `done` handoff. The
            // item keeps that provenance, so a gone pane still proves the handoff.
            item.native_done |= mark.native_done;
        }
        if let Some(closed) = mark.closed_ms {
            item.closed_ms.get_or_insert(closed);
            item.signal = None;
        }
        if let Some(dropped) = mark.dropped_ms {
            item.dropped_ms.get_or_insert(dropped);
        }
        if item.dropped_ms.is_none() && mark.branch.is_some() {
            item.branch.clone_from(&mark.branch);
        }
    }
}

/// Deliver nonterminal child events through the same live-custodian route as replies.
pub fn deliver_signals(fleet: &crate::hosts::Fleet, hosts: &[HostReport], now: u64) -> Result<()> {
    handoffs(fleet.ledger, hosts)?;
    for item in list(fleet.ledger)? {
        let Some(signal) = item.signal.as_ref().filter(|s| s.reported_ms.is_none()) else {
            continue;
        };
        crate::custody::ensure(fleet.ledger, &item)?;
        let child = crate::inbox::ChildIdentity::capture(fleet.ledger, &item)?;
        if crate::inbox::notify_child(
            fleet,
            hosts,
            &child,
            &format!("signal-{}-{}", item.id, signal.event_id),
            signal.kind.as_str(),
            signal.text.clone(),
            now,
        )? {
            change(fleet.ledger, |items| {
                if let Some(saved) = items
                    .iter_mut()
                    .find(|i| i.id == item.id)
                    .and_then(|i| i.signal.as_mut())
                    .filter(|s| s.event_id == signal.event_id)
                {
                    saved.reported_ms.get_or_insert(now);
                }
            })?;
        }
    }
    Ok(())
}

/// An open agent item whose agent called `done` and ended that turn: its result is ready, and
/// its host is completing the guarded handoff.
pub fn finished(item: &Item, hosts: &[HostReport]) -> bool {
    item.closed_ms.is_none()
        && item.kind == "agent"
        && hosts
            .iter()
            .filter(|h| h.host.id == item.host)
            .filter_map(|h| h.agent(&item.pane))
            .any(|a| a.done)
}

/// The lifecycle state of an item, from the same owner the board reads. The result and the
/// resource closure stay separate facts: this returns only the agent's state.
pub fn lifecycle(item: &Item, hosts: &[HostReport]) -> crate::lifecycle::Derived {
    let (evidence, _) = crate::lifecycle::evidence(item, hosts);
    crate::lifecycle::classify(&evidence)
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
                .find(|h| h.host.id == item.host && h.fresh(now))
            else {
                continue;
            };
            let Some(probe) = &host.probe else {
                continue;
            };
            // Only conclusive execution-host evidence, not a missing pane, proves job completion.
            let terminal_job = host
                .jobs
                .iter()
                .find(|job| job.id == item.id && crate::jobs::terminal(job));
            let ended = item.kind == "job" && terminal_job.is_some();
            if ended {
                item.job_ended_ms = terminal_job.and_then(|job| job.ended_ms);
            }
            // A missing pane proves only that a sample taken after the pane existed lacks it.
            // Admission time is earlier than the pane: a launch can take many seconds.
            let after_launch = if let Some(confirmed) = item.launch_confirmed_ms {
                // Both timestamps belong to the child host. Equality cannot prove ordering.
                host.sampled_ms.is_some_and(|sampled| sampled > confirmed)
            } else {
                // Old items without launch evidence keep the creation-time rule.
                host.sampled_ms
                    .is_some_and(|sampled| sampled >= item.created_ms)
            };
            let gone = after_launch && !probe.panes.iter().any(|p| p.pane_id == item.pane);
            if ended || gone {
                item.closed_ms = Some(now);
                item.signal = None;
            }
        }
        ledger.write("items.json", &items)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_closed_handoff_import_keeps_branch_and_reply_delivery_stamp() {
        let mut item = Item {
            id: "launch".into(),
            host: "remote".into(),
            pane: "w2:p1".into(),
            ..Item::default()
        };
        let mut host: HostReport = serde_json::from_value(serde_json::json!({
            "id":"remote","label":"remote","target":"remote-dev",
            "service":"up","reachable":true,"sampled_ms":1,"probe":null,
            "headroom":null,"error":null,"jobs":[],"handoffs":[{
                "pane":"w2:p1","item":"launch","session":"session","marked_ms":1,
                "sealed":0,"closed_ms":2,
                "reply":{"seq":0,"text":"final reply","captured_ms":2,"done":true},
                "branch":{"repo":"/repo","name":"feature","ahead":3,"landed":false}
            }]
        }))
        .unwrap();
        merge_handoffs(std::slice::from_mut(&mut item), &[host.clone()]);
        assert_eq!(item.closed_ms, Some(2));
        assert_eq!(item.branch.as_ref().unwrap().ahead, 3);
        item.reply.as_mut().unwrap().reported_ms = Some(3);
        host.handoffs[0].branch.as_mut().unwrap().landed = true;
        merge_handoffs(std::slice::from_mut(&mut item), &[host]);
        assert_eq!(item.reply.unwrap().reported_ms, Some(3));
        assert!(item.branch.unwrap().landed);
    }

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
        assert_eq!(items[0].launch_confirmed_ms, None);
        assert_eq!(items[0].owner_session, None);
        let text = serde_json::to_string(&items[0]).unwrap();
        assert!(!text.contains("harness"), "{text}");
        assert!(!text.contains("owner_session"), "{text}");
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    /// A record from before the owner session existed has none. The session-ended rule
    /// needs a recorded identity, so it never applies to that item.
    #[test]
    fn a_record_without_the_owner_session_loads_and_records_none() {
        let item: Item = serde_json::from_value(serde_json::json!({
            "id":"a","kind":"agent","host":"local","pane":"w1:p1","owner_pane":"w1:p2",
            "name":null,"created_ms":1,"closed_ms":null
        }))
        .unwrap();
        assert_eq!(item.owner_session, None);
        let text = serde_json::to_string(&item).unwrap();
        assert!(!text.contains("owner_session"), "{text}");
    }

    /// An item from before the native `done` provenance existed has none. It never proves
    /// a handoff, and the old record still decodes.
    #[test]
    fn an_item_without_the_provenance_loads_and_proves_no_handoff() {
        let item: Item = serde_json::from_value(serde_json::json!({
            "id":"a","kind":"agent","host":"local","pane":"w1:p1","owner_pane":"w1:p2",
            "name":null,"created_ms":1,"closed_ms":null,
            "reply":{"seq":0,"text":"the final reply","captured_ms":2,"done":true}
        }))
        .unwrap();
        assert!(!item.native_done);
        assert!(item.native_done_reply().is_none());
        let text = serde_json::to_string(&item).unwrap();
        assert!(!text.contains("native_done"), "{text}");
    }

    /// The mark the child host kept carries the native `done` provenance, and the owner
    /// imports it into the item.
    #[test]
    fn the_native_done_provenance_survives_the_handoff_import() {
        let mut item = Item {
            id: "launch".into(),
            host: "remote".into(),
            pane: "w2:p1".into(),
            ..Item::default()
        };
        let host: HostReport = serde_json::from_value(serde_json::json!({
            "id":"remote","label":"remote","target":"remote-dev",
            "service":"up","reachable":true,"sampled_ms":1,"probe":null,
            "headroom":null,"error":null,"jobs":[],"handoffs":[{
                "pane":"w2:p1","item":"launch","session":"session","marked_ms":1,
                "sealed":0,"closed_ms":2,
                "native_done":true,
                "reply":{"seq":0,"text":"final reply","captured_ms":2,"done":true},
                "branch":{"repo":"/repo","name":"feature","ahead":3,"landed":false}
            }]
        }))
        .unwrap();
        merge_handoffs(std::slice::from_mut(&mut item), std::slice::from_ref(&host));
        assert!(item.native_done_reply().is_some());
        // A reply that is already there keeps the owner's stamp and gains the provenance.
        item.reply.as_mut().unwrap().reported_ms = Some(3);
        merge_handoffs(std::slice::from_mut(&mut item), std::slice::from_ref(&host));
        assert_eq!(item.reply.as_ref().unwrap().reported_ms, Some(3));
        assert!(item.native_done_reply().is_some());
    }

    /// A mark without the native `done` provenance never proves a handoff, even when it
    /// keeps a reply.
    #[test]
    fn a_mark_without_the_provenance_proves_no_handoff() {
        let mut item = Item {
            id: "launch".into(),
            host: "remote".into(),
            pane: "w2:p1".into(),
            ..Item::default()
        };
        let host: HostReport = serde_json::from_value(serde_json::json!({
            "id":"remote","label":"remote","target":"remote-dev",
            "service":"up","reachable":true,"sampled_ms":1,"probe":null,
            "headroom":null,"error":null,"jobs":[],"handoffs":[{
                "pane":"w2:p1","item":"launch","session":"session","marked_ms":1,
                "sealed":0,
                "reply":{"seq":0,"text":"final reply","captured_ms":2,"done":true}
            }]
        }))
        .unwrap();
        merge_handoffs(std::slice::from_mut(&mut item), &[host]);
        assert!(item.native_done_reply().is_none());
    }

    /// The local handoff keeps the native `done` provenance on the item it writes.
    #[test]
    fn the_local_handoff_keeps_the_native_done_provenance() {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let item = Item {
            id: "launch".into(),
            host: "local".into(),
            pane: "w2:p1".into(),
            ..Item::default()
        };
        crate::items::record(&ledger, item).unwrap();
        let reply = Reply {
            seq: 0,
            result_id: None,
            text: "the final reply".into(),
            captured_ms: 2,
            reported_ms: None,
            done: true,
        };
        set_reply_with(&ledger, "launch", reply.clone(), true).unwrap();
        let items = list(&ledger).unwrap();
        assert!(items[0].native_done_reply().is_some());
        // A plain reply carries no provenance.
        set_reply_with(&ledger, "launch", reply, false).unwrap();
        let items = list(&ledger).unwrap();
        assert!(items[0].native_done_reply().is_some());
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
}
