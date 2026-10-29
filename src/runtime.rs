//! CLI orchestration and bounded waits, separate from persisted action policy.

use crate::cli::{Command, WorkArgs};
use crate::error::{Error, Refusal, Result};
use crate::hosts::Fleet;
use crate::items::{self, Item};
use crate::jobs::{self, Job};
use crate::ledger::{new_id, now_ms};
use crate::work::{self, Action, Request};
use serde_json::{Value, json};
use std::io::Write as _;
use std::process::ExitCode;
use std::time::Duration;

/// One owner for CLI, MCP and explicit-host recovery operations.
pub fn recovery_local(
    shell: &dyn crate::shell::Shell,
    ledger: &crate::ledger::Ledger,
    command: &crate::cli::RecoveryCommand,
) -> Result<Value> {
    use crate::cli::RecoveryCommand;
    use crate::preservation;
    let value = match command {
        RecoveryCommand::List { repo } => serde_json::to_value(
            preservation::list(shell, std::path::Path::new(repo))
                .map_err(|e| Error::Invalid(e.to_string()))?,
        )?,
        RecoveryCommand::Show { id, repo } => serde_json::to_value(
            preservation::show(shell, std::path::Path::new(repo), id)
                .map_err(|e| Error::Invalid(e.to_string()))?,
        )?,
        RecoveryCommand::Restore {
            id,
            repo,
            to,
            branch,
        } => serde_json::to_value(preservation::restore(
            shell,
            ledger,
            std::path::Path::new(repo),
            id,
            std::path::Path::new(to),
            branch.as_deref(),
        )?)?,
    };
    Ok(value)
}

pub fn recovery(
    shell: &dyn crate::shell::Shell,
    ledger: &crate::ledger::Ledger,
    policy: &crate::config::Policy,
    home: &str,
    host: Option<&str>,
    command: &crate::cli::RecoveryCommand,
) -> Result<Value> {
    use crate::cli::RecoveryCommand;
    let Some(host) = host.filter(|host| *host != crate::hosts::LOCAL) else {
        return recovery_local(shell, ledger, command);
    };
    let fleet = Fleet::discover(shell, ledger, policy, home.into())?;
    let host = fleet.find(host)?;
    if host.is_local() {
        return recovery_local(shell, ledger, command);
    }
    let mut args = vec!["recovery".into()];
    match command {
        RecoveryCommand::List { repo } => {
            args.extend(["list".into(), "--repo".into(), repo.clone()]);
        }
        RecoveryCommand::Show { id, repo } => {
            args.extend(["show".into(), id.clone(), "--repo".into(), repo.clone()]);
        }
        RecoveryCommand::Restore {
            id,
            repo,
            to,
            branch,
        } => {
            args.extend([
                "restore".into(),
                id.clone(),
                "--repo".into(),
                repo.clone(),
                "--to".into(),
                to.clone(),
            ]);
            if let Some(branch) = branch {
                args.extend(["--branch".into(), branch.clone()]);
            }
        }
    }
    fleet.remote(host, &args).map_err(|error| {
        if matches!(error.root(), Error::Parse { .. } | Error::Ssh { failure: crate::hosts::Failure::RemoteCommandFailed | crate::hosts::Failure::UnexpectedOutput, .. }) {
            Error::Invalid(format!("recovery on selected host {} failed: {error}; no local or fleet fallback; ensure the peer supports host recovery", host.label))
        } else { error }
    })
}

fn caller() -> Result<String> {
    std::env::var("HERDR_PANE_ID").map_err(|_| Error::Invalid("HERDR_PANE_ID is required".into()))
}

pub fn request(fleet: &Fleet, args: &WorkArgs, action: Action) -> Result<Request> {
    request_with_id(fleet, args, action, new_id())
}

pub(crate) fn request_with_id(
    fleet: &Fleet,
    args: &WorkArgs,
    action: Action,
    id: String,
) -> Result<Request> {
    // A job request is stamped once, here, at its origin. The stamp never changes, and a
    // retry keeps it byte-identical.
    let action = match action {
        Action::Job {
            cmd,
            timeout_ms,
            idle_timeout_ms,
            ..
        } => Action::Job {
            cmd,
            timeout_ms,
            idle_timeout_ms,
            validity: Some(jobs::origin_validity(fleet.ledger)?),
        },
        action => action,
    };
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
        id,
        action,
        host: args.host.clone(),
        branch: args.branch.clone(),
        base: args.base.clone(),
        cwd,
        repository,
        workspace,
        workspace_name,
        owner,
        owner_host: Some(crate::inbox::hostname()),
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
                let error = Error::Invalid(
                    ticket
                        .error
                        .clone()
                        .unwrap_or_else(|| format!("ticket {id} cancelled")),
                );
                return Err(if let Some(diagnostic) = &ticket.diagnostic {
                    error.with_diagnostic(diagnostic.clone())
                } else {
                    error
                });
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
    start_using(fleet, request, waiting, work::submit)
}

/// Hand startup to the admission service instead of spending a tool call's budget.
pub(crate) fn start_queued(fleet: &Fleet, request: Request) -> Result<Value> {
    let ticket = work::enqueue(fleet.ledger, request, false, now_ms())?;
    Ok(json!({"queued":true,"ticket":ticket}))
}

/// Only for a caller that already owns the request's admission lock.
pub(crate) fn start_locked(fleet: &Fleet, request: Request) -> Result<Value> {
    start_using(fleet, request, false, |fleet, request, now| {
        work::allow_admission(fleet.ledger, request)?;
        if work::waits_behind(fleet, request)? {
            return Ok(None);
        }
        work::perform_locked(fleet, request, now)
    })
}

fn start_using(
    fleet: &Fleet,
    request: Request,
    waiting: bool,
    submit: fn(&Fleet, &Request, u64) -> Result<Option<Value>>,
) -> Result<Value> {
    let placement = matches!(request.action, Action::Place);
    let request_id = request.id.clone();
    let result = match submit(fleet, &request, now_ms()) {
        Ok(Some(result)) => Ok(result),
        Ok(None) => {
            let ticket = work::enqueue(fleet.ledger, request, waiting, now_ms())?;
            Ok(json!({"queued":true,"ticket":ticket}))
        }
        Err(error @ Error::Launch(_)) => {
            let ticket = work::enqueue_retry(fleet.ledger, request, waiting, &error, now_ms())?;
            Ok(json!({"queued":true,"ticket":ticket,"retrying":error.to_string()}))
        }
        Err(error) => Err(error.correlate(Some(&request_id))),
    }?;
    if placement && result["queued"] == true {
        let mut result = result;
        result["request_id"] = json!(request_id);
        Ok(result)
    } else {
        Ok(result)
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
    // The launch line holds paths of the host that runs it, so that host checks its length
    // (`launch::agent`, before it creates a pane). The caller's paths say nothing about a peer's.
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
    let request = items::list(fleet.ledger)?
        .iter()
        .rev()
        .find(|item| item.host == host.id && item.pane == pane)
        .map(crate::preservation::CloseRequest::of);
    let outcome = (|| -> Result<crate::close::Closed> {
        let closed: crate::close::Closed = if host.is_local() {
            crate::close::with_request(
                fleet.shell,
                fleet.ledger,
                pane,
                &crate::close::Caller::from_env(),
                crate::close::Options {
                    guard: crate::close::Guard::Clean,
                    request: request.as_ref(),
                },
                |closed| {
                    items::close_result(fleet.ledger, &host.id, &closed, now_ms())?;
                    Ok(closed)
                },
            )?
        } else {
            let mut args = vec!["close".into(), pane.into()];
            if let Some(request) = &request {
                args.extend([
                    "--preservation-request".into(),
                    serde_json::to_string(request)?,
                ]);
            }
            let closed = fleet.remote_setup(host, &args)?;
            items::close_result(fleet.ledger, &host.id, &closed, now_ms())?;
            closed
        };
        Ok(closed)
    })();
    let closed = match outcome {
        Ok(closed) => closed,
        Err(error) => {
            if let Some(request) = &request {
                items::close_blocker(fleet.ledger, &request.item, &error.to_string())?;
            }
            return Err(error);
        }
    };
    Ok((closed, host.label.clone()))
}

pub fn closed_line(closed: &crate::close::Closed, label: &str) -> String {
    let mut line = closed.worktree.as_ref().map_or_else(
        || format!("closed pane {} on {label}\n", closed.pane),
        |tree| {
            format!(
                "closed {} on {label}: removed {} ({})\n",
                closed.pane, tree.path, tree.branch
            )
        },
    );
    if let Some(recovery) = &closed.recovery {
        line = format!("{}; {}\n", line.trim_end(), recovery.summary());
    }
    line
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
    let tail = if item.kind == "agent" && item.closed_ms.is_some() {
        String::new()
    } else if item.kind == "job" && host.is_local() && jobs::expired(fleet.ledger, &item.id)? {
        // The job's output expired: the reader is told, and the read never fails.
        jobs::OUTPUT_EXPIRED.to_owned()
    } else {
        match crate::watchdog::item_tail(fleet, &item, host) {
            Err(error)
                if item.kind == "job"
                    && !host.is_local()
                    && matches!(
                        error.root(),
                        Error::Ssh {
                            refusal: Some(Refusal::UnknownJob),
                            ..
                        }
                    ) =>
            {
                // The owner item survives remote retention; nothing runs again.
                jobs::OUTPUT_EXPIRED.to_owned()
            }
            result => result?,
        }
    };
    Ok(json!({
        "name": item.name,
        "kind": item.kind,
        "host": host.label,
        "pane": item.pane,
        "closed": item.closed_ms.is_some(),
        "reply": item.reply.map(|r| r.text),
        "orphan_reply": item.orphan_reply.map(|r| r.text),
        "recovery": item.recovery,
        "close_blocker": item.resource_close_reason,
        "tail": tail,
    }))
}

fn place(
    fleet: &Fleet,
    resume: Option<&str>,
    args: &WorkArgs,
    json_output: bool,
) -> Result<ExitCode> {
    let id = resume.map_or_else(new_id, str::to_owned);
    eprintln!("placement request {id}");
    let outcome = (|| {
        let request = if resume.is_some() {
            work::resume_placement(fleet.ledger, &id)?
        } else {
            request_with_id(fleet, args, Action::Place, id.clone())?
        };
        let mut result = start(fleet, request, args.wait)?;
        if result["queued"] == true && args.wait {
            crate::print(true, &result, String::new)?;
            result = wait_ticket(fleet, result["ticket"].as_str().unwrap_or_default())?;
        }
        Ok::<_, Error>(result)
    })();
    match outcome {
        Ok(result) => {
            let pending = result["queued"] == true || result["status"] == "pending_release";
            crate::print(true, &result, String::new)?;
            Ok(ExitCode::from(u8::from(pending)))
        }
        Err(error) if args.wait && matches!(error.root(), Error::Deadline(_)) => {
            if json_output {
                crate::print(
                    true,
                    &json!({"request_id":id,"status":"waiting",
                    "deadline":true,"error":error.to_string()}),
                    String::new,
                )?;
            } else {
                println!("still running");
            }
            Ok(ExitCode::from(124))
        }
        Err(error) => {
            if json_output {
                let mut result = json!({"request_id":id,"error":error.to_string(),
                    "deadline":matches!(error.root(), Error::Deadline(_))});
                if let Some(diagnostic) = error.diagnostic() {
                    result["diagnostic"] = json!(diagnostic);
                }
                if let Error::Refused { code, .. } = error.root() {
                    result["code"] = json!(code.code());
                }
                crate::print(true, &result, String::new)?;
            } else {
                eprintln!("placement request {id}: {error}");
            }
            Ok(error.exit_code())
        }
    }
}

pub fn dispatch(fleet: &Fleet, command: Command, json_output: bool) -> Result<ExitCode> {
    match command {
        Command::Place {
            resume,
            host,
            branch,
            base,
            repo,
            wait,
            max,
        } => place(
            fleet,
            resume.as_deref(),
            &WorkArgs {
                host,
                branch,
                base,
                cwd: repo,
                wait,
                max,
            },
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
                validity: None,
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
        Command::Machine(command) => {
            machine(fleet.shell, command, json_output)?;
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
    if let Some(reason) = read["close_blocker"].as_str() {
        out.push_str(reason);
        out.push('\n');
    }
    if let Some(id) = read["recovery"]["id"].as_str() {
        out.push_str("saved (recovery ");
        out.push_str(id);
        out.push_str(")\n");
    }
    if let Some(reply) = read["orphan_reply"].as_str() {
        out.push_str("--- stopped orphan reply (not task completion) ---\n");
        out.push_str(reply);
        out.push('\n');
    }
    out.push_str(&text("tail"));
    out.push('\n');
    out
}

/// The host of the open item for `pane`. Pane ids repeat across hosts, so more than one is ambiguous.
fn close_host<'a>(fleet: &'a Fleet, pane: &str) -> Result<&'a crate::hosts::Host> {
    let hosts: std::collections::BTreeSet<_> = items::list(fleet.ledger)?
        .into_iter()
        .filter(|i| i.pane == pane)
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

/// Stream job output, translating remote retention only for an owner-held job.
fn logs(fleet: &Fleet, id: &str, follow: bool) -> Result<()> {
    let host = work::job_host(fleet, id, now_ms())?;
    let owned_remote_job = !host.is_local()
        && items::list(fleet.ledger)?
            .iter()
            .any(|item| item.kind == "job" && item.id == id && item.host == host.id);
    let mut offset = 0;
    loop {
        let chunk = if host.is_local() {
            jobs::logs(fleet.ledger, id, offset)?
        } else {
            match fleet.remote(
                host,
                &[
                    "logs".into(),
                    id.to_owned(),
                    "--offset".into(),
                    offset.to_string(),
                ],
            ) {
                Err(error)
                    if owned_remote_job
                        && matches!(
                            error.root(),
                            Error::Ssh {
                                refusal: Some(Refusal::UnknownJob),
                                ..
                            }
                        ) =>
                {
                    jobs::expired_chunk(offset)
                }
                result => result?,
            }
        };
        if chunk.expired {
            writeln!(std::io::stdout(), "{}", jobs::OUTPUT_EXPIRED)?;
            std::io::stdout().flush()?;
            break;
        }
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
    Ok(())
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
        Command::Cancel { ticket } => {
            let result = work::cancel(fleet.ledger, &ticket)?;
            crate::print(json_output, &result, || {
                if result["status"] == "too_late" {
                    format!("Too late: ticket {ticket} already admitted; result preserved\n")
                } else {
                    format!(
                        "Cancelled future admission for ticket {ticket}; execution outcome: {}\n",
                        result["outcome"]
                    )
                }
            })?;
            if result["status"] == "too_late" {
                return Ok(ExitCode::from(2));
            }
        }
        Command::Retry { host } => {
            let host = fleet.find(&host)?;
            fleet.retry_now(host)?;
            let report = fleet
                .status(now_ms())?
                .into_iter()
                .find(|r| r.host.id == host.id);
            crate::print(json_output, &report, || {
                report.as_ref().map_or_else(String::new, |r| {
                    format!(
                        "{}: {}\n",
                        r.host.label,
                        if r.reachable {
                            "reachable".to_owned()
                        } else {
                            r.error.clone().unwrap_or_else(|| "unreachable".into())
                        }
                    )
                })
            })?;
        }
        Command::Stop { id } => stop(fleet, &id)?,
        Command::Wait { id, .. } => return Ok(ExitCode::from(wait_job(fleet, &id)? as u8)),
        Command::Logs { id, follow } => logs(fleet, &id, follow)?,
        _ => return Err(Error::Invalid("unexpected command dispatcher".into())),
    }
    Ok(ExitCode::SUCCESS)
}

pub fn exec(
    ledger: &crate::ledger::Ledger,
    shell: &dyn crate::shell::Shell,
    id: &str,
    cmd: &[String],
    timeout_ms: Option<u64>,
    idle_timeout_ms: Option<u64>,
    job_validity: Option<u32>,
) -> Result<ExitCode> {
    jobs::validate_id(id)?;
    // The wrapper names the protocol it speaks. A wrapper that speaks another version
    // never starts new execution.
    if let Some(version) = job_validity
        && version != jobs::VALIDITY_PROTOCOL_VERSION
    {
        return Err(Error::Refused {
            code: crate::error::Refusal::JobValidityUnsupported,
            detail: crate::error::Refusal::JobValidityUnsupported
                .message()
                .to_owned(),
        });
    }
    // A job runs only from its claim. A missing record is refused here, and so is an
    // unreadable one: no process, no log, and no pane exist for a job nobody claimed.
    let job = jobs::record(ledger, id)?
        .ok_or_else(|| Error::Invalid(format!("job {id} has no claim to run")))?;
    // The requested id names the record. A record that embeds another id is not this job's
    // claim, and it never reaches the execution lock.
    if job.id != id {
        return Err(Error::Invalid(format!(
            "job {id} claim belongs to job {}",
            job.id
        )));
    }
    if job.cmd != cmd {
        return Err(Error::Invalid(
            "exec command differs from the launch record".into(),
        ));
    }
    // The claim carries the job's options. A request that names different ones asks for a
    // different job, and is refused.
    for (requested, claimed) in [
        (timeout_ms, job.timeout_ms),
        (idle_timeout_ms, job.idle_timeout_ms),
    ] {
        if requested.is_some() && requested != claimed {
            return Err(Error::Invalid(
                "exec options differ from the launch record".into(),
            ));
        }
    }
    Ok(ExitCode::from(jobs::exec(ledger, shell, id, &job)? as u8))
}

fn machine(
    shell: &dyn crate::shell::Shell,
    command: crate::cli::MachineCommand,
    json_output: bool,
) -> Result<()> {
    use crate::cli::MachineCommand as M;
    use crate::machines;
    let line = |m: &machines::Machine| {
        format!(
            "{} {} {}{}\n",
            m.label,
            m.target,
            m.id,
            if m.enabled { "" } else { " (disabled)" }
        )
    };
    match command {
        M::List => {
            let all = machines::list(shell)?;
            crate::print(json_output, &all, || all.iter().map(line).collect())
        }
        M::Rename { machine, label } => {
            let m = machines::rename(shell, &machine, &label)?;
            crate::print(json_output, &m, || line(&m))
        }
        M::Enable { machine } => {
            let m = machines::set_enabled(shell, &machine, true)?;
            crate::print(json_output, &m, || line(&m))
        }
        M::Disable { machine } => {
            let m = machines::set_enabled(shell, &machine, false)?;
            crate::print(json_output, &m, || line(&m))
        }
        M::Reconnect { machine } => {
            let pane = machines::reconnect(shell, &machine)?;
            let value = serde_json::json!({ "pane": pane });
            crate::print(json_output, &value, || {
                format!("Reconnecting {machine} in {pane}; answer its prompts there.\n")
            })
        }
        M::Add { target, label } => {
            let pane = machines::add(shell, &target, label.as_deref())?;
            let value = serde_json::json!({ "pane": pane });
            crate::print(json_output, &value, || {
                format!("Adding {target} in {pane}; answer its prompts there.\n")
            })
        }
    }
}
