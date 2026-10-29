//! Host status, local service, and capacity reservations for Herdr agents.

mod activity;
mod backoff;
mod board;
mod budget;
mod checkout;
mod claim;
mod cleanup;
mod cli;
mod close;
mod commands;
mod config;
mod context;
mod continuation;
mod custody;
mod diagnostic;
mod done;
mod drop;
mod error;
mod facts;
mod gate;
mod herdr_socket;
mod hosts;
mod inbox;
mod integration;
mod items;
mod jobs;
mod launch;
mod ledger;
mod lifecycle;
mod loss;
#[cfg(test)]
mod loss_tests;
mod machines;
mod mcp;
mod node;
mod plan;
mod preparation_retirement;
mod prepare;
#[cfg_attr(not(test), expect(dead_code, reason = "wired by P2/P3"))]
mod preservation;
mod probe;
mod profiles;
mod quota;
mod repository;
mod runtime;
mod service;
mod shell;
mod sidebar;
mod sweep;
mod transcript;
mod unit_reply;
mod unit_request;
mod unit_transport;
mod watchdog;
mod web;
mod work;
#[cfg(test)]
mod work_tests;
mod worktree;

use clap::Parser;
use cli::{Cli, Command, HookCommand, HostCommand, InboxCommand};
use error::{Error, Result};
use ledger::{Ledger, now_ms};
use node::Node;
use serde::Serialize;
use shell::System;
use std::fmt::Write as _;
use std::io::Write as _;
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.json || matches!(cli.command, Command::Unit(_));
    let hook = matches!(cli.command, Command::Hook(_));
    let peer = matches!(cli.command, Command::Host(_));
    if hook && std::env::var_os("HERDR_ENV").is_none() {
        return ExitCode::SUCCESS;
    }
    let started = std::time::Instant::now();
    let (max, waiting) = match &cli.command {
        Command::Wait { max, .. }
        | Command::Place {
            max, wait: true, ..
        } => (Some(*max), true),
        Command::Place { max, .. } => (Some(*max), false),
        Command::Run { placement, .. } | Command::Agent { placement, .. } => {
            (Some(placement.max), placement.wait)
        }
        _ => (None, false),
    };
    let max = cli.budget_ms.or(max);
    let max = if hook {
        Some(max.unwrap_or(5_000).min(5_000))
    } else {
        max
    };
    let budget = max.map_or_else(budget::Budget::default, |ms| {
        budget::Budget::until(started + std::time::Duration::from_millis(ms))
    });
    match run_recovery_or_command(cli, budget) {
        Ok(code) => code,
        // Prompt context is optional: unavailable state must not block a turn.
        Err(_) if hook => ExitCode::SUCCESS,
        Err(ref error) if waiting && matches!(error.root(), Error::Deadline(_)) => {
            println!("still running");
            ExitCode::from(124)
        }
        Err(error) => {
            if json {
                let mut body = serde_json::json!({ "error": error.to_string(), "deadline": matches!(error.root(), Error::Deadline(_)) });
                if let Error::Refused { code, .. } = error.root() {
                    body["code"] = code.code().into();
                }
                if let Some(diagnostic) = error.diagnostic() {
                    body["diagnostic"] = serde_json::json!(diagnostic);
                    if peer {
                        body["error"] = diagnostic.to_string().into();
                    }
                }
                println!("{body}");
            } else {
                eprintln!("dogsled: {error}");
            }
            error.exit_code()
        }
    }
}

fn print<T: Serialize>(json: bool, value: &T, text: impl FnOnce() -> String) -> Result<()> {
    let mut out = std::io::stdout().lock();
    if json {
        serde_json::to_writer(&mut out, value)?;
        writeln!(out)?;
    } else {
        write!(out, "{}", text())?;
    }
    out.flush()?;
    Ok(())
}

/// `dogsled done`, for the agent in `HERDR_PANE_ID`.
fn done_command(ledger: &Ledger, shell: &System, json: bool) -> Result<()> {
    let mark = done::mark(ledger, shell, &inbox::own_pane(None)?, now_ms())?;
    print(json, &mark, || {
        format!(
            "{} is done once this turn ends; dogsled closes it when its work has landed\n",
            mark.pane
        )
    })
}

/// Turn-ending command receipts share the local agent identity and output boundary.
fn turn_command(ledger: &Ledger, shell: &System, json: bool, command: Command) -> Result<()> {
    let (kind, text) = match command {
        Command::Done => return done_command(ledger, shell, json),
        Command::Ask { question } => (lifecycle::Signal::Ask, question),
        Command::Blocked { reason } => (lifecycle::Signal::Blocked, reason),
        _ => return Err(Error::Invalid("not a turn signal command".into())),
    };
    let signal = done::signal(
        ledger,
        shell,
        &inbox::own_pane(None)?,
        kind,
        &text,
        now_ms(),
    )?;
    print(json, &signal, || format!("{}\n", signal.event_id))
}

fn run(cli: Cli, budget: budget::Budget) -> Result<ExitCode> {
    let policy = config::load(&cli.config.unwrap_or_else(config::default_path))?;
    let shell = System { budget };
    let mut ledger = Ledger::new(ledger::default_dir());
    ledger.budget = budget;
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    let node = Node {
        shell: &shell,
        ledger: &ledger,
        thresholds: config::Thresholds::from(&policy),
        home: &home,
        cleanup: &policy.cleanup,
    };
    match cli.command {
        Command::Quota(args) => quota_command(&shell, &ledger, &policy, &home, &args, cli.json)?,
        Command::Serve { interval, once } => service::serve(
            &node,
            &policy,
            std::time::Duration::from_secs(interval),
            once,
        )?,
        Command::Host(cmd) => return host(cmd, &node),
        Command::Exec {
            id,
            cmd,
            timeout,
            idle_timeout,
            job_validity,
        } => {
            return runtime::exec(
                &ledger,
                &shell,
                &id,
                &cmd,
                timeout,
                idle_timeout,
                job_validity,
            );
        }
        Command::Status {
            watch,
            interval,
            line,
            brief,
        } => {
            let ask = Watch {
                watch,
                interval,
                line,
                brief,
            };
            status_command(&shell, &ledger, &policy, &home, ask, cli.json)?;
        }
        command @ Command::Board { .. } => {
            board::command(&shell, &ledger, &policy, home, command, cli.json)?;
        }
        Command::Inbox(cmd) => inbox_command(cmd, &ledger, &shell, cli.json)?,
        Command::Sidebar { owner } => sidebar::command(shell, &ledger, &policy, &home, owner)?,
        Command::Web { port } => web::serve(&ledger, &policy, &home, port)?,
        command @ (Command::Interview { .. } | Command::Review { .. }) => {
            return web::oneshot::command(&shell, &ledger, command);
        }
        Command::Unit(command) => {
            let fleet = hosts::Fleet::discover(&shell, &ledger, &policy, home)?;
            return unit_request::run(&fleet, command);
        }
        Command::Mcp => mcp::serve(policy, ledger, home)?,

        Command::Drop { id, host } => {
            let fleet = hosts::Fleet::discover(&shell, &ledger, &policy, home)?;
            let dropped = drop::drop(&fleet, &id, host.as_deref())?;
            print(cli.json, &dropped, || dropped.line())?;
        }
        command @ (Command::Done | Command::Ask { .. } | Command::Blocked { .. }) => {
            turn_command(&ledger, &shell, cli.json, command)?;
        }
        Command::Send {
            target,
            host,
            message_file,
            message,
            kind,
        } => {
            let ask = Ask::new(target, host, kind, message, message_file);
            send_message(&shell, &ledger, &policy, &home, ask, cli.json)?;
        }
        Command::LegacyContinue {
            pane,
            message_file,
            message,
            kind,
        } => {
            let ask = Ask::new(pane, None, kind, message, message_file);
            legacy_continue(&ledger, &shell, ask, cli.json)?;
        }

        Command::Hook(cmd) => hook(cmd, &ledger, &shell, &policy, home)?,
        command => {
            let fleet = hosts::Fleet::discover(&shell, &ledger, &policy, home)?;
            return dispatch(&fleet, command, cli.json);
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Route commands after fleet discovery, keeping their adapters with their owners.
fn dispatch(fleet: &hosts::Fleet<'_>, command: Command, json: bool) -> Result<ExitCode> {
    match command {
        command @ (Command::Claim { .. } | Command::Accept { .. }) => {
            claim::command(fleet, command, json)
        }
        command => runtime::dispatch(fleet, command, json),
    }
}

/// The shape of the status command: what to print, and whether to keep printing it.
#[derive(Clone, Copy)]
struct Watch {
    watch: bool,
    interval: u64,
    line: bool,
    brief: bool,
}

/// The status command: the fleet's state, printed once or watched.
fn status_command(
    shell: &System,
    ledger: &Ledger,
    policy: &config::Policy,
    home: &str,
    ask: Watch,
    json: bool,
) -> Result<()> {
    loop {
        let fleet = hosts::Fleet::discover(shell, ledger, policy, home.to_owned())?;
        let status = commands::status(&fleet)?;
        if ask.watch && !json && !ask.line && !ask.brief {
            print!("\x1b[2J\x1b[H");
        }
        print(json, &status, || {
            if ask.line {
                commands::line(&status)
            } else if ask.brief {
                commands::brief(&status)
            } else {
                commands::render(&status)
            }
        })?;
        if !ask.watch {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(ask.interval));
    }
    Ok(())
}

/// The message a send command carries.
struct Ask {
    target: String,
    host: Option<String>,
    kind: String,
    message: Option<String>,
    message_file: Option<std::path::PathBuf>,
}

impl Ask {
    fn new(
        target: String,
        host: Option<String>,
        kind: String,
        message: Option<String>,
        message_file: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            target,
            host,
            kind,
            message,
            message_file,
        }
    }
}

/// The send command: it hands a message to an agent, on any host, through its inbox.
fn send_message(
    shell: &System,
    ledger: &Ledger,
    policy: &config::Policy,
    home: &str,
    ask: Ask,
    json: bool,
) -> Result<()> {
    let text = inbox::message_text(ask.message, ask.message_file)?;
    let fleet = hosts::Fleet::discover(shell, ledger, policy, home.to_owned())?;
    let sent = inbox::send_to(
        &fleet,
        &ask.target,
        ask.host.as_deref(),
        &ask.kind,
        text,
        now_ms(),
    )?;
    print(json, &sent, || sent.line())?;
    Ok(())
}

/// The legacy continue command: it hands a message to a pane that never answered.
fn legacy_continue(ledger: &Ledger, shell: &System, ask: Ask, json: bool) -> Result<()> {
    let text = inbox::message_text(ask.message, ask.message_file)?;
    let sent = continuation::legacy(ledger, shell, &ask.target, &ask.kind, &text, now_ms())?;
    print(json, &sent, || sent.line())?;
    Ok(())
}

/// The quota command: this host's share of the fleet's capacity.
fn quota_command(
    shell: &System,
    ledger: &Ledger,
    policy: &config::Policy,
    home: &str,
    args: &cli::QuotaArgs,
    json: bool,
) -> Result<()> {
    let snapshot = quota::command(shell, ledger, policy, home, args)?;
    print(json, &snapshot, || snapshot.render(now_ms()))?;
    Ok(())
}

/// The validity evidence a peer sends with a job reservation. A peer that names another
/// protocol version, or names a time without the protocol, is refused here.
fn reservation_validity(
    job_validity: Option<u32>,
    job_created_ms: Option<u64>,
) -> Result<Option<crate::jobs::Validity>> {
    let refuse = |code: crate::error::Refusal| {
        Err(Error::Refused {
            code,
            detail: code.message().to_owned(),
        })
    };
    match (job_validity, job_created_ms) {
        (Some(version), Some(created_ms)) => {
            if version != crate::jobs::VALIDITY_PROTOCOL_VERSION {
                return refuse(crate::error::Refusal::JobValidityUnsupported);
            }
            Ok(Some(crate::jobs::Validity::new(created_ms)))
        }
        (Some(_), None) | (None, Some(_)) => refuse(crate::error::Refusal::JobValidityUnprovable),
        (None, None) => Ok(None),
    }
}

/// The harness hooks: prompt context, and the inbox for Claude Code and Codex.
fn hook(
    cmd: HookCommand,
    ledger: &Ledger,
    shell: &System,
    policy: &config::Policy,
    home: String,
) -> Result<()> {
    match cmd {
        HookCommand::Inbox { harness, event } => {
            // Keep inbox delivery working even when fleet status is unavailable.
            let board = if event == inbox::HookEvent::Stop {
                sweep::request_at_stop(ledger);
                owner_board(ledger, shell, policy, home).ok()
            } else {
                None
            };
            inbox::hook(
                ledger,
                shell,
                harness.name(),
                event,
                now_ms(),
                board.as_ref(),
            )
        }
        HookCommand::Gate => {
            let owner = inbox::own_pane(None)?;
            sweep::request_at_stop(ledger);
            let notices = gate::notices(ledger, shell, &owner)?;
            let board = owner_board(ledger, shell, policy, home)?;
            gate::deliver(ledger, &board, &owner, &notices, |reminder| {
                print(false, &(), || reminder.unwrap_or_default().to_owned())
            })
        }
        HookCommand::Context { harness, event } => {
            let fleet = hosts::Fleet::discover(shell, ledger, policy, home)?;
            let status = commands::status(&fleet)?;
            let owner = std::env::var("HERDR_PANE_ID").ok();
            let found = match (&owner, &status.sweep) {
                (Some(owner), Some(report)) => gate::findings(ledger, shell, owner, report)?,
                _ => gate::Findings::default(),
            };
            print(false, &(), || {
                context::format(harness, event, &status, owner.as_deref(), &found)
            })
        }
    }
}

/// Recovery needs no Herdr discovery for local operations.
fn run_recovery_or_command(cli: Cli, budget: budget::Budget) -> Result<ExitCode> {
    let Command::Recovery {
        ref command,
        ref host,
    } = cli.command
    else {
        return run(cli, budget);
    };
    let policy = config::load(&cli.config.unwrap_or_else(config::default_path))?;
    let shell = System { budget };
    let ledger = Ledger {
        budget,
        ..Ledger::new(ledger::default_dir())
    };
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    let value = runtime::recovery(&shell, &ledger, &policy, &home, host.as_deref(), command)?;
    print(cli.json, &value, || format!("{value}\n"))?;
    Ok(ExitCode::SUCCESS)
}

fn owner_board(
    ledger: &Ledger,
    shell: &System,
    policy: &config::Policy,
    home: String,
) -> Result<board::Board> {
    let owner = inbox::own_pane(None)?;
    let fleet = hosts::Fleet::discover(shell, ledger, policy, home)?;
    Ok(board::build(
        &commands::status(&fleet)?,
        Some(&owner),
        now_ms(),
    ))
}

fn inbox_command(cmd: InboxCommand, ledger: &Ledger, shell: &System, json: bool) -> Result<()> {
    let now = now_ms();
    match cmd {
        InboxCommand::Claim { harness, pane } => {
            let pane = inbox::own_pane(pane)?;
            let records = inbox::claim_for(ledger, shell, &pane, &harness, now)?;
            let claimed: Vec<inbox::Claimed> = records.iter().map(inbox::Claimed::from).collect();
            print(json, &claimed, || {
                records.iter().fold(String::new(), |mut out, r| {
                    let _ = write!(out, "{}\n{}\n\n", r.id, r.render());
                    out
                })
            })
        }
        InboxCommand::Ack { ids, pane } => {
            let pane = inbox::own_pane(pane)?;
            let acked = inbox::ack_for(ledger, shell, &pane, &ids, now)?;
            print(json, &serde_json::json!({ "acked": acked }), || {
                format!("Acknowledged {acked}.\n")
            })
        }
        InboxCommand::List { pane } => {
            let pane = inbox::own_pane(pane)?;
            let listing = inbox::list_for(ledger, shell, &pane, now)?;
            print(json, &listing, || listing.render())
        }
        InboxCommand::Path { pane } => {
            let pane = inbox::own_pane(pane)?;
            let (dir, file) = inbox::path_for(ledger, shell, &pane)?;
            let value = serde_json::json!({ "dir": dir, "file": file });
            print(json, &value, || {
                format!("{}\n", std::path::Path::new(&dir).join(&file).display())
            })
        }
    }
}

/// The host commands that act on one child pane.
fn child(cmd: HostCommand, node: &Node<'_>) -> Result<()> {
    match cmd {
        HostCommand::ReleaseReferences { input } => print(
            true,
            &node.release_references(&serde_json::from_str(&input)?)?,
            String::new,
        )?,
        HostCommand::ReferenceReleaseStatus { input } => print(
            true,
            &launch::reference_release_status(
                node.ledger,
                &serde_json::from_str::<unit_transport::ExecutionInput>(&input)?.execution,
            )?,
            String::new,
        )?,
        HostCommand::UnitReceipt { input } => print(
            true,
            &launch::unit_receipt(
                node.ledger,
                node.shell,
                &serde_json::from_str::<unit_transport::ExecutionInput>(&input)?.execution,
            )?,
            String::new,
        )?,
        HostCommand::UnitReceipts { input } => print(
            true,
            &launch::unit_receipts(
                node.ledger,
                node.shell,
                &serde_json::from_str::<launch::ExecutionsInput>(&input)?.executions,
            ),
            String::new,
        )?,
        HostCommand::Reply { pane } => {
            print(
                true,
                &sweep::local_reply(node.shell, node.ledger, &pane)?,
                String::new,
            )?;
        }
        // Over SSH the caller has no pane or workspace on this host.
        HostCommand::Close {
            pane,
            pushed,
            stopped_orphan,
            landed,
            writer,
        } => {
            let closed = if let Some(episode) = stopped_orphan {
                close::stopped_orphan(
                    node.shell,
                    node.ledger,
                    &pane,
                    &close::Caller::default(),
                    &serde_json::from_str(&episode)?,
                )?
            } else {
                close::local(
                    node.shell,
                    node.ledger,
                    &pane,
                    &close::Caller::default(),
                    close::Guard::from_flags(pushed, landed, writer),
                )?
            };
            print(true, &closed, String::new)?;
        }
        HostCommand::Parent { pane } => {
            watchdog::register_parent(node.ledger, node.shell, &pane)?;
            print(true, &true, String::new)?;
        }
        HostCommand::Drop { item } => print(
            true,
            &drop::local(node.shell, node.ledger, &serde_json::from_str(&item)?)?,
            String::new,
        )?,
        HostCommand::RefreshBranches { items } => print(
            true,
            &drop::refresh(node.shell, serde_json::from_str(&items)?)?,
            String::new,
        )?,
        _ => unreachable!("only child commands"),
    }
    Ok(())
}

/// Decode and validate the launch wire before ingress can publish any evidence.
fn print_host_launch(
    node: &Node<'_>,
    request: &str,
    validity: Option<u32>,
    preparation_protocol: Option<u32>,
) -> Result<()> {
    prepare::require_protocol(preparation_protocol)?;
    let launch: work::Launch = serde_json::from_str(request)?;
    let result = work::receive_launch(node.ledger, node.shell, &launch, now_ms(), validity)?;
    print(
        true,
        &prepare::launch_response(node.ledger, &launch, result)?,
        String::new,
    )
}

fn print_host_reservation(
    node: &Node<'_>,
    id: &str,
    input: Option<&str>,
    validity: Option<jobs::Validity>,
) -> Result<ExitCode> {
    let out = if let Some(input) = input {
        let envelope: node::PlacementReplay = serde_json::from_str(input)?;
        envelope.validate(id)?;
        node.reserve_placement(&envelope, now_ms())?
    } else {
        node.reserve(id, now_ms(), validity)?
    };
    print(true, &out, String::new)?;
    Ok(if out.reserved {
        ExitCode::SUCCESS
    } else {
        Error::Full.exit_code()
    })
}

fn dispose_preparation(
    node: &Node<'_>,
    request: &prepare::disposition::Request,
) -> Result<prepare::disposition::Acknowledgement> {
    work::launch_test_barrier("DISPOSITION_BEFORE_GATE")?;
    prepare::disposition::dispose(node.ledger, node.shell, request)
}

fn host_preparation(cmd: HostCommand, node: &Node<'_>) -> Result<()> {
    match cmd {
        HostCommand::PreparationLookup {
            input,
            preparation_protocol,
        } => {
            prepare::require_protocol(Some(preparation_protocol))?;
            match serde_json::from_str(&input)? {
                prepare::LookupQuery::Historical(query) => print(
                    true,
                    &prepare::historical_on_host(node.ledger, &query)?,
                    String::new,
                )?,
                prepare::LookupQuery::Placement(query) => print(
                    true,
                    &prepare::lookup_on_host(node.ledger, node.home, &query),
                    String::new,
                )?,
                prepare::LookupQuery::Preparation(query) => print(
                    true,
                    &prepare::lookup_preparation_on_host(node.ledger, node.home, &query),
                    String::new,
                )?,
            }
        }
        HostCommand::Prepared {
            input,
            preparation_protocol,
        } => {
            prepare::require_protocol(Some(preparation_protocol))?;
            let request = serde_json::from_str(&input)?;
            print(true, &dispose_preparation(node, &request)?, String::new)?;
        }
        HostCommand::Prepare {
            preparation_protocol,
            placement_replay,
            repo,
            branch,
            base,
            request_id,
        } => {
            prepare::require_protocol(preparation_protocol)?;
            let placement = placement_replay
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?;
            let prepared = prepare::on_host(
                node,
                &repo,
                &branch,
                base.as_deref(),
                request_id.as_deref(),
                placement.as_ref(),
            )?;
            print(true, &prepared, String::new)?;
        }
        _ => return Err(Error::Invalid("not a preparation command".into())),
    }
    Ok(())
}

fn host(cmd: HostCommand, node: &Node<'_>) -> Result<ExitCode> {
    match cmd {
        HostCommand::InboxPut { .. } | HostCommand::InboxState { .. } => {
            inbox::host(node.ledger, node.shell, cmd)?;
        }
        HostCommand::Launch {
            request,
            job_validity,
            preparation_protocol,
        } => print_host_launch(node, &request, job_validity, preparation_protocol)?,
        HostCommand::Job { id } => print(true, &jobs::get(node.ledger, &id)?, String::new)?,
        HostCommand::Recovery { command } => {
            print(
                true,
                &runtime::recovery_local(node.shell, node.ledger, &command)?,
                String::new,
            )?;
        }
        HostCommand::Jobs => print(true, &jobs::list(node.ledger)?, String::new)?,
        HostCommand::Logs { id, offset } => {
            print(true, &jobs::logs(node.ledger, &id, offset)?, String::new)?;
        }
        HostCommand::Stop { id } => {
            jobs::stop(node.ledger, &id)?;
            print(true, &serde_json::json!({"stopped":true}), String::new)?;
        }
        HostCommand::Checkout {
            path,
            origin,
            host_label,
        } => print(
            true,
            &checkout::local(node.shell, node.home, &host_label, &path, &origin)?,
            String::new,
        )?,
        HostCommand::Tail { kind, id } => {
            let tail = if kind == "job" {
                watchdog::job_tail(node.ledger, &id)?
            } else {
                watchdog::pane_tail(node.shell, &id)?
            };
            print(true, &tail, String::new)?;
        }
        HostCommand::Doctor => print(true, &cleanup::doctor(node, now_ms())?, String::new)?,
        HostCommand::Reply { .. }
        | HostCommand::Close { .. }
        | HostCommand::UnitReceipt { .. }
        | HostCommand::UnitReceipts { .. }
        | HostCommand::ReleaseReferences { .. }
        | HostCommand::ReferenceReleaseStatus { .. }
        | HostCommand::Drop { .. }
        | HostCommand::RefreshBranches { .. }
        | HostCommand::Parent { .. } => {
            child(cmd, node)?;
        }
        HostCommand::Prune { plan } => print(
            true,
            &cleanup::apply(node, &serde_json::from_str(&plan)?, now_ms())?,
            String::new,
        )?,
        HostCommand::Snapshot => print(true, &node.snapshot()?, String::new)?,
        HostCommand::Todos => print(true, &board::cached_todos(node.ledger)?, String::new)?,
        HostCommand::QuotaStore => print(
            true,
            &quota::store(node.ledger, std::io::stdin().lock())?,
            String::new,
        )?,
        HostCommand::Reserve {
            id,
            placement_replay,
            preparation_protocol,
            thresholds,
            job_validity,
            job_created_ms,
        } => {
            prepare::require_protocol(preparation_protocol)?;
            let node = Node {
                thresholds: thresholds.over(node.thresholds),
                ..*node
            };
            let validity = reservation_validity(job_validity, job_created_ms)?;
            return print_host_reservation(&node, &id, placement_replay.as_deref(), validity);
        }
        cmd @ (HostCommand::PreparationLookup { .. }
        | HostCommand::Prepared { .. }
        | HostCommand::Prepare { .. }) => host_preparation(cmd, node)?,
    }
    Ok(ExitCode::SUCCESS)
}
