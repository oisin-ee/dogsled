//! Prepare work in the repository checkout, never in the caller's worktree.

use crate::config;
use crate::error::{Error, Result};
use crate::hosts::{Fleet, Host};
use crate::shell::{Shell, quote};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prepared {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

pub fn main_checkout(shell: &dyn Shell, repo: &str) -> Result<PathBuf> {
    let out = shell.run_ok(
        "git",
        &[
            "-C".into(),
            repo.into(),
            "rev-parse".into(),
            "--path-format=absolute".into(),
            "--git-common-dir".into(),
        ],
    )?;
    let path = Path::new(out.stdout.trim());
    if path.as_os_str().is_empty() {
        return Err(Error::Invalid("git returned no checkout".into()));
    }
    Ok(if path.file_name().is_some_and(|name| name == ".git") {
        path.parent()
            .ok_or_else(|| Error::Invalid("git returned no checkout parent".into()))?
            .to_path_buf()
    } else {
        path.to_path_buf()
    })
}

pub fn run(
    fleet: &Fleet,
    host: &Host,
    repo: &str,
    branch: &str,
    base: Option<&str>,
) -> Result<Prepared> {
    let checkout = main_checkout(fleet.shell, repo)?;
    if host.is_local() {
        return local(
            fleet.shell,
            fleet.policy,
            &checkout.display().to_string(),
            branch,
            base,
        );
    }
    let home = Path::new(&fleet.home);
    let canonical = home.canonicalize().ok();
    let remote = checkout
        .strip_prefix(home)
        .ok()
        .or_else(|| {
            canonical
                .as_deref()
                .and_then(|home| checkout.strip_prefix(home).ok())
        })
        .unwrap_or(&checkout)
        .display()
        .to_string();
    let mut args = vec![
        "prepare".into(),
        "--repo".into(),
        remote,
        "--branch".into(),
        branch.into(),
    ];
    if let Some(base) = base {
        args.extend(["--base".into(), base.into()]);
    }
    fleet.remote(host, &args)
}

pub fn local(
    shell: &dyn Shell,
    policy: &config::Policy,
    repo: &str,
    branch: &str,
    base: Option<&str>,
) -> Result<Prepared> {
    let checkout = main_checkout(shell, repo)?;
    let argv = render(&policy.hooks.prepare, branch, base)?;
    let command = format!(
        "cd {} && exec {}",
        quote(&checkout.display().to_string()),
        argv.iter().map(|w| quote(w)).collect::<Vec<_>>().join(" ")
    );
    let out = shell.run_ok("sh", &["-c".into(), command])?;
    parse_output(&out.stdout)
}

/// Parse the template once. Substituted values are data, not templates.
fn render(template: &[String], branch: &str, base: Option<&str>) -> Result<Vec<String>> {
    if template.is_empty() {
        return Err(Error::Invalid("hooks.prepare is empty".into()));
    }
    template
        .iter()
        .filter(|word| !(word.as_str() == "{base}" && base.is_none()))
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
                output.push_str(match placeholder {
                    "{branch}" => branch,
                    "{base}" => base.unwrap_or(""),
                    _ => {
                        return Err(Error::Invalid(format!(
                            "unknown hook placeholder {placeholder}"
                        )));
                    }
                });
                rest = &rest[end + 1..];
            }
            output.push_str(rest);
            Ok(output)
        })
        .collect()
}

pub fn parse_output(stdout: &str) -> Result<Prepared> {
    for text in std::iter::once(stdout.trim()).chain(stdout.lines().rev().map(str::trim)) {
        if let Ok(prepared) = serde_json::from_str::<Prepared>(text)
            && (prepared.pane_id.is_some() || prepared.path.is_some())
        {
            return Ok(prepared);
        }
    }
    Err(Error::Hook {
        detail: stdout.trim().lines().last().unwrap_or("no output").into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_template_and_embedded_values_are_literal() {
        let template = config::Policy::default().hooks.prepare;
        assert_eq!(
            render(&template, "feat/{base}", Some("main/{branch}")).unwrap(),
            [
                "mise",
                "run",
                "-q",
                "agent:worktree",
                "--",
                "feat/{base}",
                "main/{branch}"
            ]
        );
        assert_eq!(
            render(&template, "feat/{base}", None).unwrap(),
            ["mise", "run", "-q", "agent:worktree", "--", "feat/{base}"]
        );
        assert_eq!(
            render(&["prefix:{base}".into(), String::new()], "b", None).unwrap(),
            ["prefix:", ""]
        );
        assert!(
            render(&["{repo}".into()], "b", None)
                .unwrap_err()
                .to_string()
                .contains("{repo}")
        );
        assert!(render(&["{unclosed".into()], "b", None).is_err());
    }

    #[test]
    fn prepare_accepts_path_or_pane_and_rejects_empty_results() {
        assert_eq!(
            parse_output("progress\n{\"path\":\"/repo/work\"}\n")
                .unwrap()
                .path
                .as_deref(),
            Some("/repo/work")
        );
        assert_eq!(
            parse_output("{\"pane_id\":\"w1:p1\"}")
                .unwrap()
                .pane_id
                .as_deref(),
            Some("w1:p1")
        );
        assert!(parse_output("{}").is_err());
    }
}
