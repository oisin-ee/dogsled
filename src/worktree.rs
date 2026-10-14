//! A linked worktree, the Herdr panes that hold it, and its removal through the repository's
//! `remove` hook. `close` and `prune` share these rules.

use crate::config::Hooks;
use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::probe::Pane;
use crate::shell::{Shell, quote};
use std::path::{Path, PathBuf};

/// The lock reason that marks an agent-owned worktree. Any other lock belongs to someone else.
pub const AGENT_LOCK: &str = "agent:worktree";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Linked {
    /// The main checkout.
    pub checkout: PathBuf,
    pub path: String,
    pub branch: String,
}

fn git(shell: &dyn Shell, dir: &Path, args: &[&str]) -> Result<crate::shell::Output> {
    let mut words = vec!["-C".to_string(), dir.display().to_string()];
    words.extend(args.iter().map(ToString::to_string));
    shell.run("git", &words)
}

pub fn refuse(message: &str) -> Error {
    Error::Invalid(format!("refuse: {message}; nothing was closed"))
}

/// Resolve symlinks on the longest existing ancestor, so a missing subdirectory still matches.
pub fn resolved(path: &Path) -> PathBuf {
    for ancestor in path.ancestors() {
        if let Ok(root) = ancestor.canonicalize() {
            return path
                .strip_prefix(ancestor)
                .map_or(root.clone(), |suffix| root.join(suffix));
        }
    }
    path.to_path_buf()
}

pub fn same(a: &str, b: &str) -> bool {
    a == b || resolved(Path::new(a)) == resolved(Path::new(b))
}

fn inside(cwd: &str, tree: &str) -> bool {
    resolved(Path::new(cwd)).starts_with(resolved(Path::new(tree)))
}

/// The worktree that contains `cwd`, when it is a linked worktree on a branch.
pub fn linked(shell: &dyn Shell, cwd: &str) -> Result<Option<Linked>> {
    if !Path::new(cwd).is_dir() {
        return Ok(None);
    }
    let out = git(
        shell,
        Path::new(cwd),
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
            "--git-dir",
            "--show-toplevel",
            "--abbrev-ref",
            "HEAD",
        ],
    )?;
    // Not a repository: there is no worktree to remove.
    if out.status != 0 {
        return Ok(None);
    }
    let lines: Vec<&str> = out.stdout.lines().map(str::trim).collect();
    let [common, dir, top, branch] = lines[..] else {
        return Err(Error::Invalid(format!(
            "git rev-parse in {cwd} returned unexpected output"
        )));
    };
    if common == dir {
        return Ok(None);
    }
    if branch == "HEAD" {
        return Err(refuse(&format!(
            "{top} is a detached worktree; the remove hook needs a branch"
        )));
    }
    Ok(Some(Linked {
        checkout: crate::prepare::main_checkout(shell, cwd)?,
        path: top.into(),
        branch: branch.into(),
    }))
}

/// The lock reason of `path`, if it is locked.
pub fn lock_reason(shell: &dyn Shell, checkout: &Path, path: &str) -> Result<Option<String>> {
    let out = git(shell, checkout, &["worktree", "list", "--porcelain"])?;
    let mut current = None;
    for line in out.stdout.lines() {
        if let Some(found) = line.strip_prefix("worktree ") {
            current = Some(found);
        } else if current.is_some_and(|p| same(p, path))
            && let Some(rest) = line.strip_prefix("locked")
        {
            return Ok(Some(rest.trim().to_string()));
        }
    }
    Ok(None)
}

fn listed(shell: &dyn Shell, checkout: &Path, path: &str) -> Result<bool> {
    let out = git(shell, checkout, &["worktree", "list", "--porcelain"])?;
    if !out.ok() {
        return Err(Error::Invalid(format!(
            "cannot list worktrees in {}: {}",
            checkout.display(),
            out.stderr.trim()
        )));
    }
    Ok(out
        .stdout
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .any(|found| same(found, path)))
}

/// The Herdr workspace that shows `path`, if one is open.
pub fn workspace(shell: &dyn Shell, checkout: &Path, path: &str) -> Result<Option<String>> {
    let out = shell.run_ok(
        "herdr",
        &[
            "worktree".into(),
            "list".into(),
            "--cwd".into(),
            checkout.display().to_string(),
        ],
    )?;
    let value: serde_json::Value = serde_json::from_str(&out.stdout)?;
    Ok(value
        .pointer("/result/worktrees")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .find(|w| w["path"].as_str().is_some_and(|p| same(p, path)))
        .and_then(|w| w["open_workspace_id"].as_str())
        .map(ToString::to_string))
}

/// Every pane that works in `path` or sits in the workspace that shows it.
pub fn holders<'a>(panes: &'a [Pane], path: &str, workspace: Option<&str>) -> Vec<&'a Pane> {
    panes
        .iter()
        .filter(|p| {
            (workspace.is_some() && p.workspace_id.as_deref() == workspace)
                || [p.cwd.as_deref(), p.foreground_cwd.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|cwd| inside(cwd, path))
        })
        .collect()
}

/// Why `pane` is busy, or `None` when closing it loses nothing: an idle or done agent, or a
/// shell with no foreground command. A working, blocked, or unknown agent is busy.
pub fn busy(shell: &dyn Shell, pane: &Pane) -> Result<Option<String>> {
    if let Some(agent) = &pane.agent {
        let status = pane.agent_status.as_deref().unwrap_or("unknown");
        return Ok((!matches!(status, "idle" | "done")).then(|| format!("{agent} {status}")));
    }
    let out = shell.run_ok(
        "herdr",
        &[
            "pane".into(),
            "process-info".into(),
            "--pane".into(),
            pane.pane_id.clone(),
        ],
    )?;
    let value: serde_json::Value = serde_json::from_str(&out.stdout)?;
    let info = value
        .pointer("/result/process_info")
        .ok_or_else(|| Error::Invalid(format!("{} has no process information", pane.pane_id)))?;
    let shell_pid = info["shell_pid"].as_u64();
    let command = info["foreground_processes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["pid"].as_u64() != shell_pid)
        .map(|p| p["name"].as_str().unwrap_or("a command").to_string());
    Ok(command.map(|name| format!("running {name}")))
}

/// Close the holders and the workspace, then run the remove hook in the main checkout.
/// Returns whether the branch is gone afterwards.
pub fn remove(
    shell: &dyn Shell,
    ledger: &Ledger,
    hooks: &Hooks,
    tree: &Linked,
    workspace: Option<&str>,
    holders: &[&Pane],
) -> Result<bool> {
    let argv = hooks.remove_argv(&tree.branch, &tree.path)?;
    let open = crate::cleanup::current_panes(shell)?;
    for pane in holders {
        let outside = workspace.is_none() || pane.workspace_id.as_deref() != workspace;
        if outside && open.iter().any(|p| p.pane_id == pane.pane_id) {
            shell.run_ok(
                "herdr",
                &["pane".into(), "close".into(), pane.pane_id.clone()],
            )?;
        }
    }
    if let Some(workspace) = workspace {
        shell.run_ok(
            "herdr",
            &["workspace".into(), "close".into(), workspace.into()],
        )?;
    }
    let command = format!(
        "cd {} && exec {}",
        quote(&tree.checkout.display().to_string()),
        argv.iter().map(|w| quote(w)).collect::<Vec<_>>().join(" ")
    );
    let out = shell.run_setup("sh", &["-c".into(), command])?;
    if !out.ok() {
        // The runner's summary is the last line; the cause is usually just above it.
        let lines: Vec<&str> = out
            .stderr
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect();
        let tail = lines[lines.len().saturating_sub(4)..].join(" | ");
        return Err(Error::Invalid(format!(
            "remove hook failed for {} after its panes closed: {}",
            tree.path,
            if tail.is_empty() { "no output" } else { &tail }
        )));
    }
    if listed(shell, &tree.checkout, &tree.path)? {
        return Err(Error::Invalid(format!(
            "remove hook exited 0 but {} is still a worktree",
            tree.path
        )));
    }
    crate::cleanup::forget(ledger, &tree.path)?;
    Ok(git(
        shell,
        &tree.checkout,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", tree.branch),
        ],
    )?
    .status
        != 0)
}
