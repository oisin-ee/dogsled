//! Agent profiles: a model, a thinking level, and a system prompt for a Pi child. Each lives in
//! `<config dir>/profiles/<name>/{profile.json,prompt.md}` on every host, in Shepherdr's format.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One profile. Unknown keys from Shepherdr's format (`accepts`, `prepare`, `blocking`) are
/// ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// The system prompt files, relative to the profile's directory: one name or a list. Each
    /// file is appended in order, so profiles can share a prompt such as `../_shared/lead.md`.
    #[serde(default, skip_serializing_if = "Prompts::is_empty")]
    pub prompt: Prompts,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pi_args: Vec<String>,
    /// `reader` or `writer`. Without it, a child with a branch is a writer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// The resolved prompt files, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompt_paths: Vec<String>,
    /// The profile's own prompt: the last file, which the board's profile panel edits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_path: Option<String>,
}

/// `"prompt": "prompt.md"` or `"prompt": ["../_shared/lead.md", "prompt.md"]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Prompts {
    #[default]
    None,
    One(String),
    Many(Vec<String>),
}

impl Prompts {
    fn is_empty(&self) -> bool {
        self.files().is_empty()
    }

    fn files(&self) -> &[String] {
        match self {
            Self::None => &[],
            Self::One(file) => std::slice::from_ref(file),
            Self::Many(files) => files,
        }
    }
}

/// `~/.config/dogsled/profiles`, beside the config file.
pub fn dir() -> PathBuf {
    crate::config::default_path()
        .parent()
        .map_or_else(|| PathBuf::from("profiles"), |p| p.join("profiles"))
}

fn valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn load_from(dir: &Path, name: &str) -> Result<Profile> {
    if !valid(name) {
        return Err(Error::Invalid(format!("invalid profile name {name:?}")));
    }
    let home = dir.join(name);
    let file = home.join("profile.json");
    let text = std::fs::read_to_string(&file).map_err(|e| {
        Error::Invalid(format!(
            "profile {name}: cannot read {}: {e}; see `dogsled profiles`",
            file.display()
        ))
    })?;
    let mut profile: Profile = serde_json::from_str(&text)
        .map_err(|e| Error::Invalid(format!("profile {name}: {}: {e}", file.display())))?;
    profile.name = name.into();
    if let Some(role) = &profile.role
        && !matches!(role.as_str(), "reader" | "writer")
    {
        return Err(Error::Invalid(format!(
            "profile {name}: role must be reader or writer, not {role}"
        )));
    }
    for prompt in profile.prompt.files() {
        let path = home.join(prompt);
        if !path.is_file() {
            return Err(Error::Invalid(format!(
                "profile {name}: prompt {} does not exist",
                path.display()
            )));
        }
        profile.prompt_paths.push(path.display().to_string());
    }
    profile.prompt_path = profile.prompt_paths.last().cloned();
    Ok(profile)
}

pub fn load(name: &str) -> Result<Profile> {
    load_from(&dir(), name)
}

/// Every profile in `dir`, by name. A broken profile is listed with its error as description.
pub fn list_from(dir: &Path) -> Result<Vec<Profile>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| valid(n) && dir.join(n).join("profile.json").is_file())
        .collect();
    names.sort();
    Ok(names
        .into_iter()
        .map(|name| {
            load_from(dir, &name).unwrap_or_else(|e| Profile {
                name,
                description: format!("broken: {e}"),
                ..Profile::default()
            })
        })
        .collect())
}

pub fn list() -> Result<Vec<Profile>> {
    list_from(&dir())
}

impl Profile {
    /// The Pi flags this profile sets, before the session name and the task.
    pub fn pi_flags(&self) -> Vec<String> {
        let mut args = Vec::new();
        for (flag, value) in [("--model", &self.model), ("--thinking", &self.thinking)] {
            if let Some(value) = value {
                args.extend([flag.to_owned(), value.clone()]);
            }
        }
        for path in &self.prompt_paths {
            args.extend(["--append-system-prompt".to_owned(), path.clone()]);
        }
        args.extend(self.pi_args.iter().cloned());
        args
    }
}

pub fn render(profiles: &[Profile]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for p in profiles {
        let _ = writeln!(
            out,
            "{:<18} {:<14} {:<7} {}",
            p.name,
            p.model.as_deref().unwrap_or("-"),
            p.thinking.as_deref().unwrap_or("-"),
            p.description
        );
    }
    if out.is_empty() {
        let _ = writeln!(out, "No profiles in {}.", dir().display());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, json: &str, prompt: bool) {
        let home = dir.join(name);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("profile.json"), json).unwrap();
        if prompt {
            std::fs::write(home.join("prompt.md"), "Be brief.").unwrap();
        }
    }

    #[test]
    fn shepherdr_profiles_load_and_map_to_pi_flags() {
        let dir = std::env::temp_dir().join(crate::ledger::new_id());
        write(
            &dir,
            "reviewer",
            r#"{"description":"Read-only review","model":"alias/sol","thinking":"medium",
                "prompt":"prompt.md","prepare":"prepare.mjs","accepts":["base"],"pi_args":["--no-tools"]}"#,
            true,
        );
        write(&dir, "bare", r#"{"description":"Defaults"}"#, false);
        write(&dir, "broken", r#"{"prompt":"missing.md"}"#, false);
        write(&dir, "odd", r#"{"role":"boss"}"#, false);
        let reviewer = load_from(&dir, "reviewer").unwrap();
        let prompt = dir.join("reviewer/prompt.md").display().to_string();
        assert_eq!(
            reviewer.pi_flags(),
            [
                "--model",
                "alias/sol",
                "--thinking",
                "medium",
                "--append-system-prompt",
                &prompt,
                "--no-tools"
            ]
        );
        assert_eq!(load_from(&dir, "bare").unwrap().pi_flags(), [] as [&str; 0]);
        std::fs::create_dir_all(dir.join("_shared")).unwrap();
        std::fs::write(dir.join("_shared/lead.md"), "Lead.").unwrap();
        write(
            &dir,
            "lead",
            r#"{"prompt":["../_shared/lead.md","prompt.md"]}"#,
            true,
        );
        let lead = load_from(&dir, "lead").unwrap();
        let shared = dir.join("lead/../_shared/lead.md").display().to_string();
        let own = dir.join("lead/prompt.md").display().to_string();
        assert_eq!(
            lead.pi_flags(),
            [
                "--append-system-prompt",
                &shared,
                "--append-system-prompt",
                &own
            ]
        );
        assert_eq!(lead.prompt_path.as_deref(), Some(own.as_str()));
        write(&dir, "half", r#"{"prompt":["prompt.md","gone.md"]}"#, true);
        assert!(
            load_from(&dir, "half")
                .unwrap_err()
                .to_string()
                .contains("gone.md does not exist")
        );
        assert!(
            load_from(&dir, "broken")
                .unwrap_err()
                .to_string()
                .contains("does not exist")
        );
        assert!(
            load_from(&dir, "odd")
                .unwrap_err()
                .to_string()
                .contains("reader or writer")
        );
        assert!(load_from(&dir, "../etc").is_err());
        assert!(
            load_from(&dir, "absent")
                .unwrap_err()
                .to_string()
                .contains("cannot read")
        );
        let names: Vec<_> = list_from(&dir)
            .unwrap()
            .into_iter()
            .map(|p| (p.name, p.description.starts_with("broken")))
            .collect();
        assert_eq!(
            names,
            [
                ("bare".into(), false),
                ("broken".into(), true),
                ("half".into(), true),
                ("lead".into(), false),
                ("odd".into(), true),
                ("reviewer".into(), false)
            ]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
