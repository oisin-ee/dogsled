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
        /// The request's own creation time, stamped once at its origin.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        validity: Option<crate::jobs::Validity>,
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
    /// The hostname of the machine where `owner` asked for the work. A pane ID is unique
    /// only on its own host. Requests written before this field have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_host: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Launch {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<crate::unit_transport::Seed>,
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
    /// The originating owner's hostname, copied from its request. Launch records written
    /// before this field have none, and nothing can prove their owner's host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_host: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ticket {
    pub id: String,
    pub request: Request,
    pub created_ms: u64,
    pub state: String,
    pub waiting: bool,
    pub waiting_until_ms: Option<u64>,
    /// The last host reservation refusals, not a later status snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_reason: Option<String>,
    pub notified: bool,
    pub result: Option<Value>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<crate::diagnostic::Diagnostic>,
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

/// A job request's own validity evidence. A request that is not a job has no age to
/// check: agents and prepared units are unchanged by this rule.
fn request_validity(request: &Request) -> Option<&crate::jobs::Validity> {
    match &request.action {
        Action::Job { validity, .. } => validity.as_ref(),
        _ => None,
    }
}

/// The validity evidence a launch carries in its own action.
fn launch_validity(launch: &Launch) -> Option<crate::jobs::Validity> {
    match &launch.action {
        Action::Job { validity, .. } => *validity,
        _ => None,
    }
}

/// Check a request against this host's clock. A request that may not start new execution
/// is refused here, before any placement, preparation, dispatch, or ownership change.
fn check_request_validity(ledger: &Ledger, request: &Request) -> Result<()> {
    if !matches!(request.action, Action::Job { .. }) {
        return Ok(());
    }
    crate::jobs::check_validity(ledger, request_validity(request))
}

pub fn enqueue(ledger: &Ledger, request: Request, waiting: bool, now: u64) -> Result<String> {
    ledger.with_lock("queue", || {
        // A request that may not start new execution never enters the queue. The refusal
        // is recorded on the clock, and the queue stays as it was.
        check_request_validity(ledger, &request)?;
        let mut tickets = queue(ledger)?;
        if let Some(ticket) = tickets.iter_mut().find(|t| t.request.id == request.id) {
            if serde_json::to_value(&ticket.request)? != serde_json::to_value(&request)? {
                return Err(crate::unit_transport::conflict("queued request changed"));
            }
            let id = ticket.id.clone();
            // Placement may have queued the refusal before the caller selected a wait.
            if waiting && !ticket.waiting {
                ticket.waiting = true;
                ticket.waiting_until_ms = waiting_until(ledger, now);
                ledger.write("queue.json", &tickets)?;
            }
            return Ok(id);
        }
        let id = new_id();
        tickets.push(Ticket {
            id: id.clone(),
            request,
            created_ms: now,
            state: "pending".into(),
            waiting,
            waiting_until_ms: waiting.then(|| waiting_until(ledger, now)).flatten(),
            wait_reason: None,
            notified: false,
            result: None,
            error: None,
            diagnostic: None,
            retry: crate::backoff::Backoff::default(),
        });
        ledger.write("queue.json", &tickets)?;
        Ok(id)
    })
}

fn waiting_until(ledger: &Ledger, now: u64) -> Option<u64> {
    ledger.budget.deadline.map(|end| {
        now.saturating_add(
            end.saturating_duration_since(std::time::Instant::now())
                .as_millis() as u64,
        )
    })
}

/// Save the actual placement refusals even when the first caller has not queued yet.
fn defer(
    fleet: &Fleet,
    request: &Request,
    hosts: &[crate::hosts::HostReport],
    now: u64,
) -> Result<()> {
    let pinned = request.host.as_deref().map(|h| fleet.find(h)).transpose()?;
    let reason = hosts
        .iter()
        .filter(|h| pinned.is_none_or(|p| p.id == h.host.id))
        .map(|h| {
            let reason = h
                .headroom
                .as_ref()
                .and_then(|r| r.reason.as_deref())
                .or(h.error.as_deref())
                .unwrap_or("reservation refused");
            format!("{}: {reason}", h.host.label)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let id = enqueue(fleet.ledger, request.clone(), false, now)?;
    update_ticket(fleet.ledger, &id, |t| {
        t.wait_reason = (!reason.is_empty()).then_some(reason);
    })?;
    Ok(())
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
        t.diagnostic = error.diagnostic().cloned();
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
    // Work in a repository opens where that repository is shown, never in the caller's
    // unrelated workspace.
    let mut args = match crate::worktree::home(shell, &launch.cwd)? {
        crate::worktree::Home::Shown(id) => tab(id),
        crate::worktree::Home::Unshown => vec!["workspace".into(), "create".into()],
        crate::worktree::Home::Outside => callers(shell, launch)?,
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

/// The pane's live terminal identity, or an error when the pane is not listed with one.
/// A pane that is not listed has no proven identity, and a pane that is listed without a
/// terminal id has none either.
fn live_terminal(shell: &dyn Shell, pane: &str) -> Result<String> {
    crate::cleanup::pane_snapshot(shell)?
        .1
        .get(pane)
        .cloned()
        .ok_or_else(|| Error::Invalid(format!("{pane}: cannot establish launch terminal identity")))
}

/// Retire a pane this launch allocated. Only that pane is closed, and only after the same
/// guards the launch cleanup uses: the caller's own pane, a focused pane, a pane an agent
/// owns, a busy pane, and a pane whose terminal identity changed are never closed here.
///
/// The terminal identity must be proven before the pane closes. The launch recorded one,
/// the pane is listed, and the listed terminal equals the recorded one. A missing recorded
/// identity, an unlisted pane, or a different live terminal is not proof, so the pane
/// stays open.
pub(crate) fn retire_pane(shell: &dyn Shell, pane: &str, terminal: Option<&str>) -> Result<()> {
    if std::env::var("HERDR_PANE_ID").ok().as_deref() == Some(pane)
        || crate::cleanup::current_panes(shell)?
            .iter()
            .any(|p| p.pane_id == pane && p.focused)
    {
        return Err(Error::Invalid(format!("{pane}: caller or focused pane")));
    }
    let Some(expected) = terminal.filter(|id| !id.is_empty()) else {
        return Err(Error::Invalid(format!(
            "{pane}: the launch recorded no terminal identity"
        )));
    };
    if live_terminal(shell, pane)? != expected {
        return Err(Error::Invalid(format!("{pane}: launch terminal changed")));
    }
    if crate::probe::agent_list(shell)?
        .iter()
        .any(|agent| agent.pane == pane)
    {
        return Err(Error::Invalid(format!(
            "{pane}: an agent now owns the pane"
        )));
    }
    if crate::probe::foreground(shell, pane)?.is_some() {
        return Err(Error::Invalid(format!("{pane}: the pane is busy")));
    }
    // Inspection must not authorize closing a replacement terminal.
    if live_terminal(shell, pane)? != expected {
        return Err(Error::Invalid(format!("{pane}: launch terminal changed")));
    }
    shell.run_ok("herdr", &["pane".into(), "close".into(), pane.into()])?;
    Ok(())
}

fn tab(workspace: String) -> Vec<String> {
    vec![
        "tab".into(),
        "create".into(),
        "--workspace".into(),
        workspace,
    ]
}

/// A tab in the caller's workspace, or a new workspace with the caller's workspace name.
fn callers(shell: &dyn Shell, launch: &Launch) -> Result<Vec<String>> {
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
    Ok(match workspace {
        Some(id) => tab(id),
        None => vec![
            "workspace".into(),
            "create".into(),
            "--label".into(),
            launch
                .workspace_name
                .clone()
                .ok_or_else(|| Error::Invalid("missing workspace name".into()))?,
        ],
    })
}

/// The request facts a job launch receipt keeps. The receipt names the command it
/// dispatched, so the receipt alone proves which job it belongs to. The job record keeps
/// the same facts, and the prune removes that record.
fn request_evidence(
    launch: &Launch,
    cmd: &[String],
    timeout_ms: Option<u64>,
    idle_timeout_ms: Option<u64>,
) -> Value {
    let mut evidence = json!({"cmd":cmd,"cwd":launch.cwd,"owner":launch.owner,
        "timeout_ms":timeout_ms,"idle_timeout_ms":idle_timeout_ms});
    // The validity evidence joins the receipt only when the request carries it. A legacy
    // receipt keeps its own shape, and a new request never correlates to a receipt that
    // names no request time.
    if let Some(validity) = launch_validity(launch) {
        evidence["validity"] = json!(validity);
    }
    evidence
}

/// The saved receipt of a job launch must name this request and the command it dispatched.
/// A conflicting same-id request, a receipt for another pane, and a receipt that names a
/// different command or options all refuse: a receipt never replays for another job.
///
/// The job record is a second check, not a prerequisite. The prune removes a terminal job
/// record and keeps its receipt, so the receipt's own evidence carries the replay. A
/// receipt that carries the request evidence but no pane is not complete: it names no
/// dispatch, so a replay without the job record refuses.
pub(crate) fn correlate(
    ledger: &Ledger,
    launch: &Launch,
    cmd: &[String],
    timeout_ms: Option<u64>,
    idle_timeout_ms: Option<u64>,
    saved: &Value,
) -> Result<()> {
    if saved["id"] != json!(launch.id) {
        return Err(Error::Invalid(format!(
            "job {} has a saved launch receipt for another request",
            launch.id
        )));
    }
    if saved["host"] != json!(launch.host) {
        return Err(Error::Invalid(format!(
            "job {} has a saved launch receipt for another host",
            launch.id
        )));
    }
    // The receipt's own evidence is the first check. A receipt that names another command
    // or another request dispatched another job, whether or not its job record remains.
    let evidence = request_evidence(launch, cmd, timeout_ms, idle_timeout_ms);
    if let Some(request) = saved.get("request")
        && *request != evidence
    {
        return Err(Error::Invalid(format!(
            "job {} has a saved launch receipt for another command or request",
            launch.id
        )));
    }
    // The job record is a second check, not a prerequisite. The prune removes a terminal
    // job record and keeps its receipt, so the receipt carries the replay alone.
    let Some(job) = jobs::record(ledger, &launch.id)? else {
        // Without the record, the receipt must be complete on its own: it names the
        // request it dispatched and the pane it dispatched into. A receipt that predates
        // the request evidence proves nothing about what it dispatched.
        if saved.get("request").is_none() {
            return Err(Error::Invalid(format!(
                "job {} has a legacy launch receipt without request evidence; inspect that host before resubmitting",
                launch.id
            )));
        }
        // A complete receipt also names the pane it dispatched into. Without it the receipt
        // proves no dispatch, so a replay refuses.
        if saved
            .get("pane")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(Error::Invalid(format!(
                "job {} has a launch receipt without a pane; inspect that host before resubmitting",
                launch.id
            )));
        }
        return Ok(());
    };
    if job.id != launch.id {
        return Err(Error::Invalid(format!(
            "job {} claim belongs to job {}",
            launch.id, job.id
        )));
    }
    if saved["pane"] != json!(job.pane) {
        return Err(Error::Invalid(format!(
            "job {} has a saved launch receipt for another pane",
            launch.id
        )));
    }
    if job.cmd != cmd
        || job.cwd != launch.cwd
        || job.host != launch.host
        || job.owner != launch.owner
        || job.timeout_ms != timeout_ms
        || job.idle_timeout_ms != idle_timeout_ms
        || job.validity != launch_validity(launch)
    {
        return Err(Error::Invalid(format!(
            "job {} has a saved launch receipt for another command or request",
            launch.id
        )));
    }
    Ok(())
}

/// The launch of a job on this host. The claim is published before the pane side effect, and
/// the wrapper that runs in that pane reads the claim before it runs anything.
fn launch_job(
    ledger: &Ledger,
    shell: &dyn Shell,
    launch: &Launch,
    cmd: &[String],
    timeout_ms: Option<u64>,
    idle_timeout_ms: Option<u64>,
) -> Result<Value> {
    let job = Job {
        id: launch.id.clone(),
        cmd: cmd.to_vec(),
        cwd: launch.cwd.clone(),
        host: launch.host.clone(),
        owner: launch.owner.clone(),
        pane: String::new(),
        state: jobs::CLAIMING.into(),
        exit: None,
        signal: None,
        reason: None,
        started_ms: None,
        ended_ms: None,
        timeout_ms,
        idle_timeout_ms,
        validity: launch_validity(launch),
    };
    // The job's ownership is read before any pane side effect. A job that already
    // has an owner keeps it: this launch never takes it over, never allocates a
    // second pane, and never dispatches the command again.
    let existing = match jobs::ownership(ledger, &launch.id)? {
        jobs::Ownership::Unclaimed => None,
        jobs::Ownership::Uncertain(held) => {
            return Err(Error::Invalid(format!(
                "job {} has an unresolved claim; inspect that host before resubmitting: {}",
                launch.id,
                held.reason.unwrap_or_else(|| "outcome unknown".into())
            )));
        }
        jobs::Ownership::Pending(_)
        | jobs::Ownership::Started(_)
        | jobs::Ownership::Terminal(_) => Some(()),
    };
    if existing.is_some() {
        // A claim is not a dispatch receipt. Without a saved receipt that proves
        // this request dispatched, the claim proves only that something was
        // claimed: this launch refuses, allocates nothing, dispatches nothing,
        // overwrites nothing, saves no receipt, and releases no preparation.
        return Err(Error::Invalid(format!(
            "job {} has a claim but no saved launch receipt; inspect that host before resubmitting",
            launch.id
        )));
    }
    // The intent is published before the pane side effect. It is not
    // executable: an allocation that stops here stays uncertain, and no other
    // allocation may take the job over.
    jobs::publish_intent(ledger, &job)?;
    let (pane, terminal) = pane(shell, launch)?;
    // Allocation blocks, so the request can expire during it. The recheck runs before the
    // executable claim is published and before the command is dispatched. A refusal writes
    // no receipt, releases no preparation, and retires only the pane this attempt
    // allocated.
    if let Err(refusal) = jobs::check_validity(ledger, job.validity.as_ref()) {
        if launch.pane.is_none()
            && let Err(error) = retire_pane(shell, &pane, terminal.as_deref())
        {
            eprintln!("dogsled pane close: {error}");
        }
        return Err(refusal);
    }
    // The pane is known, so the claim becomes executable. The wrapper reads it
    // before it runs anything.
    let mut claim = job.clone();
    claim.pane.clone_from(&pane);
    claim.state = jobs::PENDING.into();
    jobs::publish_claim(ledger, &claim)?;
    let executable = std::env::current_exe()?.display().to_string();
    // The wrapper carries the protocol it speaks and the request's own creation time, so
    // the wrapper checks the same age the launch checked.
    let command = format!(
        "DOGSLED_STATE={} {} exec --job-validity {} {} -- {}",
        quote(&ledger.dir.display().to_string()),
        quote(&executable),
        crate::jobs::VALIDITY_PROTOCOL_VERSION,
        quote(&launch.id),
        cmd.iter().map(|c| quote(c)).collect::<Vec<_>>().join(" ")
    );
    if let Err(error) = shell.run_ok(
        "herdr",
        &["pane".into(), "run".into(), pane.clone(), command],
    ) {
        // The dispatch failed. The wrapper's own record is the evidence: a
        // wrapper that already started or ended keeps its state, and nothing
        // overwrites it with the pending copy. A startup that was never proved
        // stays uncertain, so no later execution and no safe retry is invented.
        jobs::uncertain(ledger, &claim, error.to_string())?;
        return Err(error);
    }
    Ok(json!({"id":launch.id,"host":launch.host,"pane":pane,
        "request":request_evidence(launch, cmd, timeout_ms, idle_timeout_ms)}))
}

pub fn launch(
    ledger: &Ledger,
    shell: &dyn Shell,
    launch: &Launch,
    _now: u64,
    peer_validity: Option<u32>,
) -> Result<Value> {
    jobs::validate_id(&launch.id)?;
    if launch.unit.is_some() && !matches!(launch.action, Action::Agent { .. }) {
        return Err(crate::unit_transport::conflict(
            "seed requires an agent launch",
        ));
    }
    // A job launch needs the validity protocol. A peer that speaks another version, or
    // none, never creates a claim or a pane.
    if matches!(launch.action, Action::Job { .. })
        && peer_validity != Some(jobs::VALIDITY_PROTOCOL_VERSION)
    {
        return Err(Error::Refused {
            code: crate::error::Refusal::JobValidityUnsupported,
            detail: crate::error::Refusal::JobValidityUnsupported
                .message()
                .to_owned(),
        });
    }
    if matches!(launch.action, Action::Agent { .. }) {
        let result = crate::launch::agent(ledger, shell, launch)?;
        crate::prepare::used(ledger, &launch.cwd, Some(&launch.id))?;
        return Ok(result);
    }
    let receipts = ledger.child("launches");
    receipts.with_lock(&launch.id, || {
        let file = format!("{}.json", launch.id);
        if let Some(value) = receipts.read::<Value>(&file)? {
            // A saved receipt is the only evidence that this request dispatched. It
            // replays only when it correlates to this request and to the recorded job.
            if let Action::Job {
                cmd,
                timeout_ms,
                idle_timeout_ms,
                ..
            } = &launch.action
            {
                // A correlated receipt replays after expiry. It is read-only: it returns
                // the saved result and dispatches nothing.
                correlate(ledger, launch, cmd, *timeout_ms, *idle_timeout_ms, &value)?;
            }
            return Ok(value);
        }
        // New execution checks the request's age before it claims anything, allocates a
        // pane, or dispatches the command.
        if let Action::Job { validity, .. } = &launch.action {
            jobs::check_validity(ledger, validity.as_ref())?;
        }
        let result = match &launch.action {
            Action::Job {
                cmd,
                timeout_ms,
                idle_timeout_ms,
                ..
            } => launch_job(ledger, shell, launch, cmd, *timeout_ms, *idle_timeout_ms)?,
            Action::Agent { .. } => {
                return Err(Error::Invalid(
                    "agent launch must use its receipt owner".into(),
                ));
            }
            Action::Place => return Err(Error::Invalid("place does not launch a pane".into())),
        };
        receipts.write(&file, &result)?;
        crate::prepare::used(ledger, &launch.cwd, Some(&launch.id))?;
        Ok(result)
    })
}

/// Herdr types `agent start` into the pane as one line. A terminal in canonical mode refuses a
/// line over 1024 bytes (`agent_start_input_failed`). The task and the child contract never go on
/// that line, so its length does not depend on them; what remains is the name, the profile's
/// flags and paths on the launch host. This budget refuses a line those make too long. The
/// margin covers the newline and Herdr's own quoting, which can differ from ours.
pub const LINE_BUDGET: usize = 960;

/// The bytes of the line Herdr types for `kind` with `args`, with each argument quoted for a
/// POSIX shell.
#[must_use]
pub fn line_len(kind: &str, args: &[String]) -> usize {
    kind.len()
        + args
            .iter()
            .map(|arg| 1 + crate::shell::quote(arg).len())
            .sum::<usize>()
}

/// The files on the launch host that hold the text a launch does not type: the task and the
/// child contract. They outlive the pane for review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Files {
    pub task: String,
    pub contract: String,
}

impl Files {
    /// The paths for launch `id` in the `launches` directory `dir`. Nothing is written.
    #[must_use]
    pub fn at(dir: &std::path::Path, id: &str) -> Self {
        let path = |suffix: &str| dir.join(format!("{id}.{suffix}")).display().to_string();
        Self {
            task: path("task.md"),
            contract: path("contract.md"),
        }
    }

    /// Write the task and the child contract for launch `id`, and return their paths.
    pub fn write(dir: &std::path::Path, id: &str, message: &str) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let files = Self::at(dir, id);
        std::fs::write(&files.task, message)?;
        std::fs::write(&files.contract, crate::done::CONTRACT)?;
        Ok(files)
    }
}

/// The harness arguments after `--`: `lead` (the unit seed flags), Pi's session name and profile
/// flags, the child contract (`crate::done::CONTRACT`) as system instructions, then the task.
/// The task is always read from `files.task`: Pi's `@file`, or a pointer for Claude and Codex.
/// Pi and Claude read the contract from `files.contract`. Codex has no file form for
/// `developer_instructions`, so its contract stays inline; that text is fixed. No argument
/// holds task text, so the line is the same for any task. A line over `LINE_BUDGET` is refused
/// here, before Herdr can refuse it in the pane.
pub fn agent_args(
    kind: &str,
    name: &str,
    lead: &[String],
    files: &Files,
    profile: Option<&crate::profiles::Profile>,
) -> Result<Vec<String>> {
    let mut args = lead.to_vec();
    let task = match kind {
        "pi" => {
            args.extend(["--name".to_owned(), name.to_owned()]);
            args.extend(
                profile
                    .map(crate::profiles::Profile::pi_flags)
                    .unwrap_or_default(),
            );
            args.extend(["--append-system-prompt".to_owned(), files.contract.clone()]);
            format!("@{}", files.task)
        }
        "claude" | "codex" if profile.is_none() => {
            if kind == "claude" {
                args.extend([
                    "--append-system-prompt-file".to_owned(),
                    files.contract.clone(),
                ]);
            } else {
                // Codex adds `developer_instructions` to its own; the value is TOML.
                args.extend([
                    "-c".to_owned(),
                    format!(
                        "developer_instructions={}",
                        toml::Value::String(crate::done::CONTRACT.to_owned())
                    ),
                ]);
            }
            format!(
                "Your task is in {}. Read that file and do the task.",
                files.task
            )
        }
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
    };
    args.push(task);
    let len = line_len(kind, &args);
    if len > LINE_BUDGET {
        return Err(Error::Invalid(format!(
            "the {kind} launch line is {len} bytes, over the {LINE_BUDGET}-byte budget: Herdr \
             types it as one terminal line, which holds at most 1024 bytes; the task is not on \
             it, so shorten the name, the profile's flags or the state directory's path"
        )));
    }
    Ok(args)
}

/// A new request runs now only when no ticket that could take one of its hosts waits ahead of
/// it; otherwise the caller enqueues it, and admission serves the queue in order. `None` means
/// queue it.
pub fn submit(fleet: &Fleet, request: &Request, now: u64) -> Result<Option<Value>> {
    if waits_behind(fleet, request)? {
        return Ok(None);
    }
    perform(fleet, request, now)
}

/// The hosts a request may use: its pinned host, or every host.
fn candidates(fleet: &Fleet, request: &Request) -> Vec<String> {
    match request.host.as_deref() {
        Some(host) => vec![
            fleet
                .find(host)
                .map_or_else(|_| host.to_owned(), |h| h.id.clone()),
        ],
        None => fleet.hosts.iter().map(|h| h.id.clone()).collect(),
    }
}

/// Order holds per host. A pending ticket keeps `request` back only when the two could take the
/// same host, so a full peer never holds back work for a host with room.
pub(crate) fn waits_behind(fleet: &Fleet, request: &Request) -> Result<bool> {
    let wanted = candidates(fleet, request);
    Ok(queue(fleet.ledger)?
        .iter()
        .filter(|t| t.state == "pending")
        .any(|t| {
            candidates(fleet, &t.request)
                .iter()
                .any(|h| wanted.contains(h))
        }))
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
    pub diagnostic: Option<crate::diagnostic::Diagnostic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<crate::unit_request::Prepared>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

/// A local preparation that returned an error never launches: `perform_locked` refuses to resume
/// an attempt that stopped during preparation, so its hold has no owner left.
pub(crate) fn preparation_failed(ledger: &Ledger, request_id: &str) -> Result<bool> {
    jobs::validate_id(request_id)?;
    Ok(ledger
        .child("admissions")
        .read::<Attempt>(&format!("{request_id}.json"))?
        .is_some_and(|attempt| {
            attempt.host.as_ref().is_some_and(Host::is_local)
                && attempt.launch.is_none()
                && attempt.error.is_some()
        }))
}

pub fn perform(fleet: &Fleet, request: &Request, now: u64) -> Result<Option<Value>> {
    jobs::validate_id(&request.id)?;
    if matches!(request.action, Action::Place) {
        let Placement::Placed {
            host, reservation, ..
        } = placement(fleet, request, request.host.as_deref(), now)?
        else {
            return Ok(None);
        };
        let launch = prepare_launch(fleet, request, &host)
            .map_err(|error| prelaunch_error(error.correlate(Some(&request.id))))?;
        if host.is_local() {
            crate::prepare::used(fleet.ledger, &launch.cwd, Some(&launch.id))?;
        } else {
            let _: Value = fleet.remote_setup(
                &host,
                &[
                    "prepared".into(),
                    "--path".into(),
                    launch.cwd.clone(),
                    "--request-id".into(),
                    launch.id.clone(),
                ],
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
    // Admission checks the request's age before it places, prepares, or dispatches
    // anything. A request that may not start new execution is refused here.
    check_request_validity(fleet.ledger, request)?;
    let attempts = fleet.ledger.child("admissions");
    let file = format!("{}.json", request.id);
    if let Some(mut attempt) = attempts.read::<Attempt>(&file)? {
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
        if attempt.launch.is_some() && !matches!(request.action, Action::Agent { .. }) {
            return Err(Error::Invalid(format!(
                "request {} already attempted a non-reconcilable launch; inspect before resubmitting",
                request.id
            )));
        }
        if let Some(host) = attempt.host.clone() {
            if attempt.launch.is_none() {
                // A returned remote failure is safe to repeat: checkout validates identity,
                // and preparation replays its request-keyed receipt. An interrupted local
                // preparation, or one with no returned error, still requires inspection.
                let retry_queued = queue(fleet.ledger)?.iter().any(|t| {
                    t.request.id == request.id
                        && t.retry.attempts > 0
                        && matches!(t.state.as_str(), "pending" | "admitting")
                });
                if host.is_local() || attempt.error.is_none() || !retry_queued {
                    return Err(Error::Invalid(format!(
                        "request {} stopped during preparation on {}; inspect that host before resubmitting: {}",
                        request.id,
                        host.label,
                        attempt
                            .error
                            .as_deref()
                            .unwrap_or("preparation outcome is unknown")
                    )));
                }
                if matches!(
                    placement(fleet, request, Some(&host.id), now)?,
                    Placement::Full { .. }
                ) {
                    return Ok(None);
                }
                let prepared = prepare_launch(fleet, request, &host);
                let launch = match prepared {
                    Ok(launch) => launch,
                    Err(error) => {
                        attempt.error = Some(error.to_string());
                        attempt.diagnostic = error.diagnostic().cloned();
                        attempts.write(&file, &attempt)?;
                        return Err(prelaunch_error(error));
                    }
                };
                attempt.launch = Some(launch);
                attempt.error = None;
                attempt.diagnostic = None;
                attempts.write(&file, &attempt)?;
            }
            let launch = attempt
                .launch
                .as_ref()
                .ok_or_else(|| Error::Invalid("missing prepared launch".into()))?;
            let result = finish(fleet, request, &host, launch, now)?;
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
    let placed = placement(fleet, request, request.host.as_deref(), now)?;
    let Placement::Placed { host, .. } = placed else {
        return Ok(None);
    };
    let mut attempt = Attempt {
        request: serde_json::to_value(request)?,
        host: Some(host.clone()),
        launch: None,
        error: None,
        diagnostic: None,
        unit: attempts.read::<Attempt>(file)?.and_then(|a| a.unit),
        result: None,
    };
    attempts.write(file, &attempt)?;
    let prepared = prepare_launch(fleet, request, &host);
    let launch_request = match prepared {
        Ok(launch) => launch,
        Err(error) => {
            attempt.error = Some(error.to_string());
            attempt.diagnostic = error.diagnostic().cloned();
            attempts.write(file, &attempt)?;
            return Err(prelaunch_error(error));
        }
    };
    attempt.launch = Some(launch_request.clone());
    attempts.write(file, &attempt)?;
    let result = finish(fleet, request, &host, &launch_request, now)?;
    save_result(attempts, file, attempt, &result)?;
    Ok(Some(result))
}

fn placement(fleet: &Fleet, request: &Request, host: Option<&str>, now: u64) -> Result<Placement> {
    let validity = request_validity(request).copied();
    let placed =
        commands::place_with(fleet, host, &request.id, now, validity).map_err(prelaunch_error)?;
    if let Placement::Full { hosts, .. } = &placed {
        defer(fleet, request, hosts, now)?;
    }
    Ok(placed)
}

/// Only transport errors are retryable before a launch receipt exists. Local setup errors
/// and the peer's structured refusals remain terminal.
fn prelaunch_error(error: Error) -> Error {
    if transient_ssh(&error) {
        Error::Launch(error.to_string())
    } else {
        error
    }
}

fn prepare_launch(fleet: &Fleet, request: &Request, host: &Host) -> Result<Launch> {
    let checkout = request
        .repository
        .as_ref()
        .map(|repo| {
            crate::checkout::ensure(fleet, host, repo)
                .map_err(|error| error.correlate(Some(&request.id)).on_host(&host.label))
        })
        .transpose()?;
    let prepared = if let Some(branch) = &request.branch {
        let repo = request
            .repository
            .as_ref()
            .ok_or_else(|| Error::Invalid("branch requires repository".into()))?;
        crate::prepare::run(
            fleet,
            host,
            &repo.path,
            branch,
            request.base.as_deref(),
            &request.id,
        )
        .map_err(|error| error.correlate(Some(&request.id)).on_host(&host.label))?
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
    let unit = fleet
        .ledger
        .child("admissions")
        .read::<Attempt>(&format!("{}.json", request.id))?
        .and_then(|a| a.unit)
        .and_then(|u| u.committed)
        .map(crate::unit_transport::Seed::from);
    Ok(Launch {
        id: request.id.clone(),
        unit,
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
        owner_host: request.owner_host.clone(),
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
        launch(
            fleet.ledger,
            fleet.shell,
            launch_request,
            now,
            Some(jobs::VALIDITY_PROTOCOL_VERSION),
        )
    } else {
        let mut args = vec![
            "launch".into(),
            "--request".into(),
            serde_json::to_string(launch_request)?,
        ];
        // The validity protocol rides with a job launch. A peer that does not speak it
        // rejects the flag, and the caller reports an unsupported protocol.
        if matches!(request.action, Action::Job { .. }) {
            args.extend([
                "--job-validity".into(),
                crate::jobs::VALIDITY_PROTOCOL_VERSION.to_string(),
            ]);
        }
        fleet.remote_setup(host, &args)
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
    // Queued and still-launching requests have no item: only a successful receipt
    // reaches this point. Preserve its host-clock confirmation, not admission's `now`.
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
        launch_confirmed_ms: result["launch_confirmed_ms"].as_u64(),
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
    // Hosts that had no room for an earlier ticket in this pass. A later ticket that can use
    // only these hosts waits, so it cannot pass an earlier one; any other ticket is still served.
    let mut full: Vec<String> = Vec::new();
    for ticket in queue(fleet.ledger)? {
        let mut ticket = ticket;
        if ticket.state == "admitting" {
            let saved = fleet
                .ledger
                .child("admissions")
                .read::<Attempt>(&format!("{}.json", ticket.request.id))?;
            let recoverable = matches!(ticket.request.action, Action::Agent { .. })
                && saved.as_ref().is_some_and(|a| {
                    a.launch.is_some()
                        || (a.unit.is_some() && a.host.is_none())
                        || (ticket.retry.attempts > 0
                            && a.error.is_some()
                            && a.host.as_ref().is_some_and(|h| !h.is_local()))
                });
            ticket = update_ticket(fleet.ledger, &ticket.id, |t| {
                t.state = if recoverable { "pending" } else { "failed" }.into();
                t.diagnostic = saved.as_ref().and_then(|a| a.diagnostic.clone());
                let message = "service stopped during admission; inspect the selected host before resubmitting";
                t.error = Some(t.diagnostic.as_ref().map_or_else(
                    || message.into(),
                    |diagnostic| format!("{message}; {diagnostic}"),
                ));
            })?;
        }
        if ticket.state != "pending" || !ticket.retry.ready(now) {
            continue;
        }
        let hosts = candidates(fleet, &ticket.request);
        if hosts.iter().all(|h| full.contains(h)) {
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
        let no_room = matches!(&outcome, Ok(None));
        update_ticket(fleet.ledger, &ticket.id, |t| match outcome {
            Ok(Some(result)) => {
                t.state = "admitted".into();
                t.result = Some(result);
                t.wait_reason = None;
                t.error = None;
                t.diagnostic = None;
            }
            Ok(None) => t.state = "pending".into(),
            // A refused request fails at once; a failed launch, SSH, or Herdr call is retried.
            Err(error @ Error::Launch(_)) if t.retry.attempts + 1 < LAUNCH_TRIES => {
                t.state = "pending".into();
                t.retry.fail(now, &t.id);
                t.error = Some(format!("attempt {}: {error}", t.retry.attempts));
                t.diagnostic = error.diagnostic().cloned();
            }
            Err(error) => {
                t.state = "failed".into();
                t.diagnostic = error.diagnostic().cloned();
                t.error = Some(if t.retry.attempts > 0 {
                    format!("{error} (after {} attempts)", t.retry.attempts + 1)
                } else {
                    error.to_string()
                });
            }
        })?;
        if no_room {
            full.extend(hosts);
            if fleet.hosts.iter().all(|h| full.contains(&h.id)) {
                break;
            }
        }
    }
    Ok(())
}

/// A launch that failed after placement: Herdr did not start the agent in time, or the SSH
/// launch call failed. Refusals and deadlines are not retried.
fn retryable(error: &Error) -> bool {
    transient_ssh(error) || matches!(error.root(), Error::Command { .. } | Error::Io(_))
}

/// Retry only SSH transport categories. Authentication, remote commands, malformed replies,
/// structured refusals, and deadlines are terminal, even when wrapped in startup diagnostics.
fn transient_ssh(error: &Error) -> bool {
    use crate::hosts::Failure;
    matches!(
        error.root(),
        Error::Ssh {
            failure: Failure::Timeout
                | Failure::ConnectionRefused
                | Failure::HostNotResolved
                | Failure::Other,
            refusal: None,
            ..
        }
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
            let selected = fleet
                .ledger
                .child("admissions")
                .read::<Attempt>(&format!("{}.json", ticket.request.id))?
                .and_then(|a| a.host);
            let host = result
                .and_then(|r| r["host"].as_str().or_else(|| r["host"]["id"].as_str()))
                .or_else(|| selected.as_ref().map(|h| h.label.as_str()))
                .or_else(|| ticket.diagnostic.as_ref().and_then(|d| d.host.as_deref()))
                .or(ticket.request.host.as_deref())
                .unwrap_or("unselected");
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
                fleet,
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

#[cfg(test)]
mod line_limit_tests {
    use super::*;

    fn files() -> Files {
        Files::at(
            std::path::Path::new("/state/launches"),
            "r-1792603096562-36969-335",
        )
    }

    fn build(kind: &str, name: &str) -> Result<Vec<String>> {
        agent_args(kind, name, &[], &files(), None)
    }

    #[test]
    fn the_line_is_the_same_for_every_task_and_fits() {
        for kind in KINDS {
            let args = build(kind, "cleanup-identity-rondo").unwrap();
            assert!(line_len(kind, &args) <= LINE_BUDGET, "{kind}");
            assert!(
                args.last()
                    .unwrap()
                    .contains("r-1792603096562-36969-335.task.md")
            );
        }
    }

    #[test]
    fn pi_and_claude_read_the_contract_from_a_file() {
        for (kind, flag) in [
            ("pi", "--append-system-prompt"),
            ("claude", "--append-system-prompt-file"),
        ] {
            let args = build(kind, "n").unwrap();
            let at = args.iter().position(|a| a == flag).unwrap();
            assert_eq!(args[at + 1], files().contract, "{kind}");
            assert!(
                !args.iter().any(|a| a.contains("Dogsled started you")),
                "{kind}"
            );
        }
    }

    #[test]
    fn a_long_name_is_refused() {
        let error = build("pi", &"n".repeat(2_000)).unwrap_err().to_string();
        assert!(error.contains("budget"), "{error}");
    }

    #[test]
    fn a_long_state_path_is_refused_where_the_files_live() {
        let dir = std::path::PathBuf::from(format!("/{}", "d".repeat(1_000)));
        let files = Files::at(&dir, "r-1");
        assert!(agent_args("pi", "n", &[], &files, None).is_err());
    }
}
