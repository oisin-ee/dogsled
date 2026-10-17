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
mod error;
mod facts;
mod hosts;
mod inbox;
mod integration;
mod items;
mod jobs;
mod ledger;
mod node;
mod plan;
mod prepare;
mod probe;
mod repository;
mod runtime;
mod service;
mod shell;
mod sweep;
mod transcript;
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
    let json = cli.json;
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
                eprintln!("wrangle: {error}");
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
        Command::Board { owner, all } => {
            let fleet = hosts::Fleet::discover(&shell, &ledger, &policy, home)?;
            let status = commands::status(&fleet)?;
            let owner = if all {
                None
            } else {
                Some(
                    owner
                        .or_else(|| std::env::var("HERDR_PANE_ID").ok())
                        .ok_or_else(|| {
                            Error::Invalid(
                                "HERDR_PANE_ID or --owner is required; or pass --all".into(),
                            )
                        })?,
                )
            };
            let board = board::build(&status, owner.as_deref(), now_ms());
            print(cli.json, &board, || board.render())?;
        }
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
        Command::Hook(HookCommand::Context { harness, event }) => {
            let fleet = hosts::Fleet::discover(&shell, &ledger, &policy, home)?;
            let status = commands::status(&fleet)?;
            let owner = std::env::var("HERDR_PANE_ID").ok();
            print(false, &(), || {
                context::format(harness, event, &status, owner.as_deref())
            })?;
        }
        command => {
            let fleet = hosts::Fleet::discover(&shell, &ledger, &policy, home)?;
            return runtime::dispatch(&fleet, command, cli.json);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn inbox_command(cmd: InboxCommand, ledger: &Ledger, shell: &System, json: bool) -> Result<()> {
    let now = now_ms();
    match cmd {
        InboxCommand::Claim { harness, pane } => {
            let pane = inbox::own_pane(pane)?;
            let records = inbox::claim_for(ledger, shell, &pane, &harness, now)?;
            print(json, &records, || {
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
            let listing = inbox::list_for(ledger, shell, &pane)?;
            print(json, &listing, || listing.render(now))
        }
    }
}

/// The host commands that act on one child pane.
fn child(cmd: HostCommand, node: &Node<'_>) -> Result<()> {
    match cmd {
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
        HostCommand::Reply { .. } | HostCommand::Close { .. } => child(cmd, node)?,
        HostCommand::Prune { plan } => print(
            true,
            &cleanup::apply(node, &serde_json::from_str(&plan)?, now_ms())?,
            String::new,
        )?,
        HostCommand::Snapshot => print(
            true,
            &node.ledger.read::<node::Snapshot>("host.json")?,
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
        HostCommand::Prepare { repo, branch, base } => {
            let repo = std::path::Path::new(node.home)
                .join(repo)
                .display()
                .to_string();
            let prepared = prepare::local(node.shell, &repo, &branch, base.as_deref())?;
            if let Some(path) = &prepared.path {
                cleanup::remember(
                    node.ledger,
                    cleanup::Tree {
                        repo,
                        path: path.clone(),
                        branch,
                        head: String::new(),
                    },
                )?;
            }
            print(true, &prepared, String::new)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}
