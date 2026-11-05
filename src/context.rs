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
    let mut rows = display.fleet(crate::ledger::now_ms());
    if let Some(sweep) = &status.sweep
        && (!found.failures.is_empty() || !found.holds.is_empty())
    {
        rows.push(format!(
            "Cleanup at {} ms: {} failures and {} held worktrees in your work. Run `dogsled status` for details.",
            sweep.finished_ms,
            found.failures.len(),
            found.holds.len()
        ));
    }
    text.push_str(&rows.join("\n"));
    text.push('\n');
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
                            "additionalContext": brief(&status),
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
            brief(&status)
        );
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
