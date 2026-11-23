//! Incremental complete-line indexing, with call/result pairing across passes.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead as _, Seek as _, SeekFrom};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use portal_types::agents::ToolStatus;
use rusqlite::{Connection, OptionalExtension as _, Transaction, params};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::shell::Shell as _;

#[derive(Debug, Clone)]
pub(super) struct Binding {
    pub path: String,
    pub harness: String,
    pub pane: String,
    pub item: Option<String>,
    pub lead: Option<String>,
}

fn collect(root: &Path, files: &mut BTreeSet<PathBuf>, before: u64) -> Result<()> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect(&entry.path(), files, before)?;
        } else if kind.is_file() && entry.path().extension().is_some_and(|ext| ext == "jsonl") {
            let modified = entry
                .metadata()?
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|error| Error::Invalid(error.to_string()))?
                .as_millis();
            if modified >= u128::from(before) {
                files.insert(entry.path());
            }
        }
    }
    Ok(())
}

pub(super) fn pass(
    connection: &mut Connection,
    ledger: &Ledger,
    home: &str,
    bindings: &[Binding],
    now: u64,
) -> Result<()> {
    let home = Path::new(home);
    let roots = [
        (
            "pi",
            std::env::var_os("PI_CODING_AGENT_DIR")
                .map_or_else(|| home.join(".pi/agent"), PathBuf::from)
                .join("sessions"),
        ),
        ("pi", ledger.dir.join("launches")),
        (
            "claude",
            std::env::var_os("CLAUDE_CONFIG_DIR")
                .map_or_else(|| home.join(".claude"), PathBuf::from)
                .join("projects"),
        ),
        (
            "codex",
            std::env::var_os("CODEX_HOME")
                .map_or_else(|| home.join(".codex"), PathBuf::from)
                .join("sessions"),
        ),
    ];
    let before = now.saturating_sub(crate::jobs::RECORD_KEEP_MS);
    let mut files = BTreeMap::new();
    for (harness, root) in roots {
        let mut paths = BTreeSet::new();
        collect(&root, &mut paths, before)?;
        for path in paths {
            files.insert(path, harness.to_owned());
        }
    }
    for binding in bindings {
        let path = PathBuf::from(&binding.path);
        // Observation can bind a custom session directory outside the standard roots.
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => {
                let modified = metadata
                    .modified()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|error| Error::Invalid(error.to_string()))?
                    .as_millis();
                if modified >= u128::from(before) {
                    files.insert(path, binding.harness.clone());
                }
            }
            Ok(_) => {
                return Err(Error::Invalid(format!(
                    "{} is not a transcript file",
                    path.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    let shell = crate::shell::System {
        budget: ledger.budget,
    };
    for (path, harness) in files {
        ledger.budget.remaining("metrics index")?;
        file(connection, &path, &harness, now, &shell)?;
    }
    super::store::write(connection, |tx| {
        for binding in bindings {
            tx.execute(
                "UPDATE transcripts SET pane=?2,item=?3,lead=?4 WHERE path=?1",
                params![binding.path, binding.pane, binding.item, binding.lead],
            )?;
        }
        Ok(())
    })
}

pub(super) fn file(
    connection: &mut Connection,
    path: &Path,
    harness: &str,
    now: u64,
    shell: &crate::shell::System,
) -> Result<()> {
    let file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    let file_id = format!("{}:{}", metadata.dev(), metadata.ino());
    let path_text = path
        .to_str()
        .ok_or_else(|| Error::Invalid("non-UTF8 transcript path".into()))?;
    super::store::write(connection, |tx| {
        let previous: Option<(String, u64, Option<String>)> = tx
            .query_row(
                "SELECT file_id,offset,model FROM transcripts WHERE path=?1",
                [path_text],
                |row| {
                    Ok((
                        row.get(0)?,
                        super::store::unsigned(row.get(1)?)?,
                        row.get(2)?,
                    ))
                },
            )
            .optional()?;
        let (offset, mut model) = match previous {
            Some((id, offset, model)) if id == file_id && offset <= metadata.len() => {
                (offset, model)
            }
            _ => {
                tx.execute("DELETE FROM transcripts WHERE path=?1", [path_text])?;
                tx.execute("INSERT INTO transcripts(path,harness,file_id,offset,indexed_ms) VALUES(?1,?2,?3,0,?4)", params![path_text,harness,file_id,crate::rows::integer(now)?])?;
                (0, None)
            }
        };
        let mut reader = std::io::BufReader::new(file);
        reader.seek(SeekFrom::Start(offset))?;
        let mut offset = offset;
        let mut line = Vec::new();
        loop {
            line.clear();
            if reader.read_until(b'\n', &mut line)? == 0 || !line.ends_with(b"\n") {
                break;
            }
            let value: Value = serde_json::from_slice(&line)
                .map_err(|error| Error::Invalid(format!("{path_text} at {offset}: {error}")))?;
            header(tx, path_text, harness, &value, shell)?;
            if harness == "codex" && value["type"] == "turn_context" {
                model = value["payload"]["model"].as_str().map(str::to_owned);
            }
            if harness == "pi" && value["type"] == "custom_message" {
                pi_injection(tx, path_text, &value)?;
            }
            if let Some(entry) = crate::activity::decode_entry(harness, &value) {
                if harness == "codex" && entry.model.is_some() {
                    model.clone_from(&entry.model);
                }
                if let Some(at_ms) = entry.at_ms {
                    let call_model = if harness == "codex" {
                        model.as_deref()
                    } else {
                        entry.model.as_deref()
                    };
                    events(tx, path_text, harness, &entry, call_model, at_ms)?;
                }
            }
            offset += line.len() as u64;
        }
        tx.execute(
            "UPDATE transcripts SET offset=?2,indexed_ms=?3,model=?4 WHERE path=?1",
            params![
                path_text,
                crate::rows::integer(offset)?,
                crate::rows::integer(now)?,
                model
            ],
        )?;
        Ok(())
    })
}

fn header(
    tx: &Transaction<'_>,
    path: &str,
    harness: &str,
    value: &Value,
    shell: &crate::shell::System,
) -> Result<()> {
    let metadata = if harness == "codex" && value["type"] == "session_meta" {
        &value["payload"]
    } else {
        value
    };
    let session = match harness {
        "pi" if metadata["type"] == "session" => metadata["id"].as_str(),
        "claude" => metadata["sessionId"].as_str(),
        "codex" if value["type"] == "session_meta" => metadata["id"].as_str(),
        _ => None,
    };
    let cwd = metadata["cwd"].as_str();
    if session.is_some() || cwd.is_some() {
        let previous: Option<String> =
            tx.query_row("SELECT cwd FROM transcripts WHERE path=?1", [path], |row| {
                row.get(0)
            })?;
        let repository = if previous.is_none() {
            cwd.map(|cwd| repository(shell, cwd)).transpose()?.flatten()
        } else {
            None
        };
        tx.execute("UPDATE transcripts SET session=COALESCE(session,?2),cwd=COALESCE(cwd,?3),repository=COALESCE(repository,?4) WHERE path=?1", params![path,session,cwd,repository])?;
    }
    Ok(())
}

pub(super) fn repository(shell: &crate::shell::System, cwd: &str) -> Result<Option<String>> {
    if !Path::new(cwd).is_dir() {
        return Ok(None);
    }
    let output = shell.run(
        "git",
        &[
            "-C".into(),
            cwd.into(),
            "remote".into(),
            "get-url".into(),
            "origin".into(),
        ],
    )?;
    if !output.ok() {
        return Ok(None);
    }
    Ok(crate::checkout::identity(output.stdout.trim())
        .and_then(|identity| identity.split_once('/').map(|(_, slug)| slug.to_owned())))
}

fn text_chars(content: &Value, text_block: &str) -> u64 {
    if let Some(text) = content.as_str() {
        return text.chars().count() as u64;
    }
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == text_block)
        .filter_map(|block| block["text"].as_str())
        .map(|text| text.chars().count() as u64)
        .sum()
}

// Pi gives each injection a separate entry, including those hidden from the UI.
// The shared decoder does not identify Claude or Codex hook injections.
fn pi_injection(tx: &Transaction<'_>, path: &str, value: &Value) -> Result<()> {
    let timestamp = &value["timestamp"];
    let at_ms = timestamp.as_u64().or_else(|| {
        let time = chrono::DateTime::parse_from_rfc3339(timestamp.as_str()?).ok()?;
        u64::try_from(time.timestamp_millis()).ok()
    });
    if let Some((kind, at_ms)) = value["customType"].as_str().zip(at_ms) {
        tx.execute(
            "INSERT INTO injections(path,kind,at_ms,chars) VALUES(?1,?2,?3,?4)",
            params![
                path,
                kind,
                crate::rows::integer(at_ms)?,
                crate::rows::integer(text_chars(&value["content"], "text"))?
            ],
        )?;
    }
    Ok(())
}

fn events(
    tx: &Transaction<'_>,
    path: &str,
    harness: &str,
    entry: &crate::activity::DecodedEntry<'_>,
    model: Option<&str>,
    at_ms: u64,
) -> Result<()> {
    let at_ms = crate::rows::integer(at_ms)?;
    if entry.input {
        tx.execute(
            "INSERT OR IGNORE INTO input_entries(path,at_ms) VALUES(?1,?2)",
            params![path, at_ms],
        )?;
    }
    for tool in &entry.tools {
        if tool.status == ToolStatus::Unknown {
            if let Some(name) = &tool.name {
                tx.execute("INSERT OR IGNORE INTO tool_calls(path,call_id,tool,model,started_ms) VALUES(?1,?2,?3,?4,?5)", params![path,tool.call_id,name,model,at_ms])?;
            }
        } else {
            let error = (harness != "codex").then_some(tool.status == ToolStatus::Error);
            let (content, text_block) = if harness == "codex" {
                ("output", "input_text")
            } else {
                ("content", "text")
            };
            let chars = crate::rows::integer(text_chars(&tool.block[content], text_block))?;
            tx.execute("UPDATE tool_calls SET ended_ms=?3,error=?4,result_chars=?5 WHERE path=?1 AND call_id=?2 AND ended_ms IS NULL", params![path,tool.call_id,at_ms,error,chars])?;
        }
    }
    Ok(())
}
