//! Resolve an exact session identity, never a latest-session guess.

use std::io::{BufRead as _, Read as _};
use std::path::{Path, PathBuf};

/// The longest session header line read. Evidence reads use the same 1 MiB line limit.
const MAX_HEADER: u64 = 1024 * 1024;

/// The session ID in the header of the Pi transcript at `path`: its first line, a complete
/// `session` entry. A link, a non-file, an oversized, partial, or malformed header is an error.
pub fn pi_session_id(path: &str) -> crate::error::Result<String> {
    let invalid =
        |detail: &str| crate::error::Error::Invalid(format!("Pi transcript {path}: {detail}"));
    if !std::fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(invalid("not a regular file"));
    }
    let mut line = String::new();
    std::io::BufReader::new(std::fs::File::open(path)?.take(MAX_HEADER + 1))
        .read_line(&mut line)?;
    if line.len() as u64 > MAX_HEADER || !line.ends_with('\n') {
        return Err(invalid("the session header is partial or too long"));
    }
    let header: serde_json::Value =
        serde_json::from_str(&line).map_err(|_| invalid("the session header is malformed"))?;
    match (header["type"].as_str(), header["id"].as_str()) {
        (Some("session"), Some(id)) if !id.is_empty() => Ok(id.to_owned()),
        _ => Err(invalid("the first entry is not a session header")),
    }
}

pub fn resolve(agent: Option<&str>, kind: &str, value: &str) -> Option<String> {
    if kind == "path" {
        return Some(value.into());
    }
    if kind != "id"
        || value.is_empty()
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return None;
    }
    // Only validated UUID session IDs can name a transcript. Harness-private
    // identifiers or future formats stay unstalled rather than matching by guess.
    if value.len() != 36
        || !value.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
    {
        return None;
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let (root, depth, suffix) = match agent? {
        "claude" => (
            std::env::var_os("CLAUDE_CONFIG_DIR")
                .map_or_else(|| home.join(".claude"), PathBuf::from)
                .join("projects"),
            1,
            format!("{value}.jsonl"),
        ),
        "codex" => (
            std::env::var_os("CODEX_HOME")
                .map_or_else(|| home.join(".codex"), PathBuf::from)
                .join("sessions"),
            3,
            format!("-{value}.jsonl"),
        ),
        _ => return None,
    };
    let mut matches = Vec::new();
    find(&root, depth, &suffix, agent == Some("claude"), &mut matches)?;
    (matches.len() == 1).then(|| matches[0].display().to_string())
}

fn find(
    root: &Path,
    depth: usize,
    suffix: &str,
    exact: bool,
    matches: &mut Vec<PathBuf>,
) -> Option<()> {
    for entry in std::fs::read_dir(root).ok()? {
        let entry = entry.ok()?;
        let kind = entry.file_type().ok()?;
        if kind.is_dir() && depth > 0 {
            find(&entry.path(), depth - 1, suffix, exact, matches)?;
        } else if kind.is_file() {
            let name = entry.file_name();
            let name = name.to_str()?;
            if (exact && name == suffix)
                || (!exact && name.starts_with("rollout-") && name.ends_with(suffix))
            {
                matches.push(entry.path());
            }
        }
    }
    Some(())
}
