//! One prompt-context formatter for Pi, Claude Code, and Codex.

use crate::commands::Status;
use crate::error::Result;
use crate::gate::Findings;
use crate::observation::{Published, display::Display, read};
use clap::ValueEnum;
use serde::Serialize;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Harness {
    Claude,
    Codex,
    Pi,
}

impl Harness {
    /// The name an inbox reader registers with.
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Pi => "pi",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum, Serialize)]
#[value(rename_all = "PascalCase")]
pub enum Event {
    SessionStart,
    UserPromptSubmit,
}

/// What `format` reads.
pub(crate) fn select() -> crate::observation::ipc::Select {
    crate::observation::ipc::Select::views(
        read::BOARD
            .into_iter()
            .chain(read::FINDINGS)
            .chain(["status", "status_display"]),
    )
}

/// `owner` is the caller's Herdr pane. Its own children and jobs come first, with those that
/// have work to land named; the fleet's hosts and counts follow. The cleanup line counts only
/// `found`: the last pass's findings about work the caller created.
pub fn format(
    harness: Harness,
    event: Event,
    published: &Published,
    owner: Option<&str>,
) -> Result<String> {
    let status: Status = read::view(published, "status")?;
    let board = owner
        .map(|owner| read::board(published, Some(owner)))
        .transpose()?;
    let found = owner
        .map(|owner| read::findings(published, owner))
        .transpose()?
        .unwrap_or_default();
    let display: Display = read::view(published, "status_display")?;
    Ok(text(
        harness,
        event,
        &status,
        board.as_ref(),
        &display,
        &found,
    ))
}

fn text(
    harness: Harness,
    event: Event,
    status: &Status,
    board: Option<&crate::board::Board>,
    display: &Display,
    found: &Findings,
) -> String {
    if status.hosts.iter().all(|host| host.sampled_ms.is_none()) && status.sweep.is_none() {
        return String::new();
    }
    let mut text = board.map_or_else(String::new, crate::board::Board::owner_lines);
    for row in display.context_rows() {
        text.push_str(&row);
        text.push('\n');
    }
    if status.sweep.is_some() && (!found.failures.is_empty() || !found.holds.is_empty()) {
        use std::fmt::Write as _;
        let _ = writeln!(
            text,
            "Cleanup: {} failures and {} held worktrees in your work. Run `dogsled status` for details.",
            found.failures.len(),
            found.holds.len()
        );
    }
    match harness {
        Harness::Pi => text,
        Harness::Claude | Harness::Codex => format!(
            "{}\n",
            serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": event,
                    "additionalContext": text,
                }
            })
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::brief;
    use crate::hosts::{Host, HostReport};

    const DOWN_CONTEXT: &str = concat!(
        "local service: down · agents ?\n",
        "0 open items · 0 queued · 0 stalls. Run `dogsled status` for details.\n",
    );

    fn format(
        harness: Harness,
        event: Event,
        status: &Status,
        owner: Option<&str>,
        found: &Findings,
    ) -> String {
        let board =
            owner.map(|owner| crate::board::build(status, Some(owner), crate::ledger::now_ms()));
        text(
            harness,
            event,
            status,
            board.as_ref(),
            &Display::capture(status),
            found,
        )
    }

    fn status() -> Status {
        Status {
            hosts: vec![HostReport {
                service_lanes: Vec::new(),
                host: Host::local(),
                service: "down".into(),
                reachable: true,
                sampled_ms: Some(100),
                probe: None,
                headroom: None,
                error: None,
                failure: None,
                jobs: Vec::new(),
                handoffs: Vec::new(),
                holds: Vec::new(),
                retry_ms: None,
                job_validity: None,
            }],
            items: Vec::new(),
            queue: Vec::new(),
            orphans: Vec::new(),
            sweep: None,
            stopped_activity: std::collections::BTreeMap::new(),
            holds: Vec::new(),
        }
    }

    #[test]
    fn claude_and_codex_wrap_the_same_brief_for_each_event() {
        let status = status();
        for harness in [Harness::Claude, Harness::Codex] {
            for event in [Event::SessionStart, Event::UserPromptSubmit] {
                let output: serde_json::Value = serde_json::from_str(&format(
                    harness,
                    event,
                    &status,
                    None,
                    &Findings::default(),
                ))
                .unwrap();
                assert_eq!(
                    output,
                    serde_json::json!({
                        "hookSpecificOutput": {
                            "hookEventName": event,
                            "additionalContext": DOWN_CONTEXT,
                        }
                    })
                );
            }
        }
    }

    #[test]
    fn pi_receives_plain_brief_text() {
        let status = status();
        assert_eq!(
            format(
                Harness::Pi,
                Event::UserPromptSubmit,
                &status,
                None,
                &Findings::default()
            ),
            DOWN_CONTEXT
        );
    }

    #[test]
    fn hook_context_ignores_volatile_probe_fields_but_tracks_service_state() {
        let mut status = status();
        status.hosts[0].service = "up".into();
        status.hosts[0].probe = Some(crate::probe::Probe {
            host: "local".into(),
            label: "local".into(),
            load1: 1.0,
            cores: 8,
            cpu_ticks: None,
            busy: Some(0.125),
            memory: crate::probe::Memory {
                used_bytes: 1024,
                total_bytes: 2048,
                pressure_level: None,
            },
            disk_free_percent: 90.0,
            agents: crate::probe::AgentCounts::from_agents(&[]),
            agent_panes: Vec::new(),
            reservations: Vec::new(),
            panes: Vec::new(),
        });
        let hook = |status: &Status| {
            format(
                Harness::Pi,
                Event::UserPromptSubmit,
                status,
                None,
                &Findings::default(),
            )
        };
        let before = hook(&status);
        assert!(before.starts_with("local service: up · agents 0\n"));
        let status_before = brief(&status);
        status.hosts[0].sampled_ms = Some(30_100);
        let probe = status.hosts[0].probe.as_mut().unwrap();
        probe.busy = Some(0.875);
        probe.memory.used_bytes += 512;
        probe.disk_free_percent = 80.0;
        assert_eq!(hook(&status), before);
        assert_ne!(brief(&status), status_before);
        status.hosts[0].service = "down".into();
        assert!(hook(&status).starts_with("local service: down · agents 0\n"));
    }

    #[test]
    fn no_snapshot_produces_no_context_for_any_harness() {
        let mut status = status();
        status.hosts[0].sampled_ms = None;
        for harness in [Harness::Claude, Harness::Codex, Harness::Pi] {
            assert_eq!(
                format(
                    harness,
                    Event::SessionStart,
                    &status,
                    None,
                    &Findings::default()
                ),
                ""
            );
        }
    }
}
