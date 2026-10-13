//! Repository identity and home-relative checkout provisioning.

use crate::error::{Error, Result};
use crate::hook;
use crate::hosts::{Fleet, Host};
use crate::shell::Shell;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    pub path: String,
    pub origin: String,
    pub relative: String,
    pub working_root: String,
}

pub fn discover(
    shell: &dyn Shell,
    cwd: &str,
    home: &str,
    required: bool,
) -> Result<Option<Repository>> {
    let check = shell.run(
        "git",
        &[
            "-C".into(),
            cwd.into(),
            "rev-parse".into(),
            "--is-inside-work-tree".into(),
        ],
    )?;
    if !check.ok() {
        if required {
            return Err(Error::Invalid(format!(
                "not a repository: {cwd}: {}",
                check.stderr.trim()
            )));
        }
        return Ok(None);
    }
    let path = hook::main_checkout(shell, cwd)?;
    let origin = shell
        .run_ok(
            "git",
            &[
                "-C".into(),
                path.display().to_string(),
                "remote".into(),
                "get-url".into(),
                "origin".into(),
            ],
        )?
        .stdout
        .trim()
        .to_owned();
    if origin.is_empty() {
        return Err(Error::Invalid("repository origin is empty".into()));
    }
    let home = Path::new(home)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(home));
    let relative = path
        .strip_prefix(home)
        .unwrap_or(&path)
        .display()
        .to_string();
    let working_root = shell
        .run_ok(
            "git",
            &[
                "-C".into(),
                cwd.into(),
                "rev-parse".into(),
                "--show-toplevel".into(),
            ],
        )?
        .stdout
        .trim()
        .to_owned();
    Ok(Some(Repository {
        path: path.display().to_string(),
        origin,
        relative,
        working_root,
    }))
}

pub fn ensure(fleet: &Fleet, host: &Host, repo: &Repository) -> Result<String> {
    if host.is_local() {
        return local(
            fleet.shell,
            &fleet.home,
            &host.label,
            &repo.path,
            &repo.origin,
        );
    }
    fleet.remote(
        host,
        &[
            "checkout".into(),
            "--path".into(),
            repo.relative.clone(),
            "--origin".into(),
            repo.origin.clone(),
            "--host-label".into(),
            host.label.clone(),
        ],
    )
}

pub fn local(
    shell: &dyn Shell,
    home: &str,
    host: &str,
    path: &str,
    origin: &str,
) -> Result<String> {
    let path = Path::new(home).join(path);
    let display = path.display().to_string();
    if path.exists() {
        let actual = shell.run(
            "git",
            &[
                "-C".into(),
                display.clone(),
                "remote".into(),
                "get-url".into(),
                "origin".into(),
            ],
        )?;
        let root = shell.run(
            "git",
            &[
                "-C".into(),
                display.clone(),
                "rev-parse".into(),
                "--show-toplevel".into(),
            ],
        )?;
        let same_root = root.ok()
            && Path::new(root.stdout.trim()).canonicalize().ok() == path.canonicalize().ok();
        if !actual.ok() || actual.stdout.trim() != origin || !same_root {
            return Err(Error::Invalid(format!(
                "{host}: checkout {display}: expected origin {origin}, found {}",
                if actual.ok() {
                    actual.stdout.trim()
                } else {
                    "<not a git checkout>"
                }
            )));
        }
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let cloned = shell.run(
            "git",
            &["clone".into(), "--".into(), origin.into(), display.clone()],
        )?;
        if !cloned.ok() {
            return Err(Error::Invalid(format!(
                "{host}: clone {origin} into {display} failed: {}",
                cloned.stderr.trim()
            )));
        }
        // Only the new checkout may acquire trust. Never trust ancestors or all configs.
        let checked = shell.run(
            "mise",
            &[
                "-C".into(),
                display.clone(),
                "config".into(),
                "ls".into(),
                "--json".into(),
            ],
        )?;
        if !checked.ok() {
            let error = checked.stderr.to_lowercase();
            if !error.contains("trust") {
                return Err(Error::Invalid(format!(
                    "{host}: mise config {display}: {}",
                    checked.stderr.trim()
                )));
            }
            let config = [
                "mise.toml",
                ".mise.toml",
                ".config/mise.toml",
                ".config/mise/config.toml",
            ]
            .into_iter()
            .map(|p| path.join(p))
            .find(|p| p.is_file())
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "{host}: untrusted config outside new checkout {display}"
                ))
            })?;
            shell.run_ok("mise", &["trust".into(), config.display().to_string()])?;
            shell.run_ok(
                "mise",
                &[
                    "-C".into(),
                    display.clone(),
                    "config".into(),
                    "ls".into(),
                    "--json".into(),
                ],
            )?;
        }
    }
    Ok(display)
}
