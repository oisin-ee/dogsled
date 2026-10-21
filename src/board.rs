//! One view of what an owner started: rows grouped by what the owner must do next, with each
//! row's harness, model, host, and activity. `dogsled board`, the status text, the prompt
//! context, and the Pi panel all read this, so they never disagree.

use crate::commands::Status;
use crate::error::{Error, Result};
use crate::hosts::HostReport;
use crate::items::Item;
use crate::ledger::Ledger;
use crate::probe::Agent;
use crate::shell::Shell;
use crate::watchdog::age;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

/// Rendered presentation only. Task records never cross this boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TodoLines {
    pub session: String,
    pub branch: Option<String>,
    pub revision: u64,
    pub current: String,
    pub pane: String,
    pub width: u16,
    pub lines: Vec<String>,
}

const DEFAULT_WIDTH: u16 = 48;
const TODO_HEIGHT: usize = 10_000;
const MAX_TODO_BYTES: u64 = 1024 * 1024;

fn key(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// The same ledger path for Pi publication and service intake.
pub fn todo_path(ledger: &Ledger, pane: &str) -> Result<serde_json::Value> {
    let dir = ledger.child("todo-lines");
    std::fs::create_dir_all(&dir.dir)?;
    let width: Option<u16> = ledger
        .child("board-widths")
        .read(&format!("{}.json", key(pane)))?;
    Ok(serde_json::json!({
        "path": dir.dir.join(format!("{}.json", key(pane))),
        "width": width.unwrap_or(DEFAULT_WIDTH),
        "height": TODO_HEIGHT,
    }))
}

fn validate_todo(todo: &TodoLines) -> Result<()> {
    if todo.session.is_empty()
        || todo.pane.is_empty()
        || todo.revision == 0
        || todo.width == 0
        || todo.lines.len() > TODO_HEIGHT
        || todo.lines.iter().any(|line| line.contains(['\n', '\r']))
    {
        return Err(Error::Invalid("invalid rendered todo snapshot".into()));
    }
    Ok(())
}

/// The publication lane reads only files belonging to currently observed agent panes.
/// Each file is replaced atomically by Pi. Closed panes cannot leave visible todo rows.
pub fn collect_todos(ledger: &Ledger) -> Result<()> {
    use std::io::Read as _;
    let snapshot: Option<crate::node::Snapshot> = ledger.read("host.json")?;
    let mut todos = Vec::new();
    if let Some(snapshot) = snapshot {
        for agent in &snapshot.probe.agent_panes {
            let path = ledger
                .child("todo-lines")
                .dir
                .join(format!("{}.json", key(&agent.pane)));
            let file = match std::fs::File::open(path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            let mut bytes = Vec::new();
            file.take(MAX_TODO_BYTES + 1).read_to_end(&mut bytes)?;
            let todo = (|| -> Result<TodoLines> {
                if bytes.len() as u64 > MAX_TODO_BYTES {
                    return Err(Error::Invalid("rendered todo snapshot too large".into()));
                }
                let todo: TodoLines = serde_json::from_slice(&bytes)?;
                validate_todo(&todo)?;
                if todo.pane != agent.pane {
                    return Err(Error::Invalid("rendered todo pane mismatch".into()));
                }
                Ok(todo)
            })();
            match todo {
                Ok(todo) => todos.push(todo),
                Err(_) => eprintln!(
                    "dogsled board: invalid rendered todo snapshot for {}",
                    agent.pane
                ),
            }
        }
    }
    ledger.write("todos.json", &todos)
}

pub fn cached_todos(ledger: &Ledger) -> Result<Vec<TodoLines>> {
    let todos: Vec<TodoLines> = ledger.read("todos.json")?.unwrap_or_default();
    for todo in &todos {
        validate_todo(todo)?;
    }
    Ok(todos)
}

/// Plugin context names the opener, not the new board pane.
pub fn owner(explicit: Option<String>, all: bool) -> Result<Option<String>> {
    if all {
        return Ok(None);
    }
    explicit
        .or_else(|| {
            let context = std::env::var("HERDR_PLUGIN_CONTEXT_JSON").ok()?;
            let context: serde_json::Value = serde_json::from_str(&context).ok()?;
            context["focused_pane_id"].as_str().map(str::to_owned)
        })
        .or_else(|| std::env::var("HERDR_PANE_ID").ok())
        .map(Some)
        .ok_or_else(|| Error::Invalid("HERDR_PANE_ID or --owner is required; or pass --all".into()))
}

/// Herdr 0.9.5 creates a new plugin pane on every open. Keep one per live tab,
/// under the ledger lock, and validate its terminal identity before reuse.
pub fn open(ledger: &Ledger, shell: &dyn Shell, owner: &str) -> Result<serde_json::Value> {
    ledger.with_lock("board-open", || {
        let out = shell.run_ok("herdr", &["pane".into(), "list".into()])?;
        let value: serde_json::Value = serde_json::from_str(&out.stdout)?;
        let panes = value["result"]["panes"]
            .as_array()
            .ok_or_else(|| Error::Invalid("invalid Herdr pane list".into()))?;
        let caller = panes
            .iter()
            .find(|pane| pane["pane_id"] == owner)
            .ok_or_else(|| Error::Invalid("board owner pane is unavailable".into()))?;
        let tab = caller["tab_id"]
            .as_str()
            .ok_or_else(|| Error::Invalid("board owner tab is unavailable".into()))?;
        let registry = ledger.child("boards");
        let file = format!("{}.json", key(tab));
        if let Some(saved) = registry.read::<serde_json::Value>(&file)? {
            if panes.iter().any(|pane| {
                pane["pane_id"] == saved["pane_id"]
                    && pane["terminal_id"] == saved["terminal_id"]
                    && pane["tab_id"] == tab
            }) {
                return Ok(saved);
            }
            std::fs::remove_file(registry.dir.join(&file))?;
        }
        let out = shell.run_ok(
            "herdr",
            &[
                "plugin".into(),
                "pane".into(),
                "open".into(),
                "--plugin".into(),
                "dogsled".into(),
                "--entrypoint".into(),
                "board".into(),
                "--placement".into(),
                "split".into(),
                "--direction".into(),
                "right".into(),
                "--target-pane".into(),
                owner.into(),
                "--no-focus".into(),
            ],
        )?;
        let opened: serde_json::Value = serde_json::from_str(&out.stdout)?;
        let pane = &opened["result"]["plugin_pane"]["pane"];
        if !pane["pane_id"].is_string() || !pane["terminal_id"].is_string() {
            return Err(Error::Invalid("invalid Herdr plugin pane response".into()));
        }
        registry.write(&file, pane)?;
        Ok(pane.clone())
    })
}

fn pane_width() -> u16 {
    let size = (|| {
        let tty = std::fs::File::open("/dev/tty").ok()?;
        let output = std::process::Command::new("stty")
            .arg("size")
            .stdin(tty)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8(output.stdout)
            .ok()?
            .split_whitespace()
            .nth(1)?
            .parse::<u16>()
            .ok()
    })();
    size.filter(|width| *width > 0).unwrap_or(DEFAULT_WIDTH)
}

/// Clip board-owned labels conservatively in terminal cells. Provider lines bypass this.
fn compact(text: &str, width: u16) -> String {
    let mut remaining = usize::from(width);
    text.chars()
        .filter(|c| !c.is_control())
        .take_while(|c| {
            let cells = if c.is_ascii() { 1 } else { 2 };
            if cells > remaining {
                return false;
            }
            remaining -= cells;
            true
        })
        .collect()
}

/// Display-only quotas: never calls the collector or reads credentials.
fn quota_lines(snapshot: &crate::quota::Snapshot, now: u64) -> Vec<String> {
    if snapshot.fetched_ms == 0 {
        return vec!["QUOTAS · unavailable".into()];
    }
    let elapsed = now.saturating_sub(snapshot.fetched_ms);
    let mut lines = vec![format!(
        "QUOTAS · age {}{}",
        age(elapsed),
        if elapsed > 15 * crate::watchdog::MINUTE {
            " · STALE"
        } else {
            ""
        }
    )];
    for row in &snapshot.pools {
        let pool = match row.pool {
            crate::quota::Pool::Claude => "claude",
            crate::quota::Pool::Codex => "codex",
            crate::quota::Pool::Minimax => "minimax",
        };
        lines.push(format!("{pool} {} · {}", row.account, row.window));
        lines.push(format!(
            "  {:.1}% used{}",
            row.used_pct,
            row.error
                .as_ref()
                .map_or_else(String::new, |error| format!(" · {error}"))
        ));
    }
    if snapshot.pools.is_empty() {
        lines.push("No quota accounts".into());
    }
    lines
}

impl Board {
    pub fn render_side(
        &self,
        quota: &crate::quota::Snapshot,
        todos: &[(String, TodoLines)],
        owner: Option<&str>,
        width: u16,
        now: u64,
    ) -> String {
        let mut lines = quota_lines(quota, now);
        lines.push(String::new());
        lines.push("AGENTS AND JOBS".into());
        if self.rows.is_empty() {
            lines.push("nothing running".into());
        }
        let mut group = None;
        for row in &self.rows {
            if group != Some(row.group) {
                lines.push(row.group.title().into());
                group = Some(row.group);
            }
            lines.push(format!(
                "{} · {} · {}",
                row.name,
                row.state,
                age(row.age_ms)
            ));
            lines.push(format!("  {} · {}", row.what, row.host));
            if !row.activity.is_empty() {
                lines.push(format!("  {}", one_line(&row.activity)));
            }
        }
        lines.push(String::new());
        lines.push("TODOS".into());
        for row in self.rows.iter().filter(|row| row.kind == "agent") {
            let current = todos
                .iter()
                .find(|(host, todo)| row.host == *host && row.pane.as_deref() == Some(&todo.pane))
                .map_or("unavailable", |(_, todo)| todo.current.as_str());
            lines.push(format!("{}: {}", row.name, one_line(current)));
        }
        for (host, todo) in todos
            .iter()
            .filter(|(host, todo)| host == "local" && Some(todo.pane.as_str()) == owner)
        {
            lines.push(format!("{host} {}: {}", todo.pane, one_line(&todo.current)));
        }
        let mut lines: Vec<_> = lines.iter().map(|line| compact(line, width)).collect();
        if let Some((_, todo)) = todos
            .iter()
            .find(|(host, todo)| host == "local" && Some(todo.pane.as_str()) == owner)
        {
            lines.push(String::new());
            // No prefix, truncation, ANSI rewriting, or task-state interpretation here.
            lines.extend(todo.lines.iter().cloned());
        } else {
            lines.push("Owner todo view unavailable".into());
        }
        lines.join("\n") + "\n"
    }
}

pub fn command(
    shell: &dyn Shell,
    ledger: &Ledger,
    policy: &crate::config::Policy,
    home: String,
    command: crate::cli::Command,
    json: bool,
) -> Result<()> {
    let crate::cli::Command::Board {
        owner: requested,
        all,
        side,
        watch,
        open: opening,
        todo_path: publishing,
    } = command
    else {
        return Err(Error::Invalid("expected board command".into()));
    };
    let owner = owner(requested, all)?;
    if publishing {
        return crate::print(
            true,
            &todo_path(ledger, owner.as_deref().unwrap_or_default())?,
            String::new,
        );
    }
    if opening {
        return crate::print(
            true,
            &open(ledger, shell, owner.as_deref().unwrap_or_default())?,
            String::new,
        );
    }
    let fleet = crate::hosts::Fleet::discover(shell, ledger, policy, home)?;
    if side {
        return show_side(&fleet, owner.as_deref(), watch);
    }
    let status = crate::commands::status(&fleet)?;
    let board = build(&status, owner.as_deref(), crate::ledger::now_ms());
    crate::print(json, &board, || board.render())
}

fn show_side(fleet: &crate::hosts::Fleet<'_>, owner: Option<&str>, watch: bool) -> Result<()> {
    use std::io::Write as _;
    loop {
        let width = pane_width();
        if let Some(owner) = owner {
            fleet
                .ledger
                .child("board-widths")
                .write(&format!("{}.json", key(owner)), &width)?;
        }
        let status = crate::commands::status(fleet)?;
        let board = build(&status, owner, crate::ledger::now_ms());
        let quota = crate::quota::cached(fleet.ledger)?;
        let mut todos: Vec<_> = cached_todos(fleet.ledger)?
            .into_iter()
            .map(|todo| ("local".into(), todo))
            .collect();
        let mut errors = Vec::new();
        for host in &fleet.hosts {
            if host.is_local() || !board.rows.iter().any(|row| row.host == host.label) {
                continue;
            }
            match fleet.remote::<Vec<TodoLines>>(host, &["todos".into()]) {
                Ok(remote) => {
                    todos.extend(remote.into_iter().map(|todo| (host.label.clone(), todo)));
                }
                Err(_) => errors.push(format!("{}: todos unavailable", host.label)),
            }
        }
        // Keep only the owner's view and its visible agent rows (including remote pane collisions).
        todos.retain(|(host, todo)| {
            (host == "local" && Some(todo.pane.as_str()) == owner)
                || board.rows.iter().any(|row| {
                    row.kind == "agent"
                        && row.host == *host
                        && row.pane.as_deref() == Some(&todo.pane)
                })
        });
        if watch {
            print!("\x1b[2J\x1b[H");
        }
        print!(
            "{}",
            board.render_side(&quota, &todos, owner, width, crate::ledger::now_ms())
        );
        for error in errors {
            println!("{}", compact(&error, width));
        }
        std::io::stdout().flush()?;
        if !watch {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(10));
    }
    Ok(())
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

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
    /// When an unreachable host is tried again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_ms: Option<u64>,
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
    // Idle alone waits for input; only a child that called `done` has finished.
    let waiting = matches!(state.as_str(), "idle" | "done");
    let finished = agent.is_some_and(|a| a.done) && waiting;
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
            match quiet_ms.filter(|q| *q >= QUIET_MS && !waiting) {
                Some(quiet) if last.is_empty() => format!("quiet {}", age(quiet)),
                Some(quiet) => format!("quiet {} · last: {last}", age(quiet)),
                None => last,
            }
        }
    };
    let group = if group == Group::Running && quiet_ms.is_some_and(|q| q >= QUIET_MS) && !waiting {
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
        name: job
            .and_then(|j| j.cmd.first())
            .map(|c| c.rsplit('/').next().unwrap_or(c).to_owned())
            .or_else(|| item.name.clone())
            .unwrap_or_else(|| item.id.clone()),
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
                reason: h
                    .headroom
                    .as_ref()
                    .and_then(|h| h.reason.clone())
                    .or_else(|| (!h.reachable).then(|| h.error.clone()).flatten()),
                retry_ms: h.retry_ms,
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

    /// The finished children with work to land, as `worker (+2), web (uncommitted)`: at most
    /// five, then a count of the rest. `None` when there are none.
    pub fn land_names(&self) -> Option<String> {
        let children: Vec<String> = self
            .rows
            .iter()
            .filter(|r| r.group == Group::ToLand)
            .map(|r| {
                let detail = [
                    r.ahead.map(|n| format!("+{n}")),
                    r.dirty.then(|| "uncommitted".to_owned()),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(", ");
                format!("{} ({detail})", one_line(&r.name))
            })
            .collect();
        if children.is_empty() {
            return None;
        }
        let shown = children.len().min(5);
        let more = children.len() - shown;
        Some(format!(
            "{}{}",
            children[..shown].join(", "),
            if more == 0 {
                String::new()
            } else {
                format!(", and {more} more (see `dogsled board`)")
            }
        ))
    }

    /// The owner's own lines for prompt context, before the fleet's: what it started, by
    /// group, and the children with work to land. Empty when the owner started nothing.
    pub fn owner_lines(&self) -> String {
        if self.rows.is_empty() {
            return String::new();
        }
        let mut counts = std::collections::BTreeMap::new();
        for row in &self.rows {
            let label = match (row.group, row.state.as_str()) {
                (Group::ToLand, _) => (0, "to land"),
                (Group::Attention, _) => (1, "attention"),
                (Group::Running, "idle") => (2, "idle"),
                (Group::Running, "done") => (3, "done"),
                (Group::Running, _) => (4, "running"),
                (Group::Waiting, _) => (5, "waiting"),
            };
            *counts.entry(label).or_insert(0) += 1;
        }
        let summary = counts
            .iter()
            .map(|((_, label), n)| format!("{n} {label}"))
            .collect::<Vec<_>>()
            .join(" · ");
        let mut out = format!("Your children and jobs: {summary}\n");
        if let Some(names) = self.land_names() {
            let _ = writeln!(
                out,
                "To land: {names}. Review and integrate finished work. Ask dirty children to commit. Dogsled closes children after their work lands."
            );
        }
        out
    }

    /// Plain text, one row per line, for `dogsled board` without `--json`.
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

    fn quota_fixture() -> crate::quota::Snapshot {
        serde_json::from_value(serde_json::json!({
            "version": 1, "fetched_ms": NOW - 16 * crate::watchdog::MINUTE, "source": "mac",
            "pools": [{"pool":"claude", "account":"work", "window":"five_hour", "used_pct":42.5, "resets_ms":null, "error":null}]
        })).unwrap()
    }

    fn todo_fixture(pane: &str) -> TodoLines {
        TodoLines {
            session: "session".into(),
            branch: Some("branch".into()),
            revision: 3,
            current: "Verify changes".into(),
            pane: pane.into(),
            width: 48,
            lines: vec![
                "\u{1b}[32mProvider-owned ✓\u{1b}[0m  ".into(),
                "  second line".into(),
            ],
        }
    }

    #[test]
    fn board_side_uses_cached_quota_age_and_marks_only_older_than_fifteen_minutes() {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        ledger.write("quota.json", &quota_fixture()).unwrap();
        let quota = crate::quota::cached(&ledger).unwrap();
        let board = Board {
            hosts: vec![],
            rows: vec![],
        };
        let rendered = board.render_side(&quota, &[], None, 48, NOW);
        assert!(rendered.contains("QUOTAS · age 16m · STALE"), "{rendered}");
        assert!(rendered.contains("claude work · five_hour"));
        assert!(rendered.contains("42.5% used"));
        assert!(
            !board
                .render_side(
                    &quota,
                    &[],
                    None,
                    48,
                    quota.fetched_ms + 15 * crate::watchdog::MINUTE
                )
                .contains("STALE")
        );
        assert!(rendered.find("QUOTAS").unwrap() < rendered.find("AGENTS AND JOBS").unwrap());
        assert!(rendered.find("AGENTS AND JOBS").unwrap() < rendered.find("TODOS").unwrap());
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn board_side_prints_owner_provider_lines_byte_for_byte_and_not_remote_collisions() {
        let board = Board {
            hosts: vec![],
            rows: vec![],
        };
        let local = todo_fixture("w1:p1");
        let mut remote = local.clone();
        remote.lines = vec!["remote full view must not appear".into()];
        let rendered = board.render_side(
            &quota_fixture(),
            &[("peer".into(), remote), ("local".into(), local.clone())],
            Some("w1:p1"),
            48,
            NOW,
        );
        assert!(rendered.ends_with(&(local.lines.join("\n") + "\n")));
        assert!(!rendered.contains("remote full view"));
        assert!(rendered.contains("local w1:p1: Verify changes"));
        for width in [1, 8, 24, 48] {
            assert!(
                compact("界a界b\u{1b}", width)
                    .chars()
                    .map(|c| if c.is_ascii() { 1 } else { 2 })
                    .sum::<u16>()
                    <= width
            );
        }
    }

    #[test]
    fn board_todo_intake_roundtrips_lines_and_drops_closed_or_invalid_panes() {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let host = host("local", "local", vec![agent("w1:p1", "pi", "working")]);
        let snapshot = crate::node::Snapshot {
            sampled_ms: NOW,
            probe: host.probe.unwrap(),
            jobs: vec![],
        };
        ledger.write("host.json", &snapshot).unwrap();
        let todo = todo_fixture("w1:p1");
        let location = todo_path(&ledger, "w1:p1").unwrap();
        assert_eq!(location["width"], 48);
        ledger
            .child("todo-lines")
            .write(&format!("{}.json", key("w1:p1")), &todo)
            .unwrap();
        collect_todos(&ledger).unwrap();
        assert_eq!(cached_todos(&ledger).unwrap(), vec![todo.clone()]);
        let mut invalid = todo;
        invalid.pane = "another".into();
        ledger
            .child("todo-lines")
            .write(&format!("{}.json", key("w1:p1")), &invalid)
            .unwrap();
        collect_todos(&ledger).unwrap();
        assert_eq!(cached_todos(&ledger).unwrap(), Vec::<TodoLines>::new());
        let mut snapshot = snapshot;
        snapshot.probe.agent_panes.clear();
        ledger.write("host.json", &snapshot).unwrap();
        collect_todos(&ledger).unwrap();
        assert_eq!(cached_todos(&ledger).unwrap(), Vec::<TodoLines>::new());
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn board_open_requests_right_split_and_reuses_only_matching_live_terminal() {
        #[derive(Debug, Default)]
        struct Herdr {
            opens: std::sync::atomic::AtomicUsize,
        }
        impl Shell for Herdr {
            fn run(&self, program: &str, args: &[String]) -> Result<crate::shell::Output> {
                assert_eq!(program, "herdr");
                let pane = serde_json::json!({"pane_id":"w1:p2", "terminal_id":"term-board", "tab_id":"w1:t1"});
                let value = if args[0] == "pane" {
                    let mut panes = vec![
                        serde_json::json!({"pane_id":"w1:p1", "terminal_id":"term-owner", "tab_id":"w1:t1"}),
                    ];
                    if self.opens.load(std::sync::atomic::Ordering::Relaxed) > 0 {
                        panes.push(pane);
                    }
                    serde_json::json!({"result":{"panes":panes}})
                } else {
                    assert!(args.windows(2).any(|pair| pair == ["--direction", "right"]));
                    assert!(args.windows(2).any(|pair| pair == ["--placement", "split"]));
                    assert!(args.contains(&"--no-focus".into()));
                    self.opens
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    serde_json::json!({"result":{"plugin_pane":{"pane":pane}}})
                };
                Ok(crate::shell::Output {
                    status: 0,
                    stdout: value.to_string(),
                    stderr: String::new(),
                })
            }
        }
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let shell = Herdr::default();
        let first = open(&ledger, &shell, "w1:p1").unwrap();
        assert_eq!(open(&ledger, &shell, "w1:p1").unwrap(), first);
        assert_eq!(shell.opens.load(std::sync::atomic::Ordering::Relaxed), 1);
        ledger
            .child("boards")
            .write(
                &format!("{}.json", key("w1:t1")),
                &serde_json::json!({"pane_id":"w1:p2", "terminal_id":"old"}),
            )
            .unwrap();
        open(&ledger, &shell, "w1:p1").unwrap();
        assert_eq!(shell.opens.load(std::sync::atomic::Ordering::Relaxed), 2);
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

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
            state_change_seq: None,
            cwd: None,
            session: Session::default(),
            worktree: None,
            done: false,
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
            retry_ms: None,
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
            sweep: None,
        }
    }

    #[test]
    fn rows_group_by_what_the_owner_must_do() {
        let mut writer = agent("w2:p1", "pi", "done");
        writer.done = true;
        writer.worktree = Some(Worktree {
            ahead: 2,
            ..Worktree::default()
        });
        // Idle with commits, but it never called `done`: it waits for input.
        let mut asking = agent("w2:p6", "pi", "idle");
        asking.worktree = writer.worktree.clone();
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
                vec![writer, landed, working, quiet, blocked, asking],
            )],
            vec![
                item("web-tests", "8103", "w2:p1"),
                item("landed", "8103", "w2:p2"),
                item("review-auth", "8103", "w2:p3"),
                item("i18n", "8103", "w2:p4"),
                item("asks", "8103", "w2:p5"),
                item("question", "8103", "w2:p6"),
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
                ("question", Group::Running),
            ]
        );
        assert_eq!(board.rows[0].activity, "+2 commits");
        assert_eq!(board.rows[0].host, "netcup");
        assert_eq!(board.rows[1].activity, "quiet 11m · last: rg \"t(\" web/");
        assert_eq!(board.rows[4].what, "codex · gpt-5.6-sol");
        assert_eq!(board.rows[4].activity, "cargo test -p auth");
        assert_eq!(board.summary(), "1 to land · 2 attention · 3 running");
        assert!(board.owner_lines().starts_with("Your children and jobs: 1 to land · 2 attention · 2 idle · 1 running\nTo land: web-tests (+2)."));
        assert!(!board.owner_lines().contains("theirs"));
        assert_eq!(build(&status, None, NOW).rows.len(), 7);
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
