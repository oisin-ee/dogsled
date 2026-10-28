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

/// A todo's lifecycle state. The four sections always appear in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    InProgress,
    Pending,
    Completed,
    Abandoned,
}

const SECTION_ORDER: [TodoStatus; 4] = [
    TodoStatus::InProgress,
    TodoStatus::Pending,
    TodoStatus::Completed,
    TodoStatus::Abandoned,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TodoChild {
    pub status: TodoStatus,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub unit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TodoItem {
    pub status: TodoStatus,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub review: bool,
    pub unit: Option<String>,
    pub children: Vec<TodoChild>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TodoSection {
    pub status: TodoStatus,
    pub count: usize,
    pub items: Vec<TodoItem>,
}

/// Typed todo sections from pi-todo (protocol v3). The sidebar draws them itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TodoSections {
    pub session: String,
    pub branch: Option<String>,
    pub revision: u64,
    pub current: String,
    pub pane: String,
    pub sections: Vec<TodoSection>,
}

const MAX_TODO_BYTES: u64 = 1024 * 1024;

/// The service's cache of every live pane's sections. Protocol v3 changed its shape, so it has
/// a new name: the sidebar can start before the service upgrades, and must not read the
/// rendered-lines cache (`todos.json`) that an older service still writes.
const TODO_CACHE: &str = "todo-sections.json";

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
    Ok(serde_json::json!({
        "path": dir.dir.join(format!("{}.json", key(pane))),
    }))
}

fn validate_todo(todo: &TodoSections) -> Result<()> {
    let ordered = todo.sections.len() == SECTION_ORDER.len()
        && todo
            .sections
            .iter()
            .zip(SECTION_ORDER)
            .all(|(section, status)| {
                section.status == status && section.items.iter().all(|item| item.status == status)
            });
    if todo.session.is_empty() || todo.pane.is_empty() || todo.revision == 0 || !ordered {
        return Err(Error::Invalid("invalid todo sections snapshot".into()));
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
            let todo = (|| -> Result<TodoSections> {
                if bytes.len() as u64 > MAX_TODO_BYTES {
                    return Err(Error::Invalid("todo sections snapshot too large".into()));
                }
                let todo: TodoSections = serde_json::from_slice(&bytes)?;
                validate_todo(&todo)?;
                if todo.pane != agent.pane {
                    return Err(Error::Invalid("rendered todo pane mismatch".into()));
                }
                Ok(todo)
            })();
            match todo {
                Ok(todo) => todos.push(todo),
                Err(_) => eprintln!(
                    "dogsled board: invalid todo sections snapshot for {}",
                    agent.pane
                ),
            }
        }
    }
    ledger.write(TODO_CACHE, &todos)
}

pub fn cached_todos(ledger: &Ledger) -> Result<Vec<TodoSections>> {
    let todos: Vec<TodoSections> = ledger.read(TODO_CACHE)?.unwrap_or_default();
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

/// Cells the docked board takes beside Herdr's own sidebar.
const DOCK_WIDTH: f32 = 38.0;

fn pane_id(node: &serde_json::Value) -> Option<&str> {
    node["pane_id"].as_str()
}

/// The top-left pane: follow `first` through every split, right or down.
fn leftmost_pane(node: &serde_json::Value) -> Option<&str> {
    if node["type"] == "split" {
        leftmost_pane(&node["first"])
    } else {
        pane_id(node)
    }
}

/// Path (`false` = first, `true` = second) to the split whose first child is `pane`.
fn dock_split_path(node: &serde_json::Value, pane: &str) -> Option<Vec<bool>> {
    if node["type"] != "split" {
        return None;
    }
    if pane_id(&node["first"]) == Some(pane) {
        return Some(Vec::new());
    }
    for (side, child) in [(false, "first"), (true, "second")] {
        if let Some(mut rest) = dock_split_path(&node[child], pane) {
            rest.insert(0, side);
            return Some(rest);
        }
    }
    None
}

/// Herdr names a split `split_<n>_root` or `split_<n>_<bits>`, one bit per step (1 = second).
fn split_width(layout: &serde_json::Value, path: &[bool]) -> Option<u16> {
    let bits: String = path
        .iter()
        .map(|second| if *second { '1' } else { '0' })
        .collect();
    let bits = if bits.is_empty() {
        "root".to_owned()
    } else {
        bits
    };
    layout["splits"]
        .as_array()?
        .iter()
        .find(|split| {
            split["id"].as_str().is_some_and(|id| {
                id.split_once('_')
                    .and_then(|(_, rest)| rest.split_once('_'))
                    .is_some_and(|(_, tail)| tail == bits)
            })
        })
        .and_then(|split| u16::try_from(split["rect"]["width"].as_u64()?).ok())
}

fn dock_ratio(width: u16) -> f32 {
    (DOCK_WIDTH / f32::from(width.max(1))).clamp(f32::EPSILON, 1.0 - f32::EPSILON)
}

/// Herdr 0.9.5 creates a new plugin pane on every open. Keep one per live tab,
/// under the ledger lock, and validate its terminal identity before reuse.
/// A pane Dogsled launched is a child agent: the board belongs to its parent's
/// session, so a child never opens one (it would poll the fleet unseen).
/// A new board docks in the leftmost slot of the tab, 38 cells wide.
pub fn open(
    ledger: &Ledger,
    shell: &dyn Shell,
    socket: &crate::herdr_socket::Client,
    owner: &str,
) -> Result<serde_json::Value> {
    dock(ledger, shell, socket, owner, false)
}

/// Close this tab's live dock, or open it when there is none.
pub fn toggle(
    ledger: &Ledger,
    shell: &dyn Shell,
    socket: &crate::herdr_socket::Client,
    owner: &str,
) -> Result<serde_json::Value> {
    dock(ledger, shell, socket, owner, true)
}

fn dock(
    ledger: &Ledger,
    shell: &dyn Shell,
    socket: &crate::herdr_socket::Client,
    owner: &str,
    toggling: bool,
) -> Result<serde_json::Value> {
    if crate::launch::launched(ledger, owner)? {
        return Ok(serde_json::json!({"skipped": "child"}));
    }
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
            let live = panes.iter().any(|pane| {
                pane["pane_id"] == saved["pane_id"]
                    && pane["terminal_id"] == saved["terminal_id"]
                    && pane["tab_id"] == tab
            });
            if live && !toggling {
                return Ok(saved);
            }
            if live {
                let id = saved["pane_id"].as_str().unwrap_or_default();
                shell.run_ok(
                    "herdr",
                    &["plugin".into(), "pane".into(), "close".into(), id.into()],
                )?;
                std::fs::remove_file(registry.dir.join(&file))?;
                return Ok(serde_json::json!({"closed": id}));
            }
            std::fs::remove_file(registry.dir.join(&file))?;
        }
        let focused = panes
            .iter()
            .find(|pane| pane["focused"] == true)
            .and_then(|pane| pane["pane_id"].as_str())
            .unwrap_or(owner)
            .to_owned();
        let target = leftmost_pane(&socket.layout_export(owner)?["root"])
            .unwrap_or(owner)
            .to_owned();
        let out = shell.run_ok(
            "herdr",
            &[
                "plugin".into(),
                "pane".into(),
                "open".into(),
                "--plugin".into(),
                "dogsled".into(),
                "--entrypoint".into(),
                "sidebar".into(),
                "--placement".into(),
                "split".into(),
                "--direction".into(),
                "right".into(),
                "--target-pane".into(),
                target.clone(),
                "--no-focus".into(),
            ],
        )?;
        let opened: serde_json::Value = serde_json::from_str(&out.stdout)?;
        let pane = &opened["result"]["plugin_pane"]["pane"];
        let (Some(new), true) = (pane["pane_id"].as_str(), pane["terminal_id"].is_string()) else {
            return Err(Error::Invalid("invalid Herdr plugin pane response".into()));
        };
        shell.run_ok(
            "herdr",
            &[
                "pane".into(),
                "swap".into(),
                "--source-pane".into(),
                new.into(),
                "--target-pane".into(),
                target,
            ],
        )?;
        let exported = socket.layout_export(new)?;
        let path = dock_split_path(&exported["root"], new)
            .ok_or_else(|| Error::Invalid("herdr layout.export: dock split not found".into()))?;
        let width = split_width(&socket.pane_layout(new)?, &path).ok_or_else(|| {
            Error::Invalid("herdr pane.layout: dock split width not found".into())
        })?;
        socket.set_split_ratio(tab, &path, dock_ratio(width))?;
        socket.focus_pane(&focused)?;
        registry.write(&file, pane)?;
        Ok(pane.clone())
    })
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
        open: opening,
        toggle: toggling,
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
    if opening || toggling {
        let socket = crate::herdr_socket::Client::from_env()?;
        let owner = owner.as_deref().unwrap_or_default();
        let result = if toggling {
            toggle(ledger, shell, &socket, owner)?
        } else {
            open(ledger, shell, &socket, owner)?
        };
        return crate::print(true, &result, String::new);
    }
    let fleet = crate::hosts::Fleet::discover(shell, ledger, policy, home)?;
    let status = crate::commands::status(&fleet)?;
    let board = build(&status, owner.as_deref(), crate::ledger::now_ms());
    crate::print(json, &board, || board.render())
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
            Self::ToLand => "READY TO MERGE",
            Self::Attention => "NEEDS YOU",
            Self::Running => "WORKING",
            Self::Waiting => "QUEUED",
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_pane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<portal_types::Cleanup>,
    /// A worker in idle/done/closed state whose newest inbox message to its owner
    /// is pending or claimed, not delivered (read) or failed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unseen: bool,
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
    /// Canonical `host/owner/name` of the agent's repository, when it is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
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

/// The group a lifecycle state belongs to. NEEDS YOU holds the states the owner must act
/// on; the rest keep working. A retained branch is a result, not a state, so it has its
/// own group.
fn group_for(state: crate::lifecycle::State) -> Group {
    use crate::lifecycle::State as Life;
    match state {
        Life::Asking | Life::Blocked | Life::Stopped | Life::Lost | Life::Unknown => {
            Group::Attention
        }
        Life::Starting | Life::Working | Life::Waiting | Life::Finishing | Life::Done => {
            Group::Running
        }
        Life::Queued => Group::Waiting,
    }
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

/// A closed agent whose branch is not landed yet: its pane is gone, its branch stays.
fn closed_branch_row(
    item: &Item,
    branch: &crate::done::Branch,
    hosts: &[HostReport],
    now: u64,
) -> Row {
    Row {
        group: Group::ToLand,
        kind: "agent".into(),
        id: item.id.clone(),
        name: item.name.clone().unwrap_or_else(|| item.pane.clone()),
        what: what(item, None),
        host: label(hosts, &item.host),
        pane: None,
        owner_pane: Some(item.owner_pane.clone()),
        title: None,
        cleanup: None,
        unseen: false,
        state: "closed".into(),
        activity: format!("branch {} +{} to land", branch.name, branch.ahead),
        age_ms: now.saturating_sub(item.created_ms),
        quiet_ms: None,
        ahead: Some(branch.ahead),
        dirty: false,
        repository: None,
    }
}

fn agent_row(item: &Item, hosts: &[HostReport], now: u64) -> Row {
    if item.closed_ms.is_some()
        && let Some(branch) = &item.branch
    {
        return closed_branch_row(item, branch, hosts, now);
    }
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
    // The lifecycle owner derives the state from the evidence. The board keeps its own
    // wording and its own groups: it asks the owner, and it never derives a second
    // state of its own.
    let lifecycle = crate::items::lifecycle(item, hosts);
    let stalled = agent.is_some_and(|a| a.stalled_since_ms.is_some());
    let state = if stalled {
        "stalled".to_owned()
    } else {
        crate::lifecycle::display(lifecycle.state).to_owned()
    };
    let waiting = crate::watchdog::turn_ended(lifecycle.state);
    let quiet_eligible = lifecycle.state == crate::lifecycle::State::Working;
    let finished = lifecycle.state == crate::lifecycle::State::Done;
    let needs_attention = stalled || group_for(lifecycle.state) == Group::Attention;
    let group = if needs_attention {
        Group::Attention
    } else if finished && waiting && (dirty || (ahead.is_some() && !landed)) {
        Group::ToLand
    } else {
        group_for(lifecycle.state)
    };
    let signal = item
        .signal
        .as_ref()
        .filter(|s| agent.is_some_and(|a| a.signal.as_deref() == Some(s.kind.as_str())));
    let activity = if let Some(signal) = signal {
        signal.text.clone()
    } else {
        match (group, agent) {
            (Group::ToLand, _) => [
                ahead.map(|n| format!("+{n} commit{}", if n == 1 { "" } else { "s" })),
                dirty.then(|| "uncommitted changes".to_owned()),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · "),
            (_, None) if !host_up => format!("{} is unavailable", label(hosts, &item.host)),
            (_, None) => crate::lifecycle::display(lifecycle.state).to_owned(),
            (_, Some(a)) => {
                let last = a.session.activity.clone().unwrap_or_default();
                match quiet_ms.filter(|q| *q >= QUIET_MS && quiet_eligible) {
                    Some(quiet) if last.is_empty() => format!("quiet {}", age(quiet)),
                    Some(quiet) => format!("quiet {} · last: {last}", age(quiet)),
                    None => last,
                }
            }
        }
    };
    let group =
        if group == Group::Running && quiet_ms.is_some_and(|q| q >= QUIET_MS) && quiet_eligible {
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
        owner_pane: Some(item.owner_pane.clone()),
        title: None,
        cleanup: None,
        unseen: false,
        state,
        activity,
        age_ms: now.saturating_sub(item.created_ms),
        quiet_ms,
        ahead,
        dirty,
        repository: agent.and_then(|a| a.repository.clone()),
    }
}

/// The repository of the checkout a job runs in, when an observed agent on that host names
/// the same checkout. The board reads no Git state of its own, so a job in an unobserved
/// checkout has no repository.
fn job_repository(
    hosts: &[HostReport],
    item: &Item,
    job: Option<&crate::jobs::Job>,
) -> Option<String> {
    let job = job?;
    let host = hosts.iter().find(|h| h.host.id == item.host)?;
    host.probe
        .as_ref()?
        .agent_panes
        .iter()
        .filter(|a| {
            a.cwd
                .as_deref()
                .is_some_and(|cwd| same_checkout(cwd, &job.cwd))
        })
        .find_map(|a| a.repository.clone())
}

/// Two paths name the same checkout when one is the other, or sits inside it.
fn same_checkout(a: &str, b: &str) -> bool {
    let (a, b) = (std::path::Path::new(a), std::path::Path::new(b));
    a == b || a.starts_with(b) || b.starts_with(a)
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
        owner_pane: Some(item.owner_pane.clone()),
        title: None,
        cleanup: None,
        unseen: false,
        state,
        activity: job.map_or_else(String::new, |j| crate::activity::plain(&j.cmd.join(" "))),
        age_ms: now.saturating_sub(item.created_ms),
        quiet_ms: None,
        ahead: None,
        dirty: false,
        repository: job_repository(hosts, item, job),
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
    let reason = ticket.wait_reason.clone().or_else(|| {
        hosts
            .iter()
            .filter(|h| target.as_deref().is_none_or(|t| h.host.label == t))
            .filter_map(|h| h.headroom.as_ref()?.reason.as_ref().map(|r| (h, r)))
            .map(|(h, r)| format!("{}: {r}", h.host.label))
            .next()
    });
    Row {
        group: Group::Waiting,
        kind: kind.into(),
        id: ticket.id.clone(),
        name,
        what,
        host: target.clone().unwrap_or_else(|| "any".into()),
        pane: None,
        owner_pane: Some(request.owner.clone()),
        title: None,
        cleanup: None,
        unseen: false,
        state: crate::lifecycle::display(
            crate::lifecycle::classify(&crate::lifecycle::ticket_evidence(&ticket.state)).state,
        )
        .to_owned(),
        activity: reason.map_or_else(|| "for a host with room".to_owned(), |r| format!("for {r}")),
        age_ms: now.saturating_sub(ticket.created_ms),
        quiet_ms: None,
        ahead: None,
        dirty: false,
        repository: ticket
            .request
            .repository
            .as_ref()
            .and_then(|repo| crate::checkout::identity(&repo.origin)),
    }
}

/// The board for `owner`, or for every owner when `None`.
pub fn build(status: &Status, owner: Option<&str>, now: u64) -> Board {
    let mine = |o: &str| owner.is_none_or(|owner| owner == o);
    let mut rows: Vec<Row> = status
        .items
        .iter()
        .filter(|i| {
            mine(&i.owner_pane)
                && (i.closed_ms.is_none()
                    || (i.dropped_ms.is_none() && i.branch.as_ref().is_some_and(|b| !b.landed)))
        })
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

/// Add portal metadata on the blocking board reader, never on an HTTP request.
pub fn enrich(
    board: &mut Board,
    status: &Status,
    todos: &[TodoSections],
    ledger: &Ledger,
    shell: &dyn Shell,
) -> Result<()> {
    let messages = inbox_messages(ledger)?;
    enrich_rows(board, status, todos, &messages, shell);
    Ok(())
}

fn inbox_messages(ledger: &Ledger) -> Result<Vec<crate::inbox::Record>> {
    let mut messages = Vec::new();
    let directory = ledger.child("inbox").dir;
    match std::fs::read_dir(directory) {
        Ok(entries) => {
            for entry in entries {
                let path = entry?.path();
                if let Some(terminal) = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_suffix(".json"))
                    .filter(|n| !n.ends_with(".reader"))
                {
                    messages.extend(crate::inbox::received(ledger, terminal)?);
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(messages)
}

fn enrich_rows(
    board: &mut Board,
    status: &Status,
    todos: &[TodoSections],
    messages: &[crate::inbox::Record],
    shell: &dyn Shell,
) {
    let mut defaults = std::collections::BTreeMap::new();
    for row in &mut board.rows {
        let item = status.items.iter().find(|item| item.id == row.id);
        if row.state == "closed" {
            row.pane = item.map(|item| item.pane.clone());
        }
        row.title = todos
            .iter()
            .find(|todo| row.pane.as_deref() == Some(todo.pane.as_str()))
            .map(|todo| todo.current.clone());
        let pane = row
            .pane
            .as_deref()
            .or_else(|| item.map(|item| item.pane.as_str()));
        let ended = item.is_some_and(|item| {
            item.closed_ms.is_some()
                || crate::watchdog::turn_ended(crate::items::lifecycle(item, &status.hosts).state)
        });
        row.unseen = unseen(row, pane, messages, ended);
        if row.group != Group::ToLand {
            continue;
        }
        if row.dirty {
            row.cleanup = Some(cleanup(portal_types::CleanupCause::Dirty));
            continue;
        }
        let Some(item) = item else { continue };
        let local = status
            .hosts
            .iter()
            .any(|host| host.host.id == item.host && host.host.is_local());
        if !local {
            continue;
        }
        let tree = agent(&status.hosts, item).and_then(|(_, agent)| agent.worktree.as_ref());
        let location = tree
            .map(|tree| (tree.path.as_str(), format!("refs/heads/{}", tree.branch)))
            .or_else(|| {
                item.branch
                    .as_ref()
                    .map(|branch| (branch.repo.as_str(), format!("refs/heads/{}", branch.name)))
            });
        let Some((repo, tip)) = location else {
            continue;
        };
        let repository = row.repository.clone().unwrap_or_else(|| repo.to_owned());
        let default = defaults.entry(repository).or_insert_with(|| {
            let output = shell
                .run(
                    "git",
                    &[
                        "-C".into(),
                        repo.into(),
                        "symbolic-ref".into(),
                        "--quiet".into(),
                        "refs/remotes/origin/HEAD".into(),
                    ],
                )
                .ok()?;
            output
                .ok()
                .then(|| {
                    output
                        .stdout
                        .trim()
                        .strip_prefix("refs/remotes/origin/")
                        .map(|name| format!("refs/heads/{name}"))
                })
                .flatten()
        });
        let Some(default) = default else { continue };
        let output = shell.run(
            "git",
            &[
                "-C".into(),
                repo.into(),
                "merge-base".into(),
                "--is-ancestor".into(),
                tip,
                default.clone(),
            ],
        );
        row.cleanup = output
            .ok()
            .and_then(|output| classify_cleanup(row, output.status));
    }
}

fn cleanup(cause: portal_types::CleanupCause) -> portal_types::Cleanup {
    use portal_types::{CleanupAction, CleanupCause};
    portal_types::Cleanup {
        cause,
        action: match cause {
            CleanupCause::NeverMerged => CleanupAction::Land,
            CleanupCause::Dirty => CleanupAction::Commit,
            CleanupCause::MergedNotPushed => CleanupAction::PushMain,
        },
    }
}

fn classify_cleanup(row: &Row, local_ancestry_status: i32) -> Option<portal_types::Cleanup> {
    if row.group != Group::ToLand {
        return None;
    }
    let cause = if row.dirty {
        portal_types::CleanupCause::Dirty
    } else if row.ahead.is_some_and(|ahead| ahead > 0) {
        match local_ancestry_status {
            0 => portal_types::CleanupCause::MergedNotPushed,
            1 => portal_types::CleanupCause::NeverMerged,
            _ => return None,
        }
    } else {
        return None;
    };
    Some(cleanup(cause))
}

/// Only the newest worker-to-owner inbox message matters. Pending and claimed
/// messages are unread; delivered (acknowledged) and failed messages are not.
fn unseen(row: &Row, pane: Option<&str>, messages: &[crate::inbox::Record], ended: bool) -> bool {
    if row.kind != "agent" || !ended {
        return false;
    }
    let (Some(owner), Some(pane)) = (row.owner_pane.as_deref(), pane) else {
        return false;
    };
    messages
        .iter()
        .filter(|message| message.from.pane.as_deref() == Some(pane) && message.pane == owner)
        .max_by(|left, right| {
            left.created_ms
                .cmp(&right.created_ms)
                .then(left.id.cmp(&right.id))
        })
        .is_some_and(|message| message.state.open())
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
            let label = match row.group {
                Group::ToLand => (0, "ready to merge"),
                Group::Attention => (1, "needs you"),
                Group::Running => (2, "working"),
                Group::Waiting => (3, "queued"),
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
                "Ready to merge: {names}. Review and integrate the retained branches. Finished children close at handoff, before landing."
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

    fn todo_fixture(pane: &str) -> TodoSections {
        let section = |status, items: Vec<TodoItem>| TodoSection {
            status,
            count: items.len(),
            items,
        };
        TodoSections {
            session: "session".into(),
            branch: Some("branch".into()),
            revision: 3,
            current: "Verify changes".into(),
            pane: pane.into(),
            sections: vec![
                section(
                    TodoStatus::InProgress,
                    vec![TodoItem {
                        status: TodoStatus::InProgress,
                        text: "Land PR #112".into(),
                        description: Some("Release it.".into()),
                        review: false,
                        unit: None,
                        children: vec![],
                    }],
                ),
                section(TodoStatus::Pending, vec![]),
                section(TodoStatus::Completed, vec![]),
                section(TodoStatus::Abandoned, vec![]),
            ],
        }
    }

    #[test]
    fn todo_sections_require_four_ordered_sections() {
        let ok = todo_fixture("w1:p1");
        validate_todo(&ok).unwrap();
        let mut missing = ok.clone();
        missing.sections.pop();
        assert!(validate_todo(&missing).is_err());
        let mut swapped = ok.clone();
        swapped.sections.swap(0, 1);
        assert!(validate_todo(&swapped).is_err());
        let mut stray = ok;
        let moved = stray.sections[0].items[0].clone();
        stray.sections[1].items.push(moved);
        assert!(validate_todo(&stray).is_err());
        assert!(
            serde_json::from_str::<TodoSections>(r#"{"session":"s","branch":null,"revision":1,"current":"","pane":"p","width":48,"lines":[]}"#).is_err(),
            "the v2 rendered-lines shape must not parse"
        );
    }

    #[test]
    fn board_todo_intake_roundtrips_sections_and_drops_closed_or_invalid_panes() {
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
        assert!(location["path"].is_string());
        assert!(location.get("width").is_none());
        ledger
            .child("todo-lines")
            .write(&format!("{}.json", key("w1:p1")), &todo)
            .unwrap();
        // An older service's rendered-lines cache is a different file, so it cannot hide these.
        ledger
            .write(
                "todos.json",
                &serde_json::json!([{"session": "s", "pane": "w1:p1", "lines": [], "width": 36}]),
            )
            .unwrap();
        assert_eq!(cached_todos(&ledger).unwrap(), Vec::<TodoSections>::new());
        collect_todos(&ledger).unwrap();
        assert_eq!(cached_todos(&ledger).unwrap(), vec![todo.clone()]);
        let mut invalid = todo;
        invalid.pane = "another".into();
        ledger
            .child("todo-lines")
            .write(&format!("{}.json", key("w1:p1")), &invalid)
            .unwrap();
        collect_todos(&ledger).unwrap();
        assert_eq!(cached_todos(&ledger).unwrap(), Vec::<TodoSections>::new());
        let mut snapshot = snapshot;
        snapshot.probe.agent_panes.clear();
        ledger.write("host.json", &snapshot).unwrap();
        collect_todos(&ledger).unwrap();
        assert_eq!(cached_todos(&ledger).unwrap(), Vec::<TodoSections>::new());
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    fn leaf(id: &str) -> serde_json::Value {
        serde_json::json!({"type":"pane","pane_id":id})
    }

    #[allow(clippy::needless_pass_by_value)] // nested literals read best by value
    fn split(
        direction: &str,
        first: serde_json::Value,
        second: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({"type":"split","direction":direction,"ratio":0.5,"first":first,"second":second})
    }

    /// A fake Herdr: scripted CLI plus a socket whose export changes once the swap has run.
    struct Dock {
        shell: crate::shell::Fake,
        socket: crate::herdr_socket::fake::Server,
        ledger: Ledger,
    }

    fn dock_rig(
        before: serde_json::Value,
        after: serde_json::Value,
        split_id: &str,
        width: u16,
        target: &str,
    ) -> Dock {
        let shell = crate::shell::Fake::new();
        let panes = |extra: bool| {
            let mut panes = vec![
                serde_json::json!({"pane_id":"w1:p1","terminal_id":"term-owner","tab_id":"w1:t1","focused":true}),
            ];
            if extra {
                panes.push(serde_json::json!({"pane_id":"w1:p2","terminal_id":"term-board","tab_id":"w1:t1"}));
            }
            serde_json::json!({"result":{"panes":panes}}).to_string()
        };
        shell.on("herdr pane list", &panes(false));
        shell.on(
            &format!("herdr plugin pane open --plugin dogsled --entrypoint sidebar --placement split --direction right --target-pane {target} --no-focus"),
            &serde_json::json!({"result":{"plugin_pane":{"pane":{"pane_id":"w1:p2","terminal_id":"term-board","tab_id":"w1:t1"}}}}).to_string(),
        );
        shell.on(
            &format!("herdr pane swap --source-pane w1:p2 --target-pane {target}"),
            "{}",
        );
        let split_id = split_id.to_owned();
        let exports = std::sync::atomic::AtomicUsize::new(0);
        let socket = crate::herdr_socket::fake::Server::start(move |method, _| match method {
            "layout.export" => {
                let root = if exports.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    &before
                } else {
                    &after
                };
                serde_json::json!({"result":{"type":"layout_export","layout":{"root":root}}})
            }
            "pane.layout" => serde_json::json!({"result":{"type":"pane_layout","layout":{
                "splits":[{"id":"split_9_1","rect":{"width":1}},{"id":split_id,"rect":{"width":width}}]}}}),
            "layout.set_split_ratio" => {
                serde_json::json!({"result":{"type":"layout_split_ratio_set","layout":{}}})
            }
            "pane.focus" => serde_json::json!({"result":{"type":"pane_info","pane":{}}}),
            other => panic!("unexpected socket method {other}"),
        });
        Dock {
            shell,
            socket,
            ledger: Ledger::new(std::env::temp_dir().join(crate::ledger::new_id())),
        }
    }

    impl Dock {
        fn open(&self) -> serde_json::Value {
            open(&self.ledger, &self.shell, &self.socket.client(), "w1:p1").unwrap()
        }

        fn ratio_request(&self) -> serde_json::Value {
            let requests = self.socket.requests.lock().unwrap();
            let request = requests
                .iter()
                .find(|request| request["method"] == "layout.set_split_ratio")
                .expect("a set_split_ratio request");
            request["params"].clone()
        }

        /// The `pane.focus` requests this rig sent, in order.
        fn focus_requests(&self) -> Vec<serde_json::Value> {
            self.socket
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|request| request["method"] == "pane.focus")
                .cloned()
                .collect()
        }

        fn finish(self) {
            std::fs::remove_dir_all(&self.ledger.dir).unwrap();
        }
    }

    #[test]
    fn board_open_docks_left_of_a_single_pane_at_38_cells() {
        let rig = dock_rig(
            leaf("w1:p1"),
            split("right", leaf("w1:p2"), leaf("w1:p1")),
            "split_0_root",
            120,
            "w1:p1",
        );
        let pane = rig.open();
        assert_eq!(pane["pane_id"], "w1:p2");
        let calls = rig.shell.calls.lock().unwrap().clone();
        assert_eq!(calls[0], "herdr pane list");
        assert!(calls[1].contains("plugin pane open") && calls[1].contains("--target-pane w1:p1"));
        assert_eq!(
            calls[2],
            "herdr pane swap --source-pane w1:p2 --target-pane w1:p1"
        );
        assert_eq!(calls.len(), 3, "no other Herdr writes: {calls:?}");
        let params = rig.ratio_request();
        assert_eq!(params["tab_id"], "w1:t1");
        assert_eq!(params["path"], serde_json::json!([]));
        assert!((params["ratio"].as_f64().unwrap() - 38.0 / 120.0).abs() < 1e-6);
        let methods = rig.socket.methods();
        assert!(!methods.iter().any(|method| method == "layout.apply"));
        rig.finish();
    }

    #[test]
    fn board_open_restores_the_focus_the_caller_had() {
        let rig = dock_rig(
            leaf("w1:p1"),
            split("right", leaf("w1:p2"), leaf("w1:p1")),
            "split_0_root",
            120,
            "w1:p1",
        );
        rig.open();
        let requests = rig.socket.requests.lock().unwrap().clone();
        let last = requests.last().unwrap();
        assert_eq!(last["method"], "pane.focus");
        assert_eq!(last["params"]["pane_id"], "w1:p1");
        assert_eq!(rig.focus_requests().len(), 1);
        rig.finish();
    }

    #[test]
    fn board_open_restores_the_focus_of_whichever_pane_had_it() {
        let rig = dock_rig(
            leaf("w1:p1"),
            split("right", leaf("w1:p2"), leaf("w1:p1")),
            "split_0_root",
            120,
            "w1:p1",
        );
        rig.shell.on(
            "herdr pane list",
            &serde_json::json!({"result":{"panes":[
                {"pane_id":"w1:p1","terminal_id":"term-owner","tab_id":"w1:t1","focused":false},
                {"pane_id":"w1:p9","terminal_id":"term-other","tab_id":"w1:t1","focused":true}]}})
            .to_string(),
        );
        rig.open();
        let requests = rig.focus_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["params"]["pane_id"], "w1:p9");
        rig.finish();
    }

    #[test]
    fn board_open_falls_back_to_the_owner_when_no_pane_has_focus() {
        let rig = dock_rig(
            leaf("w1:p1"),
            split("right", leaf("w1:p2"), leaf("w1:p1")),
            "split_0_root",
            120,
            "w1:p1",
        );
        rig.shell.on(
            "herdr pane list",
            &serde_json::json!({"result":{"panes":[
                {"pane_id":"w1:p1","terminal_id":"term-owner","tab_id":"w1:t1","focused":false}]}})
            .to_string(),
        );
        rig.open();
        let requests = rig.focus_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["params"]["pane_id"], "w1:p1");
        rig.finish();
    }

    #[test]
    fn board_open_targets_the_leftmost_of_three_columns_not_the_owner() {
        let before = split(
            "right",
            leaf("w1:p5"),
            split("right", leaf("w1:p1"), leaf("w1:p3")),
        );
        let after = split(
            "right",
            split("right", leaf("w1:p2"), leaf("w1:p5")),
            split("right", leaf("w1:p1"), leaf("w1:p3")),
        );
        let rig = dock_rig(before, after, "split_1_0", 80, "w1:p5");
        rig.open();
        let calls = rig.shell.calls.lock().unwrap().clone();
        assert!(calls[1].contains("--target-pane w1:p5"), "{calls:?}");
        let params = rig.ratio_request();
        assert_eq!(params["path"], serde_json::json!([false]));
        assert!((params["ratio"].as_f64().unwrap() - 38.0 / 80.0).abs() < 1e-6);
        rig.finish();
    }

    #[test]
    fn board_open_takes_the_top_slot_of_a_split_leftmost_column() {
        let before = split(
            "right",
            split("down", leaf("w1:p5"), leaf("w1:p6")),
            leaf("w1:p1"),
        );
        let after = split(
            "right",
            split(
                "down",
                split("right", leaf("w1:p2"), leaf("w1:p5")),
                leaf("w1:p6"),
            ),
            leaf("w1:p1"),
        );
        let rig = dock_rig(before, after, "split_2_00", 100, "w1:p5");
        rig.open();
        let calls = rig.shell.calls.lock().unwrap().clone();
        assert!(calls[1].contains("--target-pane w1:p5"), "{calls:?}");
        assert_eq!(
            rig.ratio_request()["path"],
            serde_json::json!([false, false])
        );
        rig.finish();
    }

    #[test]
    fn board_open_reuses_a_live_dock_without_herdr_writes() {
        let rig = dock_rig(leaf("w1:p1"), leaf("w1:p1"), "split_0_root", 120, "w1:p1");
        let saved =
            serde_json::json!({"pane_id":"w1:p2","terminal_id":"term-board","tab_id":"w1:t1"});
        rig.ledger
            .child("boards")
            .write(&format!("{}.json", key("w1:t1")), &saved)
            .unwrap();
        rig.shell.on(
            "herdr pane list",
            &serde_json::json!({"result":{"panes":[
                {"pane_id":"w1:p1","terminal_id":"term-owner","tab_id":"w1:t1"},
                {"pane_id":"w1:p2","terminal_id":"term-board","tab_id":"w1:t1"}]}})
            .to_string(),
        );
        assert_eq!(rig.open(), saved);
        assert_eq!(*rig.shell.calls.lock().unwrap(), ["herdr pane list"]);
        assert_eq!(rig.socket.methods().len(), 0);
        assert!(
            rig.focus_requests().is_empty(),
            "a reused dock takes no focus"
        );
        rig.finish();
    }

    #[test]
    fn board_open_replaces_a_dock_whose_terminal_changed() {
        let rig = dock_rig(
            leaf("w1:p1"),
            split("right", leaf("w1:p2"), leaf("w1:p1")),
            "split_0_root",
            120,
            "w1:p1",
        );
        rig.ledger
            .child("boards")
            .write(
                &format!("{}.json", key("w1:t1")),
                &serde_json::json!({"pane_id":"w1:p2","terminal_id":"old"}),
            )
            .unwrap();
        assert_eq!(rig.open()["terminal_id"], "term-board");
        rig.finish();
    }

    #[test]
    fn board_toggle_closes_a_live_dock_and_opens_when_none() {
        let rig = dock_rig(
            leaf("w1:p1"),
            split("right", leaf("w1:p2"), leaf("w1:p1")),
            "split_0_root",
            120,
            "w1:p1",
        );
        let toggle =
            |rig: &Dock| toggle(&rig.ledger, &rig.shell, &rig.socket.client(), "w1:p1").unwrap();
        assert_eq!(toggle(&rig)["pane_id"], "w1:p2", "no dock: opens");
        let focus_after_open = rig.focus_requests().len();
        rig.shell.on(
            "herdr pane list",
            &serde_json::json!({"result":{"panes":[
                {"pane_id":"w1:p1","terminal_id":"term-owner","tab_id":"w1:t1"},
                {"pane_id":"w1:p2","terminal_id":"term-board","tab_id":"w1:t1"}]}})
            .to_string(),
        );
        rig.shell.on("herdr plugin pane close w1:p2", "{}");
        assert_eq!(toggle(&rig), serde_json::json!({"closed":"w1:p2"}));
        assert!(
            rig.shell
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| call == "herdr plugin pane close w1:p2")
        );
        assert!(
            !rig.ledger
                .child("boards")
                .dir
                .join(format!("{}.json", key("w1:t1")))
                .exists()
        );
        assert_eq!(
            rig.focus_requests().len(),
            focus_after_open,
            "closing a dock takes no focus"
        );
        rig.finish();
    }

    #[test]
    fn board_open_returns_herdrs_error_and_does_not_fall_back() {
        let rig = dock_rig(leaf("w1:p1"), leaf("w1:p1"), "split_0_root", 120, "w1:p1");
        let refusing = crate::herdr_socket::fake::Server::start(
            |_, _| serde_json::json!({"error":{"code":"layout_not_found","message":"layout target not found"}}),
        );
        let result = open(&rig.ledger, &rig.shell, &refusing.client(), "w1:p1");
        assert!(
            matches!(&result, Err(Error::Invalid(message)) if message.contains("layout target not found")),
            "{result:?}"
        );
        assert_eq!(
            *rig.shell.calls.lock().unwrap(),
            ["herdr pane list"],
            "no split, swap, or old right split"
        );
        rig.finish();
    }

    #[test]
    fn board_open_skips_a_pane_dogsled_launched() {
        #[derive(Debug)]
        struct NoHerdr;
        impl Shell for NoHerdr {
            fn run(&self, _: &str, args: &[String]) -> Result<crate::shell::Output> {
                panic!("a child pane must not reach Herdr: {args:?}")
            }
        }
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        ledger
            .child("launches")
            .write(
                "r-1-1-0.progress.json",
                &serde_json::json!({
                    "request":{"id":"r-1-1-0","action":{"type":"agent","kind":"pi","name":"worker","message":"go"},
                        "host":"local","cwd":"/checkout","pane":"w2:p2","terminal":"term_child","owner":"w1:p1"},
                    "pane":"w2:p2","terminal":"term_child","attempted":true,"confirmed":true,
                    "result":{},"error":null
                }),
            )
            .unwrap();
        assert_eq!(
            open(
                &ledger,
                &NoHerdr,
                &crate::herdr_socket::Client::at("/nonexistent"),
                "w2:p2"
            )
            .unwrap(),
            serde_json::json!({"skipped": "child"})
        );
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn board_open_skips_a_child_after_its_terminal_changes() {
        #[derive(Debug)]
        struct RestoredPane;
        impl Shell for RestoredPane {
            fn run(&self, program: &str, args: &[String]) -> Result<crate::shell::Output> {
                assert_eq!(program, "herdr");
                assert_eq!(args, ["pane", "list"], "a child must not open a board");
                Ok(crate::shell::Output {
                    status: 0,
                    stdout: serde_json::json!({"result":{"panes":[{
                        "pane_id":"w2:p2","terminal_id":"term_restored","tab_id":"tab_child"
                    }]}})
                    .to_string(),
                    stderr: String::new(),
                })
            }
        }
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        ledger.child("launches").write("child.progress.json", &serde_json::json!({
            "request":{"id":"child","action":{"type":"agent","kind":"pi","name":"worker","message":"go"},
                "host":"local","cwd":"/checkout","owner":"w1:p1"},
            "pane":"w2:p2","terminal":"term_before_restore","attempted":true,"confirmed":false,
            "result":{},"error":"response lost"
        })).unwrap();
        assert_eq!(
            crate::launch::final_pane(&ledger, &RestoredPane, "w2:p2").unwrap(),
            crate::launch::PaneLaunch::Unrecorded
        );
        assert_eq!(
            open(
                &ledger,
                &RestoredPane,
                &crate::herdr_socket::Client::at("/nonexistent"),
                "w2:p2"
            )
            .unwrap(),
            serde_json::json!({"skipped":"child"})
        );
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
            signal: None,
            terminal_id: None,
            agent_session: None,
            completion_seq: None,
            state_change_seq: None,
            cwd: None,
            session: Session::default(),
            worktree: None,
            repository: None,
            done: false,
            finishing: false,
            handoff: None,
            launch_progress: None,
            owns_open: false,
            lifecycle: None,
            lifecycle_gap: None,
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
            failure: None,
            jobs: Vec::new(),
            handoffs: Vec::new(),
            retry_ms: None,
            job_validity: None,
        }
    }

    /// The host reports the board projection reads.
    fn board_hosts() -> Vec<HostReport> {
        let mut waiting = agent("w2:p1", "pi", "idle");
        waiting.owns_open = true;
        vec![host("8103", "netcup", vec![waiting])]
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
    fn only_a_closed_item_leaves_the_board_once_its_branch_is_landed_or_dropped() {
        let branch = |landed: bool| crate::done::Branch {
            repo: "/repo".into(),
            name: "feat/x".into(),
            ahead: u64::from(!landed),
            landed,
        };
        let closed = |id: &str, landed, dropped_ms| Item {
            closed_ms: Some(NOW - 1),
            dropped_ms,
            branch: Some(branch(landed)),
            ..item(id, "8103", id)
        };
        let status = status(
            vec![host("8103", "netcup", vec![agent("open", "pi", "idle")])],
            vec![
                Item {
                    branch: Some(branch(true)),
                    ..item("open", "8103", "open")
                },
                closed("retained", false, None),
                closed("landed", true, None),
                closed("dropped", false, Some(NOW - 1)),
            ],
        );
        let board = build(&status, Some("w1:p1"), NOW);
        let names: Vec<_> = board.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["retained", "open"]);
    }

    #[test]
    fn portal_metadata_maps_owner_title_and_latest_inbox_state() -> Result<()> {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let status = status(
            vec![host("local", "local", vec![agent("worker", "pi", "idle")])],
            vec![item("child", "local", "worker")],
        );
        let mut board = build(&status, None, NOW);
        let mut todo = todo_fixture("worker");
        todo.current = "Implement inbox".into();
        let record = |id: &str, created_ms, state| crate::inbox::Record {
            id: id.into(),
            terminal: "term_owner".into(),
            pane: "w1:p1".into(),
            from: crate::inbox::Sender {
                pane: Some("worker".into()),
                ..Default::default()
            },
            kind: "message".into(),
            text: "Ready".into(),
            created_ms,
            state,
            lease: crate::backoff::Backoff::default(),
            reason: None,
            done_ms: None,
        };
        let unread = record("one", 1, crate::inbox::State::Pending);
        ledger
            .child("inbox")
            .write("term_owner.json", &vec![unread.clone()])?;
        let shell = crate::shell::Fake::new();
        enrich(&mut board, &status, &[todo], &ledger, &shell)?;
        assert_eq!(board.rows[0].owner_pane.as_deref(), Some("w1:p1"));
        assert_eq!(board.rows[0].title.as_deref(), Some("Implement inbox"));
        assert!(board.rows[0].unseen);
        let read = record("two", 2, crate::inbox::State::Delivered);
        let messages = [unread, read];
        assert!(!unseen(&board.rows[0], Some("worker"), &messages, true));
        assert!(unseen(&board.rows[0], Some("worker"), &messages[..1], true));
        assert!(!unseen(
            &board.rows[0],
            Some("worker"),
            &messages[..1],
            false
        ));
        board.rows[0].owner_pane = None;
        assert!(!unseen(
            &board.rows[0],
            Some("worker"),
            &messages[..1],
            true
        ));
        std::fs::remove_dir_all(ledger.dir)?;
        Ok(())
    }

    #[test]
    fn unread_blocked_agent_message_uses_lifecycle_not_the_display_label() {
        let status = status(
            vec![host(
                "local",
                "local",
                vec![agent("worker", "pi", "blocked")],
            )],
            vec![item("child", "local", "worker")],
        );
        let mut board = build(&status, None, NOW);
        let message = crate::inbox::Record {
            id: "unread".into(),
            terminal: "term_owner".into(),
            pane: "w1:p1".into(),
            from: crate::inbox::Sender {
                pane: Some("worker".into()),
                ..Default::default()
            },
            kind: "message".into(),
            text: "Approval required".into(),
            created_ms: 1,
            state: crate::inbox::State::Pending,
            lease: crate::backoff::Backoff::default(),
            reason: None,
            done_ms: None,
        };
        enrich_rows(
            &mut board,
            &status,
            &[],
            &[message],
            &crate::shell::Fake::new(),
        );
        assert!(board.rows[0].unseen);
    }

    #[test]
    fn explicit_questions_and_blockers_need_the_owner() {
        use crate::lifecycle::{Evidence, Signal, classify};
        for signal in [Signal::Ask, Signal::Blocked] {
            let state = classify(&Evidence {
                terminal: Some("idle"),
                owns_open: true,
                signal: Some(signal),
                ..Evidence::default()
            })
            .state;
            assert_eq!(group_for(state), Group::Attention);
        }
    }

    #[test]
    fn cleanup_reads_default_once_and_ancestry_once_per_row() {
        let mut first = agent("first", "pi", "idle");
        first.done = true;
        first.repository = Some("github.com/owner/repo".into());
        first.worktree = Some(Worktree {
            path: "/repo".into(),
            branch: "feature".into(),
            ahead: 2,
            ..Worktree::default()
        });
        let mut second = first.clone();
        second.pane = "second".into();
        second.worktree.as_mut().expect("worktree").path = "/repo/.worktrees/second".into();
        let status = status(
            vec![host("local", "local", vec![first, second])],
            vec![
                item("one", "local", "first"),
                item("two", "local", "second"),
            ],
        );
        let mut board = build(&status, None, NOW);
        let shell = crate::shell::Fake::new();
        shell.on(
            "git -C /repo symbolic-ref --quiet refs/remotes/origin/HEAD",
            "refs/remotes/origin/trunk\n",
        );
        let command = "git -C /repo merge-base --is-ancestor refs/heads/feature refs/heads/trunk";
        shell.on(command, "");
        let second_command = "git -C /repo/.worktrees/second merge-base --is-ancestor refs/heads/feature refs/heads/trunk";
        shell.on(second_command, "");
        enrich_rows(&mut board, &status, &[], &[], &shell);
        assert!(board.rows.iter().all(|row| {
            row.cleanup
                .as_ref()
                .is_some_and(|cleanup| cleanup.cause == portal_types::CleanupCause::MergedNotPushed)
        }));
        assert_eq!(shell.calls.lock().expect("calls").len(), 3);
        for command in [command, second_command] {
            shell.on_output(
                command,
                crate::shell::Output {
                    status: 1,
                    stdout: String::new(),
                    stderr: String::new(),
                },
            );
        }
        enrich_rows(&mut board, &status, &[], &[], &shell);
        assert!(board.rows.iter().all(|row| {
            row.cleanup
                .as_ref()
                .is_some_and(|cleanup| cleanup.cause == portal_types::CleanupCause::NeverMerged)
        }));
    }

    #[test]
    fn cleanup_causes_follow_ancestry_and_unknown_is_not_a_guess() {
        use portal_types::{CleanupAction, CleanupCause};
        let status = status(vec![], vec![item("child", "local", "worker")]);
        let mut row = build(&status, None, NOW).rows.remove(0);
        row.group = Group::ToLand;
        row.ahead = Some(2);
        assert_eq!(
            classify_cleanup(&row, 1),
            Some(portal_types::Cleanup {
                cause: CleanupCause::NeverMerged,
                action: CleanupAction::Land,
            })
        );
        assert_eq!(
            classify_cleanup(&row, 0),
            Some(portal_types::Cleanup {
                cause: CleanupCause::MergedNotPushed,
                action: CleanupAction::PushMain,
            })
        );
        assert_eq!(classify_cleanup(&row, 128), None);
        row.dirty = true;
        assert_eq!(
            classify_cleanup(&row, 128),
            Some(portal_types::Cleanup {
                cause: CleanupCause::Dirty,
                action: CleanupAction::Commit,
            })
        );
        row.group = Group::Running;
        assert_eq!(classify_cleanup(&row, 0), None);
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
                ("landed", Group::Attention),
                ("i18n", Group::Attention),
                ("asks", Group::Attention),
                ("question", Group::Attention),
                ("review-auth", Group::Running),
            ]
        );
        assert_eq!(board.rows[0].activity, "+2 commits");
        assert_eq!(board.rows[0].host, "netcup");
        assert_eq!(board.rows[2].activity, "quiet 11m · last: rg \"t(\" web/");
        assert_eq!(board.rows[5].what, "codex · gpt-5.6-sol");
        assert_eq!(board.rows[5].activity, "cargo test -p auth");
        assert_eq!(
            board.summary(),
            "1 ready to merge · 4 needs you · 1 working"
        );
        assert!(board.owner_lines().starts_with("Your children and jobs: 1 ready to merge · 4 needs you · 1 working\nReady to merge: web-tests (+2)."));
        assert!(!board.owner_lines().contains("theirs"));
        assert_eq!(build(&status, None, NOW).rows.len(), 7);
    }

    #[test]
    fn a_quiet_finishing_child_stays_working() {
        let mut finishing = agent("w2:p1", "pi", "idle");
        finishing.finishing = true;
        finishing.handoff = Some("accepted".into());
        finishing.transcript_modified = Some(NOW - 3 * QUIET_MS);
        finishing.session.activity = Some("recording handoff".into());
        let board = build(
            &status(
                vec![host("8103", "netcup", vec![finishing])],
                vec![item("finishing", "8103", "w2:p1")],
            ),
            Some("w1:p1"),
            NOW,
        );
        assert_eq!(board.rows.len(), 1);
        assert_eq!(board.rows[0].quiet_ms, Some(3 * QUIET_MS));
        assert_eq!(board.rows[0].group, Group::Running);
        assert!(!board.rows.iter().any(|row| row.group == Group::Attention));
        assert_eq!(board.rows[0].activity, "recording handoff");
        assert_eq!(board.summary(), "1 working");
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

    /// The production board projection must read the state from the one owner, so a
    /// consumer can never derive a second, different state.
    #[test]
    fn the_board_projection_reads_the_shared_lifecycle_owner() {
        let mut waiting = agent("w2:p1", "pi", "idle");
        waiting.owns_open = true;
        let stopped = agent("w2:p2", "pi", "idle");
        let blocked = agent("w2:p3", "pi", "blocked");
        let mut finishing = agent("w2:p4", "pi", "idle");
        finishing.finishing = true;
        finishing.handoff = Some("accepted".into());
        let mut done = agent("w2:p5", "pi", "idle");
        done.done = true;
        done.handoff = Some("recorded".into());
        let working = agent("w2:p6", "pi", "working");
        let unknown = agent("w2:p7", "pi", "unknown");
        let host = host(
            "8103",
            "netcup",
            vec![waiting, stopped, blocked, finishing, done, working, unknown],
        );
        let items: Vec<Item> = [
            "w2:p1", "w2:p2", "w2:p3", "w2:p4", "w2:p5", "w2:p6", "w2:p7",
        ]
        .iter()
        .enumerate()
        .map(|(i, pane)| item(&format!("i{i}"), "8103", pane))
        .collect();
        let board = build(&status(vec![host], items), None, NOW);
        // The board shows the display name of the lifecycle state the owner derived.
        let words: Vec<(&str, &str)> = board
            .rows
            .iter()
            .map(|r| (r.id.as_str(), r.state.as_str()))
            .collect();
        assert_eq!(
            words,
            [
                ("i1", "idle"),    // stopped: a turn that ended with no signal
                ("i2", "blocked"), // explicit terminal blocker
                ("i6", "unknown"), // an unreadable terminal state
                ("i0", "idle"),    // owns open work
                ("i3", "idle"),    // finishing
                ("i4", "done"),    // a recorded final handoff
                ("i5", "working"),
            ]
        );
        // The board's group comes from the owner's derivation: an unreadable agent, a
        // stopped one and a question need the owner, and the rest keep working.
        let groups: Vec<(&str, Group)> = board
            .rows
            .iter()
            .map(|r| (r.id.as_str(), r.group))
            .collect();
        assert_eq!(
            groups,
            [
                ("i1", Group::Attention),
                ("i2", Group::Attention),
                ("i6", Group::Attention),
                ("i0", Group::Running),
                ("i3", Group::Running),
                ("i4", Group::Running),
                ("i5", Group::Running),
            ]
        );
        // The owner's raw terminal states stay intact: the board did not rewrite them.
        for row in &board.rows {
            assert_ne!(row.state, "stalled");
        }
        // The shared owner gives the same answer as the board's own row.
        let item = item("i0", "8103", "w2:p1");
        let hosts = board_hosts();
        assert_eq!(
            crate::items::lifecycle(&item, &hosts).state,
            crate::lifecycle::State::Waiting
        );
    }

    /// NEEDS YOU holds every state the owner must act on: a question, a blocker, a
    /// stopped turn, a lost agent, and an unreadable one. A Herdr `done` without the
    /// native `done` is a stopped turn, never a finished handoff.
    #[test]
    fn the_states_that_need_the_owner_are_under_needs_you() {
        let mut stopped = agent("w2:p1", "pi", "idle");
        stopped.handoff = Some("none".into());
        let blocked = agent("w2:p2", "pi", "blocked");
        let second_stopped = agent("w2:p3", "pi", "idle");
        let unknown = agent("w2:p5", "pi", "unknown");
        // Herdr reports `done`, but the agent never called the native `done`.
        let mut herdr_done = agent("w2:p6", "pi", "done");
        herdr_done.handoff = Some("none".into());
        let agents = vec![stopped, blocked, second_stopped, unknown, herdr_done];
        let panes = ["w2:p1", "w2:p2", "w2:p3", "w2:p5", "w2:p6"];
        let items: Vec<Item> = panes
            .iter()
            .enumerate()
            .map(|(i, pane)| item(&format!("i{i}"), "8103", pane))
            .collect();
        let board = build(
            &status(vec![host("8103", "netcup", agents)], items),
            None,
            NOW,
        );
        let groups: Vec<(&str, Group)> = board
            .rows
            .iter()
            .map(|r| (r.id.as_str(), r.group))
            .collect();
        assert_eq!(
            groups,
            [
                ("i0", Group::Attention), // stopped
                ("i1", Group::Attention), // blocked
                ("i2", Group::Attention), // stopped
                ("i3", Group::Attention), // unknown
                ("i4", Group::Attention), // Herdr done without the native done
            ],
            "{groups:?}"
        );
        // Herdr's `done` is not the native `done`: the row never reads as done.
        let herdr_done = board.rows.iter().find(|r| r.id == "i4").expect("the row");
        assert_ne!(herdr_done.state, "done", "{herdr_done:?}");
        assert_eq!(herdr_done.group, Group::Attention);
        // A lost agent is one whose host cannot be read: it needs the owner too.
        let mut down = host("8103", "netcup", vec![agent("w2:p4", "pi", "working")]);
        down.service = "down".into();
        let lost = build(
            &status(vec![down], vec![item("lost", "8103", "w2:p4")]),
            None,
            NOW,
        );
        assert_eq!(lost.rows[0].group, Group::Attention);
        assert_eq!(lost.rows[0].state, "unavailable");
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
            validity: None,
            output_expired: false,
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

    #[test]
    fn a_job_row_carries_the_repository_of_its_checkout() {
        let mut worker = agent("w2:p1", "pi", "working");
        worker.cwd = Some("/r".into());
        worker.repository = Some("github.com/oisin-ee/dogsled".into());
        let mut local = host("local", "local", vec![worker]);
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
            validity: None,
            output_expired: false,
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
        let row = board
            .rows
            .iter()
            .find(|r| r.kind == "job")
            .expect("the job has a row");
        assert_eq!(
            row.repository.as_deref(),
            Some("github.com/oisin-ee/dogsled")
        );
    }

    #[test]
    fn a_job_row_without_an_observed_checkout_has_no_repository() {
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
            validity: None,
            output_expired: false,
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
        let row = board
            .rows
            .iter()
            .find(|r| r.kind == "job")
            .expect("the job has a row");
        assert_eq!(row.repository, None);
    }

    #[test]
    fn a_row_serialises_its_repository_when_it_is_known() {
        let mut worker = agent("w2:p1", "pi", "working");
        worker.repository = Some("github.com/oisin-ee/dogsled".into());
        let board = build(
            &status(
                vec![host("8103", "netcup", vec![worker])],
                vec![item("child", "8103", "w2:p1")],
            ),
            None,
            NOW,
        );
        let text = serde_json::to_string(&board.rows[0]).unwrap();
        assert!(
            text.contains(r#""repository":"github.com/oisin-ee/dogsled""#),
            "{text}"
        );
    }

    #[test]
    fn a_row_without_a_repository_does_not_serialise_one() {
        let board = build(
            &status(
                vec![host(
                    "8103",
                    "netcup",
                    vec![agent("w2:p1", "pi", "working")],
                )],
                vec![item("child", "8103", "w2:p1")],
            ),
            None,
            NOW,
        );
        let text = serde_json::to_string(&board.rows[0]).unwrap();
        assert!(!text.contains("repository"), "{text}");
    }

    #[test]
    fn a_queued_row_carries_the_repository_of_its_request() {
        let ticket = crate::work::Ticket {
            id: "t1".into(),
            request: crate::work::Request {
                id: "t1".into(),
                action: crate::work::Action::Job {
                    cmd: vec!["cargo".into(), "test".into()],
                    timeout_ms: None,
                    idle_timeout_ms: None,
                    validity: None,
                },
                host: None,
                branch: None,
                base: None,
                cwd: "/r".into(),
                repository: Some(crate::checkout::Repository {
                    path: "/r".into(),
                    origin: "git@github.com:oisin-ee/dogsled.git".into(),
                    relative: "r".into(),
                    working_root: "/r".into(),
                }),
                workspace: None,
                workspace_name: None,
                owner: "w1:p1".into(),
                owner_host: None,
            },
            created_ms: NOW - 1_000,
            state: "pending".into(),
            waiting: false,
            waiting_until_ms: None,
            wait_reason: None,
            notified: false,
            result: None,
            error: None,
            diagnostic: None,
            retry: crate::backoff::Backoff::default(),
        };
        let mut status = status(vec![host("8103", "netcup", Vec::new())], Vec::new());
        status.queue.push(ticket);
        let board = build(&status, None, NOW);
        let row = board
            .rows
            .iter()
            .find(|r| r.group == Group::Waiting)
            .expect("the queued job has a row");
        assert_eq!(
            row.repository.as_deref(),
            Some("github.com/oisin-ee/dogsled")
        );
    }
}
