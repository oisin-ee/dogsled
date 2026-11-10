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

/// Fresh-read reference for the incremental reader's tests.
#[cfg(test)]
pub(crate) fn try_conversation(agent: &str, path: &str) -> std::io::Result<History> {
    conversation_with_limit(agent, path, CONVERSATION_BYTES)
}

/// Read newest text and tool records independently, then restore transcript order.
/// Claude's adjacent message blocks remain one text record, even across tool rows.
#[cfg(test)]
pub(crate) fn conversation_with_limit(
    agent: &str,
    path: &str,
    limit: u64,
) -> std::io::Result<History> {
    let mut window = Window::default();
    let cut = lines_backwards(path, limit, CHUNK_BYTES, |offset, line| {
        window.push(agent, ParsedLine::new(agent, offset, line))
    })?;
    Ok(window.finish(cut))
}

#[derive(Debug, Default)]
struct Window {
    history: History,
    texts: usize,
    tools: usize,
    thoughts: usize,
    deliveries: std::collections::HashSet<String>,
}

impl Window {
    fn push(&mut self, agent: &str, line: ParsedLine) -> bool {
        let Self {
            history,
            texts,
            tools,
            thoughts,
            deliveries,
        } = self;
        history.truncated |= line.truncated;
        for mut record in line.records.into_iter().rev() {
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
                        tool.call = tool.call.take().or(result.call);
                        tool.output = tool.output.take().or(result.output);
                        tool.output_lines = tool.output_lines.or(result.output_lines);
                    }
                } else if *tools == CONVERSATION_TOOLS {
                    history.truncated = true;
                    continue;
                } else {
                    *tools += 1;
                }
            } else if record.role == Role::Thinking {
                if *thoughts == CONVERSATION_THOUGHTS {
                    history.truncated = true;
                    continue;
                }
                *thoughts += 1;
            } else {
                let group = (agent == "claude")
                    .then(|| {
                        history
                            .turns
                            .iter()
                            .rposition(|t| t.tool.is_none() && t.role != Role::Thinking)
                    })
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
                    if *texts == CONVERSATION_TURNS {
                        history.truncated = true;
                        return false;
                    }
                    *texts += 1;
                }
            }
            history.turns.push(record);
        }
        true
    }

    fn finish(mut self, cut: bool) -> History {
        self.history.truncated |= cut;
        self.history.turns.reverse();
        self.history
    }
}

/// Parsed, unmerged rows retain their byte positions. Replaying these bounded
/// rows preserves the backward reader's pairing and grouping when the byte
/// floor moves, without reading or parsing the old transcript again.
#[derive(Debug, Clone)]
struct ParsedLine {
    offset: u64,
    records: Vec<Turn>,
    truncated: bool,
}

impl ParsedLine {
    fn new(agent: &str, offset: u64, line: &str) -> Self {
        let mut parsed = Self {
            offset,
            records: Vec::new(),
            truncated: false,
        };
        if !line.trim().is_empty() && record_kind(agent, line) {
            #[cfg(test)]
            {
                VALUE_PARSES.with(|count| count.set(count.get() + 1));
                VALUE_BYTES.with(|count| count.set(count.get() + line.len()));
            }
            if let Ok(value) = serde_json::from_str::<Value>(line) {
                parsed.records = records(agent, &value, offset, &mut parsed.truncated);
            }
        }
        parsed
    }
}

#[cfg(test)]
thread_local! {
    static VALUE_PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static VALUE_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Ignore large custom state entries without allocating a JSON tree. On an
/// ambiguous envelope, let the full parser decide rather than dropping a turn.
fn record_kind(agent: &str, line: &str) -> bool {
    #[derive(serde::Deserialize)]
    struct Kind<'a> {
        #[serde(rename = "type", borrow)]
        kind: std::borrow::Cow<'a, str>,
    }
    let Ok(kind) = serde_json::from_str::<Kind<'_>>(line) else {
        return true;
    };
    match agent {
        "pi" => matches!(kind.kind.as_ref(), "message" | "custom_message"),
        "claude" => matches!(kind.kind.as_ref(), "user" | "assistant"),
        "codex" => kind.kind == "response_item",
        _ => false,
    }
}

/// Tool rows cannot consume the text-message allowance.
const CONVERSATION_TOOLS: usize = 400;

/// Thinking cannot consume the text-message or tool allowances.
const CONVERSATION_THOUGHTS: usize = 400;

/// The most text turns a conversation returns.
const CONVERSATION_TURNS: usize = 200;

/// The most transcript bytes a conversation reads.
pub(crate) const CONVERSATION_BYTES: u64 = 32 * 1024 * 1024;

/// The size of one backward read.
const CHUNK_BYTES: u64 = 1024 * 1024;

/// A per-session reader. Growth parses appended bytes and compares at most 8 KiB:
/// the first line (up to 4 KiB) and the window ending at the previous length.
/// A mismatch, replacement, or truncation rescans the byte window.
///
/// A rewrite preserving both fingerprints but changing the middle is not
/// detected. Pi migrations change the header version; forks use a new file.
/// Unchanged metadata needs no byte reads. An unfinished line is retained.
#[derive(Debug, Default)]
pub(crate) struct Incremental {
    stamp: Option<std::fs::Metadata>,
    agent: String,
    offset: u64,
    line_start: u64,
    pending: Vec<u8>,
    fingerprints: Fingerprints,
    // Keep unmerged records within the byte window, not only published turns:
    // an expired call, result, or delivery can change the retained grouping.
    rows: std::collections::VecDeque<ParsedLine>,
    history: History,
    #[cfg(test)]
    pub(crate) reads: usize,
    // Window/suffix input bytes; fingerprint comparisons are counted separately.
    #[cfg(test)]
    bytes_read: u64,
}

impl Incremental {
    pub(crate) fn advance(&mut self, agent: &str, path: &str) -> std::io::Result<&History> {
        self.advance_with_limit(agent, path, CONVERSATION_BYTES)
    }

    fn advance_with_limit(
        &mut self,
        agent: &str,
        path: &str,
        limit: u64,
    ) -> std::io::Result<&History> {
        use std::os::unix::fs::MetadataExt as _;
        let mut file = std::fs::File::open(path)?;
        let stamp = file.metadata()?;
        let same = self.agent == agent
            && self
                .stamp
                .as_ref()
                .is_some_and(|old| old.dev() == stamp.dev() && old.ino() == stamp.ino());
        if same
            && stamp.len() == self.offset
            && self
                .stamp
                .as_ref()
                .and_then(|old| old.modified().ok())
                .zip(stamp.modified().ok())
                .is_some_and(|(old, new)| old == new)
        {
            return Ok(&self.history);
        }
        #[cfg(test)]
        {
            self.reads += 1;
        }
        let result = (|| {
            if same
                && stamp.len() >= self.offset
                && self.fingerprints.matches(&mut file, self.offset)?
            {
                self.append(&mut file, agent, stamp.len(), limit)
            } else {
                self.rescan(&mut file, agent, stamp.len(), limit)
            }
        })();
        if let Err(error) = result {
            // A failed read must not leave an apparently reusable partial update.
            self.stamp = None;
            return Err(error);
        }
        self.offset = stamp.len();
        agent.clone_into(&mut self.agent);
        self.stamp = Some(stamp);
        self.rebuild(agent, limit);
        Ok(&self.history)
    }

    fn rescan(
        &mut self,
        file: &mut std::fs::File,
        agent: &str,
        len: u64,
        limit: u64,
    ) -> std::io::Result<()> {
        self.rows.clear();
        self.pending.clear();
        self.line_start = len;
        if len > 0 && !newline_before(file, len)? {
            // A trailing line cut by the window cannot become eligible on growth.
            self.line_start = 0;
        }
        let mut fingerprints = Fingerprints::default();
        let mut header = vec![
            0;
            usize::try_from(len.min(FINGERPRINT_BYTES as u64))
                .map_err(std::io::Error::other)?
        ];
        lines_backwards_file(
            file,
            len,
            limit,
            CHUNK_BYTES,
            |offset, line| {
                if offset + line.len() as u64 == len {
                    self.line_start = offset;
                    self.pending.extend_from_slice(line);
                } else {
                    let parsed = ParsedLine::new(agent, offset, &String::from_utf8_lossy(line));
                    if !parsed.records.is_empty() || parsed.truncated {
                        self.rows.push_front(parsed);
                    }
                }
                true
            },
            |offset, bytes| {
                if offset + bytes.len() as u64 == len {
                    fingerprints.remember(bytes);
                }
                if offset < header.len() as u64 {
                    let start = offset as usize;
                    let take = bytes.len().min(header.len() - start);
                    header[start..start + take].copy_from_slice(&bytes[..take]);
                }
            },
        )?;
        if len > limit {
            // The header lies outside the backward window on large sessions.
            file.seek(SeekFrom::Start(0))?;
            file.read_exact(&mut header)?;
        }
        fingerprints.head.clear();
        fingerprints.remember_head(&header);
        #[cfg(test)]
        {
            fingerprints.compared = self.fingerprints.compared;
        }
        self.fingerprints = fingerprints;
        #[cfg(test)]
        {
            self.bytes_read += len.min(limit);
        }
        Ok(())
    }

    fn append(
        &mut self,
        file: &mut std::fs::File,
        agent: &str,
        len: u64,
        limit: u64,
    ) -> std::io::Result<()> {
        let floor = len.saturating_sub(limit);
        if self.line_start < floor {
            self.pending.clear();
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut at = self.offset;
        let mut bytes =
            vec![0; usize::try_from((len - at).min(CHUNK_BYTES)).map_err(std::io::Error::other)?];
        while at < len {
            let size =
                usize::try_from((len - at).min(CHUNK_BYTES)).map_err(std::io::Error::other)?;
            let chunk = &mut bytes[..size];
            file.read_exact(chunk)?;
            self.fingerprints.remember(chunk);
            #[cfg(test)]
            {
                self.bytes_read += size as u64;
            }
            for part in chunk.split_inclusive(|byte| *byte == b'\n') {
                at += part.len() as u64;
                if self.line_start >= floor {
                    self.pending.extend_from_slice(part);
                }
                if part.last() == Some(&b'\n') {
                    if self.line_start >= floor {
                        let parsed = ParsedLine::new(
                            agent,
                            self.line_start,
                            &String::from_utf8_lossy(&self.pending),
                        );
                        if !parsed.records.is_empty() || parsed.truncated {
                            self.rows.push_back(parsed);
                        }
                    }
                    self.pending.clear();
                    self.line_start = at;
                }
            }
        }
        Ok(())
    }

    fn rebuild(&mut self, agent: &str, limit: u64) {
        let floor = self.offset.saturating_sub(limit);
        while self.rows.front().is_some_and(|line| line.offset < floor) {
            self.rows.pop_front();
        }
        let mut window = Window::default();
        // The fresh reader accepts valid JSON at EOF without a newline. Preview
        // it, but do not commit it: a later append may complete or invalidate it.
        let preview = self.line_start >= floor
            && window.push(
                agent,
                ParsedLine::new(
                    agent,
                    self.line_start,
                    &String::from_utf8_lossy(&self.pending),
                ),
            );
        if preview {
            for line in self.rows.iter().rev() {
                if !window.push(agent, line.clone()) {
                    break;
                }
            }
        }
        self.history = window.finish(floor > 0);
    }
}

const FINGERPRINT_BYTES: usize = 4 * 1024;

/// Byte-exact rewrite probes. Appends update them from the bytes just read.
#[derive(Debug, Default)]
struct Fingerprints {
    head: Vec<u8>,
    tail: Vec<u8>,
    #[cfg(test)]
    compared: u64,
}

impl Fingerprints {
    fn remember_head(&mut self, bytes: &[u8]) {
        if self.head.len() < FINGERPRINT_BYTES && self.head.last() != Some(&b'\n') {
            let end = bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |at| at + 1);
            let take = end.min(FINGERPRINT_BYTES - self.head.len());
            self.head.extend_from_slice(&bytes[..take]);
        }
    }

    fn remember(&mut self, bytes: &[u8]) {
        self.remember_head(bytes);
        let suffix = &bytes[bytes.len().saturating_sub(FINGERPRINT_BYTES)..];
        let discard = (self.tail.len() + suffix.len()).saturating_sub(FINGERPRINT_BYTES);
        self.tail.drain(..discard);
        self.tail.extend_from_slice(suffix);
    }

    fn matches(&mut self, file: &mut std::fs::File, len: u64) -> std::io::Result<bool> {
        let mut bytes = [0; FINGERPRINT_BYTES];
        for (offset, expected) in [(0, &self.head), (len - self.tail.len() as u64, &self.tail)] {
            file.seek(SeekFrom::Start(offset))?;
            let actual = &mut bytes[..expected.len()];
            file.read_exact(actual)?;
            #[cfg(test)]
            {
                self.compared += expected.len() as u64;
            }
            if actual != expected {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// Give the complete lines of `path` to `visit`, newest first, until `visit`
/// returns `false` or `limit` bytes are read, `step` bytes at a time. A line
/// cut by the limit is not given.
#[cfg(test)]
fn lines_backwards(
    path: &str,
    limit: u64,
    step: u64,
    mut visit: impl FnMut(u64, &str) -> bool,
) -> std::io::Result<bool> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    lines_backwards_file(
        &mut file,
        len,
        limit,
        step,
        |offset, line| visit(offset, &String::from_utf8_lossy(line)),
        |_, _| {},
    )
}

fn lines_backwards_file(
    file: &mut std::fs::File,
    len: u64,
    limit: u64,
    step: u64,
    mut visit: impl FnMut(u64, &[u8]) -> bool,
    mut observe: impl FnMut(u64, &[u8]),
) -> std::io::Result<bool> {
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
        observe(start, &bytes);
        bytes.extend_from_slice(&carry);
        let chunk = bytes;
        // The first line is complete at the start of the file, or at the read
        // limit when a newline comes just before it. Elsewhere the bytes before
        // the first newline end a line that starts earlier.
        let whole = start == 0 || (start == floor && newline_before(file, start)?);
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
            if !line.is_empty() && !visit(offset, line) {
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

/// A call line shared with the Pi activity producer's `callLine` rule.
#[must_use]
pub fn call_line(name: &str, arguments: &Value) -> String {
    let decoded = arguments
        .as_str()
        .and_then(|text| serde_json::from_str::<Value>(text).ok());
    let arguments = decoded.as_ref().unwrap_or(arguments);
    if let Some(command) = arguments
        .get("command")
        .filter(|v| !v.is_null())
        .or_else(|| arguments.get("cmd"))
        .and_then(Value::as_str)
    {
        return format!("$ {command}");
    }
    if let Some(path) = arguments
        .get("path")
        .filter(|v| !v.is_null())
        .or_else(|| arguments.get("file_path"))
        .and_then(Value::as_str)
    {
        return format!("{} {path}", name.to_lowercase());
    }
    format!("{name} {arguments}")
}

fn prefix(text: &str, max: usize) -> String {
    text.split_at(text.floor_char_boundary(max)).0.to_owned()
}

fn tail_bytes(text: &str, max: usize) -> String {
    text.split_at(text.ceil_char_boundary(text.len().saturating_sub(max)))
        .1
        .to_owned()
}

fn result_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_owned());
    }
    let blocks = content.as_array()?;
    let text: Vec<_> = blocks
        .iter()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect();
    (!text.is_empty()).then(|| text.join("\n"))
}

/// Convert supported calls and results into bounded transcript detail.
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
    let call = (status == ToolStatus::Unknown).then(|| {
        let arguments = if agent == "claude" {
            &block["input"]
        } else {
            &block["arguments"]
        };
        prefix(
            &call_line(name.as_deref().unwrap_or_default(), arguments),
            4 * 1024,
        )
    });
    let result = (status != ToolStatus::Unknown)
        .then(|| {
            result_text(if agent == "codex" {
                &block["output"]
            } else {
                &block["content"]
            })
        })
        .flatten();
    let output_lines = result
        .as_ref()
        .map(|text| u32::try_from(text.lines().count()).unwrap_or(u32::MAX));
    let output = result.map(|text| {
        let lines: Vec<_> = text.lines().collect();
        let recent = lines
            .get(lines.len().saturating_sub(20)..)
            .unwrap_or_default()
            .join("\n");
        tail_bytes(&recent, 4 * 1024)
    });
    record.tool = Some(ToolSummary {
        call_id,
        name: name.map(|name| short(&plain(&name), 120)),
        status,
        call,
        output,
        output_lines,
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

fn records(agent: &str, value: &Value, offset: u64, truncated: &mut bool) -> Vec<Turn> {
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
        for (index, block) in blocks.iter().enumerate() {
            if matches!(agent, "pi" | "claude")
                && block["type"] == "thinking"
                && let Some(thinking) = block["thinking"].as_str().filter(|text| !text.is_empty())
            {
                let mut thought = envelope(agent, value, offset);
                thought.id = format!("{}:thinking:{index}", thought.id);
                thought.role = Role::Thinking;
                thought.text = prefix(thinking, 32 * 1024);
                *truncated |= thinking.len() > 32 * 1024;
                found.push(thought);
            }
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

    fn incremental_fixture(agent: &str) -> Vec<Value> {
        let mut lines = Vec::new();
        for index in 0..CONVERSATION_TOOLS + 5 {
            let id = format!("m-{index}");
            let call = format!("call-{index}");
            let pair = match agent {
                "pi" => [
                    json!({"type":"message","id":id,"message":{"role":"assistant","content":[{"type":"thinking","thinking":"界 thought"},{"type":"toolCall","id":call,"name":"read","arguments":{}},{"type":"text","text":id}]}}),
                    json!({"type":"message","message":{"role":"toolResult","toolCallId":call,"toolName":"read","isError":index % 2 == 0,"content":"result"}}),
                ],
                "claude" => [
                    json!({"type":"assistant","message":{"id":id,"content":[{"type":"thinking","thinking":"界 thought"},{"type":"tool_use","id":call,"name":"read","input":{}},{"type":"text","text":id}]}}),
                    json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":call,"content":"result"}]}}),
                ],
                _ => [
                    json!({"type":"response_item","payload":{"type":"function_call","call_id":call,"name":"read","arguments":"{}"}}),
                    json!({"type":"response_item","payload":{"type":"function_call_output","call_id":call,"output":"result"}}),
                ],
            };
            lines.extend(pair);
            if agent == "claude" {
                lines.push(json!({"type":"assistant","message":{"id":id,"content":[{"type":"text","text":"second block"}]}}));
            }
            // Keep enough tool/thought rows after the newest text turn to
            // exercise their independent caps, not only the text cutoff.
            if index < CONVERSATION_TURNS + 5 {
                if agent == "codex" {
                    lines.push(json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":id}]}}));
                }
            } else if agent != "codex" {
                for value in lines
                    .iter_mut()
                    .rev()
                    .take(if agent == "claude" { 3 } else { 2 })
                {
                    if let Some(blocks) = value["message"]["content"].as_array_mut() {
                        blocks.retain(|block| block["type"] != "text");
                    }
                }
            }
        }
        if agent == "pi" {
            let delivery = json!({"type":"custom_message","customType":"dogsled","display":true,"details":{"records":[{"id":"delivery","sender":"lead","kind":"message","created_ms":7,"text":"request"}]}});
            lines.insert(0, delivery.clone());
            lines.push(delivery);
            lines.push(json!({"type":"message","message":{"role":"assistant","content":[{"type":"thinking","thinking":"界".repeat(12000)}]}}));
        }
        lines
    }

    #[test]
    fn incremental_matches_fresh_reads_after_random_and_midline_appends() {
        for agent in ["pi", "claude", "codex"] {
            let path = transcript(&[]);
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            let mut incremental = Incremental::default();
            let mut random = 17_u64;
            let mut appended = 0;
            for value in incremental_fixture(agent) {
                let mut line = value.to_string().into_bytes();
                line.push(b'\n');
                let mut start = 0;
                while start < line.len() {
                    random = random
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1);
                    let end = (start + 1 + (random as usize % 1024)).min(line.len());
                    file.write_all(&line[start..end]).unwrap();
                    appended += (end - start) as u64;
                    assert_eq!(
                        incremental.advance(agent, &path).unwrap(),
                        &conversation_with_limit(agent, &path, CONVERSATION_BYTES).unwrap(),
                        "{agent} at byte {appended}"
                    );
                    start = end;
                }
            }
            assert!(incremental.history.truncated);
            assert_eq!(
                incremental.bytes_read, appended,
                "suffix reads exceeded the append size"
            );
            let reads = incremental.reads;
            incremental.advance(agent, &path).unwrap();
            assert_eq!(incremental.reads, reads);
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn incremental_independent_caps_and_byte_window_match_fresh_reads() {
        for agent in ["pi", "claude", "codex"] {
            let mut lines = incremental_fixture(agent);
            // Remove text so all tools and thoughts must use their own caps.
            for line in &mut lines {
                for key in ["message", "payload"] {
                    if let Some(blocks) = line[key]["content"].as_array_mut() {
                        blocks.retain(|block| {
                            !matches!(block["type"].as_str(), Some("text" | "output_text"))
                        });
                    }
                }
            }
            let path = transcript(&lines);
            let mut incremental = Incremental::default();
            assert_eq!(
                incremental.advance(agent, &path).unwrap(),
                &conversation(agent, &path)
            );
            assert_eq!(
                incremental
                    .history
                    .turns
                    .iter()
                    .filter(|turn| turn.tool.is_some())
                    .count(),
                CONVERSATION_TOOLS
            );
            if agent != "codex" {
                assert_eq!(
                    incremental
                        .history
                        .turns
                        .iter()
                        .filter(|turn| turn.role == Role::Thinking)
                        .count(),
                    CONVERSATION_THOUGHTS
                );
            }
            let mut incremental = Incremental::default();
            let limit = 2048;
            incremental.advance_with_limit(agent, &path, limit).unwrap();
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            for line in &lines[..20] {
                for byte in format!("{line}\n").bytes() {
                    file.write_all(&[byte]).unwrap();
                    assert_eq!(
                        incremental.advance_with_limit(agent, &path, limit).unwrap(),
                        &conversation_with_limit(agent, &path, limit).unwrap()
                    );
                }
            }
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn incremental_keeps_unmatched_results_when_calls_cross_the_byte_floor() {
        for agent in ["pi", "claude", "codex"] {
            let lines = incremental_fixture(agent);
            let start = usize::from(agent == "pi");
            let path = transcript(&lines[start..start + 2]);
            let size = std::fs::metadata(&path).unwrap().len();
            let limit = size + 10;
            let mut incremental = Incremental::default();
            incremental.advance_with_limit(agent, &path, limit).unwrap();
            let call = incremental
                .history
                .turns
                .iter()
                .find_map(|turn| turn.tool.as_ref())
                .unwrap();
            assert!(call.call.is_some());
            assert!(call.output.is_some());
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            // This short ignored row moves the floor into the call, but leaves
            // the result whole. Its published identity must now be the result's.
            let padding = b"{\"type\":\"ignored\"}\n";
            file.write_all(padding).unwrap();
            assert_eq!(
                incremental.advance_with_limit(agent, &path, limit).unwrap(),
                &conversation_with_limit(agent, &path, limit).unwrap()
            );
            assert_eq!(incremental.bytes_read, size + padding.len() as u64);
            let tool = incremental
                .history
                .turns
                .iter()
                .find_map(|turn| turn.tool.as_ref())
                .unwrap();
            assert!(tool.call.is_none());
            assert_eq!(tool.output.as_deref(), Some("result"));
            // An unfinished line larger than the window must stay excluded,
            // even after its newline arrives. Following rows become eligible.
            file.write_all(&vec![b'x'; limit as usize + 1]).unwrap();
            assert_eq!(
                incremental.advance_with_limit(agent, &path, limit).unwrap(),
                &conversation_with_limit(agent, &path, limit).unwrap()
            );
            writeln!(file, "\n{}", lines[start + 1]).unwrap();
            assert_eq!(
                incremental.advance_with_limit(agent, &path, limit).unwrap(),
                &conversation_with_limit(agent, &path, limit).unwrap()
            );
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn incremental_previews_valid_unterminated_json_without_committing_it() {
        let path = transcript(&[]);
        let mut incremental = Incremental::default();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(br#"{"type":"message","message":{"role":"user","content":"preview"}}"#)
            .unwrap();
        assert_eq!(
            incremental.advance("pi", &path).unwrap(),
            &conversation("pi", &path)
        );
        assert_eq!(incremental.history.turns[0].text, "preview");
        file.write_all(b"invalid suffix\n").unwrap();
        assert_eq!(
            incremental.advance("pi", &path).unwrap(),
            &conversation("pi", &path)
        );
        assert_eq!(incremental.history.turns, Vec::<Turn>::new());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn incremental_rescans_after_truncation_and_replacement_and_keeps_io_errors() {
        let lines = incremental_fixture("pi");
        let path = transcript(&lines[..5]);
        let mut incremental = Incremental::default();
        incremental.advance("pi", &path).unwrap();
        std::fs::write(&path, format!("{}\n", lines[3])).unwrap();
        assert_eq!(
            incremental.advance("pi", &path).unwrap(),
            &conversation("pi", &path)
        );
        let replacement = transcript(&lines[..2]);
        std::fs::rename(&replacement, &path).unwrap();
        assert_eq!(
            incremental.advance("pi", &path).unwrap(),
            &conversation("pi", &path)
        );
        assert_eq!(incremental.reads, 3);
        std::fs::remove_file(&path).unwrap();
        assert!(incremental.advance("pi", &path).is_err());
        assert!(
            incremental
                .advance("pi", &std::env::temp_dir().to_string_lossy())
                .is_err()
        );
    }

    #[test]
    fn incremental_rescans_same_inode_longer_rewrites() {
        use std::os::unix::fs::MetadataExt as _;
        let path = transcript(&[
            json!({"type":"session","version":1}),
            json!({"type":"message","message":{"role":"user","content":"old"}}),
        ]);
        let mut incremental = Incremental::default();
        incremental.advance("pi", &path).unwrap();
        let identity = std::fs::metadata(&path).unwrap().ino();
        // Keep the header identical: the old-end fingerprint must detect this.
        let replacement = format!(
            "{}\n{}\n",
            json!({"type":"session","version":1}),
            json!({"type":"message","message":{"role":"user","content":"replacement task".repeat(400)}})
        );
        let read = incremental.bytes_read;
        std::fs::write(&path, &replacement).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().ino(), identity);
        assert_eq!(
            incremental.advance("pi", &path).unwrap(),
            &conversation("pi", &path)
        );
        assert_eq!(
            incremental.bytes_read - read,
            replacement.len() as u64,
            "rewrite did not rescan"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn incremental_matches_zero_then_partial_in_place_rewrites() {
        let path = transcript(&incremental_fixture("pi")[..5]);
        let mut incremental = Incremental::default();
        incremental.advance("pi", &path).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        assert_eq!(
            incremental.advance("pi", &path).unwrap(),
            &conversation("pi", &path)
        );
        let replacement = format!(
            "{}\n{}\n",
            json!({"type":"session","version":3}),
            json!({"type":"message","message":{"role":"user","content":"rewritten 界 task"}})
        );
        // Observe every byte, including partial JSON and a split UTF-8 codepoint.
        for byte in replacement.bytes() {
            file.write_all(&[byte]).unwrap();
            assert_eq!(
                incremental.advance("pi", &path).unwrap(),
                &conversation("pi", &path)
            );
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn incremental_rescans_header_only_rewrites_even_when_the_length_is_unchanged() {
        use std::os::unix::fs::MetadataExt as _;
        let header = json!({"type":"session","version":1});
        let path = transcript(&[
            header,
            json!({"type":"message","message":{"role":"user","content":"original task"}}),
            json!({"type":"custom","customType":"todo-state","data":"x".repeat(12 * 1024)}),
        ]);
        let mut incremental = Incremental::default();
        incremental.advance("pi", &path).unwrap();
        let stamp = std::fs::metadata(&path).unwrap();
        let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        writeln!(file, "{}", json!({"type":"session","version":2})).unwrap();
        file.set_modified(stamp.modified().unwrap() + std::time::Duration::from_secs(1))
            .unwrap();
        let read = incremental.bytes_read;
        let compared = incremental.fingerprints.compared;
        assert_eq!(
            incremental.advance("pi", &path).unwrap(),
            &conversation("pi", &path)
        );
        let after = std::fs::metadata(&path).unwrap();
        assert_eq!((stamp.ino(), stamp.len()), (after.ino(), after.len()));
        assert_eq!(
            incremental.bytes_read - read,
            after.len(),
            "header-only rewrite did not rescan"
        );
        assert!(
            incremental.fingerprints.compared - compared < FINGERPRINT_BYTES as u64,
            "header mismatch should short circuit before checking the identical tail"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn incremental_skips_two_megabyte_todo_state_without_a_value_parse() {
        let line =
            json!({"type":"custom","customType":"todo-state","data":"x".repeat(2 * 1024 * 1024)});
        let path = transcript(&[line]);
        let before = VALUE_PARSES.with(std::cell::Cell::get);
        let mut incremental = Incremental::default();
        assert_eq!(
            incremental.advance("pi", &path).unwrap().turns,
            Vec::<Turn>::new()
        );
        assert_eq!(VALUE_PARSES.with(std::cell::Cell::get), before);
        // Escaped discriminants and arbitrary field order still reach records.
        let line = r#"{"message":{"role":"user","content":"kept"},"type":"mess\u0061ge"}"#;
        let parsed = ParsedLine::new("pi", 0, line);
        assert_eq!(parsed.records[0].text, "kept");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn incremental_times_a_thirty_megabyte_fixture_append() {
        let state =
            json!({"type":"custom","customType":"todo-state","data":"x".repeat(2 * 1024 * 1024)});
        let mut lines =
            vec![json!({"type":"message","message":{"role":"user","content":"initial task"}})];
        lines.extend(std::iter::repeat_n(state, 15));
        let path = transcript(&lines);
        let mut incremental = Incremental::default();
        incremental.advance("pi", &path).unwrap();
        let append = format!(
            "{}\n",
            json!({"type":"message","message":{"role":"assistant","content":"appended"}})
        );
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(append.as_bytes())
            .unwrap();
        let before = std::time::Instant::now();
        // The pre-incremental reader materialized every line as a Value.
        let mut window = Window::default();
        let cut = lines_backwards(&path, CONVERSATION_BYTES, CHUNK_BYTES, |offset, line| {
            let mut parsed = ParsedLine {
                offset,
                records: Vec::new(),
                truncated: false,
            };
            if let Ok(value) = serde_json::from_str::<Value>(line) {
                parsed.records = records("pi", &value, offset, &mut parsed.truncated);
            }
            window.push("pi", parsed)
        })
        .unwrap();
        let fresh = window.finish(cut);
        let before = before.elapsed();
        let read = incremental.bytes_read;
        let compared = incremental.fingerprints.compared;
        let parsed = VALUE_BYTES.with(std::cell::Cell::get);
        let after = std::time::Instant::now();
        assert_eq!(incremental.advance("pi", &path).unwrap(), &fresh);
        let after = after.elapsed();
        let read = incremental.bytes_read - read;
        let parsed = VALUE_BYTES.with(std::cell::Cell::get) - parsed;
        let compared = incremental.fingerprints.compared - compared;
        let size = std::fs::metadata(&path).unwrap().len();
        eprintln!(
            "{size}-byte fixture append, debug profile: original backward read {before:?}; incremental {after:?}; appended/read/Value-parsed/compared bytes: {}/{read}/{parsed}/{compared}",
            append.len()
        );
        assert_eq!(read, append.len() as u64);
        assert_eq!(parsed, append.len());
        assert_eq!(
            compared,
            incremental.fingerprints.head.len() as u64 + FINGERPRINT_BYTES as u64
        );
        assert!(compared <= 8 * 1024);
        assert_eq!(incremental.bytes_read, size);
        std::fs::remove_file(path).unwrap();
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
                (Role::Thinking, "hm"),
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
    fn recorded_tools_correlate_calls_results_and_thinking_without_claiming_live_work() {
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
            assert_eq!(turns.len(), if agent == "codex" { 3 } else { 4 }, "{agent}");
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
                turns
                    .last()
                    .expect("unmatched call")
                    .tool
                    .as_ref()
                    .expect("tool")
                    .status,
                ToolStatus::Unknown
            );
            assert!(
                turns[1..]
                    .iter()
                    .filter(|turn| turn.role == Role::Tool)
                    .all(|turn| turn.text.is_empty())
            );
            assert_eq!(
                summary.call.as_deref(),
                Some(if agent == "codex" {
                    "read \"raw-arguments\""
                } else {
                    "read {\"secret\":\"raw-arguments\"}"
                })
            );
            assert_eq!(summary.output.as_deref(), Some("raw-output"));
            assert_eq!(summary.output_lines, Some(1));
            if agent != "codex" {
                assert_eq!(turns[2].role, Role::Thinking);
                assert_eq!(turns[2].text, "raw-reasoning");
                assert_eq!(turns[2].id, "call-entry:thinking:2");
            }
        }
    }

    #[test]
    fn call_lines_match_the_shared_live_and_transcript_cases() {
        let cases: Vec<Value> =
            serde_json::from_str(include_str!("../tests/fixtures/call-lines.json"))
                .expect("call-line cases");
        for case in cases {
            assert_eq!(
                call_line(case["name"].as_str().expect("name"), &case["arguments"]),
                case["line"].as_str().expect("line")
            );
        }
    }

    #[test]
    fn pi_and_claude_thoughts_calls_and_result_text_blocks_stay_in_recorded_order() {
        for agent in ["pi", "claude"] {
            let content = json!([
                {"type":"thinking","thinking":"first thought"},
                {"type":"thinking","thinking":""},
                {"type":"text","text":"checking"},
                {"type":"thinking","thinking":"second thought"},
                {"type":if agent == "pi" {"toolCall"} else {"tool_use"},"id":"a","name":"bash","arguments":{"command":"pwd\nls"},"input":{"command":"pwd\nls"}}
            ]);
            let assistant = if agent == "pi" {
                json!({"type":"message","id":"message","message":{"role":"assistant","content":content}})
            } else {
                json!({"type":"assistant","message":{"id":"message","content":content}})
            };
            let result = if agent == "pi" {
                json!({"type":"message","message":{"role":"toolResult","toolCallId":"a","content":[{"type":"text","text":"first"},{"type":"image","data":"ignored"},{"type":"text","text":"second"}]}})
            } else {
                json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"a","content":[{"type":"text","text":"first"},{"type":"image","data":"ignored"},{"type":"text","text":"second"}]}]}})
            };
            let history = conversation(agent, &transcript(&[assistant, result]));
            assert!(!history.truncated);
            let turns = history.turns;
            assert_eq!(
                turns.iter().map(|turn| turn.role).collect::<Vec<_>>(),
                [Role::Thinking, Role::Assistant, Role::Thinking, Role::Tool]
            );
            assert_eq!(turns[0].id, "message:thinking:0");
            assert_eq!(turns[0].text, "first thought");
            assert_eq!(turns[2].id, "message:thinking:3");
            let tool = turns[3].tool.as_ref().expect("tool");
            assert_eq!(tool.call.as_deref(), Some("$ pwd\nls"));
            assert_eq!(tool.output.as_deref(), Some("first\nsecond"));
            assert_eq!(tool.output_lines, Some(2));
        }
    }

    #[test]
    fn thinking_has_its_own_count_bound_and_a_utf8_prefix_bound() {
        let mut lines = vec![
            json!({"type":"message","id":"useful","message":{"role":"user","content":"useful text"}}),
            json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","id":"a","name":"read","arguments":{}}]}}),
        ];
        for index in 0..CONVERSATION_THOUGHTS + 5 {
            lines.push(json!({"type":"message","id":format!("m-{index}"),"message":{"role":"assistant","content":[{"type":"thinking","thinking":"thought"}]}}));
        }
        let history = conversation("pi", &transcript(&lines));
        assert!(history.truncated);
        assert_eq!(history.turns.len(), CONVERSATION_THOUGHTS + 2);
        assert_eq!(history.turns[0].text, "useful text");
        assert_eq!(history.turns[1].role, Role::Tool);
        assert_eq!(history.turns[2].id, "m-5:thinking:0");
        for agent in ["pi", "claude"] {
            let value = if agent == "pi" {
                json!({"type":"message","message":{"role":"assistant","content":[{"type":"thinking","thinking":"界".repeat(12000)}]}})
            } else {
                json!({"type":"assistant","message":{"content":[{"type":"thinking","thinking":"界".repeat(12000)}]}})
            };
            let history = conversation(agent, &transcript(&[value]));
            assert!(history.truncated);
            assert_eq!(history.turns[0].text, "界".repeat(10922));
        }
    }

    #[test]
    fn tool_call_prefixes_and_output_tails_obey_byte_and_line_bounds() {
        let output = (1..=30)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = [
            json!({"type":"response_item","payload":{"type":"function_call","call_id":"a","name":"bash","arguments":json!({"command":"界".repeat(2000)}).to_string()}}),
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"a","output":output}}),
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"b","output":"界".repeat(2000)}}),
        ];
        let history = conversation("codex", &transcript(&lines));
        let tool = history.turns[0].tool.as_ref().expect("tool");
        assert_eq!(tool.call, Some(format!("$ {}", "界".repeat(1364))));
        assert_eq!(tool.output_lines, Some(30));
        assert_eq!(tool.output.as_ref().expect("output").lines().count(), 20);
        assert!(
            tool.output
                .as_ref()
                .expect("output")
                .starts_with("line 11\n")
        );
        assert!(tool.output.as_ref().expect("output").ends_with("line 30"));
        let tool = history.turns[1].tool.as_ref().expect("result");
        assert_eq!(tool.output, Some("界".repeat(1365)));
        assert_eq!(tool.output_lines, Some(1));
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
