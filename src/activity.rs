//! Bounded, display-only transcript activity. Unknown formats fall back to pane text.
use portal_types::agents::{BlockedPrompt, PromptOption, Role, Turn};
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};

/// The longest question a blocked prompt carries. A longer one is cut, so a
/// screen with a paragraph of text cannot flood the portal.
const MAX_QUESTION: usize = 400;

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

/// The agent's final reply: the text of its newest assistant message, in `values` (newest
/// first). Tool calls and thinking are not part of it.
pub fn reply(agent: &str, values: &[Value]) -> Option<String> {
    let texts = |content: &Value| -> Vec<String> {
        if let Some(text) = content.as_str() {
            return vec![text.to_owned()];
        }
        content
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| matches!(b["type"].as_str(), Some("text" | "output_text")))
            .filter_map(|b| b["text"].as_str().map(str::to_owned))
            .collect()
    };
    let join = |parts: Vec<String>| {
        let text = parts.join("\n\n").trim().to_owned();
        (!text.is_empty()).then_some(text)
    };
    match agent {
        "pi" => values.iter().find_map(|v| {
            (v["type"] == "message" && v["message"]["role"] == "assistant")
                .then(|| join(texts(&v["message"]["content"])))
                .flatten()
        }),
        "codex" => values.iter().find_map(|v| {
            let p = &v["payload"];
            (v["type"] == "response_item" && p["type"] == "message" && p["role"] == "assistant")
                .then(|| join(texts(&p["content"])))
                .flatten()
        }),
        // Claude writes one line per content block; the blocks of one message share its id.
        "claude" => {
            let newest = values.iter().position(|v| {
                v["type"] == "assistant" && !texts(&v["message"]["content"]).is_empty()
            })?;
            let id = &values[newest]["message"]["id"];
            let mut parts: Vec<String> = values[newest..]
                .iter()
                .take_while(|v| v["type"] != "user")
                .filter(|v| v["type"] == "assistant" && (id.is_null() || v["message"]["id"] == *id))
                .flat_map(|v| texts(&v["message"]["content"]))
                .collect();
            parts.reverse();
            join(parts)
        }
        _ => None,
    }
}

pub fn recent(agent: &str, path: &str) -> Option<String> {
    latest(agent, &tail(path)?).map(|step| step.detail())
}

/// The text of a content block list. Only text blocks count: tool calls and
/// thinking are not part of a message the owner reads.
fn text_blocks(content: &Value) -> Vec<String> {
    if let Some(text) = content.as_str() {
        return vec![text.to_owned()];
    }
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| {
            matches!(
                b["type"].as_str(),
                Some("text" | "input_text" | "output_text")
            )
        })
        .filter_map(|b| b["text"].as_str().map(str::to_owned))
        .collect()
}

/// The text of a message, or `None` when it carries none.
fn joined(parts: &[String]) -> Option<String> {
    let text = parts.join("\n\n").trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// The conversation in `agent`'s transcript, oldest turn first.
///
/// A turn is one message of the owner or of the agent. Tool calls, tool
/// results and thinking are not turns. Claude writes one line per content
/// block, and the blocks of one message share its id: they are one turn.
///
/// The transcript is read backwards until it gives the newest
/// [`CONVERSATION_TURNS`] text turns, or [`CONVERSATION_BYTES`] are read. Tool
/// output can be megabytes after the last text turn, so a fixed tail window
/// is not enough.
pub fn conversation(agent: &str, path: &str) -> Vec<Turn> {
    let mut newest: Vec<(Option<String>, Turn)> = Vec::new();
    lines_backwards(path, CONVERSATION_BYTES, CHUNK_BYTES, |line| {
        if let Ok(value) = serde_json::from_str::<Value>(line)
            && let Some(found) = turn(agent, &value)
        {
            newest.push(found);
        }
        newest.len() < CONVERSATION_TURNS
    });
    // The read runs newest first, and a conversation reads oldest first.
    let mut turns: Vec<(Option<String>, Turn)> = Vec::new();
    for (id, turn) in newest.into_iter().rev() {
        if let Some(last) = turns.last_mut()
            && let Some(id) = &id
            && last.0.as_ref() == Some(id)
            && last.1.role == turn.role
        {
            last.1.text = joined(&[last.1.text.clone(), turn.text]).unwrap_or_default();
            continue;
        }
        turns.push((id, turn));
    }
    turns.into_iter().map(|(_, turn)| turn).collect()
}

/// The most text turns a conversation returns.
const CONVERSATION_TURNS: usize = 200;

/// The most transcript bytes a conversation reads.
const CONVERSATION_BYTES: u64 = 32 * 1024 * 1024;

/// The size of one backward read.
const CHUNK_BYTES: u64 = 1024 * 1024;

/// Give the complete lines of `path` to `visit`, newest first, until `visit`
/// returns `false` or `limit` bytes are read, `step` bytes at a time. A line
/// cut by the limit is not given.
fn lines_backwards(path: &str, limit: u64, step: u64, mut visit: impl FnMut(&str) -> bool) {
    let Ok(mut file) = std::fs::File::open(path) else {
        return;
    };
    let Ok(len) = file.metadata().map(|meta| meta.len()) else {
        return;
    };
    let floor = len.saturating_sub(limit);
    let mut end = len;
    // The bytes after this chunk that belong to a line starting in it.
    let mut carry: Vec<u8> = Vec::new();
    while end > floor {
        let start = end.saturating_sub(step).max(floor);
        let Ok(size) = usize::try_from(end - start) else {
            return;
        };
        let mut bytes = vec![0; size];
        if file.seek(SeekFrom::Start(start)).is_err() || file.read_exact(&mut bytes).is_err() {
            return;
        }
        bytes.extend_from_slice(&carry);
        let chunk = bytes;
        // The first line is complete at the start of the file, or at the read
        // limit when a newline comes just before it. Elsewhere the bytes before
        // the first newline end a line that starts earlier.
        let whole = start == 0 || (start == floor && newline_before(&mut file, start));
        let (head, lines) = if whole {
            (&[][..], &chunk[..])
        } else {
            match chunk.iter().position(|byte| *byte == b'\n') {
                Some(index) => (&chunk[..index], &chunk[index + 1..]),
                None => (&chunk[..], &[][..]),
            }
        };
        for line in lines.rsplit(|byte| *byte == b'\n') {
            if !line.is_empty() && !visit(&String::from_utf8_lossy(line)) {
                return;
            }
        }
        carry = head.to_vec();
        end = start;
    }
}

/// Whether the byte before `at` is a newline.
fn newline_before(file: &mut std::fs::File, at: u64) -> bool {
    let mut byte = [0];
    file.seek(SeekFrom::Start(at - 1)).is_ok()
        && file.read_exact(&mut byte).is_ok()
        && byte[0] == b'\n'
}

/// The task an agent was given: the first line of its first own message.
///
/// Headings and tags are skipped, so a brief that opens with `## Outcome`
/// gives the sentence under it. Only the first [`TITLE_BYTES`] are read.
pub fn title(agent: &str, path: &str) -> Option<String> {
    use std::io::BufRead;
    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file.take(TITLE_BYTES));
    for line in reader.split(b'\n').map_while(Result::ok) {
        let Ok(value) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        let Some((_, found)) = turn(agent, &value) else {
            continue;
        };
        if found.role != Role::User {
            continue;
        }
        if let Some(line) = title_line(&found.text) {
            return Some(short(&line, TITLE_CHARS));
        }
    }
    None
}

/// The line of a message that names its task.
///
/// A loaded skill (`<skill>` to `</skill>`) is not the task, and neither are
/// headings, tag lines, or the paths of attached files before the words. A
/// `Title:` label is dropped.
fn title_line(text: &str) -> Option<String> {
    let mut in_skill = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with("<skill") {
            in_skill = !line.contains("</skill>");
            continue;
        }
        if in_skill {
            in_skill = !line.contains("</skill>");
            continue;
        }
        if line.starts_with('#') || line.starts_with('<') {
            continue;
        }
        let words: Vec<&str> = line
            .split_whitespace()
            .skip_while(|word| word.starts_with('/') || word.starts_with("~/"))
            .collect();
        let line = words.join(" ");
        let line = line.strip_prefix("Title:").unwrap_or(&line).trim();
        if !line.is_empty() {
            return Some(line.to_owned());
        }
    }
    None
}

/// The most transcript bytes a title read takes.
const TITLE_BYTES: u64 = 4 * 1024 * 1024;

/// The longest title.
const TITLE_CHARS: usize = 80;

/// `text` cut to `max` characters, with an ellipsis when it was cut.
fn short(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

/// One turn of a conversation, with the id that groups Claude's blocks.
fn turn(agent: &str, value: &Value) -> Option<(Option<String>, Turn)> {
    match agent {
        "pi" => pi_turn(value),
        "claude" => claude_turn(value),
        "codex" => codex_turn(value),
        _ => None,
    }
}

/// A turn of a Pi transcript.
fn pi_turn(value: &Value) -> Option<(Option<String>, Turn)> {
    if value["type"] != "message" {
        return None;
    }
    let message = &value["message"];
    let role = match message["role"].as_str()? {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        _ => return None,
    };
    let text = joined(&text_blocks(&message["content"]))?;
    Some((None, Turn { role, text }))
}

/// A turn of a Claude transcript.
fn claude_turn(value: &Value) -> Option<(Option<String>, Turn)> {
    let role = match value["type"].as_str()? {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        _ => return None,
    };
    let message = &value["message"];
    let text = joined(&text_blocks(&message["content"]))?;
    let id = message["id"].as_str().map(str::to_owned);
    Some((id, Turn { role, text }))
}

/// A turn of a Codex transcript.
fn codex_turn(value: &Value) -> Option<(Option<String>, Turn)> {
    if value["type"] != "response_item" {
        return None;
    }
    let payload = &value["payload"];
    if payload["type"] != "message" {
        return None;
    }
    let role = match payload["role"].as_str()? {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        _ => return None,
    };
    let text = joined(&text_blocks(&payload["content"]))?;
    Some((None, Turn { role, text }))
}

/// The prompt a blocked agent shows on its screen, if it shows one.
///
/// A prompt is a question the owner must answer. Its choices are the
/// numbered or lettered lines at the bottom of the screen, and the question
/// is the line above them. Ordinary output is not a prompt: a question that
/// asks for no decision, such as a heading over a list of steps, is not one.
pub fn blocked_prompt(screen: &str) -> Option<BlockedPrompt> {
    let text = plain(screen);
    let lines: Vec<&str> = text.lines().collect();
    // The prompt sits at the bottom of the screen. Older lines are the agent's
    // own output, not the prompt.
    let start = lines.len().saturating_sub(SCREEN_LINES);
    let lines = &lines[start..];
    // The choices are the last lines of the screen.
    let mut options: Vec<PromptOption> = Vec::new();
    let mut above = lines.len();
    while above > 0 {
        let line = lines[above - 1].trim();
        if line.is_empty() {
            // A blank line between the question and the choices. A blank line
            // below the last choice is the end of the screen.
            if options.is_empty() {
                break;
            }
            above -= 1;
            continue;
        }
        let Some((key, label)) = option(line) else {
            break;
        };
        options.insert(0, PromptOption { key, label });
        above -= 1;
    }
    // The question is the last line above the choices.
    let question = lines[..above]
        .iter()
        .rev()
        .map(|line| line.trim())
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    if !asks_for_a_decision(question) {
        return None;
    }
    Some(BlockedPrompt {
        question: cut(question, MAX_QUESTION),
        options,
    })
}

/// The lines of a screen that can hold a prompt.
const SCREEN_LINES: usize = 60;

/// Whether a question asks the owner to decide something.
fn asks_for_a_decision(question: &str) -> bool {
    let question = question.to_lowercase();
    [
        "allow",
        "approve",
        "proceed",
        "permission",
        "confirm",
        "choose",
        "deny",
        "continue",
        "select",
        "want to",
        "would you like",
        "yes or no",
        "run this",
        "execute",
    ]
    .iter()
    .any(|word| question.contains(word))
}

/// One choice of a prompt, with the key that picks it.
///
/// A choice is a key, a separator and a label: `1. Yes`, `2) No`, `a) Ask
/// again`, or `(a) Ask again`. A bare word is not a choice.
fn option(line: &str) -> Option<(String, String)> {
    let line = marker_free(line.trim());
    // A bracketed key, as in `(a) Yes`.
    if let Some(rest) = line.strip_prefix('(')
        && let Some(end) = rest.find(')')
    {
        let key = rest[..end].trim();
        let label = rest[end + 1..].trim();
        return (key.len() == 1 && !label.is_empty()).then(|| (key.to_owned(), label.to_owned()));
    }
    let first = line.chars().next()?;
    let key = if first.is_ascii_digit() {
        let end = line
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(line.len());
        line[..end].to_owned()
    } else if first.is_ascii_alphabetic() {
        first.to_string()
    } else {
        return None;
    };
    // The separator comes before the label. Without one, the line is the
    // agent's own output.
    let rest = line[key.len()..].trim_start();
    let rest = rest.strip_prefix(['.', ')', ':', '-'])?;
    let label = rest.trim();
    (!label.is_empty()).then(|| (key, label.to_owned()))
}

/// A line without the marker that shows the selected choice.
fn marker_free(line: &str) -> &str {
    for marker in ['❯', '>', '»', '▸', '•'] {
        if let Some(rest) = line.strip_prefix(marker) {
            return rest.trim_start();
        }
    }
    line
}

/// A text of at most `limit` characters.
fn cut(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    text.chars().take(limit).collect()
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
    fn reply_is_the_newest_assistant_text_for_each_harness() {
        let pi_lines = [
            json!({"type":"message","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hm"},{"type":"text","text":"Done: 3 bugs."}]}}),
            json!({"type":"message","message":{"role":"toolResult","content":[{"type":"text","text":"output"}]}}),
            json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","name":"bash","arguments":{}}]}}),
        ];
        assert_eq!(reply("pi", &pi_lines).as_deref(), Some("Done: 3 bugs."));
        let codex_lines = [
            json!({"type":"response_item","payload":{"type":"function_call","name":"exec","arguments":"{}"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"All green."}]}}),
        ];
        assert_eq!(reply("codex", &codex_lines).as_deref(), Some("All green."));
        // Newest first: two blocks of message m2, then the user turn, then the older m1.
        let claude_lines = [
            json!({"type":"assistant","message":{"id":"m2","content":[{"type":"text","text":"second part"}]}}),
            json!({"type":"assistant","message":{"id":"m2","content":[{"type":"text","text":"first part"}]}}),
            json!({"type":"user","message":{"content":"next"}}),
            json!({"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"old"}]}}),
        ];
        assert_eq!(
            reply("claude", &claude_lines).as_deref(),
            Some("first part\n\nsecond part")
        );
        assert_eq!(reply("pi", &pi_lines[1..]), None);
        assert_eq!(reply("other", &pi_lines), None);
    }

    #[test]
    fn strips_ansi_and_control_strings() {
        assert_eq!(
            plain("\x1b[31mred\x1b[0m\x1b]0;title\x07\x1bPdata\x1b\\\u{9b}2J\x00\nnext"),
            "red\nnext"
        );
    }

    /// Write a transcript to a file in a temp dir, and return its path.
    fn transcript(lines: &[serde_json::Value]) -> String {
        let path = std::env::temp_dir().join(format!("activity-{}.jsonl", crate::ledger::new_id()));
        let mut text = String::new();
        for line in lines {
            text.push_str(&line.to_string());
            text.push('\n');
        }
        std::fs::write(&path, text).expect("the transcript");
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn the_conversation_reads_the_turns_of_a_transcript_oldest_first() {
        let path = transcript(&[
            json!({"type":"message","message":{"role":"user","content":[{"type":"text","text":"fix the bug"}]}}),
            json!({"type":"message","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hm"},{"type":"text","text":"on it"}]}}),
            json!({"type":"message","message":{"role":"toolResult","content":[{"type":"text","text":"output"}]}}),
            json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","name":"bash","arguments":{}}]}}),
            json!({"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"fixed"}]}}),
        ]);
        let turns = conversation("pi", &path);
        assert_eq!(
            turns,
            vec![
                Turn {
                    role: Role::User,
                    text: "fix the bug".into()
                },
                Turn {
                    role: Role::Assistant,
                    text: "on it".into()
                },
                Turn {
                    role: Role::Assistant,
                    text: "fixed".into()
                },
            ]
        );
    }

    #[test]
    fn the_conversation_keeps_the_blocks_of_one_claude_message_together() {
        let path = transcript(&[
            json!({"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"first"}]}}),
            json!({"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"second"}]}}),
            json!({"type":"user","message":{"content":"next"}}),
        ]);
        let turns = conversation("claude", &path);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].role, Role::Assistant);
        assert_eq!(turns[0].text, "first\n\nsecond");
        assert_eq!(turns[1].role, Role::User);
        assert_eq!(turns[1].text, "next");
    }

    #[test]
    fn the_conversation_of_a_codex_transcript_holds_both_roles() {
        let path = transcript(&[
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"go"}]}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}}),
        ]);
        let turns = conversation("codex", &path);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].role, Role::User);
        assert_eq!(turns[1].role, Role::Assistant);
        assert_eq!(turns[1].text, "done");
    }

    #[test]
    fn the_conversation_reaches_turns_before_megabytes_of_tool_output() {
        let big = "x".repeat(64 * 1024);
        let mut lines = vec![
            json!({"type":"message","message":{"role":"user","content":[{"type":"text","text":"check the portal"}]}}),
            json!({"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"looking"}]}}),
        ];
        for _ in 0..40 {
            lines.push(json!({"type":"message","message":{"role":"toolResult","content":[{"type":"text","text":big}]}}));
        }
        let path = transcript(&lines);
        assert!(std::fs::metadata(&path).expect("size").len() > 2 * 1024 * 1024);
        let turns = conversation("pi", &path);
        let texts: Vec<_> = turns.iter().map(|turn| turn.text.as_str()).collect();
        assert_eq!(texts, ["check the portal", "looking"]);
    }

    #[test]
    fn backward_lines_join_lines_that_cross_a_chunk_boundary() {
        let path = std::env::temp_dir().join(format!("lines-{}.txt", crate::ledger::new_id()));
        std::fs::write(&path, "first line\nsecond much longer line\nthird\n").expect("file");
        let path = path.to_string_lossy().into_owned();
        for step in [1, 3, 7, 64] {
            let mut seen = Vec::new();
            lines_backwards(&path, u64::MAX, step, |line| {
                seen.push(line.to_owned());
                true
            });
            assert_eq!(
                seen,
                ["third", "second much longer line", "first line"],
                "step {step}"
            );
        }
        let mut seen = Vec::new();
        lines_backwards(&path, 30, 7, |line| {
            seen.push(line.to_owned());
            true
        });
        assert_eq!(
            seen,
            ["third", "second much longer line"],
            "a line that starts at the limit is whole"
        );
        let mut seen = Vec::new();
        lines_backwards(&path, 20, 7, |line| {
            seen.push(line.to_owned());
            true
        });
        assert_eq!(seen, ["third"], "a line cut by the limit is not given");
    }

    #[test]
    fn the_title_is_the_first_line_of_the_first_owner_message() {
        let path = transcript(&[
            json!({"type":"session","id":"s"}),
            json!({"type":"message","message":{"role":"user","content":[{"type":"text","text":"## Outcome\nShip the portal redesign\nmore"}]}}),
            json!({"type":"message","message":{"role":"user","content":[{"type":"text","text":"later"}]}}),
        ]);
        assert_eq!(
            title("pi", &path).as_deref(),
            Some("Ship the portal redesign")
        );
        let long = transcript(&[
            json!({"type":"message","message":{"role":"user","content":"y".repeat(200)}}),
        ]);
        assert_eq!(
            title("pi", &long).map(|t| t.chars().count()),
            Some(TITLE_CHARS)
        );
        assert_eq!(title("pi", "/nonexistent"), None);
        let skill = transcript(&[
            json!({"type":"message","message":{"role":"user","content":"<skill name=\"x\">\nReferences are relative to /s.\n</skill>\n\n/tmp/shot.png redesign the portal"}}),
        ]);
        assert_eq!(title("pi", &skill).as_deref(), Some("redesign the portal"));
        let brief = transcript(&[
            json!({"type":"message","message":{"role":"user","content":"<file name=\"/t.md\">\nTitle: Compare layouts\nDeliverable: x"}}),
        ]);
        assert_eq!(title("pi", &brief).as_deref(), Some("Compare layouts"));
    }

    #[test]
    fn the_conversation_of_an_unknown_harness_or_a_missing_file_is_empty() {
        let path = transcript(&[json!({"type":"message","message":{"role":"user","content":"x"}})]);
        assert_eq!(conversation("other", &path), Vec::new());
        assert_eq!(
            conversation("pi", "/nonexistent/transcript.jsonl"),
            Vec::new()
        );
    }

    #[test]
    fn a_blocked_screen_shows_its_question_and_its_choices() {
        let screen = "\
I want to run a command.
Allow the command?
❯ 1. Yes
  2. No
  3. Ask again
";
        let prompt = blocked_prompt(screen).expect("a prompt");
        assert_eq!(prompt.question, "Allow the command?");
        assert_eq!(prompt.options.len(), 3);
        assert_eq!(prompt.options[0].key, "1");
        assert_eq!(prompt.options[0].label, "Yes");
        assert_eq!(prompt.options[1].key, "2");
        assert_eq!(prompt.options[2].label, "Ask again");
    }

    #[test]
    fn a_blocked_screen_without_choices_still_shows_its_question() {
        let prompt = blocked_prompt("Do you want to continue?").expect("a prompt");
        assert_eq!(prompt.question, "Do you want to continue?");
        assert_eq!(prompt.options, Vec::new());
    }

    #[test]
    fn ordinary_output_is_not_a_prompt() {
        assert!(blocked_prompt("cargo test\ntest result: ok").is_none());
        assert!(blocked_prompt("").is_none());
        assert!(blocked_prompt("What is the next step?").is_none());
    }

    #[test]
    fn a_prompt_is_read_from_the_bottom_of_a_long_screen() {
        let mut screen = "old line\n".repeat(200);
        screen.push_str("Allow this command?\n1. Yes\n2. No\n");
        let prompt = blocked_prompt(&screen).expect("a prompt");
        assert_eq!(prompt.question, "Allow this command?");
        assert_eq!(prompt.options.len(), 2);
    }

    #[test]
    fn a_prompt_with_a_colon_and_lettered_choices_is_read() {
        let screen = "\
Do you want to proceed?
❯ (a) Yes
  (b) No
";
        let prompt = blocked_prompt(screen).expect("a prompt");
        assert_eq!(prompt.question, "Do you want to proceed?");
        assert_eq!(prompt.options.len(), 2);
        assert_eq!(prompt.options[0].key, "a");
        assert_eq!(prompt.options[0].label, "Yes");
    }

    #[test]
    fn a_prompt_with_a_heading_over_its_choices_is_read() {
        let screen = "\
Allow this command?
1. Yes
2. No
";
        let prompt = blocked_prompt(screen).expect("a prompt");
        assert_eq!(prompt.question, "Allow this command?");
        assert_eq!(prompt.options.len(), 2);
    }
}
