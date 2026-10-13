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

fn request(fleet: &Fleet, args: &WorkArgs, action: Action) -> Result<Request> {
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

fn submit(fleet: &Fleet, args: &WorkArgs, action: Action, json_output: bool) -> Result<ExitCode> {
    let request = request(fleet, args, action)?;
    let job = matches!(request.action, Action::Job { .. });
    let result = if let Some(result) = work::perform(fleet, &request, now_ms())? {
        result
    } else {
        let ticket = work::enqueue(fleet.ledger, request, args.wait, now_ms())?;
        crate::print(true, &json!({"queued":true,"ticket":ticket}), String::new)?;
        if !args.wait {
            return Ok(ExitCode::from(1));
        }
        wait_ticket(fleet, &ticket)?
    };
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
            placement,
        } => submit(
            fleet,
            &placement,
            Action::Agent {
                kind,
                name,
                message,
            },
            true,
        ),
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
                fleet.remote(host, &["doctor".into()])?
            };
            crate::print(json_output, &plan, || crate::cleanup::render(&plan))?;
        }
        Command::Prune { host, yes, classes } => {
            let host = fleet.find(host.as_deref().unwrap_or("local"))?;
            let mut plan: crate::cleanup::Plan = if host.is_local() {
                crate::cleanup::doctor(&fleet.node(), now_ms())?
            } else {
                fleet.remote(host, &["doctor".into()])?
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
            let result = if host.is_local() {
                crate::cleanup::apply(&fleet.node(), &plan, now_ms())?
            } else {
                fleet.remote(
                    host,
                    &[
                        "prune".into(),
                        "--plan".into(),
                        serde_json::to_string(&plan)?,
                    ],
                )?
            };
            crate::print(json_output, &result, || result.join("\n") + "\n")?;
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
            let host = match host {
                Some(host) => fleet.find(&host)?,
                None => close_host(fleet, &pane)?,
            };
            let closed: crate::close::Closed = if host.is_local() {
                crate::close::local(fleet.shell, &pane, &crate::close::Caller::from_env())?
            } else {
                fleet.remote_setup(host, &["close".into(), pane.clone()])?
            };
            items::close(fleet.ledger, &host.id, &pane, now_ms())?;
            crate::print(json_output, &closed, || {
                closed.worktree.as_ref().map_or_else(
                    || format!("closed pane {} on {}\n", closed.pane, host.label),
                    |tree| {
                        format!(
                            "closed {} on {}: removed {} ({})\n",
                            closed.pane, host.label, tree.path, tree.branch
                        )
                    },
                )
            })?;
        }
        command => return inspect(fleet, command, json_output),
    }
    Ok(ExitCode::SUCCESS)
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
        Command::Stop { id } => {
            let host = work::job_host(fleet, &id, now_ms())?;
            if host.is_local() {
                jobs::stop(fleet.ledger, &id)?;
            } else {
                let _: Value = fleet.remote(host, &["stop".into(), id])?;
            }
        }
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
