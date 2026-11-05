//! Host status, local service, and capacity reservations for Herdr agents.

mod activity;
mod admissions;
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
mod holds;
mod host_store;
mod hosts;
mod inbox;
mod integration;
mod items;
mod job_store;
mod jobs;
mod launch;
mod launch_store;
mod lead_store;
#[cfg_attr(not(test), expect(dead_code, reason = "wired by lead lifetime in L2"))]
mod leads;
mod ledger;
mod lifecycle;
mod loss;
#[cfg(test)]
mod loss_tests;
mod machines;
mod mcp;
mod node;
mod observation;
mod plan;
mod preparation_retirement;
mod preparation_store;
mod prepare;
#[cfg_attr(not(test), expect(dead_code, reason = "wired by P2/P3"))]
mod preservation;
mod probe;
mod profiles;
mod quota;
mod repos;
mod repository;
mod rows;
mod runtime;
mod service;
mod shell;
mod sidebar;
mod store;
mod sweep;
mod tables;
mod transcript;
mod unit_reply;
mod unit_request;
mod unit_transport;
mod watchdog;
mod web;
mod web_store;
mod work;
#[cfg(test)]
mod work_tests;
mod worktree;

use clap::Parser;
use cli::{Cli, Command, HookCommand, HostCommand, InboxCommand, StateCommand};
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
                let diagnostic = if peer {
                    let args: Vec<String> = std::env::args()
                        .skip(1)
                        .skip_while(|arg| arg != "host")
                        .skip(1)
                        .collect();
                    let diagnostic = diagnostic::Diagnostic::for_error(&error, &args);
                    (diagnostic.operation != diagnostic::Operation::Other).then_some(diagnostic)
                } else {
                    error.diagnostic().cloned()
                };
                if let Some(diagnostic) = diagnostic {
                    body["diagnostic"] = serde_json::json!(diagnostic);
                    if peer
                        && error
                            .diagnostic()
                            .is_some_and(|d| d.operation != diagnostic::Operation::Other)
                    {
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
    let config_path = cli.config.unwrap_or_else(config::default_path);
    let policy = config::load(&config_path)?;
    let shell = System { budget };
    let mut ledger = Ledger::new(ledger::default_dir());
    ledger.budget = budget;
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    let node = Node::new(&shell, &ledger, &policy, &home);
    let fleet = hosts::Fleet::cold(
        &shell,
        &ledger,
        &policy,
        home.clone(),
        vec![hosts::Host::local()],
    );
    match cli.command {
        Command::Quota(args) => quota_command(&ledger, &args, cli.json)?,
        Command::Repo(cli::RepoCommand::List) => {
            repo_command(&observation::ipc::Client::at(&ledger.dir), cli.json)?;
        }
        Command::Lead(args) => lead_command(&node, &config_path, args, cli.json)?,
        Command::Serve { interval, once } => service::serve(
            &node,
            &policy,
            std::time::Duration::from_secs(interval),
            once,
        )?,
        Command::Host(HostCommand::Lead { .. }) => host_lead(&node, &config_path)?,
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
            let _ = interval;
            let ask = Watch { watch, line, brief };
            status_command(&observation::ipc::Client::at(&ledger.dir), ask, cli.json)?;
        }
        command @ Command::Board { .. } => {
            board::command(&shell, &ledger, &policy, home, command, cli.json)?;
        }
        Command::Inbox(cmd) => inbox_command(cmd, &ledger, &shell, cli.json)?,
        Command::Sidebar { owner } => sidebar::command(shell, &ledger, &policy, &home, owner)?,
        Command::Web { port } => web::serve(&ledger, &policy, &home, port, &config_path)?,
        command @ (Command::Interview { .. } | Command::Review { .. }) => {
            return web::oneshot::command(&shell, &ledger, command);
        }
        Command::Unit(command) => return unit_request::run(&fleet, command),
        Command::Mcp => mcp::serve(policy, ledger, home, config_path)?,

        Command::Drop { id, host } => {
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

        Command::Hook(cmd) => hook(cmd, &ledger, &shell)?,
        command => return dispatch(&fleet, command, cli.json),
    }
    Ok(ExitCode::SUCCESS)
}

/// `dogsled lead`: the shared creator, printing the lead's name and pane.
fn lead_command(
    node: &Node<'_>,
    config_path: &std::path::Path,
    args: cli::LeadArgs,
    json: bool,
) -> Result<()> {
    let prompt = inbox::message_text(args.message, args.message_file)?;
    let context = leads::CreateContext {
        shell: node.shell,
        ledger: node.ledger,
        config_path,
        home: node.home,
        herdr: None,
    };
    let started = leads::create(&context, &args.repo, &prompt)?;
    print(json, &started, || {
        format!("{}\t{}\t{}\n", started.host, started.name, started.pane)
    })
}

fn host_lead(node: &Node<'_>, config_path: &std::path::Path) -> Result<()> {
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin().lock(), &mut input)?;
    let context = leads::CreateContext {
        shell: node.shell,
        ledger: node.ledger,
        config_path,
        home: node.home,
        herdr: None,
    };
    print(true, &leads::peer(&context, &input)?, String::new)
}

fn repo_command(client: &observation::ipc::Client, json: bool) -> Result<()> {
    let list: portal_types::agents::RepoList =
        observation::read::view(&client.current()?, "repos")?;
    print(json, &list, || repos::render(&list))
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
    line: bool,
    brief: bool,
}

/// The status command: the fleet's state, printed once or watched.
fn status_command(client: &observation::ipc::Client, ask: Watch, json: bool) -> Result<()> {
    let mut subscription = ask.watch.then(|| client.subscribe()).transpose()?;
    let mut published = match &subscription {
        Some(subscription) => (*subscription.current().publication).clone(),
        None => client.current()?,
    };
    loop {
        let status: commands::Status = observation::read::view(&published, "status")?;
        if ask.watch && !json && !ask.line && !ask.brief {
            print!("\x1b[2J\x1b[H");
        }
        let text = if json {
            String::new()
        } else {
            let display: observation::display::Display =
                observation::read::view(&published, "status_display")?;
            if ask.line {
                display.line()
            } else if ask.brief {
                display.brief(now_ms())
            } else {
                display.render(now_ms())
            }
        };
        print(json, &status, || text)?;
        if !ask.watch {
            break;
        }
        let subscription = subscription
            .as_mut()
            .ok_or_else(|| Error::Invalid("observation subscription missing".into()))?;
        loop {
            let connection = subscription.changed(std::time::Duration::from_secs(3600));
            if let observation::ipc::Status::Disconnected(detail) = connection.status {
                return Err(Error::Invalid(format!(
                    "observation disconnected/stale; retained revision {}: {detail}",
                    connection.publication.revision
                )));
            }
            if connection.publication.revision != published.revision {
                published = (*connection.publication).clone();
                break;
            }
        }
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
    let fleet = hosts::Fleet::cold(
        shell,
        ledger,
        policy,
        home.to_owned(),
        vec![crate::hosts::Host::local()],
    );
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
fn quota_command(ledger: &Ledger, args: &cli::QuotaArgs, json: bool) -> Result<()> {
    let snapshot = quota::command(ledger, args)?;
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
fn hook(cmd: HookCommand, ledger: &Ledger, shell: &System) -> Result<()> {
    let client = observation::ipc::Client::at(&ledger.dir);
    match cmd {
        HookCommand::Inbox { harness, event } => {
            // Keep inbox delivery working even when fleet status is unavailable.
            let published = if event == inbox::HookEvent::Stop {
                sweep::request_at_stop(ledger);
                client.current().ok()
            } else {
                None
            };
            inbox::hook(
                ledger,
                shell,
                harness.name(),
                event,
                now_ms(),
                published.as_ref(),
            )
        }
        HookCommand::Gate => {
            let owner = inbox::own_pane(None)?;
            sweep::request_at_stop(ledger);
            let published = client.current()?;
            let notices = gate::notices(ledger, &published, &owner)?;
            let board = observation::read::board(&published, Some(&owner))?;
            gate::deliver(ledger, &board, &owner, &notices, |reminder| {
                print(false, &(), || reminder.unwrap_or_default().to_owned())
            })
        }
        HookCommand::Context { harness, event } => {
            let published = client.current()?;
            let owner = std::env::var("HERDR_PANE_ID").ok();
            let text = context::format(harness, event, &published, owner.as_deref())?;
            print(false, &(), || text)
        }
    }
}

/// Recovery needs no Herdr discovery for local operations.
/// Commands that only read the local observer's publication. `None` routes onward.
fn published_command(client: &observation::ipc::Client, cli: &Cli) -> Result<Option<ExitCode>> {
    match &cli.command {
        Command::Repo(cli::RepoCommand::List) => {
            repo_command(client, cli.json)?;
            return Ok(Some(ExitCode::SUCCESS));
        }
        Command::Status {
            watch, line, brief, ..
        } => {
            status_command(
                client,
                Watch {
                    watch: *watch,
                    line: *line,
                    brief: *brief,
                },
                cli.json,
            )?;
            return Ok(Some(ExitCode::SUCCESS));
        }
        Command::Board {
            owner,
            all,
            open: false,
            toggle: false,
            watch,
        } => {
            let owner = board::owner(owner.clone(), *all)?;
            if *watch {
                let owner =
                    owner.ok_or_else(|| Error::Invalid("board --watch needs an owner".into()))?;
                board::watch(client, &owner, cli.json)?;
                return Ok(Some(ExitCode::SUCCESS));
            }
            let board = observation::read::board(&client.current()?, owner.as_deref())?;
            print(cli.json, &board, || board.render())?;
            return Ok(Some(ExitCode::SUCCESS));
        }
        Command::Host(HostCommand::AgentDetail { pane, input }) => {
            let reply =
                web::agents::peer_command(pane.clone(), input, Ledger::new(ledger::default_dir()))?;
            print(true, &reply, String::new)?;
            return Ok(Some(ExitCode::SUCCESS));
        }
        Command::Host(HostCommand::Snapshot) => {
            let report: node::SnapshotReport =
                observation::read::local(&client.current()?, "host")?;
            print(true, &report, String::new)?;
            return Ok(Some(ExitCode::SUCCESS));
        }
        Command::Host(HostCommand::Todos) => {
            let todos: Vec<board::TodoSections> =
                observation::read::view(&client.current()?, "todos")?;
            print(true, &todos, String::new)?;
            return Ok(Some(ExitCode::SUCCESS));
        }
        _ => {}
    }
    Ok(None)
}

fn run_recovery_or_command(cli: Cli, budget: budget::Budget) -> Result<ExitCode> {
    // Observation readers do not load policy or discover a fleet.
    let client = observation::ipc::Client::at(&ledger::default_dir());
    if let Command::Hook(cmd) = cli.command {
        let mut ledger = Ledger::new(ledger::default_dir());
        ledger.budget = budget;
        hook(cmd, &ledger, &System { budget })?;
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(code) = published_command(&client, &cli)? {
        return Ok(code);
    }
    if let Command::Items { .. } | Command::Ps { .. } = cli.command {
        return runtime::published_inventory(&client.current()?, cli.command, cli.json);
    }
    if let Command::Observation { command } = cli.command {
        return observation::command(&command);
    }
    if let Command::TodoPublish { ref pane, clear } = cli.command {
        let ledger = Ledger {
            budget,
            ..Ledger::new(ledger::default_dir())
        };
        board::publish_todo(&ledger, pane, clear)?;
        return Ok(ExitCode::SUCCESS);
    }
    if let Command::State(cmd) = cli.command {
        let ledger = Ledger {
            budget,
            ..Ledger::new(ledger::default_dir())
        };
        state(cmd, &ledger)?;
        return Ok(ExitCode::SUCCESS);
    }
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
fn release_host_references(node: &Node<'_>, input: &str) -> Result<()> {
    let value: serde_json::Value = serde_json::from_str(input)?;
    if value["version"] == 2 {
        print(
            true,
            &launch::release_ordinary(node.ledger, &serde_json::from_value(value)?)?,
            String::new,
        )
    } else {
        print(
            true,
            &node.release_references(&serde_json::from_value(value)?)?,
            String::new,
        )
    }
}

fn child(cmd: HostCommand, node: &Node<'_>) -> Result<()> {
    match cmd {
        HostCommand::ReleaseReferences { input } => release_host_references(node, &input)?,
        HostCommand::ExecutionEvidence { id } => print(
            true,
            &launch::ordinary_evidence(node.ledger, &id)?,
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
            preservation_request,
            landed,
            writer,
        } => {
            let episode = stopped_orphan
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?;
            let request = preservation_request
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?;
            let mode = episode.as_ref().map_or_else(
                || close::Guard::from_flags(pushed, landed, writer),
                |episode| close::Guard::StoppedOrphan { episode },
            );
            let closed = close::with_request(
                node.shell,
                node.ledger,
                &pane,
                &close::Caller::default(),
                close::Options {
                    guard: mode,
                    request: request.as_ref(),
                },
                |closed| {
                    items::saved_close(node.ledger, hosts::LOCAL, &closed)?;
                    Ok(closed)
                },
            )?;
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
            &observation::ipc::Client::at(&node.ledger.dir).peer(observation::peers::Request {
                interests: serde_json::from_str(&items)?,
                ..observation::peers::Request::default()
            })?,
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
    request_validity: Option<u32>,
) -> Result<()> {
    prepare::require_protocol(preparation_protocol)?;
    let launch: work::Launch = serde_json::from_str(request)?;
    let protocol = if matches!(launch.action, work::Action::Job { .. }) {
        validity
    } else {
        request_validity
    };
    let result = work::receive_launch(node.ledger, node.shell, &launch, now_ms(), protocol)?;
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
    (owner, owner_host): (&str, &str),
) -> Result<ExitCode> {
    let out = if let Some(input) = input {
        let envelope: node::PlacementReplay = serde_json::from_str(input)?;
        envelope.validate(id)?;
        node.reserve_placement(&envelope, now_ms())?
    } else {
        node.reserve(id, now_ms(), validity, owner, owner_host)?
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
            preparation_envelope_version,
            request_created_ms,
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
            let envelope = prepare::Envelope {
                version: preparation_envelope_version.unwrap_or_default(),
                validity: request_created_ms.map(jobs::Validity::new),
                request_id: request_id.ok_or_else(|| {
                    Error::Invalid(
                        "request validity is unprovable; requestless preparation refused".into(),
                    )
                })?,
                repo,
                branch,
                base,
            };
            let prepared = prepare::on_host(node, &envelope, placement.as_ref())?;
            print(true, &prepared, String::new)?;
        }
        _ => return Err(Error::Invalid("not a preparation command".into())),
    }
    Ok(())
}

fn host_reserve(cmd: HostCommand, node: &Node<'_>) -> Result<ExitCode> {
    let HostCommand::Reserve {
        id,
        owner,
        owner_host,
        placement_replay,
        preparation_protocol,
        thresholds,
        job_validity,
        job_created_ms,
        request_validity,
        request_created_ms,
    } = cmd
    else {
        return Err(Error::Invalid("not a reservation command".into()));
    };
    prepare::require_protocol(preparation_protocol)?;
    let node = Node {
        thresholds: thresholds.over(node.thresholds),
        ..*node
    };
    reserve_with_protocol(
        &node,
        &id,
        placement_replay.as_deref(),
        (job_validity, job_created_ms),
        (request_validity, request_created_ms),
        (&owner, &owner_host),
    )
}

fn reserve_with_protocol(
    node: &Node<'_>,
    id: &str,
    placement_replay: Option<&str>,
    (job_validity, job_created_ms): (Option<u32>, Option<u64>),
    (request_validity, request_created_ms): (Option<u32>, Option<u64>),
    (owner, owner_host): (&str, &str),
) -> Result<ExitCode> {
    if request_validity.is_some() {
        if request_validity != Some(jobs::REQUEST_VALIDITY_PROTOCOL_VERSION)
            || job_validity.is_some()
            || job_created_ms.is_some()
        {
            return Err(Error::Invalid(
                "unsupported non-job request validity protocol".into(),
            ));
        }
        if placement_replay.is_none() {
            let validity = request_created_ms.map(jobs::Validity::new);
            let out = node.reserve(id, now_ms(), validity, owner, owner_host)?;
            print(true, &out, String::new)?;
            return Ok(if out.reserved {
                ExitCode::SUCCESS
            } else {
                Error::Full.exit_code()
            });
        }
    }
    let validity = reservation_validity(job_validity, job_created_ms)?;
    print_host_reservation(node, id, placement_replay, validity, (owner, owner_host))
}

fn checkout_authority(
    protocol: Option<u32>,
    id: Option<&str>,
    created_ms: Option<u64>,
) -> Result<Option<(&str, jobs::Validity)>> {
    if protocol != Some(jobs::REQUEST_VALIDITY_PROTOCOL_VERSION) {
        return Err(Error::Invalid(
            "unsupported non-job request validity protocol".into(),
        ));
    }
    Ok(id.zip(created_ms.map(jobs::Validity::new)))
}

fn print_checkout(
    node: &Node<'_>,
    path: &str,
    origin: &str,
    host_label: &str,
    request_validity: Option<u32>,
    (request_id, request_created_ms): (Option<&str>, Option<u64>),
) -> Result<()> {
    let authority = checkout_authority(request_validity, request_id, request_created_ms)?;
    print(
        true,
        &checkout::local(
            node.shell,
            node.home,
            host_label,
            path,
            origin,
            node.ledger,
            authority,
        )?,
        String::new,
    )?;
    Ok(())
}

fn print_tail(node: &Node<'_>, kind: &str, id: &str) -> Result<()> {
    let tail = if kind == "job" {
        watchdog::job_tail(node.ledger, id)?
    } else {
        watchdog::pane_tail(node.shell, id)?
    };
    print(true, &tail, String::new)
}

fn print_host_recovery(node: &Node<'_>, command: &cli::RecoveryCommand) -> Result<()> {
    print(
        true,
        &runtime::recovery_local(node.shell, node.ledger, command)?,
        String::new,
    )
}

fn print_placement_finished(node: &Node<'_>, input: &str) -> Result<()> {
    print(
        true,
        &node.finish_placement(&serde_json::from_str(input)?)?,
        String::new,
    )
}

fn state_authority(cmd: StateCommand, ledger: &ledger::Ledger) -> Result<()> {
    match cmd {
        StateCommand::Reservations { put } => {
            if put {
                job_store::put_reservations(
                    ledger,
                    &serde_json::from_reader::<_, Vec<ledger::Reservation>>(
                        std::io::stdin().lock(),
                    )?,
                )?;
            } else {
                print(true, &job_store::reservations(ledger)?, String::new)?;
            }
        }
        StateCommand::RefuseClockWrites => {
            store::write(ledger, |tx| {
                tx.execute_batch("CREATE TRIGGER refuse_job_clock_insert BEFORE INSERT ON job_clock BEGIN SELECT RAISE(ABORT, 'clock write refused'); END; CREATE TRIGGER refuse_job_clock_update BEFORE UPDATE ON job_clock BEGIN SELECT RAISE(ABORT, 'clock write refused'); END;")?;
                Ok(())
            })?;
        }
        StateCommand::Validity { id, put, delete } => {
            jobs::validate_id(&id)?;
            if put {
                let validity: jobs::Validity = serde_json::from_reader(std::io::stdin().lock())?;
                store::write(ledger, |tx| job_store::put_validity(tx, &id, validity))?;
            } else if delete {
                store::write(ledger, |tx| {
                    tx.execute("DELETE FROM job_validity WHERE id = ?1", [&id])?;
                    Ok(())
                })?;
            } else {
                print(
                    true,
                    &store::read(ledger, |tx| job_store::validity(tx, &id))?,
                    String::new,
                )?;
            }
        }
        StateCommand::Ingress { id, put, delete } => {
            jobs::validate_id(&id)?;
            if put {
                #[derive(serde::Deserialize)]
                struct Ingress {
                    version: u32,
                }
                let incoming: Ingress = serde_json::from_reader(std::io::stdin().lock())?;
                if incoming.version != 1 {
                    return Err(Error::Invalid("ingress version".into()));
                }
                store::write(ledger, |tx| job_store::put_ingress(tx, &id))?;
            } else if delete {
                store::write(ledger, |tx| {
                    tx.execute("DELETE FROM request_ingress WHERE id = ?1", [&id])?;
                    Ok(())
                })?;
            } else {
                let present = store::read(ledger, |tx| job_store::ingress(tx, &id))?;
                print(
                    true,
                    &present.then(|| serde_json::json!({"version":1})),
                    String::new,
                )?;
            }
        }
        StateCommand::Clock { put, delete } => {
            if put {
                let clock: jobs::Clock = serde_json::from_reader(std::io::stdin().lock())?;
                store::write(ledger, |tx| job_store::put_clock(tx, &clock))?;
            } else if delete {
                store::write(ledger, |tx| {
                    tx.execute("DELETE FROM job_clock", [])?;
                    Ok(())
                })?;
            } else {
                print(true, &jobs::recorded_clock(ledger)?, String::new)?;
            }
        }
        _ => return Err(Error::Invalid("not a request authority command".into())),
    }
    Ok(())
}

fn state(cmd: StateCommand, ledger: &ledger::Ledger) -> Result<()> {
    match cmd {
        #[cfg(debug_assertions)]
        StateCommand::Project => print(true, &observation::stored::project(ledger)?, String::new)?,
        StateCommand::Schema => println!("{}", store::creation()),
        command @ (StateCommand::Launches
        | StateCommand::Launch { .. }
        | StateCommand::Fault { .. }
        | StateCommand::Preparations { .. }
        | StateCommand::Preparation { .. }) => state_records(command, ledger)?,
        command @ (StateCommand::Validity { .. }
        | StateCommand::Ingress { .. }
        | StateCommand::Clock { .. }
        | StateCommand::Reservations { .. }
        | StateCommand::RefuseClockWrites) => state_authority(command, ledger)?,
        StateCommand::HostFiles(command) => {
            print(true, &host_store::run(command, ledger)?, String::new)?;
        }
        StateCommand::DoneIdentity { pane } => {
            print(true, &done::identity(ledger, &pane)?, String::new)?;
        }
        StateCommand::Admissions => print(true, &admissions::values(ledger)?, String::new)?,
        command @ (StateCommand::Jobs
        | StateCommand::JobStop { .. }
        | StateCommand::Cancellations
        | StateCommand::Job { .. }
        | StateCommand::Selection { .. }
        | StateCommand::Cancellation { .. }) => state_jobs(command, ledger)?,
        StateCommand::Items { put: true } => {
            let items: Vec<items::Item> = serde_json::from_reader(std::io::stdin().lock())?;
            store::write(ledger, |tx| {
                tx.execute("DELETE FROM items", [])?;
                tables::sync_items(tx, &items)
            })?;
        }
        StateCommand::Items { put: false } => {
            print(true, &store::read(ledger, tables::items)?, String::new)?;
        }
        StateCommand::Tickets { put: true } => {
            let tickets: Vec<work::Ticket> = serde_json::from_reader(std::io::stdin().lock())?;
            store::write(ledger, |tx| {
                tx.execute("DELETE FROM tickets", [])?;
                tables::sync_tickets(tx, &tickets)
            })?;
        }
        StateCommand::Tickets { put: false } => {
            print(true, &store::read(ledger, tables::tickets)?, String::new)?;
        }
        command @ (StateCommand::Inbox { .. }
        | StateCommand::InboxReader { .. }
        | StateCommand::Outbox { .. }
        | StateCommand::NotificationFailures { .. }
        | StateCommand::Todo { .. }
        | StateCommand::Sidebar { .. }
        | StateCommand::Dock { .. }
        | StateCommand::Question { .. }
        | StateCommand::Review { .. }
        | StateCommand::Attachment { .. }) => {
            print(
                true,
                &web_store::command::run(ledger, command)?,
                String::new,
            )?;
        }
        StateCommand::Admission { id, put, delete } => {
            jobs::validate_id(&id)?;
            if put {
                let attempt: serde_json::Value = serde_json::from_reader(std::io::stdin().lock())?;
                admissions::put_value(ledger, &id, &attempt)?;
            } else if delete {
                admissions::remove(ledger, &id)?;
            } else {
                print(true, &admissions::value(ledger, &id)?, String::new)?;
            }
        }
    }
    Ok(())
}

/// Launch and preparation records, and the storage fault seams tests set up.
fn state_records(cmd: StateCommand, ledger: &ledger::Ledger) -> Result<()> {
    match cmd {
        StateCommand::Launches => print(true, &launch_store::ids(ledger)?, String::new)?,
        StateCommand::Launch {
            id,
            record,
            put,
            delete,
        } => {
            jobs::validate_id(&id)?;
            if put {
                let value = serde_json::from_reader(std::io::stdin().lock())?;
                launch_store::put_value(ledger, record, &id, value)?;
            } else if delete {
                launch_store::remove(ledger, record, &id)?;
            } else {
                print(
                    true,
                    &launch_store::value(ledger, record, &id)?,
                    String::new,
                )?;
            }
        }
        StateCommand::Fault { store, write } => {
            if !preparation_store::fault(ledger, &store, write)?
                && !host_store::fault(ledger, &store, write)?
            {
                return Err(error::Error::Invalid("unknown state fault".into()));
            }
        }
        StateCommand::Preparations { put } => {
            if put {
                preparation_store::put_pending(
                    ledger,
                    &serde_json::from_reader::<_, Vec<prepare::Pending>>(std::io::stdin().lock())?,
                )?;
            } else {
                print(true, &preparation_store::pending(ledger)?, String::new)?;
            }
        }
        StateCommand::Preparation {
            kind,
            id,
            put,
            delete,
        } => {
            macro_rules! record {
                ($module:ident, $ty:ty) => {{
                    use preparation_store::$module as records;
                    match id.as_deref() {
                        Some(id) if put => {
                            let record: $ty = serde_json::from_reader(std::io::stdin().lock())?;
                            records::put(ledger, id, &record)?;
                        }
                        Some(id) if delete => records::remove(ledger, id)?,
                        Some(id) => print(true, &records::get(ledger, id)?, String::new)?,
                        None if !put && !delete => {
                            print(true, &records::ids(ledger)?, String::new)?
                        }
                        None => {
                            return Err(error::Error::Invalid(
                                "preparation record id required".into(),
                            ));
                        }
                    }
                }};
            }
            match kind.as_str() {
                "receipt" => record!(receipts, prepare::PreparationReceipt),
                "disposition" => record!(dispositions, prepare::disposition::Receipt),
                "grant" => record!(grants, node::PlacementGrant),
                "lifecycle" => record!(lifecycles, node::GrantLifecycle),
                "expectation" => record!(expectations, prepare::PreparationLookup),
                "retirement" => record!(retirements, preparation_retirement::Intent),
                _ => {
                    return Err(error::Error::Invalid(
                        "unknown preparation record kind".into(),
                    ));
                }
            }
        }
        _ => return Err(Error::Invalid("another state command".into())),
    }
    Ok(())
}

/// Job records, stop requests, placement selections, and cancellations.
fn state_jobs(cmd: StateCommand, ledger: &ledger::Ledger) -> Result<()> {
    match cmd {
        StateCommand::Jobs => print(true, &job_store::all(ledger)?, String::new)?,
        StateCommand::JobStop { id } => {
            jobs::validate_id(&id)?;
            print(true, &job_store::stop_requested(ledger, &id)?, String::new)?;
        }
        StateCommand::Cancellations => {
            print(true, &job_store::cancellations(ledger)?, String::new)?;
        }
        StateCommand::Job { id, put, delete } => {
            jobs::validate_id(&id)?;
            if put {
                let job: jobs::Job = serde_json::from_reader(std::io::stdin().lock())?;
                job_store::put_named(ledger, &id, &job)?;
            } else if delete {
                job_store::remove(ledger, &id)?;
            } else {
                print(true, &job_store::get(ledger, &id)?, String::new)?;
            }
        }
        StateCommand::Selection { id, put, delete } => {
            jobs::validate_id(&id)?;
            if put {
                #[derive(serde::Deserialize)]
                struct Selection {
                    host: String,
                }
                let selection: Option<Selection> =
                    serde_json::from_reader(std::io::stdin().lock())?;
                job_store::put_selection(ledger, &id, selection.as_ref().map(|s| s.host.as_str()))?;
            } else if delete {
                job_store::put_selection(ledger, &id, None)?;
            } else {
                print(
                    true,
                    &job_store::selection(ledger, &id)?
                        .map(|host| serde_json::json!({"host":host})),
                    String::new,
                )?;
            }
        }
        StateCommand::Cancellation { id, put, delete } => {
            jobs::validate_id(&id)?;
            if put {
                let saved: work::Cancellation = serde_json::from_reader(std::io::stdin().lock())?;
                if saved.ticket.request.id != id {
                    return Err(Error::Invalid("cancellation identity changed".into()));
                }
                job_store::put_cancellation(ledger, &saved)?;
            } else if delete {
                job_store::remove_cancellation(ledger, &id)?;
            } else {
                print(true, &job_store::cancellation(ledger, &id)?, String::new)?;
            }
        }
        _ => return Err(Error::Invalid("another state command".into())),
    }
    Ok(())
}

fn host(cmd: HostCommand, node: &Node<'_>) -> Result<ExitCode> {
    match cmd {
        HostCommand::Lead { .. } | HostCommand::AgentDetail { .. } => {
            return Err(Error::Invalid(
                "this host command is dispatched before host routing".into(),
            ));
        }
        HostCommand::InboxPut { .. } | HostCommand::InboxState { .. } => {
            inbox::host(node.ledger, node.shell, cmd)?;
        }
        HostCommand::Launch {
            request,
            job_validity,
            preparation_protocol,
            request_validity,
        } => print_host_launch(
            node,
            &request,
            job_validity,
            preparation_protocol,
            request_validity,
        )?,
        HostCommand::Job { id } => print(true, &jobs::get(node.ledger, &id)?, String::new)?,
        HostCommand::Recovery { command } => print_host_recovery(node, &command)?,
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
            request_validity,
            request_id,
            request_created_ms,
        } => {
            print_checkout(
                node,
                &path,
                &origin,
                &host_label,
                request_validity,
                (request_id.as_deref(), request_created_ms),
            )?;
        }
        HostCommand::Tail { kind, id } => print_tail(node, &kind, &id)?,
        HostCommand::Doctor => print(true, &cleanup::doctor(node, now_ms())?, String::new)?,
        HostCommand::Reply { .. }
        | HostCommand::Close { .. }
        | HostCommand::UnitReceipt { .. }
        | HostCommand::UnitReceipts { .. }
        | HostCommand::ReleaseReferences { .. }
        | HostCommand::ExecutionEvidence { .. }
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
        HostCommand::Observation { request } => print(
            true,
            &observation::ipc::Client::at(&node.ledger.dir)
                .peer(serde_json::from_str(&request)?)?,
            String::new,
        )?,
        HostCommand::Snapshot => print(true, &node.snapshot()?, String::new)?,
        HostCommand::Todos => print(
            true,
            &observation::read::view::<Vec<board::TodoSections>>(
                &observation::ipc::Client::at(&node.ledger.dir).current()?,
                "todos",
            )?,
            String::new,
        )?,
        HostCommand::QuotaStore => print(
            true,
            &quota::store(node.ledger, std::io::stdin().lock())?,
            String::new,
        )?,
        HostCommand::PlacementFinished { input } => print_placement_finished(node, &input)?,
        HostCommand::ReleaseUnstarted { input } => print(
            true,
            &jobs::release_unstarted(node.ledger, &serde_json::from_str(&input)?)?,
            String::new,
        )?,
        cmd @ HostCommand::Reserve { .. } => return host_reserve(cmd, node),
        cmd @ (HostCommand::PreparationLookup { .. }
        | HostCommand::Prepared { .. }
        | HostCommand::Prepare { .. }) => host_preparation(cmd, node)?,
    }
    Ok(ExitCode::SUCCESS)
}
