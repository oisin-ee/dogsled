//! Host status, local service, and capacity reservations for Herdr agents.

mod cli;
mod commands;
mod config;
mod error;
mod hook;
mod hosts;
mod ledger;
mod node;
mod plan;
mod probe;
mod shell;

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
            if once {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(interval));
        },
        Command::Host(cmd) => return host(cmd, &node, &policy),
        Command::Status {
            watch,
            interval,
            line,
            brief,
        } => loop {
            let fleet = hosts::Fleet::discover(&shell, &ledger, &policy, home.clone())?;
            let status = commands::Status {
                hosts: fleet.status(now_ms())?,
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
        Command::Place { host, branch, repo } => {
            let fleet = hosts::Fleet::discover(&shell, &ledger, &policy, home)?;
            let repo = repo.map_or_else(
                || std::env::current_dir().map(|p| p.display().to_string()),
                Ok,
            )?;
            let placed =
                commands::place(&fleet, host.as_deref(), branch.as_deref(), &repo, now_ms())?;
            let full = matches!(placed, commands::Placement::Full { .. });
            print(cli.json || full, &placed, || {
                commands::render_place(&placed)
            })?;
            if full {
                return Ok(ExitCode::from(error::EXIT_FULL));
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn host(cmd: HostCommand, node: &Node<'_>, policy: &config::Policy) -> Result<ExitCode> {
    match cmd {
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
        HostCommand::Bind { id, pane } => {
            node.bind(&id, &pane, now_ms())?;
            print(true, &serde_json::json!({"bound": true}), String::new)?;
        }
        HostCommand::Release { id } => {
            node.release(&id, now_ms())?;
            print(true, &serde_json::json!({"released": true}), String::new)?;
        }
        HostCommand::Prepare { repo, branch } => {
            print(
                true,
                &hook::local(node.shell, policy, &repo, &branch)?,
                String::new,
            )?;
        }
    }
    Ok(ExitCode::SUCCESS)
}
