//! Bounded, display-only transcript activity. Unknown formats fall back to pane text.
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};

/// Strip terminal controls, including CSI, OSC, DCS, and C1 forms.
pub fn plain(text: &str) -> String {
    let mut chars = text.chars().peekable();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        let control = if c == '\u{1b}' {
            chars.next()
        } else {
            match c {
                '\u{9b}' => Some('['),
                '\u{9d}' => Some(']'),
                '\u{90}' => Some('P'),
                _ => None,
            }
        };
        match control {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']' | 'P' | '^' | '_') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' || c == '\u{9c}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            None if !c.is_control() || matches!(c, '\n' | '\t') => out.push(c),
            _ => (),
        }
    }
    out
}

/// The last 256 KiB of a transcript, as JSON values, newest first. Lines cut by the window or
/// written partly are skipped.
pub fn tail(path: &str) -> Option<Vec<Value>> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(262_144)))
        .ok()?;
    let mut bytes = Vec::new();
    file.take(262_144).read_to_end(&mut bytes).ok()?;
    Some(
        String::from_utf8_lossy(&bytes)
            .lines()
            .rev()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect(),
    )
}

/// One step of an agent's work: a tool call or a message.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Tool { name: String, arguments: Value },
    Text(String),
}

impl Step {
    /// Up to five lines, for a check-in note.
    fn detail(&self) -> String {
        let text = match self {
            Self::Tool { name, arguments } => format!("tool {name}: {arguments}"),
            Self::Text(text) => text.clone(),
        };
        plain(&text)
            .lines()
            .take(5)
            .map(|line| line.chars().take(500).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// One short line, for the board: the command, the path, or the first line of text.
    pub fn line(&self) -> String {
        let text = match self {
            Self::Tool { name, arguments } => {
                // Codex sends its arguments as a JSON string.
                let arguments = arguments
                    .as_str()
                    .and_then(|s| serde_json::from_str::<Value>(s).ok())
                    .unwrap_or_else(|| arguments.clone());
                let field = |keys: &[&str]| {
                    keys.iter()
                        .find_map(|k| arguments[*k].as_str().map(str::to_owned))
                };
                if let Some(command) = field(&["command", "cmd"]) {
                    command
                } else if let Some(path) = field(&["path", "file_path"]) {
                    format!("{} {path}", name.to_lowercase())
                } else {
                    name.clone()
                }
            }
            Self::Text(text) => text.clone(),
        };
        let line = plain(&text)
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or_default()
            .to_owned();
        if line.chars().count() > 120 {
            format!("{}…", line.chars().take(119).collect::<String>())
        } else {
            line
        }
    }
}

/// The newest step in `values` (newest first) for `agent`'s transcript format.
pub fn latest(agent: &str, values: &[Value]) -> Option<Step> {
    values.iter().find_map(|value| match agent {
        "pi" => pi(value),
        "claude" => claude(value),
        "codex" => codex(value),
        _ => None,
    })
}

pub fn recent(agent: &str, path: &str) -> Option<String> {
    latest(agent, &tail(path)?).map(|step| step.detail())
}

fn content(value: &Value, call: &str, arguments: &str) -> Option<Step> {
    if let Some(text) = value.as_str() {
        return (!text.is_empty()).then(|| Step::Text(text.into()));
    }
    value.as_array()?.iter().rev().find_map(|block| {
        if block["type"] == call {
            Some(Step::Tool {
                name: block["name"].as_str()?.into(),
                arguments: block[arguments].clone(),
            })
        } else if matches!(
            block["type"].as_str(),
            Some("text" | "input_text" | "output_text")
        ) {
            block["text"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(|s| Step::Text(s.into()))
        } else {
            None
        }
    })
}

fn pi(value: &Value) -> Option<Step> {
    if value["type"] != "message" {
        return None;
    }
    let message = &value["message"];
    if !matches!(
        message["role"].as_str(),
        Some("assistant" | "user" | "toolResult")
    ) {
        return None;
    }
    content(&message["content"], "toolCall", "arguments")
}

fn claude(value: &Value) -> Option<Step> {
    if !matches!(value["type"].as_str(), Some("assistant" | "user")) {
        return None;
    }
    content(&value["message"]["content"], "tool_use", "input")
}

fn codex(value: &Value) -> Option<Step> {
    if value["type"] != "response_item" {
        return None;
    }
    let payload = &value["payload"];
    match payload["type"].as_str()? {
        "function_call" => Some(Step::Tool {
            name: payload["name"].as_str()?.into(),
            arguments: payload["arguments"].clone(),
        }),
        "message" => content(&payload["content"], "function_call", "arguments"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn harness_activity_extracts_tool_calls_and_messages() {
        assert!(pi(&json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","name":"bash","arguments":{"command":"cat /tmp/hang.fifo"}}]}})).unwrap().detail().contains("cat /tmp/hang.fifo"));
        assert!(claude(&json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"make test"}}]}})).unwrap().detail().contains("make test"));
        assert!(codex(&json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"make test\"}"}})).unwrap().detail().contains("make test"));
        assert_eq!(
            pi(
                &json!({"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"checking"}]}})
            ),
            Some(Step::Text("checking".into()))
        );
        assert_eq!(
            pi(&json!({"type":"message","message":{"role":"system","content":"secret"}})),
            None
        );
    }
    #[test]
    fn board_line_names_the_command_or_path() {
        let step = codex(&json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"cargo test -p auth\"}"}})).unwrap();
        assert_eq!(step.line(), "cargo test -p auth");
        let step = pi(&json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","name":"edit","arguments":{"path":"src/api/dups.rs"}}]}})).unwrap();
        assert_eq!(step.line(), "edit src/api/dups.rs");
        let long = Step::Text(format!("\n  \x1b[1m{}\x1b[0m\nsecond", "x".repeat(200)));
        assert_eq!(long.line().chars().count(), 120);
        assert!(long.line().ends_with('…'));
    }

    #[test]
    fn strips_ansi_and_control_strings() {
        assert_eq!(
            plain("\x1b[31mred\x1b[0m\x1b]0;title\x07\x1bPdata\x1b\\\u{9b}2J\x00\nnext"),
            "red\nnext"
        );
    }
}
