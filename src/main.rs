//! Host status, local service, and capacity reservations for Herdr agents.

mod activity;
mod backoff;
mod board;
mod budget;
mod checkout;
mod cleanup;
mod cli;
mod close;
mod commands;
mod config;
mod context;
mod done;
mod error;
mod facts;
mod gate;
mod hosts;
mod inbox;
mod integration;
mod items;
mod jobs;
mod launch;
mod ledger;
mod loss;
#[cfg(test)]
mod loss_tests;
mod machines;
mod mcp;
mod node;
mod plan;
mod prepare;
mod probe;
mod profiles;
mod quota;
mod repository;
mod runtime;
mod service;
mod shell;
mod sweep;
mod transcript;
mod unit_reply;
mod unit_request;
mod unit_transport;
mod watchdog;
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
    match run(cli, budget) {
        Ok(code) => code,
        // Prompt context is optional: unavailable state must not block a turn.
        Err(_) if hook => ExitCode::SUCCESS,
        Err(Error::Deadline(_)) if waiting => {
            println!("still running");
            ExitCode::from(124)
        }
        Err(error) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "error": error.to_string(), "deadline": matches!(error, Error::Deadline(_)) })
                );
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
        Command::Quota(args) => {
            let snapshot = quota::command(&shell, &ledger, &policy, &home, &args)?;
            print(cli.json, &snapshot, || snapshot.render(now_ms()))?;
        }
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
        } => return runtime::exec(&ledger, &shell, id, cmd, timeout, idle_timeout),
        Command::Status {
            watch,
            interval,
            line,
            brief,
        } => loop {
            let fleet = hosts::Fleet::discover(&shell, &ledger, &policy, home.clone())?;
            let status = commands::status(&fleet)?;
            if watch && !cli.json && !line && !brief {
                print!("\x1b[2J\x1b[H");
            }
            print(cli.json, &status, || {
                if line {
                    commands::line(&status)
                } else if brief {
                    commands::brief(&status)
                } else {
                    commands::render(&status)
                }
            })?;
            if !watch {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(interval));
        },
        command @ Command::Board { .. } => {
            board::command(&shell, &ledger, &policy, home, command, cli.json)?;
        }
        Command::Unit(command) => {
            let fleet = hosts::Fleet::discover(&shell, &ledger, &policy, home)?;
            return unit_request::run(&fleet, command);
        }
        Command::Mcp => mcp::serve(policy, ledger, home)?,
        Command::Done => done_command(&ledger, &shell, cli.json)?,
        Command::Inbox(cmd) => inbox_command(cmd, &ledger, &shell, cli.json)?,
        Command::Send {
            target,
            host,
            message_file,
            message,
            kind,
        } => {
            let text = inbox::message_text(message, message_file)?;
            let fleet = hosts::Fleet::discover(&shell, &ledger, &policy, home)?;
            let sent = inbox::send_to(&fleet, &target, host.as_deref(), &kind, text, now_ms())?;
            print(cli.json, &sent, || sent.line())?;
        }
        Command::Hook(cmd) => hook(cmd, &ledger, &shell, &policy, home)?,
        command => {
            let fleet = hosts::Fleet::discover(&shell, &ledger, &policy, home)?;
            return runtime::dispatch(&fleet, command, cli.json);
        }
    }
    Ok(ExitCode::SUCCESS)
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
            print(false, &(), || {
                context::format(harness, event, &status, owner.as_deref())
            })
        }
    }
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
        HostCommand::UnitReceipt { input } => print(
            true,
            &launch::unit_receipt(
                node.ledger,
                &serde_json::from_str::<unit_transport::ExecutionInput>(&input)?.execution,
            )?,
            String::new,
        )?,
        HostCommand::Reply { pane } => {
            print(true, &sweep::local_reply(node.shell, &pane)?, String::new)?;
        }
        // Over SSH the caller has no pane or workspace on this host.
        HostCommand::Close {
            pane,
            pushed,
            landed,
            writer,
        } => {
            let guard = close::Guard::from_flags(pushed, landed, writer);
            let closed = close::local(
                node.shell,
                node.ledger,
                &pane,
                &close::Caller::default(),
                guard,
            )?;
            print(true, &closed, String::new)?;
        }
        _ => unreachable!("only child commands"),
    }
    Ok(())
}

fn host(cmd: HostCommand, node: &Node<'_>) -> Result<ExitCode> {
    match cmd {
        HostCommand::InboxPut { .. } | HostCommand::InboxState { .. } => {
            inbox::host(node.ledger, cmd)?;
        }
        HostCommand::Launch { request } => print(
            true,
            &work::launch(
                node.ledger,
                node.shell,
                &serde_json::from_str(&request)?,
                now_ms(),
            )?,
            String::new,
        )?,
        HostCommand::Job { id } => print(true, &jobs::get(node.ledger, &id)?, String::new)?,
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
        HostCommand::Parent { pane } => {
            watchdog::register_parent(node.ledger, node.shell, &pane)?;
            print(true, &true, String::new)?;
        }
        HostCommand::Tail { kind, id } => {
            let tail = if kind == "job" {
                watchdog::job_tail(node.ledger, &id)?
            } else {
                watchdog::pane_tail(node.shell, &id)?
            };
            print(true, &tail, String::new)?;
        }
        HostCommand::Doctor => print(true, &cleanup::doctor(node, now_ms())?, String::new)?,
        HostCommand::Reply { .. } | HostCommand::Close { .. } | HostCommand::UnitReceipt { .. } => {
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
        HostCommand::Reserve { id, thresholds } => {
            let node = Node {
                thresholds: thresholds.over(node.thresholds),
                ..*node
            };
            let out = node.reserve(&id, now_ms())?;
            print(true, &out, String::new)?;
            if !out.reserved {
                return Ok(Error::Full.exit_code());
            }
        }
        HostCommand::Prepared { path, request_id } => {
            prepare::used(node.ledger, &path, request_id.as_deref())?;
            print(true, &serde_json::json!({"prepared": true}), String::new)?;
        }
        HostCommand::Prepare {
            repo,
            branch,
            base,
            request_id,
        } => {
            let prepared =
                prepare::on_host(node, &repo, branch, base.as_deref(), request_id.as_deref())?;
            print(true, &prepared, String::new)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}
