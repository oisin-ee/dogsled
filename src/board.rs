//! One view of what an owner started: rows grouped by what the owner must do next, with each
//! row's harness, model, host, and activity. `wrangle board`, the status text, the prompt
//! context, and the Pi panel all read this, so they never disagree.

use crate::commands::Status;
use crate::hosts::HostReport;
use crate::items::Item;
use crate::probe::Agent;
use crate::watchdog::age;
use serde::Serialize;
use std::fmt::Write as _;

/// What the owner must do about a row, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    /// Finished with commits that are not yet in the default branch, or with uncommitted work.
    ToLand,
    /// Stalled, blocked, or on an unavailable host.
    Attention,
    Running,
    /// Queued until a host has room.
    Waiting,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Self::ToLand => "TO LAND",
            Self::Attention => "ATTENTION",
            Self::Running => "RUNNING",
            Self::Waiting => "WAITING",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    pub group: Group,
    pub kind: String,
    pub id: String,
    pub name: String,
    /// `pi · astra→opus 5.5`, `codex · gpt-5.6-sol`, or `job`.
    pub what: String,
    /// The host label. Never a host id.
    pub host: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    /// `idle`, `working`, `blocked`, `stalled`, `done`, `running`, `exit 1`, `queued`, …
    pub state: String,
    /// The latest step, the job command, or why the row waits.
    pub activity: String,
    pub age_ms: u64,
    /// Time since the transcript last changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quiet_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HostLine {
    pub label: String,
    pub service: String,
    pub reachable: bool,
    /// Busy cores over total cores, when measured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub busy: Option<f64>,
    pub cores: u32,
    pub eligible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Board {
    pub hosts: Vec<HostLine>,
    pub rows: Vec<Row>,
}

/// Quiet this long while working is worth a look, even before the watchdog calls it a stall.
const QUIET_MS: u64 = 10 * crate::watchdog::MINUTE;

fn label(hosts: &[HostReport], id: &str) -> String {
    hosts
        .iter()
        .find(|h| h.host.id == id || h.host.label == id)
        .map_or_else(|| id.to_owned(), |h| h.host.label.clone())
}

fn agent<'a>(hosts: &'a [HostReport], item: &Item) -> Option<(&'a HostReport, &'a Agent)> {
    let host = hosts.iter().find(|h| h.host.id == item.host)?;
    let agent = host
        .probe
        .as_ref()?
        .agent_panes
        .iter()
        .find(|a| a.pane == item.pane)?;
    Some((host, agent))
}

/// `pi · astra→opus 5.5`: the harness, then the measured model, else the requested one.
fn what(item: &Item, agent: Option<&Agent>) -> String {
    let harness = agent
        .and_then(|a| a.agent.clone())
        .or_else(|| item.harness.clone())
        .unwrap_or_else(|| "agent".into());
    let model =
        agent
            .and_then(|a| a.session.label())
            .or_else(|| match (&item.profile, &item.model) {
                (Some(profile), Some(model)) if profile != model => {
                    Some(format!("{profile}→{model}"))
                }
                (profile, model) => model.clone().or_else(|| profile.clone()),
            });
    model.map_or_else(|| harness.clone(), |model| format!("{harness} · {model}"))
}

fn agent_row(item: &Item, hosts: &[HostReport], now: u64) -> Row {
    let found = agent(hosts, item);
    let host_up = hosts
        .iter()
        .any(|h| h.host.id == item.host && h.service == "up");
    let sampled = found.and_then(|(h, _)| h.sampled_ms).unwrap_or(now);
    let agent = found.map(|(_, a)| a);
    let quiet_ms = agent
        .and_then(|a| a.transcript_modified)
        .map(|t| sampled.max(now).saturating_sub(t));
    let tree = agent.and_then(|a| a.worktree.as_ref());
    let ahead = tree.map(|t| t.ahead).filter(|n| *n > 0);
    let dirty = tree.is_some_and(|t| t.dirty);
    let landed = tree.is_some_and(|t| t.landed);
    let state = match agent {
        Some(a) if a.stalled_since_ms.is_some() => "stalled".to_owned(),
        Some(a) => a.state.clone(),
        None if host_up => "starting".to_owned(),
        None => "unavailable".to_owned(),
    };
    let finished = matches!(state.as_str(), "idle" | "done");
    let group = if !host_up || matches!(state.as_str(), "stalled" | "blocked" | "unknown") {
        Group::Attention
    } else if finished && (dirty || (ahead.is_some() && !landed)) {
        Group::ToLand
    } else {
        Group::Running
    };
    let activity = match (group, agent) {
        (Group::ToLand, _) => [
            ahead.map(|n| format!("+{n} commit{}", if n == 1 { "" } else { "s" })),
            dirty.then(|| "uncommitted changes".to_owned()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · "),
        (_, None) if !host_up => format!("{} is unavailable", label(hosts, &item.host)),
        (_, None) => "starting".to_owned(),
        (_, Some(a)) => {
            let last = a.session.activity.clone().unwrap_or_default();
            match quiet_ms.filter(|q| *q >= QUIET_MS && !finished) {
                Some(quiet) if last.is_empty() => format!("quiet {}", age(quiet)),
                Some(quiet) => format!("quiet {} · last: {last}", age(quiet)),
                None => last,
            }
        }
    };
    let group = if group == Group::Running && quiet_ms.is_some_and(|q| q >= QUIET_MS) && !finished {
        Group::Attention
    } else {
        group
    };
    Row {
        group,
        kind: "agent".into(),
        id: item.id.clone(),
        name: item.name.clone().unwrap_or_else(|| item.pane.clone()),
        what: what(item, agent),
        host: label(hosts, &item.host),
        pane: Some(item.pane.clone()),
        state,
        activity,
        age_ms: now.saturating_sub(item.created_ms),
        quiet_ms,
        ahead,
        dirty,
    }
}

fn job_row(item: &Item, hosts: &[HostReport], now: u64) -> Row {
    let job = hosts
        .iter()
        .filter(|h| h.host.id == item.host)
        .flat_map(|h| &h.jobs)
        .find(|j| j.id == item.id);
    let state = job.map_or_else(
        || "starting".to_owned(),
        |j| match (j.ended_ms, j.exit) {
            (Some(_), Some(0)) => "done".to_owned(),
            (Some(_), Some(code)) => format!("exit {code}"),
            _ => j.state.clone(),
        },
    );
    let failed = state.starts_with("exit ") || state == "blocked";
    Row {
        group: if failed {
            Group::Attention
        } else {
            Group::Running
        },
        kind: "job".into(),
        id: item.id.clone(),
        name: job.and_then(|j| j.cmd.first()).map_or_else(
            || item.id.clone(),
            |c| c.rsplit('/').next().unwrap_or(c).into(),
        ),
        what: "job".into(),
        host: label(hosts, &item.host),
        pane: Some(item.pane.clone()),
        state,
        activity: job.map_or_else(String::new, |j| crate::activity::plain(&j.cmd.join(" "))),
        age_ms: now.saturating_sub(item.created_ms),
        quiet_ms: None,
        ahead: None,
        dirty: false,
    }
}

fn waiting(ticket: &crate::work::Ticket, hosts: &[HostReport], now: u64) -> Row {
    let request = &ticket.request;
    let (kind, name, what) = match &request.action {
        crate::work::Action::Agent { kind, name, .. } => ("agent", name.clone(), kind.clone()),
        crate::work::Action::Job { cmd, .. } => (
            "job",
            cmd.first().map_or_else(|| ticket.id.clone(), Clone::clone),
            "job".to_owned(),
        ),
        crate::work::Action::Place => (
            "place",
            request.branch.clone().unwrap_or_else(|| ticket.id.clone()),
            "place".to_owned(),
        ),
    };
    let target = request.host.as_deref().map(|h| label(hosts, h));
    let reason = hosts
        .iter()
        .filter(|h| target.as_deref().is_none_or(|t| h.host.label == t))
        .filter_map(|h| h.headroom.as_ref()?.reason.as_ref().map(|r| (h, r)))
        .map(|(h, r)| format!("{}: {r}", h.host.label))
        .next();
    Row {
        group: Group::Waiting,
        kind: kind.into(),
        id: ticket.id.clone(),
        name,
        what,
        host: target.clone().unwrap_or_else(|| "any".into()),
        pane: None,
        state: "queued".into(),
        activity: reason.map_or_else(|| "for a host with room".to_owned(), |r| format!("for {r}")),
        age_ms: now.saturating_sub(ticket.created_ms),
        quiet_ms: None,
        ahead: None,
        dirty: false,
    }
}

/// The board for `owner`, or for every owner when `None`.
pub fn build(status: &Status, owner: Option<&str>, now: u64) -> Board {
    let mine = |o: &str| owner.is_none_or(|owner| owner == o);
    let mut rows: Vec<Row> = status
        .items
        .iter()
        .filter(|i| i.closed_ms.is_none() && mine(&i.owner_pane))
        .map(|i| {
            if i.kind == "job" {
                job_row(i, &status.hosts, now)
            } else {
                agent_row(i, &status.hosts, now)
            }
        })
        .chain(
            status
                .queue
                .iter()
                .filter(|t| t.state == "pending" && mine(&t.request.owner))
                .map(|t| waiting(t, &status.hosts, now)),
        )
        .collect();
    // Stable: within a group, the oldest first.
    rows.sort_by(|a, b| a.group.cmp(&b.group).then(b.age_ms.cmp(&a.age_ms)));
    Board {
        hosts: status
            .hosts
            .iter()
            .map(|h| HostLine {
                label: h.host.label.clone(),
                service: h.service.clone(),
                reachable: h.reachable,
                busy: h
                    .probe
                    .as_ref()
                    .and_then(|p| p.busy.map(|b| b * f64::from(p.cores))),
                cores: h.probe.as_ref().map_or(0, |p| p.cores),
                eligible: h.headroom.as_ref().is_some_and(|h| h.eligible),
                reason: h.headroom.as_ref().and_then(|h| h.reason.clone()),
            })
            .collect(),
        rows,
    }
}

impl Board {
    /// `3 running · 1 to land · 1 waiting`.
    pub fn summary(&self) -> String {
        let mut counts: Vec<(Group, usize)> = Vec::new();
        for row in &self.rows {
            match counts.iter_mut().find(|(g, _)| *g == row.group) {
                Some((_, n)) => *n += 1,
                None => counts.push((row.group, 1)),
            }
        }
        counts
            .iter()
            .map(|(g, n)| format!("{n} {}", g.title().to_lowercase()))
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// Plain text, one row per line, for `wrangle board` without `--json`.
    pub fn render(&self) -> String {
        if self.rows.is_empty() {
            return "nothing running\n".into();
        }
        let mut out = format!("{}\n", self.summary());
        let mut group = None;
        for row in &self.rows {
            if group != Some(row.group) {
                out.push_str(row.group.title());
                out.push('\n');
                group = Some(row.group);
            }
            let _ = writeln!(
                out,
                "  {}  {}  {}  {}  {}",
                row.name,
                row.what,
                row.host,
                if row.activity.is_empty() {
                    &row.state
                } else {
                    &row.activity
                },
                age(row.age_ms)
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{Session, Worktree};
    use crate::hosts::Host;
    use crate::probe::{AgentCounts, Memory, Probe};

    const NOW: u64 = 100 * crate::watchdog::MINUTE;

    fn agent(pane: &str, kind: &str, state: &str) -> Agent {
        Agent {
            pane: pane.into(),
            agent: Some(kind.into()),
            state: state.into(),
            name: None,
            transcript: Some(format!("/t/{pane}")),
            transcript_modified: Some(NOW - 60_000),
            stalled_since_ms: None,
            state_since_ms: None,
            terminal_id: None,
            completion_seq: None,
            cwd: None,
            session: Session::default(),
            worktree: None,
        }
    }

    fn host(id: &str, label: &str, agents: Vec<Agent>) -> HostReport {
        HostReport {
            host: Host {
                id: id.into(),
                label: label.into(),
                target: (id != "local").then(|| format!("{label}-dev")),
            },
            service: "up".into(),
            reachable: true,
            sampled_ms: Some(NOW),
            probe: Some(Probe {
                host: id.into(),
                label: label.into(),
                load1: 0.0,
                cores: 16,
                cpu_ticks: None,
                busy: Some(0.125),
                memory: Memory::default(),
                disk_free_percent: 50.0,
                agents: AgentCounts::default(),
                agent_panes: agents,
                reservations: Vec::new(),
                panes: Vec::new(),
            }),
            headroom: None,
            error: None,
            jobs: Vec::new(),
        }
    }

    fn item(id: &str, host: &str, pane: &str) -> Item {
        Item {
            id: id.into(),
            kind: "agent".into(),
            host: host.into(),
            pane: pane.into(),
            owner_pane: "w1:p1".into(),
            name: Some(id.into()),
            created_ms: NOW - 14 * crate::watchdog::MINUTE,
            ..Item::default()
        }
    }

    fn status(hosts: Vec<HostReport>, items: Vec<Item>) -> Status {
        Status {
            hosts,
            items,
            queue: Vec::new(),
            orphans: Vec::new(),
        }
    }

    #[test]
    fn rows_group_by_what_the_owner_must_do() {
        let mut writer = agent("w2:p1", "pi", "done");
        writer.worktree = Some(Worktree {
            ahead: 2,
            ..Worktree::default()
        });
        let mut landed = agent("w2:p2", "pi", "idle");
        landed.worktree = Some(Worktree {
            ahead: 2,
            landed: true,
            ..Worktree::default()
        });
        let mut working = agent("w2:p3", "codex", "working");
        working.session = Session {
            model: Some("gpt-5.6-sol".into()),
            activity: Some("cargo test -p auth".into()),
            ..Session::default()
        };
        let mut quiet = agent("w2:p4", "pi", "working");
        quiet.transcript_modified = Some(NOW - 11 * crate::watchdog::MINUTE);
        quiet.session.activity = Some("rg \"t(\" web/".into());
        let blocked = agent("w2:p5", "claude", "blocked");
        let status = status(
            vec![host(
                "8103",
                "netcup",
                vec![writer, landed, working, quiet, blocked],
            )],
            vec![
                item("web-tests", "8103", "w2:p1"),
                item("landed", "8103", "w2:p2"),
                item("review-auth", "8103", "w2:p3"),
                item("i18n", "8103", "w2:p4"),
                item("asks", "8103", "w2:p5"),
                Item {
                    owner_pane: "w9:p9".into(),
                    ..item("theirs", "8103", "w2:p3")
                },
            ],
        );
        let board = build(&status, Some("w1:p1"), NOW);
        let groups: Vec<_> = board
            .rows
            .iter()
            .map(|r| (r.name.as_str(), r.group))
            .collect();
        assert_eq!(
            groups,
            [
                ("web-tests", Group::ToLand),
                ("i18n", Group::Attention),
                ("asks", Group::Attention),
                ("landed", Group::Running),
                ("review-auth", Group::Running),
            ]
        );
        assert_eq!(board.rows[0].activity, "+2 commits");
        assert_eq!(board.rows[0].host, "netcup");
        assert_eq!(board.rows[1].activity, "quiet 11m · last: rg \"t(\" web/");
        assert_eq!(board.rows[4].what, "codex · gpt-5.6-sol");
        assert_eq!(board.rows[4].activity, "cargo test -p auth");
        assert_eq!(board.summary(), "1 to land · 2 attention · 2 running");
        assert_eq!(build(&status, None, NOW).rows.len(), 6);
    }

    #[test]
    fn a_child_on_a_down_host_needs_attention_and_names_the_host() {
        let mut down = host("8103", "netcup", Vec::new());
        down.service = "down".into();
        let board = build(
            &status(vec![down], vec![item("x", "8103", "w2:p1")]),
            None,
            NOW,
        );
        assert_eq!(board.rows[0].group, Group::Attention);
        assert_eq!(board.rows[0].state, "unavailable");
        assert_eq!(board.rows[0].activity, "netcup is unavailable");
    }

    #[test]
    fn a_child_before_its_first_answer_shows_the_requested_model() {
        let item = Item {
            harness: Some("pi".into()),
            profile: Some("astra".into()),
            model: Some("alias/astra".into()),
            ..item("new", "local", "w2:p1")
        };
        assert_eq!(what(&item, None), "pi · astra→alias/astra");
        let item = Item {
            profile: None,
            ..item
        };
        assert_eq!(what(&item, None), "pi · alias/astra");
    }

    #[test]
    fn jobs_say_job_and_give_the_command() {
        let mut local = host("local", "local", Vec::new());
        local.jobs.push(crate::jobs::Job {
            id: "j1".into(),
            cmd: vec!["cargo".into(), "test".into()],
            cwd: "/r".into(),
            host: "local".into(),
            owner: "w1:p1".into(),
            pane: "w3:p1".into(),
            state: "running".into(),
            exit: None,
            signal: None,
            reason: None,
            started_ms: Some(NOW),
            ended_ms: None,
            timeout_ms: None,
            idle_timeout_ms: None,
        });
        let board = build(
            &status(
                vec![local],
                vec![Item {
                    kind: "job".into(),
                    ..item("j1", "local", "w3:p1")
                }],
            ),
            None,
            NOW,
        );
        let row = &board.rows[0];
        assert_eq!(
            (row.what.as_str(), row.name.as_str(), row.activity.as_str()),
            ("job", "cargo", "cargo test")
        );
        assert_eq!(row.group, Group::Running);
    }
}
