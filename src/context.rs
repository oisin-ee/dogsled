//! One prompt-context formatter for Pi, Claude Code, and Codex.

use crate::commands::{Status, brief, close_line};
use clap::ValueEnum;
use serde::Serialize;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Harness {
    Claude,
    Codex,
    Pi,
}

#[derive(Debug, Clone, Copy, ValueEnum, Serialize)]
#[value(rename_all = "PascalCase")]
pub enum Event {
    SessionStart,
    UserPromptSubmit,
}

/// `owner` is the caller's Herdr pane. Its finished children get their close commands.
pub fn format(harness: Harness, event: Event, status: &Status, owner: Option<&str>) -> String {
    if status.hosts.iter().all(|host| host.sampled_ms.is_none()) {
        return String::new();
    }
    let mut text = brief(status);
    if let Some(line) = owner.and_then(|owner| close_line(status, owner)) {
        text.push_str(&line);
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
    use crate::hosts::{Host, HostReport};

    fn status() -> Status {
        Status {
            hosts: vec![HostReport {
                host: Host::local(),
                service: "down".into(),
                reachable: true,
                sampled_ms: Some(100),
                probe: None,
                headroom: None,
                error: None,
                jobs: Vec::new(),
            }],
            items: Vec::new(),
            queue: Vec::new(),
        }
    }

    #[test]
    fn claude_and_codex_wrap_the_same_brief_for_each_event() {
        let status = status();
        for harness in [Harness::Claude, Harness::Codex] {
            for event in [Event::SessionStart, Event::UserPromptSubmit] {
                let output: serde_json::Value =
                    serde_json::from_str(&format(harness, event, &status, None)).unwrap();
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
            format(Harness::Pi, Event::UserPromptSubmit, &status, None),
            brief(&status)
        );
    }

    #[test]
    fn no_snapshot_produces_no_context_for_any_harness() {
        let mut status = status();
        status.hosts[0].sampled_ms = None;
        for harness in [Harness::Claude, Harness::Codex, Harness::Pi] {
            assert_eq!(format(harness, Event::SessionStart, &status, None), "");
        }
    }
}
