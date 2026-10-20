//! Placement-backed actions, pane creation, and caller-owned admission tickets.

use crate::checkout::Repository;
use crate::commands::{self, Placement};
use crate::error::{Error, Result};
use crate::hosts::{Fleet, Host};
use crate::items::{self, Item};
use crate::jobs::{self, Job};
use crate::ledger::{Ledger, new_id};
use crate::shell::{Shell, quote};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Action {
    Place,
    Job {
        cmd: Vec<String>,
        timeout_ms: Option<u64>,
        idle_timeout_ms: Option<u64>,
    },
    Agent {
        kind: String,
        name: String,
        message: String,
        /// A Pi profile, resolved on the launch host.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<String>,
    },
}

/// The harnesses that take their first task as a launch argument.
pub const KINDS: [&str; 3] = ["pi", "claude", "codex"];
/// Launch attempts before a ticket fails.
const LAUNCH_TRIES: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub action: Action,
    pub host: Option<String>,
    pub branch: Option<String>,
    pub base: Option<String>,
    pub cwd: String,
    pub repository: Option<Repository>,
    pub workspace: Option<String>,
    pub workspace_name: Option<String>,
    pub owner: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Launch {
    pub id: String,
    pub action: Action,
    pub host: String,
    pub cwd: String,
    pub pane: Option<String>,
    /// Terminal identity captured by preparation, before transport to this host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
    pub workspace: Option<String>,
    pub workspace_name: Option<String>,
    pub owner: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ticket {
    pub id: String,
    pub request: Request,
    pub created_ms: u64,
    pub state: String,
    pub waiting: bool,
    pub waiting_until_ms: Option<u64>,
    pub notified: bool,
    pub result: Option<Value>,
    pub error: Option<String>,
    /// Failed launch attempts, and when the next may run.
    #[serde(default)]
    pub retry: crate::backoff::Backoff,
}

pub fn queue(ledger: &Ledger) -> Result<Vec<Ticket>> {
    let mut tickets: Vec<Ticket> = ledger.read("queue.json")?.unwrap_or_default();
    for ticket in &mut tickets {
        if ticket
            .waiting_until_ms
            .is_some_and(|end| crate::ledger::now_ms() >= end)
        {
            ticket.waiting = false;
        }
    }
    Ok(tickets)
}

pub fn enqueue(ledger: &Ledger, request: Request, waiting: bool, now: u64) -> Result<String> {
    ledger.with_lock("queue", || {
        let mut tickets = queue(ledger)?;
        if let Some(ticket) = tickets.iter().find(|t| t.request.id == request.id) {
            if serde_json::to_value(&ticket.request)? != serde_json::to_value(&request)? {
                return Err(crate::unit_transport::conflict("queued request changed"));
            }
            return Ok(ticket.id.clone());
        }
        let id = new_id();
        tickets.push(Ticket {
            id: id.clone(),
            request,
            created_ms: now,
            state: "pending".into(),
            waiting,
            waiting_until_ms: if waiting {
                ledger.budget.deadline.map(|end| {
                    now.saturating_add(
                        end.saturating_duration_since(std::time::Instant::now())
                            .as_millis() as u64,
                    )
                })
            } else {
                None
            },
            notified: false,
            result: None,
            error: None,
            retry: crate::backoff::Backoff::default(),
        });
        ledger.write("queue.json", &tickets)?;
        Ok(id)
    })
}

/// Queue `request` after its first launch failed, so the service retries it with backoff.
pub fn enqueue_retry(
    ledger: &Ledger,
    request: Request,
    waiting: bool,
    error: &Error,
    now: u64,
) -> Result<String> {
    let id = enqueue(ledger, request, waiting, now)?;
    update_ticket(ledger, &id, |t| {
        t.retry.fail(now, &t.id);
        t.error = Some(format!("attempt 1: {error}"));
    })?;
    Ok(id)
}

pub fn cancel(ledger: &Ledger, id: &str) -> Result<()> {
    ledger.with_lock("queue", || {
        let mut tickets = queue(ledger)?;
        let ticket = tickets
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(|| Error::Invalid(format!("unknown ticket {id}")))?;
        if ticket.state != "pending" {
            return Err(Error::Invalid(format!("ticket {id} is {}", ticket.state)));
        }
        ticket.state = "cancelled".into();
        ledger.write("queue.json", &tickets)
    })
}

pub fn workspace(shell: &dyn Shell, id: &str) -> Result<String> {
    let value = herdr(shell, &["workspace".into(), "list".into()])?;
    value
        .pointer("/result/workspaces")
        .and_then(Value::as_array)
        .and_then(|rows| rows.iter().find(|w| w["workspace_id"] == id))
        .and_then(|w| w["label"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| Error::Invalid(format!("caller workspace {id} has no label")))
}

fn herdr(shell: &dyn Shell, args: &[String]) -> Result<Value> {
    let out = shell.run_ok("herdr", args)?;
    Ok(serde_json::from_str(&out.stdout)?)
}

pub(crate) fn pane(shell: &dyn Shell, launch: &Launch) -> Result<(String, Option<String>)> {
    if !std::path::Path::new(&launch.cwd).is_dir() {
        return Err(Error::Invalid(format!(
            "{}: cwd does not exist: {}",
            launch.host, launch.cwd
        )));
    }
    if let Some(pane) = &launch.pane {
        return Ok((pane.clone(), launch.terminal.clone()));
    }
    let workspace = if let Some(id) = &launch.workspace {
        Some(id.clone())
    } else {
        let name = launch
            .workspace_name
            .as_ref()
            .ok_or_else(|| Error::Invalid("caller workspace name is required".into()))?;
        let value = herdr(shell, &["workspace".into(), "list".into()])?;
        value
            .pointer("/result/workspaces")
            .and_then(Value::as_array)
            .and_then(|rows| rows.iter().find(|w| w["label"] == *name))
            .and_then(|w| w["workspace_id"].as_str())
            .map(str::to_owned)
    };
    let mut args = if let Some(id) = workspace {
        vec!["tab".into(), "create".into(), "--workspace".into(), id]
    } else {
        vec![
            "workspace".into(),
            "create".into(),
            "--label".into(),
            launch
                .workspace_name
                .clone()
                .ok_or_else(|| Error::Invalid("missing workspace name".into()))?,
        ]
    };
    args.extend(["--cwd".into(), launch.cwd.clone(), "--no-focus".into()]);
    let value = herdr(shell, &args)?;
    let pane = value
        .pointer("/result/root_pane/pane_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::Invalid(format!("herdr create returned no root pane: {value}")))?;
    let terminal = value
        .pointer("/result/root_pane/terminal_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned);
    Ok((pane, terminal))
}

pub fn launch(ledger: &Ledger, shell: &dyn Shell, launch: &Launch, now: u64) -> Result<Value> {
    jobs::validate_id(&launch.id)?;
    if matches!(launch.action, Action::Agent { .. }) {
        let result = crate::launch::agent(ledger, shell, launch)?;
        crate::prepare::used(ledger, &launch.cwd)?;
        return Ok(result);
    }
    let receipts = ledger.child("launches");
    receipts.with_lock(&launch.id, || {
        let file = format!("{}.json", launch.id);
        if let Some(value) = receipts.read::<Value>(&file)? {
            return Ok(value);
        }
        let (pane, _) = pane(shell, launch)?;
        let result = match &launch.action {
            Action::Job {
                cmd,
                timeout_ms,
                idle_timeout_ms,
            } => {
                let job = Job {
                    id: launch.id.clone(),
                    cmd: cmd.clone(),
                    cwd: launch.cwd.clone(),
                    host: launch.host.clone(),
                    owner: launch.owner.clone(),
                    pane: pane.clone(),
                    state: "pending".into(),
                    exit: None,
                    signal: None,
                    reason: None,
                    started_ms: None,
                    ended_ms: None,
                    timeout_ms: *timeout_ms,
                    idle_timeout_ms: *idle_timeout_ms,
                };
                jobs::save(ledger, &job)?;
                let executable = std::env::current_exe()?.display().to_string();
                let command = format!(
                    "DOGSLED_STATE={} {} exec {} -- {}",
                    quote(&ledger.dir.display().to_string()),
                    quote(&executable),
                    quote(&launch.id),
                    cmd.iter().map(|c| quote(c)).collect::<Vec<_>>().join(" ")
                );
                if let Err(error) = shell.run_ok(
                    "herdr",
                    &["pane".into(), "run".into(), pane.clone(), command],
                ) {
                    let mut failed = job;
                    failed.exit = Some(2);
                    failed.reason = Some(error.to_string());
                    failed.state = "blocked".into();
                    failed.ended_ms = Some(now);
                    jobs::save(ledger, &failed)?;
                    return Err(error);
                }
                json!({"id":launch.id,"host":launch.host,"pane":pane})
            }
            Action::Agent { .. } => {
                return Err(Error::Invalid(
                    "agent launch must use its receipt owner".into(),
                ));
            }
            Action::Place => return Err(Error::Invalid("place does not launch a pane".into())),
        };
        receipts.write(&file, &result)?;
        crate::prepare::used(ledger, &launch.cwd)?;
        Ok(result)
    })
}

/// Keep the task in a file on the launch host: Herdr refuses a launch argument with a newline,
/// and the file outlives the pane for review.
pub(crate) fn task_file(ledger: &Ledger, id: &str, message: &str) -> Result<String> {
    let dir = ledger.child("launches").dir;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{id}.task.md"));
    std::fs::write(&path, message)?;
    Ok(path.display().to_string())
}

/// The harness arguments after `--`: Pi's session name and profile flags, then the task. A
/// one-line task that cannot be read as an option or a file goes inline; any other is read from
/// `task` (Pi's `@file`, or a pointer for Claude and Codex).
pub fn agent_args(
    kind: &str,
    name: &str,
    message: &str,
    task: &str,
    profile: Option<&crate::profiles::Profile>,
) -> Result<Vec<String>> {
    let inline = !message.contains(['\n', '\r'])
        && !message.starts_with(['-', '@'])
        && message.len() <= 4_000;
    let mut args = Vec::new();
    match kind {
        "pi" => {
            args.extend(["--name".to_owned(), name.to_owned()]);
            args.extend(
                profile
                    .map(crate::profiles::Profile::pi_flags)
                    .unwrap_or_default(),
            );
            args.push(if inline {
                message.to_owned()
            } else {
                format!("@{task}")
            });
        }
        "claude" | "codex" if profile.is_none() => args.push(if inline {
            message.to_owned()
        } else {
            format!("Your task is in {task}. Read that file and do the task.")
        }),
        "claude" | "codex" => {
            return Err(Error::Invalid(
                "profiles configure Pi children; omit --profile or use --kind pi".into(),
            ));
        }
        other => {
            return Err(Error::Invalid(format!(
                "kind {other} cannot take its task at launch; use one of {}",
                KINDS.join(", ")
            )));
        }
    }
    Ok(args)
}

/// A new request runs now only when no ticket waits ahead of it; otherwise the caller
/// enqueues it, and admission serves the queue in order. `None` means queue it.
pub fn submit(fleet: &Fleet, request: &Request, now: u64) -> Result<Option<Value>> {
    if queue(fleet.ledger)?.iter().any(|t| t.state == "pending") {
        return Ok(None);
    }
    perform(fleet, request, now)
}

/// Caller-owned progress is durable before any preparation or launch side effect. An
/// interrupted preparation cannot be adopted: only a saved launch may be resumed.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Attempt {
    pub request: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<Host>,
    pub launch: Option<Launch>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<crate::unit_request::Prepared>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

pub fn perform(fleet: &Fleet, request: &Request, now: u64) -> Result<Option<Value>> {
    jobs::validate_id(&request.id)?;
    if matches!(request.action, Action::Place) {
        let Placement::Placed {
            host, reservation, ..
        } = commands::place(fleet, request.host.as_deref(), now)?
        else {
            return Ok(None);
        };
        let launch = prepare_launch(fleet, request, &host)?;
        if host.is_local() {
            crate::prepare::used(fleet.ledger, &launch.cwd)?;
        } else {
            let _: Value = fleet.remote_setup(
                &host,
                &["prepared".into(), "--path".into(), launch.cwd.clone()],
            )?;
        }
        return Ok(Some(
            json!({"host":host,"reservation":reservation,"path":request.branch.as_ref().map(|_| launch.cwd),"pane_id":launch.pane}),
        ));
    }
    let attempts = fleet.ledger.child("admissions");
    attempts.with_lock(&request.id, || perform_locked(fleet, request, now))
}

/// The caller owns the request's admission lock. Commit uses this path so the lock is
/// never reacquired while `runtime::start` enters admission.
pub(crate) fn perform_locked(fleet: &Fleet, request: &Request, now: u64) -> Result<Option<Value>> {
    let attempts = fleet.ledger.child("admissions");
    let file = format!("{}.json", request.id);
    if let Some(attempt) = attempts.read::<Attempt>(&file)? {
        if attempt.request != serde_json::to_value(request)? {
            return Err(Error::Invalid(format!(
                "request {} changed after placement",
                request.id
            )));
        }
        if let Some(unit) = &attempt.unit {
            if unit.committed.is_none() || unit.abandoned.is_some() {
                return Err(crate::unit_transport::conflict("request is not committed"));
            }
            if let Some(result) = &attempt.result {
                return Ok(Some(result.clone()));
            }
        }
        if !matches!(request.action, Action::Agent { .. }) {
            return Err(Error::Invalid(format!(
                "request {} already attempted a non-reconcilable launch; inspect before resubmitting",
                request.id
            )));
        }
        if let Some(host) = &attempt.host {
            let launch = attempt.launch.as_ref().ok_or_else(|| Error::Invalid(format!(
                    "request {} stopped during preparation on {}; inspect that host before resubmitting: {}",
                    request.id, host.label,
                    attempt.error.as_deref().unwrap_or("preparation outcome is unknown")
                )))?;
            let result = finish(fleet, request, host, launch, now)?;
            save_result(&attempts, &file, attempt, &result)?;
            return Ok(Some(result));
        }
        if attempt.unit.is_none() {
            return Err(Error::Invalid("admission has no selected host".into()));
        }
    }
    begin(fleet, request, &attempts, &file, now)
}

fn save_result(attempts: &Ledger, file: &str, mut attempt: Attempt, result: &Value) -> Result<()> {
    if attempt.unit.is_some() {
        attempt.result = Some(result.clone());
        attempts.write(file, &attempt)?;
    }
    Ok(())
}

fn begin(
    fleet: &Fleet,
    request: &Request,
    attempts: &Ledger,
    file: &str,
    now: u64,
) -> Result<Option<Value>> {
    let placed = commands::place(fleet, request.host.as_deref(), now)?;
    let Placement::Placed { host, .. } = placed else {
        return Ok(None);
    };
    let mut attempt = Attempt {
        request: serde_json::to_value(request)?,
        host: Some(host.clone()),
        launch: None,
        error: None,
        unit: attempts.read::<Attempt>(file)?.and_then(|a| a.unit),
        result: None,
    };
    attempts.write(file, &attempt)?;
    let prepared = prepare_launch(fleet, request, &host);
    let launch_request = match prepared {
        Ok(launch) => launch,
        Err(error) => {
            attempt.error = Some(error.to_string());
            attempts.write(file, &attempt)?;
            return Err(error);
        }
    };
    attempt.launch = Some(launch_request.clone());
    attempts.write(file, &attempt)?;
    let result = finish(fleet, request, &host, &launch_request, now)?;
    save_result(attempts, file, attempt, &result)?;
    Ok(Some(result))
}

fn prepare_launch(fleet: &Fleet, request: &Request, host: &Host) -> Result<Launch> {
    let checkout = request
        .repository
        .as_ref()
        .map(|repo| crate::checkout::ensure(fleet, host, repo))
        .transpose()?;
    let prepared = if let Some(branch) = &request.branch {
        let repo = request
            .repository
            .as_ref()
            .ok_or_else(|| Error::Invalid("branch requires repository".into()))?;
        crate::prepare::run(fleet, host, &repo.path, branch, request.base.as_deref())?
    } else {
        crate::prepare::Prepared::default()
    };
    let cwd = if request.branch.is_some() {
        prepared
            .path
            .clone()
            .ok_or_else(|| Error::Invalid("the prepared worktree has no path".into()))?
    } else if host.is_local() {
        request.cwd.clone()
    } else if let (Some(checkout), Some(repo)) = (checkout, &request.repository) {
        let relative = std::path::Path::new(&request.cwd)
            .strip_prefix(&repo.working_root)
            .map_err(|_| Error::Invalid("repository cwd is outside its working root".into()))?;
        std::path::Path::new(&checkout)
            .join(relative)
            .display()
            .to_string()
    } else {
        request.cwd.clone()
    };
    Ok(Launch {
        id: request.id.clone(),
        action: request.action.clone(),
        host: host.id.clone(),
        cwd,
        pane: if matches!(request.action, Action::Agent { .. } | Action::Place) {
            prepared.pane_id
        } else {
            None
        },
        terminal: if matches!(request.action, Action::Agent { .. }) {
            prepared.terminal_id
        } else {
            None
        },
        workspace: host.is_local().then(|| request.workspace.clone()).flatten(),
        workspace_name: request.workspace_name.clone(),
        owner: request.owner.clone(),
    })
}

fn finish(
    fleet: &Fleet,
    request: &Request,
    host: &Host,
    launch_request: &Launch,
    now: u64,
) -> Result<Value> {
    let launched = if host.is_local() {
        launch(fleet.ledger, fleet.shell, launch_request, now)
    } else {
        fleet.remote_setup(
            host,
            &[
                "launch".into(),
                "--request".into(),
                serde_json::to_string(launch_request)?,
            ],
        )
    };
    let result = launched.map_err(|e| {
        if retryable(&e) && matches!(request.action, Action::Agent { .. }) {
            Error::Launch(e.to_string())
        } else {
            e
        }
    })?;
    let pane = result["pane"]
        .as_str()
        .ok_or_else(|| Error::Invalid("launch returned no pane".into()))?;
    items::record(
        fleet.ledger,
        item(request, host.id.clone(), pane, &result, now)?,
    )?;
    Ok(result)
}

/// The owner's record of what `request` started. The profile's role wins; otherwise a child
/// with a branch is a writer.
fn item(request: &Request, host: String, pane: &str, result: &Value, now: u64) -> Result<Item> {
    let text = |key: &str| result[key].as_str().map(str::to_owned);
    let (kind, name, harness) = match &request.action {
        // A job is named for its program, so the board shows it before any snapshot has it.
        Action::Job { cmd, .. } => (
            "job",
            cmd.first()
                .map(|c| c.rsplit('/').next().unwrap_or(c).to_owned()),
            None,
        ),
        Action::Agent { name, kind, .. } => ("agent", Some(name.clone()), Some(kind.clone())),
        Action::Place => return Err(Error::Invalid("place cannot create an item".into())),
    };
    let role = harness.as_ref().map(|_| {
        text("role").unwrap_or_else(|| {
            if request.branch.is_some() {
                "writer"
            } else {
                "reader"
            }
            .into()
        })
    });
    Ok(Item {
        id: request.id.clone(),
        kind: kind.into(),
        host,
        pane: pane.into(),
        owner_pane: request.owner.clone(),
        name,
        created_ms: now,
        closed_ms: None,
        harness,
        role,
        profile: text("profile"),
        model: text("model"),
        ..Item::default()
    })
}

fn update_ticket(ledger: &Ledger, id: &str, update: impl FnOnce(&mut Ticket)) -> Result<Ticket> {
    ledger.with_lock("queue", || {
        let mut tickets = queue(ledger)?;
        let ticket = tickets
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(|| Error::Invalid(format!("unknown ticket {id}")))?;
        update(ticket);
        let result = ticket.clone();
        ledger.write("queue.json", &tickets)?;
        Ok(result)
    })
}

fn admit(fleet: &Fleet, now: u64) -> Result<()> {
    for ticket in queue(fleet.ledger)? {
        let mut ticket = ticket;
        if ticket.state == "admitting" {
            let saved = fleet
                .ledger
                .child("admissions")
                .read::<Attempt>(&format!("{}.json", ticket.request.id))?;
            let recoverable = matches!(ticket.request.action, Action::Agent { .. })
                && saved
                    .is_some_and(|a| a.launch.is_some() || (a.unit.is_some() && a.host.is_none()));
            ticket = update_ticket(fleet.ledger, &ticket.id, |t| {
                t.state = if recoverable { "pending" } else { "failed" }.into();
                t.error = Some("service stopped during admission; inspect the selected host before resubmitting".into());
            })?;
        }
        if ticket.state != "pending" || !ticket.retry.ready(now) {
            continue;
        }
        let claimed = update_ticket(fleet.ledger, &ticket.id, |t| {
            if t.state == "pending" {
                t.state = "admitting".into();
            }
        })?;
        if claimed.state != "admitting" {
            continue;
        }
        // No store lock spans SSH, cloning, hooks, or agent readiness.
        let outcome = perform(fleet, &claimed.request, now);
        let full = matches!(&outcome, Ok(None));
        update_ticket(fleet.ledger, &ticket.id, |t| match outcome {
            Ok(Some(result)) => {
                t.state = "admitted".into();
                t.result = Some(result);
            }
            Ok(None) => t.state = "pending".into(),
            // A refused request fails at once; a failed launch, SSH, or Herdr call is retried.
            Err(error @ Error::Launch(_)) if t.retry.attempts + 1 < LAUNCH_TRIES => {
                t.state = "pending".into();
                t.retry.fail(now, &t.id);
                t.error = Some(format!("attempt {}: {error}", t.retry.attempts));
            }
            Err(error) => {
                t.state = "failed".into();
                t.error = Some(if t.retry.attempts > 0 {
                    format!("{error} (after {} attempts)", t.retry.attempts + 1)
                } else {
                    error.to_string()
                });
            }
        })?;
        if full {
            break;
        }
    }
    Ok(())
}

/// A launch that failed after placement: Herdr did not start the agent in time, or the SSH
/// launch call failed. Refusals and deadlines are not retried.
fn retryable(error: &Error) -> bool {
    matches!(
        error,
        Error::Command { .. } | Error::Ssh { .. } | Error::Io(_)
    )
}

pub fn tick(fleet: &Fleet, now: u64) -> Result<()> {
    let reports = fleet.status(now)?;
    fleet.ledger.with_lock("admission", || {
        crate::unit_request::recover(fleet.ledger, now)?;
        admit(fleet, now)?;
        for ticket in queue(fleet.ledger)? {
            if ticket.waiting
                || ticket.notified
                || !matches!(ticket.state.as_str(), "admitted" | "failed")
            {
                continue;
            }
            let result = ticket.result.as_ref();
            let host = result
                .and_then(|r| r["host"].as_str().or_else(|| r["host"]["id"].as_str()))
                .unwrap_or("unknown");
            let summary = ticket
                .error
                .clone()
                .unwrap_or_else(|| result.map_or_else(String::new, Value::to_string));
            let text = format!(
                "dogsled: ticket {} {} on {host}: {summary}",
                ticket.id, ticket.state
            );
            // The owner's inbox reader shows it. An owner without a reader hears nothing until
            // it has one; nothing types into its terminal.
            let id = format!("ticket-{}", ticket.id);
            if !crate::inbox::notify(
                fleet.ledger,
                &reports,
                &ticket.request.owner,
                &id,
                "ticket",
                text,
                now,
            )? {
                continue;
            }
            update_ticket(fleet.ledger, &ticket.id, |t| t.notified = true)?;
        }
        Ok(())
    })
}

pub fn job_host<'a>(fleet: &'a Fleet<'_>, id: &str, now: u64) -> Result<&'a Host> {
    jobs::validate_id(id)?;
    if jobs::get(fleet.ledger, id).is_ok() {
        return fleet.find("local");
    }
    if let Some(item) = items::list(fleet.ledger)?
        .iter()
        .find(|i| i.kind == "job" && i.id == id)
    {
        return fleet.find(&item.host);
    }
    for host in fleet.status(now)? {
        if host.jobs.iter().any(|j| j.id == id) {
            return fleet.find(&host.host.id);
        }
    }
    Err(Error::Invalid(format!("unknown job {id}")))
}
