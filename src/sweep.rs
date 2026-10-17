//! The sweep in `serve` ends finished work. When a child completes a turn (Herdr's
//! `completion_seq`), the sweep captures its final reply into its item, then closes it once that
//! loses nothing: a reader with no commits outside the default branch, or a child whose branch
//! landed there. A child with work to land stays open. Blocked and working children never close,
//! and nothing closes on a timer. The sweep also applies doctor entries whose removal loses
//! nothing, and closes an orphaned child only when it is idle or done, clean, and pushed. Each
//! removal is logged to stderr, the service journal.

use crate::close::Guard;
use crate::error::Result;
use crate::hosts::{Fleet, HostReport};
use crate::items::{Item, Reply};
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
/// Why each finished child with an owner stays open, so the journal logs only changes.
const LANDING: &str = "landing.json";
/// A reply longer than this is cut; the transcript keeps all of it.
const REPLY_MAX: usize = 32 * 1024;

/// A finished child's final reply, as its host reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Final {
    pub seq: u64,
    pub text: String,
}

/// The final reply of the agent in `pane` on this host: the newest assistant text of the
/// completion Herdr numbered. `None` while it works, or when nothing can be read.
pub fn local_reply(shell: &dyn crate::shell::Shell, pane: &str) -> Result<Option<Final>> {
    let Some(agent) = crate::probe::agent_list(shell)?
        .into_iter()
        .find(|a| a.pane == pane)
    else {
        return Ok(None);
    };
    let (Some(seq), Some(kind), Some(path)) = (
        agent.completion_seq,
        agent.agent.as_deref(),
        agent.transcript.as_deref(),
    ) else {
        return Ok(None);
    };
    if !matches!(agent.state.as_str(), "idle" | "done") {
        return Ok(None);
    }
    Ok(crate::activity::tail(path)
        .and_then(|values| crate::activity::reply(kind, &values))
        .map(|text| Final {
            seq,
            text: cut(&text, path),
        }))
}

fn cut(text: &str, path: &str) -> String {
    if text.len() <= REPLY_MAX {
        return text.to_owned();
    }
    let mut end = REPLY_MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… (cut; the whole reply is in {path})", &text[..end])
}

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
    let children = children(fleet, now);
    doctor.and(children)
}

/// Settle each open agent item: an owned child that finished, and an orphan that is idle or
/// done, clean, and pushed, close exactly as `wrangle close` would. Orphans that stay are
/// recorded for status and check-ins.
fn children(fleet: &Fleet, now: u64) -> Result<()> {
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
        let (orphaned, owned): (Vec<&Item>, Vec<&Item>) =
            items.iter().partition(|i| !live.contains(&i.owner_pane));
        let reports = fleet.status(now)?;
        landing(fleet, &owned, &reports, now)?;
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
    match close(fleet, item, &report.host, Guard::Pushed) {
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
        Err(error) => Some(format!("{owner}; {}", reason(&error))),
    }
}

/// Settle each owned child, and journal why one stays open when the reason changes.
fn landing(fleet: &Fleet, owned: &[&Item], reports: &[HostReport], now: u64) -> Result<()> {
    let previous: BTreeMap<String, String> = fleet.ledger.read(LANDING)?.unwrap_or_default();
    let mut kept = BTreeMap::new();
    for item in owned {
        let Some(reason) = finish(fleet, item, reports, now) else {
            continue;
        };
        if previous.get(&item.id) != Some(&reason) {
            eprintln!(
                "wrangle sweep: keep {} on {}: {reason}",
                item.pane, item.host
            );
        }
        kept.insert(item.id.clone(), reason);
    }
    if kept != previous {
        fleet.ledger.write(LANDING, &kept)?;
    }
    Ok(())
}

/// Capture a finished child's reply, then close it when its role allows. `Some` says why a
/// finished child stays open; `None` means it closed, or is not finished.
fn finish(fleet: &Fleet, item: &Item, reports: &[HostReport], now: u64) -> Option<String> {
    let report = reports
        .iter()
        .find(|h| h.host.id == item.host && h.service == "up")?;
    let agent = report
        .probe
        .as_ref()?
        .agent_panes
        .iter()
        .find(|a| a.pane == item.pane)?;
    // Idle without a completed turn is a child that has not started its task yet.
    let seq = agent.completion_seq?;
    if !matches!(agent.state.as_str(), "idle" | "done") {
        return None;
    }
    if item.reply.as_ref().is_none_or(|r| r.seq != seq) {
        let read = if report.host.is_local() {
            local_reply(fleet.shell, &item.pane)
        } else {
            fleet.remote(&report.host, &["reply".into(), item.pane.clone()])
        };
        let captured = match read {
            Ok(Some(captured)) => captured,
            Ok(None) => return Some("its final reply cannot be read".into()),
            Err(error) => return Some(format!("cannot read its final reply: {error}")),
        };
        let reply = Reply {
            seq: captured.seq,
            text: captured.text,
            captured_ms: now,
            reported_ms: None,
        };
        if let Err(error) = crate::items::set_reply(fleet.ledger, &item.id, reply) {
            return Some(format!("cannot keep its final reply: {error}"));
        }
    }
    let guard = if item.role.as_deref() == Some("writer") {
        Guard::Writer
    } else {
        Guard::Reader
    };
    // The snapshot's facts say when a close would refuse; the close re-checks on the host.
    if let Some(tree) = &agent.worktree {
        let commits = |n: u64| format!("+{n} commit{} to land", if n == 1 { "" } else { "s" });
        if tree.dirty {
            return Some("uncommitted changes".into());
        }
        if tree.ahead > 0 && !tree.landed {
            return Some(commits(tree.ahead));
        }
    }
    match close(fleet, item, &report.host, guard) {
        Ok(closed) => {
            if let Err(error) = crate::items::close(fleet.ledger, &item.host, &item.pane, now) {
                eprintln!("wrangle sweep: failed: close item {}: {error}", item.id);
            }
            eprintln!(
                "wrangle sweep: closed finished {} on {}{}",
                item.pane,
                report.host.label,
                closed.worktree.map_or_else(String::new, |tree| format!(
                    "; removed {} ({})",
                    tree.path, tree.branch
                ))
            );
            None
        }
        Err(error) => Some(reason(&error)),
    }
}

/// A close refusal without its framing, local or over SSH.
fn reason(error: &crate::error::Error) -> String {
    let text = error.to_string();
    let text = text
        .split_once("refuse: ")
        .map_or(text.as_str(), |(_, r)| r);
    text.trim_end_matches("; nothing was closed").to_owned()
}

fn close(
    fleet: &Fleet,
    item: &Item,
    host: &crate::hosts::Host,
    guard: Guard,
) -> Result<crate::close::Closed> {
    if host.is_local() {
        crate::close::local(
            fleet.shell,
            fleet.ledger,
            &item.pane,
            &crate::close::Caller::from_env(),
            guard,
        )
    } else {
        let mut args = vec!["close".to_owned(), item.pane.clone()];
        args.extend(guard.flags().iter().map(|f| (*f).to_owned()));
        fleet.remote_setup(host, &args)
    }
}
