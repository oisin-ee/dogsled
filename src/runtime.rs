//! CLI orchestration and bounded waits, separate from persisted action policy.

use crate::cli::{Command, WorkArgs};
use crate::error::{Error, Result};
use crate::hosts::Fleet;
use crate::items::{self, Item};
use crate::jobs::{self, Job};
use crate::ledger::{new_id, now_ms};
use crate::work::{self, Action, Request};
use serde_json::{Value, json};
use std::io::Write as _;
use std::process::ExitCode;
use std::time::Duration;

fn caller() -> Result<String> {
    std::env::var("HERDR_PANE_ID").map_err(|_| Error::Invalid("HERDR_PANE_ID is required".into()))
}

pub fn request(fleet: &Fleet, args: &WorkArgs, action: Action) -> Result<Request> {
    let cwd = args.cwd.clone().map_or_else(
        || std::env::current_dir().map(|p| p.display().to_string()),
        Ok,
    )?;
    let repository = if matches!(action, Action::Place) && args.branch.is_none() {
        None
    } else {
        crate::checkout::discover(fleet.shell, &cwd, &fleet.home, args.branch.is_some())?
    };
    let owner = if matches!(action, Action::Place) {
        std::env::var("HERDR_PANE_ID").unwrap_or_default()
    } else {
        caller()?
    };
    let workspace = std::env::var("HERDR_WORKSPACE_ID").ok();
    let workspace_name = if matches!(action, Action::Place) {
        None
    } else {
        Some(work::workspace(
            fleet.shell,
            workspace
                .as_deref()
                .ok_or_else(|| Error::Invalid("HERDR_WORKSPACE_ID is required".into()))?,
        )?)
    };
    Ok(Request {
        id: new_id(),
        action,
        host: args.host.clone(),
        branch: args.branch.clone(),
        base: args.base.clone(),
        cwd,
        repository,
        workspace,
        workspace_name,
        owner,
    })
}

fn wait_ticket(fleet: &Fleet, id: &str) -> Result<Value> {
    loop {
        let tickets = work::queue(fleet.ledger)?;
        let ticket = tickets
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| Error::Invalid(format!("unknown ticket {id}")))?;
        match ticket.state.as_str() {
            "admitted" => {
                return ticket
                    .result
                    .clone()
                    .ok_or_else(|| Error::Invalid("missing admission result".into()));
            }
            "failed" | "cancelled" => {
                return Err(Error::Invalid(
                    ticket
                        .error
                        .clone()
                        .unwrap_or_else(|| format!("ticket {id} cancelled")),
                ));
            }
            _ => (),
        }
        fleet.ledger.budget.pause()?;
    }
}

pub fn wait_job(fleet: &Fleet, id: &str) -> Result<i32> {
    let host = work::job_host(fleet, id, now_ms())?;
    loop {
        let job: Job = if host.is_local() {
            jobs::get(fleet.ledger, id)?
        } else {
            fleet.remote(host, &["job".into(), id.into()])?
        };
        if job.ended_ms.is_some() {
            return Ok(job.exit.unwrap_or(2));
        }
        fleet.ledger.budget.pause()?;
    }
}

/// Start `request` now, or queue it; never waits. A queued result is
/// `{"queued":true,"ticket":…}`, with `retrying` when the first launch failed and the service
/// retries it with backoff.
pub fn start(fleet: &Fleet, request: Request, waiting: bool) -> Result<Value> {
    match work::submit(fleet, &request, now_ms()) {
        Ok(Some(result)) => Ok(result),
        Ok(None) => {
            let ticket = work::enqueue(fleet.ledger, request, waiting, now_ms())?;
            Ok(json!({"queued":true,"ticket":ticket}))
        }
        Err(error @ Error::Launch(_)) => {
            let ticket = work::enqueue_retry(fleet.ledger, request, waiting, &error, now_ms())?;
            Ok(json!({"queued":true,"ticket":ticket,"retrying":error.to_string()}))
        }
        Err(error) => Err(error),
    }
}

fn submit(fleet: &Fleet, args: &WorkArgs, action: Action, json_output: bool) -> Result<ExitCode> {
    let request = request(fleet, args, action)?;
    let job = matches!(request.action, Action::Job { .. });
    let mut result = start(fleet, request, args.wait)?;
    if result["queued"] == true {
        crate::print(true, &result, String::new)?;
        if !args.wait {
            return Ok(ExitCode::from(1));
        }
        let ticket = result["ticket"].as_str().unwrap_or_default().to_owned();
        result = wait_ticket(fleet, &ticket)?;
    }
    crate::print(
        json_output || !matches!(result["host"], Value::Object(_)),
        &result,
        || format!("{result}\n"),
    )?;
    if args.wait && job {
        let id = result["id"]
            .as_str()
            .ok_or_else(|| Error::Invalid("launch returned no id".into()))?;
        return Ok(ExitCode::from(wait_job(fleet, id)? as u8));
    }
    Ok(ExitCode::SUCCESS)
}

/// The launch of a `kind` child with `message`, checked before anything is placed: a profile
/// is for Pi, and a local child's profile must load.
pub fn agent_action(
    fleet: &Fleet,
    kind: String,
    name: String,
    message: String,
    profile: Option<String>,
    host: Option<&str>,
) -> Result<Action> {
    if !work::KINDS.contains(&kind.as_str()) {
        return Err(Error::Invalid(format!(
            "kind {kind} cannot take its task at launch; use one of {}",
            work::KINDS.join(", ")
        )));
    }
    if profile.is_some() && kind != "pi" {
        return Err(Error::Invalid(
            "profiles configure Pi children; omit the profile or use kind pi".into(),
        ));
    }
    // A peer checks its own profiles.
    if let Some(profile) = &profile
        && host.is_none_or(|h| fleet.find(h).is_ok_and(crate::hosts::Host::is_local))
    {
        crate::profiles::load(profile)?;
    }
    if message.trim().is_empty() {
        return Err(Error::Invalid("the task is empty".into()));
    }
    Ok(Action::Agent {
        kind,
        name,
        message,
        profile,
    })
}

/// Close `pane` on `host`, or on the host of its open item.
pub fn close(
    fleet: &Fleet,
    pane: &str,
    host: Option<&str>,
) -> Result<(crate::close::Closed, String)> {
    let host = match host {
        Some(host) => fleet.find(host)?,
        None => close_host(fleet, pane)?,
    };
    let closed: crate::close::Closed = if host.is_local() {
        crate::close::local(
            fleet.shell,
            fleet.ledger,
            pane,
            &crate::close::Caller::from_env(),
            crate::close::Guard::Clean,
        )?
    } else {
        fleet.remote_setup(host, &["close".into(), pane.into()])?
    };
    items::close(fleet.ledger, &host.id, pane, now_ms())?;
    Ok((closed, host.label.clone()))
}

pub fn closed_line(closed: &crate::close::Closed, label: &str) -> String {
    closed.worktree.as_ref().map_or_else(
        || format!("closed pane {} on {label}\n", closed.pane),
        |tree| {
            format!(
                "closed {} on {label}: removed {} ({})\n",
                closed.pane, tree.path, tree.branch
            )
        },
    )
}

pub fn stop(fleet: &Fleet, id: &str) -> Result<()> {
    let host = work::job_host(fleet, id, now_ms())?;
    if host.is_local() {
        jobs::stop(fleet.ledger, id)
    } else {
        let _: Value = fleet.remote(host, &["stop".into(), id.into()])?;
        Ok(())
    }
}

/// The newest output of a child or job of the caller, by name, pane, or id, with its captured
/// final reply. Without such an item, any agent on the fleet by name or pane.
pub fn read(fleet: &Fleet, target: &str, host: Option<&str>) -> Result<Value> {
    let pinned = host.map(|h| fleet.find(h)).transpose()?;
    let owner = std::env::var("HERDR_PANE_ID").ok();
    let item = items::list(fleet.ledger)?
        .into_iter()
        .rev()
        .filter(|i| owner.as_ref().is_none_or(|o| *o == i.owner_pane))
        .filter(|i| pinned.is_none_or(|h| h.id == i.host))
        .find(|i| i.id == target || i.pane == target || i.name.as_deref() == Some(target));
    let item = if let Some(item) = item {
        item
    } else {
        let found = crate::inbox::resolve(fleet, target, host, now_ms())?;
        Item {
            kind: "agent".into(),
            host: found.host.id.clone(),
            pane: found.agent.pane,
            name: found.agent.name,
            ..Item::default()
        }
    };
    let host = fleet.find(&item.host)?;
    let tail = crate::watchdog::item_tail(fleet, &item, host)?;
    Ok(json!({
        "name": item.name,
        "kind": item.kind,
        "host": host.label,
        "pane": item.pane,
        "closed": item.closed_ms.is_some(),
        "reply": item.reply.map(|r| r.text),
        "tail": tail,
    }))
}

pub fn dispatch(fleet: &Fleet, command: Command, json_output: bool) -> Result<ExitCode> {
    match command {
        Command::Place {
            host,
            branch,
            base,
            repo,
            wait,
            max,
        } => submit(
            fleet,
            &WorkArgs {
                host,
                branch,
                base,
                cwd: repo,
                wait,
                max,
            },
            Action::Place,
            json_output,
        ),
        Command::Run {
            placement,
            timeout,
            idle_timeout,
            cmd,
        } => submit(
            fleet,
            &placement,
            Action::Job {
                cmd,
                timeout_ms: timeout,
                idle_timeout_ms: idle_timeout,
            },
            json_output,
        ),
        Command::Agent {
            kind,
            name,
            message,
            message_file,
            profile,
            placement,
        } => {
            let message = crate::inbox::message_text(message, message_file)?;
            let action = agent_action(
                fleet,
                kind,
                name,
                message,
                profile,
                placement.host.as_deref(),
            )?;
            submit(fleet, &placement, action, true)
        }
        Command::Read { target, host } => {
            let read = read(fleet, &target, host.as_deref())?;
            crate::print(json_output, &read, || read_text(&read))?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Profiles => {
            let profiles = crate::profiles::list()?;
            crate::print(json_output, &profiles, || {
                crate::profiles::render(&profiles)
            })?;
            Ok(ExitCode::SUCCESS)
        }
        command => cleanup_command(fleet, command, json_output),
    }
}

fn cleanup_command(fleet: &Fleet, command: Command, json_output: bool) -> Result<ExitCode> {
    match command {
        Command::Doctor { host } => {
            let host = fleet.find(host.as_deref().unwrap_or("local"))?;
            let plan = if host.is_local() {
                crate::cleanup::doctor(&fleet.node(), now_ms())?
            } else {
                fleet.remote_setup(host, &["doctor".into()])?
            };
            crate::print(json_output, &plan, || crate::cleanup::render(&plan))?;
        }
        Command::Prune { host, yes, classes } => {
            let host = fleet.find(host.as_deref().unwrap_or("local"))?;
            let mut plan: crate::cleanup::Plan = if host.is_local() {
                crate::cleanup::doctor(&fleet.node(), now_ms())?
            } else {
                fleet.remote_setup(host, &["doctor".into()])?
            };
            plan.entries
                .retain(|e| classes.is_empty() || classes.contains(&e.class));
            crate::print(json_output, &plan, || crate::cleanup::render(&plan))?;
            if !yes {
                eprint!("Remove these eligible entries? [y/N] ");
                std::io::stderr().flush()?;
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                if !matches!(answer.trim(), "y" | "Y" | "yes") {
                    return Ok(ExitCode::SUCCESS);
                }
            }
            let applied: crate::cleanup::Applied = if host.is_local() {
                crate::cleanup::apply(&fleet.node(), &plan, now_ms())?
            } else {
                fleet.remote_setup(
                    host,
                    &[
                        "prune".into(),
                        "--plan".into(),
                        serde_json::to_string(&plan)?,
                    ],
                )?
            };
            crate::print(json_output, &applied, || applied.render())?;
            if !applied.failures.is_empty() {
                return Ok(ExitCode::from(2));
            }
        }
        Command::Track { host, pane, name } => {
            let host = fleet.find(&host)?;
            let item = Item {
                id: new_id(),
                kind: "agent".into(),
                host: host.id.clone(),
                pane,
                owner_pane: caller()?,
                name,
                created_ms: now_ms(),
                closed_ms: None,
                ..Item::default()
            };
            items::record(fleet.ledger, item.clone())?;
            if host.is_local() {
                crate::watchdog::register_parent(fleet.ledger, fleet.shell, &item.pane)?;
            } else {
                let _: bool = fleet.remote(host, &["parent".into(), item.pane.clone()])?;
            }
            crate::print(true, &item, String::new)?;
        }
        Command::Close { pane, host } => {
            let (closed, label) = close(fleet, &pane, host.as_deref())?;
            crate::print(json_output, &closed, || closed_line(&closed, &label))?;
        }
        command => return inspect(fleet, command, json_output),
    }
    Ok(ExitCode::SUCCESS)
}

pub fn read_text(read: &Value) -> String {
    let text = |key: &str| read[key].as_str().unwrap_or_default().to_owned();
    let mut out = format!(
        "{} {} on {}{}\n",
        read["name"].as_str().unwrap_or(&text("pane")),
        text("pane"),
        text("host"),
        if read["closed"] == true {
            " (closed)"
        } else {
            ""
        }
    );
    if let Some(reply) = read["reply"].as_str() {
        out.push_str("--- final reply ---\n");
        out.push_str(reply);
        out.push_str("\n--- end final reply ---\n");
    }
    out.push_str(&text("tail"));
    out.push('\n');
    out
}

/// The host of the open item for `pane`. Pane ids repeat across hosts, so more than one is ambiguous.
fn close_host<'a>(fleet: &'a Fleet, pane: &str) -> Result<&'a crate::hosts::Host> {
    let hosts: std::collections::BTreeSet<_> = items::list(fleet.ledger)?
        .into_iter()
        .filter(|i| i.closed_ms.is_none() && i.pane == pane)
        .map(|i| i.host)
        .collect();
    match hosts.len() {
        0 => fleet.find("local"),
        1 => fleet.find(hosts.first().map_or("local", String::as_str)),
        _ => Err(Error::Invalid(format!(
            "pane {pane} has open items on {}; pass --host",
            hosts.into_iter().collect::<Vec<_>>().join(", ")
        ))),
    }
}

fn inspect(fleet: &Fleet, command: Command, json_output: bool) -> Result<ExitCode> {
    match command {
        Command::Items { owner } => {
            let items: Vec<_> = items::list(fleet.ledger)?
                .into_iter()
                .filter(|i| owner.as_ref().is_none_or(|o| *o == i.owner_pane))
                .collect();
            let queue: Vec<_> = work::queue(fleet.ledger)?
                .into_iter()
                .filter(|t| owner.as_ref().is_none_or(|o| *o == t.request.owner))
                .collect();
            let output = json!({"items":items,"queue":queue});
            crate::print(json_output, &output, || format!("{output}\n"))?;
        }
        Command::Ps { all } => {
            let mut jobs = Vec::new();
            for host in fleet.status(now_ms())? {
                let host_jobs = if host.host.is_local() {
                    jobs::list(fleet.ledger)?
                } else {
                    host.jobs
                };
                jobs.extend(
                    host_jobs
                        .into_iter()
                        .filter(|j| all || j.ended_ms.is_none()),
                );
            }
            crate::print(json_output, &jobs, || {
                jobs.iter()
                    .map(|j| {
                        format!(
                            "{} {} {} exit={} {}",
                            j.id,
                            j.host,
                            j.state,
                            j.exit.map_or_else(|| "-".into(), |e| e.to_string()),
                            j.cmd.join(" ")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n"
            })?;
        }
        Command::Cancel { ticket } => work::cancel(fleet.ledger, &ticket)?,
        Command::Stop { id } => stop(fleet, &id)?,
        Command::Wait { id, .. } => return Ok(ExitCode::from(wait_job(fleet, &id)? as u8)),
        Command::Logs { id, follow } => {
            let host = work::job_host(fleet, &id, now_ms())?;
            let mut offset = 0;
            loop {
                let chunk = if host.is_local() {
                    jobs::logs(fleet.ledger, &id, offset)?
                } else {
                    fleet.remote(
                        host,
                        &[
                            "logs".into(),
                            id.clone(),
                            "--offset".into(),
                            offset.to_string(),
                        ],
                    )?
                };
                std::io::stdout().write_all(&chunk.bytes)?;
                std::io::stdout().flush()?;
                offset = chunk.next;
                if chunk.bytes.is_empty() {
                    if !follow || chunk.done {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
        _ => return Err(Error::Invalid("unexpected command dispatcher".into())),
    }
    Ok(ExitCode::SUCCESS)
}

pub fn exec(
    ledger: &crate::ledger::Ledger,
    shell: &dyn crate::shell::Shell,
    id: String,
    cmd: Vec<String>,
    timeout_ms: Option<u64>,
    idle_timeout_ms: Option<u64>,
) -> Result<ExitCode> {
    jobs::validate_id(&id)?;
    let job = match jobs::store(ledger).read::<Job>(&format!("{id}.json"))? {
        Some(job) => {
            if job.cmd != cmd {
                return Err(Error::Invalid(
                    "exec command differs from launch record".into(),
                ));
            }
            job
        }
        None => Job {
            id,
            cmd,
            cwd: std::env::current_dir()?.display().to_string(),
            host: "local".into(),
            owner: std::env::var("HERDR_PANE_ID").unwrap_or_default(),
            pane: std::env::var("HERDR_PANE_ID").unwrap_or_default(),
            state: "pending".into(),
            exit: None,
            signal: None,
            reason: None,
            started_ms: None,
            ended_ms: None,
            timeout_ms,
            idle_timeout_ms,
        },
    };
    Ok(ExitCode::from(jobs::exec(ledger, shell, job)? as u8))
}
