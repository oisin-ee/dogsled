//! Host status, local service, and capacity reservations for Herdr agents.

mod checkout;
mod cli;
mod commands;
mod config;
mod error;
mod hook;
mod hosts;
mod items;
mod jobs;
mod ledger;
mod node;
mod plan;
mod probe;
mod runtime;
mod shell;
mod work;
#[cfg(test)]
mod work_tests;

use clap::Parser;
use cli::{Cli, Command, HostCommand};
use error::{Error, Result};
use ledger::{Ledger, now_ms};
use node::Node;
use serde::Serialize;
use shell::System;
use std::io::Write as _;
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.json;
    match run(cli) {
        Ok(code) => code,
        Err(error) => {
            if json {
                println!("{}", serde_json::json!({ "error": error.to_string() }));
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

fn run(cli: Cli) -> Result<ExitCode> {
    let policy = config::load(&cli.config.unwrap_or_else(config::default_path))?;
    let shell = System;
    let ledger = Ledger::new(ledger::default_dir());
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    let node = Node {
        shell: &shell,
        ledger: &ledger,
        thresholds: config::Thresholds::from(&policy),
        home: &home,
    };
    match cli.command {
        Command::Serve { interval, once } => loop {
            node.tick(now_ms())?;
            if !items::list(&ledger)?.is_empty() || !work::queue(&ledger)?.is_empty() {
                let fleet = hosts::Fleet::discover(&shell, &ledger, &policy, home.clone())?;
                work::tick(&fleet, now_ms())?;
            }
            if once {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(interval));
        },
        Command::Host(cmd) => return host(cmd, &node, &policy),
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
            let status = commands::Status {
                hosts: fleet.status(now_ms())?,
                items: items::list(&ledger)?,
                queue: work::queue(&ledger)?,
            };
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
        command => {
            let fleet = hosts::Fleet::discover(&shell, &ledger, &policy, home)?;
            return runtime::dispatch(&fleet, command, cli.json);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn host(cmd: HostCommand, node: &Node<'_>, policy: &config::Policy) -> Result<ExitCode> {
    match cmd {
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
            print(
                true,
                &hook::local(
                    node.shell,
                    policy,
                    &std::path::Path::new(node.home)
                        .join(repo)
                        .display()
                        .to_string(),
                    &branch,
                    base.as_deref(),
                )?,
                String::new,
            )?;
        }
    }
    Ok(ExitCode::SUCCESS)
}
