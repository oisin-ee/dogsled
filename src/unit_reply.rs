//! Execution evidence, not task policy. Read exactly the acknowledged Pi branch and
//! retain its final reply atomically with the host acknowledgement.

use crate::error::Result;
use crate::ledger::Ledger;
use crate::unit_transport::{Seed, TerminalAck, conflict};
use crate::work::Launch;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead as _, BufReader, Read as _};
use std::path::Path;

const MAX_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;
const MAX_LINE: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub ack: TerminalAck,
    pub session_id: String,
    pub reply_entry_id: String,
    pub text: String,
}

/// Additive host evidence: the existing returned JSON object remains unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HostEvidence {
    Refused(Box<Refusal>),
    Returned(Box<Receipt>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Refusal {
    pub kind: RefusalKind,
    pub launch: Launch,
    pub failed: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalKind {
    Refused,
}

pub fn capture(
    ledger: &Ledger,
    launch: &Launch,
    seed: &Seed,
    ack: &TerminalAck,
    path: &str,
) -> Result<Receipt> {
    seed.validate(&launch.id, &launch.owner)?;
    if ack.execution != seed.execution
        || ack.unit_ref != seed.binding.contract
        || ack.digest != seed.exported.digest
        || ack.seal_entry_id.is_empty()
        || ack.session_leaf_id.is_empty()
    {
        return Err(conflict("terminal acknowledgement does not match seed"));
    }
    let session_directory = ledger
        .child("launches")
        .dir
        .join(format!("{}.sessions", launch.id));
    if std::fs::symlink_metadata(&session_directory)?
        .file_type()
        .is_symlink()
        || !std::fs::symlink_metadata(path)?.file_type().is_file()
    {
        return Err(conflict(
            "transcript and session directory must not be symbolic links",
        ));
    }
    let directory = session_directory.canonicalize()?;
    let path = Path::new(path).canonicalize()?;
    if path.parent() != Some(directory.as_path()) || path.extension().is_none_or(|e| e != "jsonl") {
        return Err(conflict(
            "transcript is outside this execution's session directory",
        ));
    }
    let file = std::fs::File::open(&path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > MAX_BYTES {
        return Err(conflict("transcript is not a bounded regular file"));
    }
    let entries = read_entries(file, launch)?;
    let (reply_entry_id, text) = correlated_reply(&entries, ack)?;
    Ok(Receipt {
        ack: ack.clone(),
        session_id: launch.id.clone(),
        reply_entry_id,
        text,
    })
}

fn read_entries(file: std::fs::File, launch: &Launch) -> Result<BTreeMap<String, Value>> {
    let mut reader = BufReader::new(file.take(MAX_BYTES + 1));
    let mut entries = BTreeMap::new();
    let mut header = None;
    let mut total = 0;
    loop {
        let mut line = String::new();
        let n = reader.by_ref().take(MAX_LINE + 1).read_line(&mut line)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if n as u64 > MAX_LINE || total > MAX_BYTES || !line.ends_with('\n') {
            return Err(conflict("transcript is partial or exceeds evidence bounds"));
        }
        let value: Value =
            serde_json::from_str(&line).map_err(|_| conflict("malformed transcript entry"))?;
        if header.is_none() {
            if value["type"] != "session"
                || value["version"] != 3
                || value["id"] != launch.id
                || value.get("parentSession").is_some()
                || value["cwd"]
                    .as_str()
                    .and_then(|p| Path::new(p).canonicalize().ok())
                    != Some(Path::new(&launch.cwd).canonicalize()?)
            {
                return Err(conflict("transcript header does not match fresh launch"));
            }
            header = Some(value);
            continue;
        }
        let id = value["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| conflict("transcript entry lacks identity"))?
            .to_owned();
        match value.get("parentId") {
            Some(Value::Null) => (),
            Some(Value::String(parent)) if entries.contains_key(parent) => (),
            _ => {
                return Err(conflict(
                    "transcript parent is missing, cyclic, or not append ordered",
                ));
            }
        }
        if entries.len() >= MAX_ENTRIES || entries.insert(id, value).is_some() {
            return Err(conflict(
                "transcript has duplicate identities or too many entries",
            ));
        }
    }
    if header.is_none() {
        return Err(conflict("transcript header is missing"));
    }
    Ok(entries)
}

fn correlated_reply(
    entries: &BTreeMap<String, Value>,
    ack: &TerminalAck,
) -> Result<(String, String)> {
    let mut current = ack.session_leaf_id.as_str();
    let mut seen = BTreeSet::new();
    let mut reply = None;
    loop {
        if !seen.insert(current) {
            return Err(conflict("cyclic transcript ancestry"));
        }
        let entry = entries
            .get(current)
            .ok_or_else(|| conflict("acknowledged ancestry is not durable"))?;
        if current == ack.seal_entry_id {
            // Seal meaning belongs to the provider; only its acknowledged immutable entry
            // and ordering are checked here. Never infer a seal from transcript contents.
            if entry["type"] != "custom" {
                return Err(conflict("seal is not a persisted extension entry"));
            }
            break;
        }
        if reply.is_none()
            && !matches!(
                entry["type"].as_str(),
                Some(
                    "message"
                        | "custom"
                        | "usage"
                        | "label"
                        | "session_info"
                        | "model_change"
                        | "thinking_level_change"
                )
            )
        {
            return Err(conflict(
                "substantive boundary entry follows the final reply",
            ));
        }
        if reply.is_none() && entry["type"] == "message" {
            let message = &entry["message"];
            if message["role"] != "assistant" || message["stopReason"] != "stop" {
                return Err(conflict(
                    "acknowledged boundary has no final assistant reply",
                ));
            }
            let blocks = message["content"]
                .as_array()
                .ok_or_else(|| conflict("final reply has no content"))?;
            if blocks.iter().any(|b| b["type"] == "toolCall") {
                return Err(conflict("tool-call message is not a final reply"));
            }
            let text = blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            if text.trim().is_empty() {
                return Err(conflict("final reply is empty"));
            }
            reply = Some((current.to_owned(), text));
        }
        current = entry["parentId"]
            .as_str()
            .ok_or_else(|| conflict("seal is not an ancestor of the final reply"))?;
    }
    reply.ok_or_else(|| conflict("no reply after seal on acknowledged branch"))
}
