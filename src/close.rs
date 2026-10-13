//! Close a finished child: its Herdr workspace, its worktree (through Worktrunk, so the
//! repository's remove hooks run), and its pane.

use crate::error::{Error, Result};
use crate::probe::Pane;
use crate::shell::Shell;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The lock reason that marks an agent-owned worktree. Any other lock is refused.
const AGENT_LOCK: &str = "agent:worktree";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    pub path: String,
    pub branch: String,
    /// The Herdr workspace that showed the worktree, closed before removal.
    pub workspace: Option<String>,
    /// `wt remove` deletes the branch only when it is merged.
    pub branch_deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Closed {
    pub pane: String,
    /// The linked worktree that was removed, if the pane was in one.
    pub worktree: Option<Worktree>,
    /// True when wrangle closed the pane itself, because no workspace close covered it.
    pub pane_closed: bool,
}

/// The caller's own pane and workspace. Over SSH both are empty.
#[derive(Debug, Default)]
pub struct Caller {
    pub pane: Option<String>,
    pub workspace: Option<String>,
}

impl Caller {
    pub fn from_env() -> Self {
        Self {
            pane: std::env::var("HERDR_PANE_ID").ok(),
            workspace: std::env::var("HERDR_WORKSPACE_ID").ok(),
        }
    }
}

struct Tree {
    checkout: PathBuf,
    path: String,
    branch: String,
}

fn git(shell: &dyn Shell, dir: &Path, args: &[&str]) -> Result<crate::shell::Output> {
    let mut words = vec!["-C".to_string(), dir.display().to_string()];
    words.extend(args.iter().map(ToString::to_string));
    shell.run("git", &words)
}

fn refuse(message: &str) -> Error {
    Error::Invalid(format!("refuse: {message}; nothing was closed"))
}

/// An agent that is not idle or done is never closed: Shepherdr reports a closed working
/// or blocked pane as a failure, and `unknown` proves nothing.
fn check_idle(pane: &Pane) -> Result<()> {
    if pane.agent.is_some() && !matches!(pane.agent_status.as_deref(), Some("idle" | "done")) {
        return Err(refuse(&format!(
            "{} has a {} agent; close only an idle or done child",
            pane.pane_id,
            pane.agent_status.as_deref().unwrap_or("unknown")
        )));
    }
    Ok(())
}

/// Every refusal happens before anything changes.
pub fn local(shell: &dyn Shell, pane: &str, caller: &Caller) -> Result<Closed> {
    let panes = crate::cleanup::current_panes(shell)?;
    let target = panes
        .iter()
        .find(|p| p.pane_id == pane)
        .ok_or_else(|| Error::Invalid(format!("pane {pane} does not exist on this host")))?;
    let own = |workspace: Option<&String>| {
        caller.workspace.is_some() && workspace == caller.workspace.as_ref()
    };
    if caller.pane.as_deref() == Some(pane) || own(target.workspace_id.as_ref()) {
        return Err(refuse(&format!("{pane} is in the caller's own workspace")));
    }
    check_idle(target)?;
    let Some(tree) = target
        .cwd
        .as_deref()
        .map(|cwd| linked_worktree(shell, cwd))
        .transpose()?
        .flatten()
    else {
        shell.run_ok("herdr", &["pane".into(), "close".into(), pane.into()])?;
        return Ok(Closed {
            pane: pane.into(),
            worktree: None,
            pane_closed: true,
        });
    };
    let dir = Path::new(&tree.path);
    let status = git(shell, dir, &["status", "--porcelain"])?;
    if status.status != 0 || !status.stdout.trim().is_empty() {
        return Err(refuse(&format!("{} has uncommitted changes", tree.path)));
    }
    let lock = lock_reason(shell, &tree)?;
    if lock.as_deref().is_some_and(|reason| reason != AGENT_LOCK) {
        return Err(refuse(&format!(
            "{} is locked ({}); it is not an agent worktree",
            tree.path,
            lock.unwrap_or_default()
        )));
    }
    let workspace = workspace_of(shell, &tree)?;
    if let Some(workspace) = &workspace {
        if own(Some(workspace)) {
            return Err(refuse(&format!(
                "workspace {workspace} is the caller's own"
            )));
        }
        for other in panes
            .iter()
            .filter(|p| p.workspace_id.as_ref() == Some(workspace))
        {
            check_idle(other)?;
        }
    }

    let branch_deleted = remove(shell, &tree, workspace.as_deref(), lock.is_some())?;
    // A pane outside the worktree's workspace is still open.
    let open = crate::cleanup::current_panes(shell)?
        .iter()
        .any(|p| p.pane_id == pane);
    if open {
        shell.run_ok("herdr", &["pane".into(), "close".into(), pane.into()])?;
    }
    Ok(Closed {
        pane: pane.into(),
        worktree: Some(Worktree {
            path: tree.path,
            branch: tree.branch,
            workspace,
            branch_deleted,
        }),
        pane_closed: open,
    })
}

/// Close the workspace, then remove the worktree with Worktrunk from the main checkout,
/// so the repository's remove hooks run. Returns whether `wt` deleted the merged branch.
fn remove(shell: &dyn Shell, tree: &Tree, workspace: Option<&str>, locked: bool) -> Result<bool> {
    let dir = Path::new(&tree.path);
    if let Some(workspace) = workspace {
        shell.run_ok(
            "herdr",
            &["workspace".into(), "close".into(), workspace.into()],
        )?;
    }
    let checkout = tree.checkout.display().to_string();
    if locked {
        git(shell, &tree.checkout, &["worktree", "unlock", &tree.path])?;
    }
    // No force flags: wt refuses a dirty worktree and keeps an unmerged branch.
    let removed = shell.run_setup(
        "wt",
        &[
            "-C".into(),
            checkout,
            "remove".into(),
            "--foreground".into(),
            tree.branch.clone(),
        ],
    )?;
    if removed.status != 0 {
        if locked && dir.is_dir() {
            git(
                shell,
                &tree.checkout,
                &["worktree", "lock", "--reason", AGENT_LOCK, &tree.path],
            )?;
        }
        return Err(Error::Invalid(format!(
            "wt remove {} failed after its workspace closed: {}",
            tree.branch,
            removed.stderr.trim().lines().last().unwrap_or("no output")
        )));
    }
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

/// The worktree that contains `cwd`, when it is a linked worktree on a branch.
fn linked_worktree(shell: &dyn Shell, cwd: &str) -> Result<Option<Tree>> {
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
            "{top} is a detached worktree; wt remove needs a branch"
        )));
    }
    Ok(Some(Tree {
        checkout: crate::prepare::main_checkout(shell, cwd)?,
        path: top.into(),
        branch: branch.into(),
    }))
}

fn lock_reason(shell: &dyn Shell, tree: &Tree) -> Result<Option<String>> {
    let out = git(shell, &tree.checkout, &["worktree", "list", "--porcelain"])?;
    let mut current = None;
    for line in out.stdout.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current = Some(path);
        } else if current.is_some_and(|p| same(p, &tree.path))
            && let Some(rest) = line.strip_prefix("locked")
        {
            return Ok(Some(rest.trim().to_string()));
        }
    }
    Ok(None)
}

fn same(a: &str, b: &str) -> bool {
    let canonical = |p: &str| Path::new(p).canonicalize().unwrap_or_else(|_| p.into());
    a == b || canonical(a) == canonical(b)
}

fn workspace_of(shell: &dyn Shell, tree: &Tree) -> Result<Option<String>> {
    let out = shell.run_ok(
        "herdr",
        &[
            "worktree".into(),
            "list".into(),
            "--cwd".into(),
            tree.checkout.display().to_string(),
        ],
    )?;
    let value: serde_json::Value = serde_json::from_str(&out.stdout)?;
    Ok(value
        .pointer("/result/worktrees")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .find(|w| w["path"].as_str().is_some_and(|p| same(p, &tree.path)))
        .and_then(|w| w["open_workspace_id"].as_str())
        .map(ToString::to_string))
}
