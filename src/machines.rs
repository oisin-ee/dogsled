//! The saved Herdr machines that wrangle uses as hosts. Rename, enable, and disable run at
//! once. Add and reconnect can ask for SSH authentication, so they run in a new Herdr tab
//! that the user answers; nothing types into an agent's pane.

use crate::error::{Error, Result};
use crate::shell::{Shell, quote};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Machine {
    pub id: String,
    pub label: String,
    pub target: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

fn enabled() -> bool {
    true
}

/// Every saved machine, enabled or not.
pub fn list(shell: &dyn Shell) -> Result<Vec<Machine>> {
    let out = shell.run_ok("herdr", &["machine".into(), "list".into(), "--json".into()])?;
    Ok(serde_json::from_str(&out.stdout)?)
}

fn find(shell: &dyn Shell, machine: &str) -> Result<Machine> {
    list(shell)?
        .into_iter()
        .find(|m| m.id == machine || m.label == machine)
        .ok_or_else(|| Error::UnknownMachine(machine.into()))
}

/// Labels name hosts in every view, so they stay short and printable.
fn valid_label(label: &str) -> Result<()> {
    if label.is_empty()
        || label.len() > 64
        || !label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(Error::Invalid(format!(
            "machine label must be 1-64 characters from [A-Za-z0-9._-], not {label:?}"
        )));
    }
    Ok(())
}

pub fn rename(shell: &dyn Shell, machine: &str, label: &str) -> Result<Machine> {
    valid_label(label)?;
    let found = find(shell, machine)?;
    shell.run_ok(
        "herdr",
        &[
            "machine".into(),
            "rename".into(),
            "--label".into(),
            label.into(),
            found.id.clone(),
        ],
    )?;
    find(shell, &found.id)
}

pub fn set_enabled(shell: &dyn Shell, machine: &str, on: bool) -> Result<Machine> {
    let found = find(shell, machine)?;
    let verb = if on { "enable" } else { "disable" };
    shell.run_ok("herdr", &["machine".into(), verb.into(), found.id.clone()])?;
    find(shell, &found.id)
}

/// Open a focused tab in the caller's workspace and run `command` there, for the user to
/// answer SSH prompts. Returns the new pane.
fn in_tab(shell: &dyn Shell, label: &str, command: &str) -> Result<String> {
    let workspace = std::env::var("HERDR_WORKSPACE_ID")
        .map_err(|_| Error::Invalid("HERDR_WORKSPACE_ID is required to open a tab".into()))?;
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    let out = shell.run_ok(
        "herdr",
        &[
            "tab".into(),
            "create".into(),
            "--workspace".into(),
            workspace,
            "--cwd".into(),
            home,
            "--label".into(),
            label.into(),
            "--focus".into(),
        ],
    )?;
    let value: Value = serde_json::from_str(&out.stdout)?;
    let pane = value
        .pointer("/result/root_pane/pane_id")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Invalid(format!("herdr tab create returned no pane: {value}")))?
        .to_owned();
    shell.run_ok(
        "herdr",
        &["pane".into(), "run".into(), pane.clone(), command.into()],
    )?;
    Ok(pane)
}

pub fn reconnect(shell: &dyn Shell, machine: &str) -> Result<String> {
    let found = find(shell, machine)?;
    in_tab(
        shell,
        &format!("reconnect {}", found.label),
        &format!("herdr machine reconnect {}", quote(&found.id)),
    )
}

pub fn add(shell: &dyn Shell, target: &str, label: Option<&str>) -> Result<String> {
    if target.is_empty() || target.starts_with('-') || target.contains(char::is_whitespace) {
        return Err(Error::Invalid(format!("not an SSH target: {target:?}")));
    }
    let command = match label {
        Some(label) => {
            valid_label(label)?;
            format!(
                "herdr machine add {} --label {}",
                quote(target),
                quote(label)
            )
        }
        None => format!("herdr machine add {}", quote(target)),
    };
    in_tab(shell, &format!("add {target}"), &command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_short_printable_words() {
        assert!(valid_label("netcup-2").is_ok());
        for bad in ["", "a b", "x;rm", &"a".repeat(65)] {
            assert!(valid_label(bad).is_err(), "{bad}");
        }
    }
}
