//! A linked worktree, the Herdr panes that hold it, and its removal: the repository's
//! `teardown`, `git worktree remove`, the branch when it is integrated, and the anchor workspace
//! after the last child. `close` and `prune` share these rules.

use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::probe::Pane;
use crate::repository::{Settings, Values};
use crate::shell::Shell;
use std::path::{Path, PathBuf};

/// The lock reason that marks an agent-owned worktree. Any other lock belongs to someone else.
pub const AGENT_LOCK: &str = "agent:worktree";

/// The file in the Git common directory that names the main checkout's Herdr workspace when
/// `place` opened it. The last child's removal closes that workspace.
pub const ANCHOR_FILE: &str = "agent-worktree-anchor";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Linked {
    /// The main checkout.
    pub checkout: PathBuf,
    pub path: String,
    /// Empty for a detached worktree.
    pub branch: String,
}

/// One entry of `git worktree list --porcelain`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listed {
    pub path: String,
    pub branch: Option<String>,
    pub lock: Option<String>,
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
            "{top} is a detached worktree; `dogsled prune` removes it once its commit is on a remote"
        )));
    }
    Ok(Some(Linked {
        checkout: crate::prepare::main_checkout(shell, cwd)?,
        path: top.into(),
        branch: branch.into(),
    }))
}

/// Every worktree of the repository at `checkout`, the main checkout first.
pub fn list(shell: &dyn Shell, checkout: &Path) -> Result<Vec<Listed>> {
    let out = git(shell, checkout, &["worktree", "list", "--porcelain"])?;
    if !out.ok() {
        return Err(Error::Invalid(format!(
            "cannot list worktrees in {}: {}",
            checkout.display(),
            out.stderr.trim()
        )));
    }
    let mut result: Vec<Listed> = Vec::new();
    for line in out.stdout.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            result.push(Listed {
                path: path.into(),
                ..Listed::default()
            });
        } else if let Some(current) = result.last_mut() {
            if let Some(branch) = line.strip_prefix("branch refs/heads/") {
                current.branch = Some(branch.into());
            } else if let Some(reason) = line.strip_prefix("locked") {
                current.lock = Some(reason.trim().into());
            }
        }
    }
    Ok(result)
}

/// The lock reason of `path`, if it is locked. An empty reason is a lock without one.
pub fn lock_reason(shell: &dyn Shell, checkout: &Path, path: &str) -> Result<Option<String>> {
    Ok(list(shell, checkout)?
        .into_iter()
        .find(|w| same(&w.path, path))
        .and_then(|w| w.lock))
}

fn listed(shell: &dyn Shell, checkout: &Path, path: &str) -> Result<bool> {
    Ok(list(shell, checkout)?.iter().any(|w| same(&w.path, path)))
}

/// Herdr's view of the repository's worktrees: each path and its open workspace.
fn herdr_worktrees(shell: &dyn Shell, checkout: &Path) -> Result<Vec<serde_json::Value>> {
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
        .cloned()
        .unwrap_or_default())
}

/// The Herdr workspace that shows `path`, if one is open.
pub fn workspace(shell: &dyn Shell, checkout: &Path, path: &str) -> Result<Option<String>> {
    Ok(herdr_worktrees(shell, checkout)?
        .iter()
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
    Ok(crate::probe::foreground(shell, &pane.pane_id)?.map(|name| format!("running {name}")))
}

/// The Git common directory of the repository at `checkout`.
pub fn common_dir(shell: &dyn Shell, checkout: &Path) -> Result<PathBuf> {
    let out = git(
        shell,
        checkout,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let dir = out.stdout.trim();
    if !out.ok() || dir.is_empty() {
        return Err(Error::Invalid(format!(
            "{} is not a Git checkout: {}",
            checkout.display(),
            out.stderr.trim()
        )));
    }
    Ok(dir.into())
}

/// Record `workspace` as the anchor that `place` opened for the main checkout.
pub fn record_anchor(shell: &dyn Shell, checkout: &Path, workspace: &str) -> Result<()> {
    std::fs::write(
        common_dir(shell, checkout)?.join(ANCHOR_FILE),
        format!("{workspace}\n"),
    )?;
    Ok(())
}

/// What removal did besides removing the worktree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Removed {
    /// The branch is gone: it was integrated, so removal deleted it, or it was already gone.
    pub branch_deleted: bool,
    /// The anchor workspace closed after its last child.
    pub anchor_closed: Option<String>,
}

/// Remove `tree`: run the repository's `teardown` in it, close its holders and workspace,
/// unlock it, and `git worktree remove` it without force. A teardown failure keeps everything.
/// Then delete its branch when `integration::check` finds it integrated, and close the anchor
/// workspace when no linked worktree shows in Herdr any more. The caller has already refused a
/// dirty, foreign-locked, or busy worktree.
pub fn remove(
    shell: &dyn Shell,
    ledger: &Ledger,
    tree: &Linked,
    workspace: Option<&str>,
    holders: &[&Pane],
    caller_workspace: Option<&str>,
    evidence: &crate::loss::Evidence,
) -> Result<Removed> {
    let settings = crate::repository::load(&tree.checkout)?;
    teardown(shell, &settings, tree)?;
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
    let lock = lock_reason(shell, &tree.checkout, &tree.path)?;
    match lock.as_deref() {
        None => (),
        Some(AGENT_LOCK) => {
            git_ok(shell, &tree.checkout, &["worktree", "unlock", &tree.path])?;
        }
        Some(other) => {
            return Err(Error::Invalid(format!(
                "{} is locked ({other}) after its panes closed; it is not an agent worktree",
                tree.path
            )));
        }
    }
    let out = git(shell, &tree.checkout, &["worktree", "remove", &tree.path])?;
    if !out.ok() {
        if lock.is_some() && Path::new(&tree.path).exists() {
            git_ok(
                shell,
                &tree.checkout,
                &["worktree", "lock", "--reason", AGENT_LOCK, &tree.path],
            )?;
        }
        return Err(Error::Invalid(format!(
            "git worktree remove kept {} after its panes closed: {}",
            tree.path,
            out.stderr.trim()
        )));
    }
    if listed(shell, &tree.checkout, &tree.path)? {
        return Err(Error::Invalid(format!(
            "git worktree remove exited 0 but {} is still a worktree",
            tree.path
        )));
    }
    crate::cleanup::forget(ledger, &tree.path)?;
    let branch_deleted = !tree.branch.is_empty() && delete_held(shell, tree, evidence)?;
    let anchor_closed = close_anchor(shell, &tree.checkout, caller_workspace)?;
    Ok(Removed {
        branch_deleted,
        anchor_closed,
    })
}

fn git_ok(shell: &dyn Shell, dir: &Path, args: &[&str]) -> Result<()> {
    let out = git(shell, dir, args)?;
    if out.ok() {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "git {} in {}: {}",
            args.join(" "),
            dir.display(),
            out.stderr.trim()
        )))
    }
}

/// The repository's `teardown` commands, in the worktree.
fn teardown(shell: &dyn Shell, settings: &Settings, tree: &Linked) -> Result<()> {
    crate::repository::run(
        shell,
        "teardown",
        &settings.worktree.teardown,
        Values {
            main: &tree.checkout,
            path: &tree.path,
            branch: &tree.branch,
        },
    )
    .map_err(|e| Error::Invalid(format!("{e}; {} and its panes stay", tree.path)))
}

/// Delete the branch of a removed worktree when its work is in the default branch. A squash
/// merge is not an ancestor, so this is `-D` after dogsled's own check, not `-d`.
fn delete_held(shell: &dyn Shell, tree: &Linked, evidence: &crate::loss::Evidence) -> Result<bool> {
    let loss = evidence.assess(shell, &format!("refs/heads/{}", tree.branch), None)?;
    if !loss.loses_nothing() {
        return Ok(false);
    }
    git_ok(shell, &tree.checkout, &["branch", "-D", &tree.branch])?;
    Ok(true)
}

/// Close the anchor workspace that `place` recorded, once no linked worktree is open in Herdr,
/// while it is unfocused, is not the caller's, and has one pane with no agent. A record that no
/// longer names the main checkout's workspace is dropped.
fn close_anchor(
    shell: &dyn Shell,
    checkout: &Path,
    caller_workspace: Option<&str>,
) -> Result<Option<String>> {
    let file = common_dir(shell, checkout)?.join(ANCHOR_FILE);
    let anchor = match std::fs::read_to_string(&file) {
        Ok(text) => text.trim().to_string(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let worktrees = herdr_worktrees(shell, checkout)?;
    let main = worktrees
        .iter()
        .find(|w| w["is_linked_worktree"] == false)
        .and_then(|w| w["open_workspace_id"].as_str());
    if anchor.is_empty() || main != Some(anchor.as_str()) {
        std::fs::remove_file(&file)?;
        return Ok(None);
    }
    let children = worktrees
        .iter()
        .any(|w| w["is_linked_worktree"] == true && w["open_workspace_id"].is_string());
    if children || caller_workspace == Some(anchor.as_str()) {
        return Ok(None);
    }
    let out = shell.run_ok("herdr", &["workspace".into(), "list".into()])?;
    let value: serde_json::Value = serde_json::from_str(&out.stdout)?;
    let idle = value
        .pointer("/result/workspaces")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .find(|w| w["workspace_id"] == anchor.as_str())
        .is_some_and(|w| w["focused"] == false && w["pane_count"] == 1);
    let agentless = crate::cleanup::current_panes(shell)?
        .iter()
        .filter(|p| p.workspace_id.as_deref() == Some(anchor.as_str()))
        .all(|p| p.agent.is_none());
    if !idle || !agentless {
        return Ok(None);
    }
    shell.run_ok(
        "herdr",
        &["workspace".into(), "close".into(), anchor.clone()],
    )?;
    std::fs::remove_file(&file)?;
    Ok(Some(anchor))
}
