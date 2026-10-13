//! Resolve an exact session identity, never a latest-session guess.

use std::path::{Path, PathBuf};

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
