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

pub fn recent(agent: &str, path: &str) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(262_144)))
        .ok()?;
    let mut bytes = Vec::new();
    file.take(262_144).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    text.lines().rev().find_map(|line| {
        let value: Value = serde_json::from_str(line).ok()?;
        let activity = match agent {
            "pi" => pi(&value),
            "claude" => claude(&value),
            "codex" => codex(&value),
            _ => None,
        }?;
        let clean = plain(&activity);
        Some(
            clean
                .lines()
                .take(5)
                .map(|line| line.chars().take(500).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n"),
        )
    })
}

fn content(value: &Value, call: &str, arguments: &str) -> Option<String> {
    if let Some(text) = value.as_str() {
        return (!text.is_empty()).then(|| text.into());
    }
    value.as_array()?.iter().rev().find_map(|block| {
        if block["type"] == call {
            Some(format!(
                "tool {}: {}",
                block["name"].as_str()?,
                block[arguments]
            ))
        } else if matches!(
            block["type"].as_str(),
            Some("text" | "input_text" | "output_text")
        ) {
            block["text"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        } else {
            None
        }
    })
}

fn pi(value: &Value) -> Option<String> {
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

fn claude(value: &Value) -> Option<String> {
    if !matches!(value["type"].as_str(), Some("assistant" | "user")) {
        return None;
    }
    content(&value["message"]["content"], "tool_use", "input")
}

fn codex(value: &Value) -> Option<String> {
    if value["type"] != "response_item" {
        return None;
    }
    let payload = &value["payload"];
    match payload["type"].as_str()? {
        "function_call" => Some(format!(
            "tool {}: {}",
            payload["name"].as_str()?,
            payload["arguments"].as_str()?
        )),
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
        assert!(pi(&json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","name":"bash","arguments":{"command":"cat /tmp/hang.fifo"}}]}})).unwrap().contains("cat /tmp/hang.fifo"));
        assert!(claude(&json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"make test"}}]}})).unwrap().contains("make test"));
        assert!(codex(&json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"make test\"}"}})).unwrap().contains("make test"));
        assert_eq!(
            pi(
                &json!({"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"checking"}]}})
            ),
            Some("checking".into())
        );
        assert_eq!(
            pi(&json!({"type":"message","message":{"role":"system","content":"secret"}})),
            None
        );
    }
    #[test]
    fn strips_ansi_and_control_strings() {
        assert_eq!(
            plain("\x1b[31mred\x1b[0m\x1b]0;title\x07\x1bPdata\x1b\\\u{9b}2J\x00\nnext"),
            "red\nnext"
        );
    }
}
