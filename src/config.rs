//! Host policy, including the prepare hook.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Default `prepare` hook: the agent harness task that creates a writer's
/// worktree and linked Herdr workspace, printing `{pane_id, workspace_id}`.
pub const DEFAULT_PREPARE: &[&str] = &[
    "mise",
    "run",
    "-q",
    "agent:worktree",
    "--",
    "{branch}",
    "{base}",
];

/// Default `remove` hook: the agent harness task that removes a branch worktree with the
/// repository's remove hooks, unlocking an `agent:worktree` lock first.
pub const DEFAULT_REMOVE: &[&str] = &[
    "mise",
    "run",
    "-q",
    "agent:worktree-remove",
    "--",
    "{branch}",
];

/// Default way to run `wrangle` on a remote host.
pub const DEFAULT_REMOTE_COMMAND: &[&str] = &["mise", "x", "--", "wrangle"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    /// Admit while `load1 / cores` stays below this.
    pub load_per_core_max: f64,
    /// Admit while the home filesystem keeps at least this much free.
    pub disk_free_min_percent: f64,
    /// Load units each unexpired reservation adds to admission.
    pub reservation_load: f64,
    /// A reservation older than this is dropped on the next probe.
    pub reservation_ttl_ms: u64,
    /// argv prefix that reaches `wrangle` on a remote host over
    /// `ssh -T -o BatchMode=yes`. mise shims are not on the non-interactive
    /// PATH, but `mise` itself is, so the default goes through `mise x`.
    pub remote_command: Vec<String>,
    pub hooks: Hooks,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Hooks {
    /// argv template run in the checkout. Placeholders: `{branch}` and `{base}`.
    /// Must print JSON with `pane_id` or `path`.
    pub prepare: Vec<String>,
    /// argv template run in the main checkout to remove a linked worktree after wrangle
    /// closed its panes and workspace. Placeholders: `{branch}` and `{path}`. It must remove
    /// a worktree that carries an `agent:worktree` lock, and keep an unmerged branch.
    pub remove: Vec<String>,
}

impl Hooks {
    pub fn prepare_argv(&self, branch: &str, base: Option<&str>) -> Result<Vec<String>> {
        render(
            "prepare",
            &self.prepare,
            &[("{branch}", Some(branch)), ("{base}", base)],
        )
    }

    pub fn remove_argv(&self, branch: &str, path: &str) -> Result<Vec<String>> {
        render(
            "remove",
            &self.remove,
            &[("{branch}", Some(branch)), ("{path}", Some(path))],
        )
    }
}

/// Parse the template once. Substituted values are data, not templates. A word that is
/// exactly a placeholder without a value is dropped.
fn render(hook: &str, template: &[String], values: &[(&str, Option<&str>)]) -> Result<Vec<String>> {
    if template.is_empty() {
        return Err(Error::Invalid(format!("hooks.{hook} is empty")));
    }
    let value = |placeholder: &str| values.iter().find(|(name, _)| *name == placeholder);
    template
        .iter()
        .filter(|word| !value(word).is_some_and(|(_, v)| v.is_none()))
        .map(|word| {
            let mut output = String::new();
            let mut rest = word.as_str();
            while let Some(start) = rest.find('{') {
                output.push_str(&rest[..start]);
                let end = rest[start..]
                    .find('}')
                    .map(|end| start + end)
                    .ok_or_else(|| {
                        Error::Invalid(format!("unclosed hook placeholder {}", &rest[start..]))
                    })?;
                let placeholder = &rest[start..=end];
                let (_, substituted) = value(placeholder).ok_or_else(|| {
                    Error::Invalid(format!("unknown hook placeholder {placeholder}"))
                })?;
                output.push_str(substituted.unwrap_or(""));
                rest = &rest[end + 1..];
            }
            output.push_str(rest);
            Ok(output)
        })
        .collect()
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            load_per_core_max: 1.5,
            disk_free_min_percent: 15.0,
            reservation_load: 1.0,
            reservation_ttl_ms: 90_000,
            remote_command: DEFAULT_REMOTE_COMMAND
                .iter()
                .map(ToString::to_string)
                .collect(),
            hooks: Hooks::default(),
        }
    }
}

impl Default for Hooks {
    fn default() -> Self {
        Self {
            prepare: DEFAULT_PREPARE.iter().map(ToString::to_string).collect(),
            remove: DEFAULT_REMOVE.iter().map(ToString::to_string).collect(),
        }
    }
}

/// The subset of the policy a remote host needs to re-check admission under
/// its own lock. Serialised on the command line of `wrangle host reserve`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Thresholds {
    pub load_per_core_max: f64,
    pub disk_free_min_percent: f64,
    pub reservation_load: f64,
    pub reservation_ttl_ms: u64,
}

impl From<&Policy> for Thresholds {
    fn from(p: &Policy) -> Self {
        Self {
            load_per_core_max: p.load_per_core_max,
            disk_free_min_percent: p.disk_free_min_percent,
            reservation_load: p.reservation_load,
            reservation_ttl_ms: p.reservation_ttl_ms,
        }
    }
}

/// `$WRANGLE_CONFIG`, else `$XDG_CONFIG_HOME/wrangle/config.toml`, else
/// `~/.config/wrangle/config.toml`.
#[must_use]
pub fn default_path() -> PathBuf {
    if let Some(p) = std::env::var_os("WRANGLE_CONFIG") {
        return PathBuf::from(p);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("wrangle").join("config.toml")
}

/// A missing file means defaults. A present but invalid file is an error, so a
/// typo never silently reverts to the defaults.
pub fn load(path: &Path) -> Result<Policy> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text).map_err(|source| Error::Config {
            path: path.display().to_string(),
            source,
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Policy::default()),
        Err(e) => Err(e.into()),
    }
}

pub fn parse(text: &str) -> std::result::Result<Policy, toml::de::Error> {
    toml::from_str(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_is_defaults() {
        assert_eq!(parse("").unwrap(), Policy::default());
    }

    #[test]
    fn partial_override_keeps_other_defaults() {
        let p = parse("load_per_core_max = 2.0\n[hooks]\nprepare = [\"true\"]\n").unwrap();
        assert!((p.load_per_core_max - 2.0).abs() < f64::EPSILON);
        assert!((p.disk_free_min_percent - 15.0).abs() < f64::EPSILON);
        assert_eq!(p.hooks.prepare, vec!["true".to_string()]);
    }

    #[test]
    fn remove_hook_defaults_to_the_agent_task_and_takes_branch_and_path() {
        let hooks = Hooks::default();
        assert_eq!(
            hooks.remove_argv("feat/{path}", "/r/t").unwrap(),
            [
                "mise",
                "run",
                "-q",
                "agent:worktree-remove",
                "--",
                "feat/{path}"
            ]
        );
        let hooks = parse("[hooks]\nremove = [\"git\", \"worktree\", \"remove\", \"{path}\"]\n")
            .unwrap()
            .hooks;
        assert_eq!(
            hooks.remove_argv("b", "/r/t").unwrap(),
            ["git", "worktree", "remove", "/r/t"]
        );
        assert_eq!(hooks.prepare, Hooks::default().prepare);
        let hooks = parse("[hooks]\nremove = [\"x\", \"{base}\"]\n")
            .unwrap()
            .hooks;
        assert!(
            hooks
                .remove_argv("b", "/p")
                .unwrap_err()
                .to_string()
                .contains("{base}")
        );
    }

    #[test]
    fn unknown_key_is_rejected() {
        assert!(parse("max_agents = 3\n").is_err());
    }

    #[test]
    fn missing_file_is_defaults() {
        let p = load(Path::new("/nonexistent/wrangle/config.toml")).unwrap();
        assert_eq!(p, Policy::default());
    }
}
