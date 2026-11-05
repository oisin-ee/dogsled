//! Bounded, display-only transcript activity. Unknown formats fall back to pane text.
use portal_types::agents::{
    BlockedPrompt, PromptOption, Role, Source, TimeMeaning, Timestamp, ToolStatus, ToolSummary,
    Turn,
};
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

/// A bounded transcript snapshot. This is history, not evidence of live activity.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct History {
    pub turns: Vec<Turn>,
    pub truncated: bool,
}

#[cfg(test)]
fn conversation(agent: &str, path: &str) -> History {
    try_conversation(agent, path).unwrap_or_default()
}

/// Detail publication preserves I/O failures instead of publishing empty success.
pub(crate) fn try_conversation(agent: &str, path: &str) -> std::io::Result<History> {
    conversation_with_limit(agent, path, CONVERSATION_BYTES)
}

/// Read newest text and tool records independently, then restore transcript order.
/// Claude's adjacent message blocks remain one text record, even across tool rows.
pub(crate) fn conversation_with_limit(
    agent: &str,
    path: &str,
    limit: u64,
) -> std::io::Result<History> {
    let mut history = History::default();
    let mut texts = 0;
    let mut tools = 0;
    let mut deliveries = std::collections::HashSet::new();
    let cut = lines_backwards(path, limit, CHUNK_BYTES, |offset, line| {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return true;
        };
        for mut record in records(agent, &value, offset).into_iter().rev() {
            if let Source::Dogsled { .. } = &record.source
                && !deliveries.insert(record.id.clone())
            {
                continue;
            }
            if let Some(tool) = &mut record.tool {
                if let Some(index) = history.turns.iter().position(|previous| {
                    previous
                        .tool
                        .as_ref()
                        .is_some_and(|old| old.call_id == tool.call_id)
                }) {
                    let previous = history.turns.remove(index);
                    if let Some(result) = previous.tool {
                        if result.status != ToolStatus::Unknown {
                            tool.status = result.status;
                        }
                        if tool.name.is_none() {
                            tool.name = result.name;
                        }
                    }
                } else if tools == CONVERSATION_TOOLS {
                    history.truncated = true;
                    continue;
                } else {
                    tools += 1;
                }
            } else {
                let group = (agent == "claude")
                    .then(|| history.turns.iter().rposition(|t| t.tool.is_none()))
                    .flatten()
                    .filter(|&index| {
                        history.turns.get(index).is_some_and(|previous| {
                            previous.id == record.id
                                && previous.role == record.role
                                && previous.source == record.source
                        })
                    });
                if let Some(index) = group {
                    let previous = history.turns.remove(index);
                    record.timestamp = record.timestamp.or(previous.timestamp);
                    record.text = joined(&[record.text, previous.text]).unwrap_or_default();
                } else {
                    if texts == CONVERSATION_TURNS {
                        history.truncated = true;
                        return false;
                    }
                    texts += 1;
                }
            }
            history.turns.push(record);
        }
        true
    });
    history.truncated |= cut?;
    history.turns.reverse();
    Ok(history)
}

/// Tool rows cannot consume the text-message allowance.
const CONVERSATION_TOOLS: usize = 400;

/// The most text turns a conversation returns.
const CONVERSATION_TURNS: usize = 200;

/// The most transcript bytes a conversation reads.
pub(crate) const CONVERSATION_BYTES: u64 = 32 * 1024 * 1024;

/// The size of one backward read.
const CHUNK_BYTES: u64 = 1024 * 1024;

/// Give the complete lines of `path` to `visit`, newest first, until `visit`
/// returns `false` or `limit` bytes are read, `step` bytes at a time. A line
/// cut by the limit is not given.
fn lines_backwards(
    path: &str,
    limit: u64,
    step: u64,
    mut visit: impl FnMut(u64, &str) -> bool,
) -> std::io::Result<bool> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let floor = len.saturating_sub(limit);
    let mut end = len;
    // The bytes after this chunk that belong to a line starting in it.
    let mut carry: Vec<u8> = Vec::new();
    while end > floor {
        let start = end.saturating_sub(step).max(floor);
        let size = usize::try_from(end - start).map_err(std::io::Error::other)?;
        let mut bytes = vec![0; size];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut bytes)?;
        bytes.extend_from_slice(&carry);
        let chunk = bytes;
        // The first line is complete at the start of the file, or at the read
        // limit when a newline comes just before it. Elsewhere the bytes before
        // the first newline end a line that starts earlier.
        let whole = start == 0 || (start == floor && newline_before(&mut file, start)?);
        let (head, lines) = if whole {
            (&[][..], &chunk[..])
        } else {
            match chunk.iter().position(|byte| *byte == b'\n') {
                Some(index) => (&chunk[..index], &chunk[index + 1..]),
                None => (&chunk[..], &[][..]),
            }
        };
        let mut offset = start + u64::try_from(chunk.len()).map_err(std::io::Error::other)?;
        for line in lines.rsplit(|byte| *byte == b'\n') {
            offset =
                offset.saturating_sub(u64::try_from(line.len()).map_err(std::io::Error::other)?);
            if !line.is_empty() && !visit(offset, &String::from_utf8_lossy(line)) {
                return Ok(true);
            }
            offset = offset.saturating_sub(1);
        }
        carry = head.to_vec();
        end = start;
    }
    Ok(floor > 0)
}

/// Whether the byte before `at` is a newline.
fn newline_before(file: &mut std::fs::File, at: u64) -> std::io::Result<bool> {
    let mut byte = [0];
    file.seek(SeekFrom::Start(at - 1))?;
    file.read_exact(&mut byte)?;
    Ok(byte[0] == b'\n')
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
        let Some(found) = turn(agent, &value, 0) else {
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

/// Text content and metadata for one supported harness message.
fn turn(agent: &str, value: &Value, offset: u64) -> Option<Turn> {
    let (role, message) = match agent {
        "pi" if value["type"] == "message" => {
            (value["message"]["role"].as_str()?, &value["message"])
        }
        "claude" => (value["type"].as_str()?, &value["message"]),
        "codex" if value["type"] == "response_item" && value["payload"]["type"] == "message" => {
            (value["payload"]["role"].as_str()?, &value["payload"])
        }
        _ => return None,
    };
    let role = match role {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        _ => return None,
    };
    let text = joined(&text_blocks(&message["content"]))?;
    let record = Turn {
        role,
        text,
        ..envelope(agent, value, offset)
    };
    Some(record)
}

fn string(value: &Value) -> Option<String> {
    value.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

fn milliseconds(value: &Value, meaning: TimeMeaning) -> Option<Timestamp> {
    let ms = value.as_u64()?;
    chrono::DateTime::from_timestamp_millis(i64::try_from(ms).ok()?)?;
    Some(Timestamp { ms, meaning })
}

fn recorded_time(value: &Value) -> Option<Timestamp> {
    let time = chrono::DateTime::parse_from_rfc3339(value.as_str()?).ok()?;
    Some(Timestamp {
        ms: u64::try_from(time.timestamp_millis()).ok()?,
        meaning: TimeMeaning::Recorded,
    })
}

/// Native identity wins; the absolute byte offset remains stable as the tail grows.
fn envelope(agent: &str, value: &Value, offset: u64) -> Turn {
    let message = if agent == "codex" {
        &value["payload"]
    } else {
        &value["message"]
    };
    let id = match agent {
        "pi" => string(&value["id"]).or_else(|| string(&message["id"])),
        "claude" => string(&message["id"]).or_else(|| string(&value["uuid"])),
        _ => string(&message["id"]).or_else(|| string(&value["id"])),
    }
    .unwrap_or_else(|| format!("{agent}:offset:{offset}"));
    let timestamp = (agent == "pi")
        .then(|| milliseconds(&message["timestamp"], TimeMeaning::Message))
        .flatten()
        .or_else(|| recorded_time(&value["timestamp"]));
    let assistant =
        message["role"] == "assistant" || (agent == "claude" && value["type"] == "assistant");
    Turn {
        id,
        role: Role::Context,
        text: String::new(),
        source: Source::Harness {
            harness: agent.to_owned(),
            model: assistant.then(|| string(&message["model"])).flatten(),
            provider: assistant.then(|| string(&message["provider"])).flatten(),
        },
        timestamp,
        tool: None,
    }
}

/// Displayed extension messages are context, never messages from the owner.
fn custom_records(value: &Value, offset: u64) -> Vec<Turn> {
    if value["display"] != true {
        return Vec::new();
    }
    let Some(custom_type) = string(&value["customType"]) else {
        return Vec::new();
    };
    if custom_type == "dogsled"
        && let Some(records) = value["details"]["records"].as_array()
    {
        return records
            .iter()
            .filter_map(|record| {
                let id = string(&record["id"])?;
                let sender = string(&record["sender"])?;
                let delivery_kind = string(&record["kind"])?;
                let text = record["text"].as_str()?.to_owned();
                Some(Turn {
                    id,
                    role: Role::Context,
                    text,
                    source: Source::Dogsled {
                        sender,
                        delivery_kind,
                    },
                    timestamp: milliseconds(&record["created_ms"], TimeMeaning::Created),
                    tool: None,
                })
            })
            .collect();
    }
    joined(&text_blocks(&value["content"]))
        .map(|text| Turn {
            text,
            source: Source::Custom { custom_type },
            ..envelope("pi", value, offset)
        })
        .into_iter()
        .collect()
}

/// Convert only supported call/result metadata. No arguments or result content leave here.
fn tool_record(agent: &str, value: &Value, block: &Value, offset: u64) -> Option<Turn> {
    let (call_id, name, status) = match (agent, block["type"].as_str()) {
        ("pi", Some("toolCall")) | ("claude", Some("tool_use")) => (
            string(&block["id"])?,
            string(&block["name"]),
            ToolStatus::Unknown,
        ),
        ("pi", _) if block["role"] == "toolResult" => (
            string(&block["toolCallId"])?,
            string(&block["toolName"]),
            result_status(&block["isError"]),
        ),
        ("claude", Some("tool_result")) => (
            string(&block["tool_use_id"])?,
            None,
            result_status(&block["is_error"]),
        ),
        ("codex", Some("function_call")) => (
            string(&block["call_id"])?,
            string(&block["name"]),
            ToolStatus::Unknown,
        ),
        ("codex", Some("function_call_output")) => {
            (string(&block["call_id"])?, None, ToolStatus::Completed)
        }
        _ => return None,
    };
    let mut record = envelope(agent, value, offset);
    record.id = format!("{}:tool:{call_id}", record.id);
    record.role = Role::Tool;
    record.tool = Some(ToolSummary {
        call_id,
        name: name.map(|name| short(&plain(&name), 120)),
        status,
    });
    Some(record)
}

fn result_status(error: &Value) -> ToolStatus {
    if error == true {
        ToolStatus::Error
    } else {
        ToolStatus::Completed
    }
}

fn records(agent: &str, value: &Value, offset: u64) -> Vec<Turn> {
    if agent == "pi" && value["type"] == "custom_message" {
        return custom_records(value, offset);
    }
    let message = match agent {
        "pi" if value["type"] == "message" => &value["message"],
        "claude" if matches!(value["type"].as_str(), Some("user" | "assistant")) => {
            &value["message"]
        }
        "codex" if value["type"] == "response_item" => &value["payload"],
        _ => return Vec::new(),
    };
    let mut text = turn(agent, value, offset);
    let mut found = Vec::new();
    if let Some(blocks) = message["content"].as_array() {
        for block in blocks {
            if matches!(
                block["type"].as_str(),
                Some("text" | "input_text" | "output_text")
            ) && let Some(turn) = text.take()
            {
                found.push(turn);
            }
            if (agent != "pi" || message["role"] == "assistant")
                && let Some(tool) = tool_record(agent, value, block, offset)
            {
                found.push(tool);
            }
        }
    }
    if let Some(text) = text {
        found.push(text);
    }
    if let Some(tool) = tool_record(agent, value, message, offset) {
        found.push(tool);
    }
    found
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
    use std::io::Write as _;
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
        let turns = conversation("pi", &path).turns;
        assert_eq!(
            turns
                .iter()
                .map(|turn| (turn.role, turn.text.as_str()))
                .collect::<Vec<_>>(),
            [
                (Role::User, "fix the bug"),
                (Role::Assistant, "on it"),
                (Role::Assistant, "fixed")
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
        let turns = conversation("claude", &path).turns;
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
        let turns = conversation("codex", &path).turns;
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
        let turns = conversation("pi", &path).turns;
        let texts: Vec<_> = turns.iter().map(|turn| turn.text.as_str()).collect();
        assert_eq!(texts, ["check the portal", "looking"]);
    }

    #[test]
    fn pi_preserves_native_identity_message_time_and_per_message_models() {
        let path = transcript(&[
            json!({"type":"message","id":"owner","timestamp":"2026-11-04T12:00:00Z","message":{"role":"user","timestamp":42,"content":"request"}}),
            json!({"type":"message","id":"first","message":{"role":"assistant","timestamp":43,"model":"model-a","provider":"provider-a","content":[{"type":"text","text":"first"}]}}),
            json!({"type":"model_change","modelId":"not-message-provenance","provider":"other"}),
            json!({"type":"message","id":"second","message":{"role":"assistant","timestamp":44,"model":"model-b","provider":"provider-b","content":[{"type":"text","text":"second"}]}}),
            json!({"type":"message","id":"unknown","timestamp":"invalid","message":{"role":"assistant","timestamp":-1,"content":[{"type":"text","text":"unknown"}]}}),
            json!({"type":"message","id":"recorded","timestamp":"1970-01-01T00:00:01.234Z","message":{"role":"assistant","content":"recorded"}}),
        ]);
        let history = conversation("pi", &path);
        assert!(!history.truncated);
        let turns = history.turns;
        assert_eq!(turns[0].id, "owner");
        assert_eq!(
            turns[0].timestamp,
            Some(Timestamp {
                ms: 42,
                meaning: TimeMeaning::Message
            })
        );
        assert_eq!(
            turns[1].source,
            Source::Harness {
                harness: "pi".into(),
                model: Some("model-a".into()),
                provider: Some("provider-a".into())
            }
        );
        assert_eq!(
            turns[2].source,
            Source::Harness {
                harness: "pi".into(),
                model: Some("model-b".into()),
                provider: Some("provider-b".into())
            }
        );
        assert_eq!(turns[3].timestamp, None);
        assert_eq!(
            turns[3].source,
            Source::Harness {
                harness: "pi".into(),
                model: None,
                provider: None
            }
        );
        assert_eq!(
            turns[4].timestamp,
            Some(Timestamp {
                ms: 1234,
                meaning: TimeMeaning::Recorded
            })
        );
        assert_eq!(milliseconds(&json!(u64::MAX), TimeMeaning::Message), None);
        assert_eq!(milliseconds(&json!(1.5), TimeMeaning::Message), None);
        assert_eq!(recorded_time(&json!("before time")), None);
    }

    #[test]
    fn claude_and_codex_keep_envelope_time_without_inventing_model_metadata() {
        for (agent, lines) in [
            (
                "claude",
                vec![
                    json!({"type":"user","uuid":"native-user","timestamp":"1970-01-01T00:00:03+00:00","message":{"content":"request"}}),
                    json!({"type":"assistant","uuid":"block-a","timestamp":"1970-01-01T00:00:02Z","message":{"id":"native-assistant","model":"claude-recorded","content":[{"type":"text","text":"part one"}]}}),
                    json!({"type":"assistant","uuid":"block-b","timestamp":"1970-01-01T00:00:04Z","message":{"id":"native-assistant","model":"claude-recorded","content":[{"type":"text","text":"part two"}]}}),
                ],
            ),
            (
                "codex",
                vec![
                    json!({"type":"response_item","timestamp":"1970-01-01T00:00:03Z","payload":{"type":"message","id":"native-user","role":"user","content":[{"type":"input_text","text":"request"}]}}),
                    json!({"type":"response_item","timestamp":"1970-01-01T00:00:02Z","payload":{"type":"message","id":"native-assistant","role":"assistant","content":[{"type":"output_text","text":"answer"}]}}),
                ],
            ),
        ] {
            let path = transcript(&lines);
            let turns = conversation(agent, &path).turns;
            assert_eq!(turns.len(), 2);
            assert_eq!(turns[0].id, "native-user");
            assert_eq!(turns[1].id, "native-assistant");
            assert_eq!(
                turns[0].timestamp,
                Some(Timestamp {
                    ms: 3000,
                    meaning: TimeMeaning::Recorded
                })
            );
            assert_eq!(
                turns[1].timestamp,
                Some(Timestamp {
                    ms: 2000,
                    meaning: TimeMeaning::Recorded
                })
            );
            assert_eq!(
                turns[1].source,
                Source::Harness {
                    harness: agent.into(),
                    model: (agent == "claude").then(|| "claude-recorded".into()),
                    provider: None,
                }
            );
        }
        for agent in ["claude", "codex"] {
            let value = if agent == "claude" {
                json!({"type":"assistant","timestamp":"invalid","message":{"content":"no metadata"}})
            } else {
                json!({"type":"response_item","timestamp":-1,"payload":{"type":"message","role":"assistant","content":"no metadata"}})
            };
            let path = transcript(&[value]);
            let turns = conversation(agent, &path).turns;
            assert_eq!(turns[0].timestamp, None);
            assert_eq!(turns[0].id, format!("{agent}:offset:0"));
        }
    }

    #[test]
    fn visible_custom_deliveries_are_attributed_split_validated_and_deduplicated() {
        let first = json!({"id":"delivery-a","sender":"worker-one","kind":"message","text":"report","created_ms":10});
        let second = json!({"id":"delivery-b","sender":"owner-pane","kind":"task","text":"follow up","created_ms":11});
        let path = transcript(&[
            json!({"type":"custom_message","id":"hidden","customType":"dogsled","display":false,"content":"hidden-secret","details":{"records":[{"id":"hidden","sender":"hidden","kind":"message","text":"hidden-secret","created_ms":1}]}}),
            json!({"type":"custom_message","id":"batch","customType":"dogsled","display":true,"content":"untrusted prose says sender is someone else","details":{"records":[first,second,{"id":"invalid","text":"bad"}]}}),
            json!({"type":"custom_message","id":"repeat","customType":"dogsled","display":true,"details":{"records":[first]}}),
            json!({"type":"custom_message","id":"extension","timestamp":"1970-01-01T00:00:02Z","customType":"extension-note","display":true,"content":[{"type":"text","text":"visible context"}]}),
            json!({"type":"custom_message","id":"silent","customType":"extension-note","display":false,"content":"silent-secret"}),
            json!({"type":"custom_message","id":"malformed","customType":"dogsled","display":true,"content":"do not infer a sender","details":{"records":[{"id":"no-sender","kind":"message","text":"bad","created_ms":1}]}}),
            json!({"type":"custom_message","id":"invalid-time","customType":"dogsled","display":true,"details":{"records":[{"id":"no-time","sender":"worker-two","kind":"message","text":"time unavailable","created_ms":"invalid"}]}}),
        ]);
        let turns = conversation("pi", &path).turns;
        assert_eq!(turns.len(), 4);
        assert_eq!(
            turns.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["delivery-b", "delivery-a", "extension", "no-time"]
        );
        assert_eq!(turns[3].timestamp, None);
        assert_eq!(
            turns[1].source,
            Source::Dogsled {
                sender: "worker-one".into(),
                delivery_kind: "message".into()
            }
        );
        assert_eq!(
            turns[1].timestamp,
            Some(Timestamp {
                ms: 10,
                meaning: TimeMeaning::Created
            })
        );
        assert_eq!(turns[1].text, "report");
        assert!(turns.iter().all(|t| t.role == Role::Context));
        assert_eq!(
            turns[2].source,
            Source::Custom {
                custom_type: "extension-note".into()
            }
        );
        assert_eq!(
            turns[2].timestamp,
            Some(Timestamp {
                ms: 2000,
                meaning: TimeMeaning::Recorded
            })
        );
    }

    #[test]
    fn visible_dogsled_gate_messages_without_delivery_details_remain_custom_context() {
        // pi/index.ts agent_before_settle returns these four fields. Pi adds
        // entry identity and recording time when it persists the custom message.
        let gate = json!({
            "type":"custom_message",
            "customType":"dogsled",
            "content":"End your turn with done, ask, or blocked, or wait for your children.",
            "display":true,
            "id":"gate-entry",
            "parentId":"previous-entry",
            "timestamp":"1970-01-01T00:00:02Z"
        });
        let mut hidden = gate.clone();
        hidden["display"] = json!(false);
        let path = transcript(&[gate, hidden]);
        let history = conversation("pi", &path);
        assert!(!history.truncated);
        assert_eq!(history.turns.len(), 1);
        let record = &history.turns[0];
        assert_eq!(record.id, "gate-entry");
        assert_eq!(record.role, Role::Context);
        assert_eq!(
            record.source,
            Source::Custom {
                custom_type: "dogsled".into()
            }
        );
        assert_eq!(
            record.timestamp,
            Some(Timestamp {
                ms: 2000,
                meaning: TimeMeaning::Recorded
            })
        );
        assert_eq!(
            record.text,
            "End your turn with done, ask, or blocked, or wait for your children."
        );
        assert_eq!(record.tool, None);
    }

    #[test]
    fn recorded_tools_correlate_results_without_exposing_payloads_or_claiming_live_work() {
        for (agent, lines) in [
            (
                "pi",
                vec![
                    json!({"type":"message","id":"call-entry","message":{"role":"assistant","timestamp":10,"content":[{"type":"text","text":"checking"},{"type":"toolCall","id":"a","name":"read","arguments":{"secret":"raw-arguments"}},{"type":"thinking","thinking":"raw-reasoning"}]}}),
                    json!({"type":"message","message":{"role":"toolResult","toolCallId":"a","toolName":"read","isError":true,"content":[{"type":"text","text":"raw-output"}]}}),
                    json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","id":"b","name":"write","arguments":{}}]}}),
                ],
            ),
            (
                "claude",
                vec![
                    json!({"type":"assistant","message":{"id":"call-entry","content":[{"type":"text","text":"checking"},{"type":"tool_use","id":"a","name":"read","input":{"secret":"raw-arguments"}},{"type":"thinking","thinking":"raw-reasoning"}]}}),
                    json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"a","is_error":true,"content":"raw-output"}]}}),
                    json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"b","name":"write","input":{}}]}}),
                ],
            ),
            (
                "codex",
                vec![
                    json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"checking"}]}}),
                    json!({"type":"response_item","payload":{"id":"call-entry","type":"function_call","call_id":"a","name":"read","arguments":"raw-arguments"}}),
                    json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"a","output":"raw-output"}}),
                    json!({"type":"response_item","payload":{"type":"reasoning","summary":"raw-reasoning"}}),
                    json!({"type":"response_item","payload":{"type":"function_call","call_id":"b","name":"write","arguments":"{}"}}),
                ],
            ),
        ] {
            let path = transcript(&lines);
            let history = conversation(agent, &path);
            let turns = history.turns;
            assert_eq!(turns.len(), 3, "{agent}");
            assert_eq!(turns[0].text, "checking");
            let summary = turns[1].tool.as_ref().expect("tool summary");
            assert_eq!(summary.call_id, "a");
            assert_eq!(summary.name.as_deref(), Some("read"));
            assert_eq!(
                summary.status,
                if agent == "codex" {
                    ToolStatus::Completed
                } else {
                    ToolStatus::Error
                }
            );
            assert!(turns[1].id.starts_with("call-entry:tool:"));
            assert_eq!(
                turns[2].tool.as_ref().expect("unmatched call").status,
                ToolStatus::Unknown
            );
            assert!(
                turns[1..]
                    .iter()
                    .all(|turn| turn.role == Role::Tool && turn.text.is_empty())
            );
            let wire = serde_json::to_string(&turns).expect("wire");
            for secret in ["raw-arguments", "raw-output", "raw-reasoning"] {
                assert!(!wire.contains(secret), "{agent} leaked {secret}");
            }
        }
    }

    #[test]
    fn unmatched_results_preserve_only_known_metadata() {
        for (agent, line, name, status) in [
            (
                "pi",
                json!({"type":"message","id":"result","message":{"role":"toolResult","toolCallId":"orphan","toolName":"read","isError":false,"timestamp":5,"content":"private-output"}}),
                Some("read"),
                ToolStatus::Completed,
            ),
            (
                "claude",
                json!({"type":"user","uuid":"result","timestamp":"1970-01-01T00:00:00.005Z","message":{"content":[{"type":"tool_result","tool_use_id":"orphan","is_error":true,"content":"private-output"}]}}),
                None,
                ToolStatus::Error,
            ),
            (
                "codex",
                json!({"type":"response_item","id":"result","timestamp":"1970-01-01T00:00:00.005Z","payload":{"type":"function_call_output","call_id":"orphan","output":"private-output"}}),
                None,
                ToolStatus::Completed,
            ),
        ] {
            let path = transcript(&[line]);
            let history = conversation(agent, &path);
            assert_eq!(history.turns.len(), 1);
            let record = &history.turns[0];
            let tool = record.tool.as_ref().expect("orphan result");
            assert_eq!(tool.name.as_deref(), name);
            assert_eq!(tool.status, status);
            assert_eq!(record.id, "result:tool:orphan");
            assert_eq!(record.timestamp.as_ref().map(|time| time.ms), Some(5));
            assert_eq!(record.text, "");
        }
    }

    #[test]
    fn claude_grouping_survives_tool_rows_and_stays_at_the_first_text_block() {
        let path = transcript(&[
            json!({"type":"assistant","message":{"id":"same","content":[{"type":"text","text":"first"}]}}),
            json!({"type":"assistant","message":{"id":"same","content":[{"type":"tool_use","id":"a","name":"read","input":{}}]}}),
            json!({"type":"assistant","timestamp":"1970-01-01T00:00:00.005Z","message":{"id":"same","content":[{"type":"text","text":"second"}]}}),
        ]);
        let turns = conversation("claude", &path).turns;
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].id, "same");
        assert_eq!(turns[0].text, "first\n\nsecond");
        assert_eq!(
            turns[0].timestamp,
            Some(Timestamp {
                ms: 5,
                meaning: TimeMeaning::Recorded
            })
        );
        assert_eq!(turns[1].role, Role::Tool);
    }

    #[test]
    fn tools_have_an_independent_bound_and_cannot_evict_useful_text() {
        let mut lines = vec![
            json!({"type":"message","id":"useful","message":{"role":"user","content":"useful text"}}),
        ];
        for index in 0..CONVERSATION_TOOLS + 5 {
            lines.push(json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","id":format!("call-{index}"),"name":"read","arguments":{}}]}}));
            lines.push(json!({"type":"message","message":{"role":"toolResult","toolCallId":format!("call-{index}"),"toolName":"read","isError":false,"content":[{"type":"text","text":"x".repeat(8192)}]}}));
        }
        let path = transcript(&lines);
        let history = conversation("pi", &path);
        assert!(history.truncated);
        assert_eq!(history.turns.len(), CONVERSATION_TOOLS + 1);
        assert_eq!(history.turns[0].text, "useful text");
        assert!(
            history.turns[1..]
                .iter()
                .all(|t| t.tool.as_ref().expect("tool").status == ToolStatus::Completed)
        );
    }

    #[test]
    fn text_bound_and_absolute_offset_identity_are_stable_across_appends() {
        let lines: Vec<_> = (0..=CONVERSATION_TURNS)
            .map(|index| json!({"type":"message","message":{"role":"user","content":format!("message {index}")}}))
            .collect();
        let path = transcript(&lines);
        let before = conversation("pi", &path);
        assert!(before.truncated);
        assert_eq!(before.turns.len(), CONVERSATION_TURNS);
        assert_eq!(before.turns[0].text, "message 1");
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("file"),
            "{}",
            json!({"type":"message","message":{"role":"assistant","content":"appended"}})
        )
        .expect("append");
        let after = conversation("pi", &path);
        assert_eq!(before.turns[1].id, after.turns[0].id);
        assert_eq!(before.turns[199].id, after.turns[198].id);
    }

    #[test]
    fn byte_limit_exposes_incomplete_history_without_reading_a_cut_tool_record() {
        let path = transcript(&[
            json!({"type":"message","message":{"role":"user","content":"outside read bound"}}),
            json!({"type":"message","message":{"role":"toolResult","toolCallId":"huge","toolName":"read","isError":false,"content":[{"type":"text","text":"x".repeat(usize::try_from(CONVERSATION_BYTES).expect("limit"))}]}}),
        ]);
        let history = conversation("pi", &path);
        assert!(history.truncated);
        assert_eq!(history.turns, Vec::new());
    }

    #[test]
    fn backward_lines_join_lines_that_cross_a_chunk_boundary() {
        let path = std::env::temp_dir().join(format!("lines-{}.txt", crate::ledger::new_id()));
        std::fs::write(&path, "first line\nsecond much longer line\nthird\n").expect("file");
        let path = path.to_string_lossy().into_owned();
        for step in [1, 3, 7, 64] {
            let mut seen = Vec::new();
            lines_backwards(&path, u64::MAX, step, |_, line| {
                seen.push(line.to_owned());
                true
            })
            .expect("transcript read");
            assert_eq!(
                seen,
                ["third", "second much longer line", "first line"],
                "step {step}"
            );
        }
        let mut seen = Vec::new();
        lines_backwards(&path, 30, 7, |_, line| {
            seen.push(line.to_owned());
            true
        })
        .expect("transcript read");
        assert_eq!(
            seen,
            ["third", "second much longer line"],
            "a line that starts at the limit is whole"
        );
        let mut seen = Vec::new();
        lines_backwards(&path, 20, 7, |_, line| {
            seen.push(line.to_owned());
            true
        })
        .expect("transcript read");
        assert_eq!(seen, ["third"], "a line cut by the limit is not given");
    }

    #[test]
    fn backward_lines_report_an_unreadable_source() {
        let directory = std::env::temp_dir();
        assert!(lines_backwards(&directory.to_string_lossy(), 100, 10, |_, _| true).is_err());
        assert!(try_conversation("pi", &directory.to_string_lossy()).is_err());
        let missing = directory.join(crate::ledger::new_id());
        assert!(try_conversation("pi", &missing.to_string_lossy()).is_err());
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
        assert_eq!(conversation("other", &path).turns, Vec::new());
        assert_eq!(
            conversation("pi", "/nonexistent/transcript.jsonl").turns,
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
