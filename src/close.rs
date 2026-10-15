//! Close a finished child: the panes and Herdr workspace that hold its worktree, the worktree
//! (after the repository's `teardown`), its branch when integrated, and its pane.

use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::worktree::{self, AGENT_LOCK, refuse};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    pub path: String,
    pub branch: String,
    /// The Herdr workspace that showed the worktree, closed before removal.
    pub workspace: Option<String>,
    /// The branch is deleted only when `integration::check` finds it integrated.
    pub branch_deleted: bool,
    /// The main checkout's anchor workspace, closed after its last child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_closed: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Closed {
    pub pane: String,
    /// The linked worktree that was removed, if the pane was in one.
    pub worktree: Option<Worktree>,
    /// True when wrangle closed the pane itself, because no workspace close covered it.
    pub pane_closed: bool,
}

/// The caller's own pane, workspace, and directory. Over SSH all are empty.
#[derive(Debug, Default)]
pub struct Caller {
    pub pane: Option<String>,
    pub workspace: Option<String>,
    pub cwd: Option<String>,
}

impl Caller {
    pub fn from_env() -> Self {
        Self {
            pane: std::env::var("HERDR_PANE_ID").ok(),
            workspace: std::env::var("HERDR_WORKSPACE_ID").ok(),
            cwd: std::env::current_dir()
                .ok()
                .map(|d| d.display().to_string()),
        }
    }
}

/// Every refusal happens before anything changes. A child tab in the caller's workspace is
/// closed; the caller's own pane, and a worktree the caller works in or shows, are refused.
/// With `pushed`, a worktree with a commit on no remote ref is refused too: the sweep closes an
/// orphan only when that loses nothing.
pub fn local(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    pane: &str,
    caller: &Caller,
    pushed: bool,
) -> Result<Closed> {
    let panes = crate::cleanup::current_panes(shell)?;
    let target = panes
        .iter()
        .find(|p| p.pane_id == pane)
        .ok_or_else(|| Error::Invalid(format!("pane {pane} does not exist on this host")))?;
    if caller.pane.as_deref() == Some(pane) {
        return Err(refuse(&format!("{pane} is the caller's own pane")));
    }
    if let Some(reason) = worktree::busy(shell, target)? {
        return Err(refuse(&format!(
            "{pane} is busy ({reason}); close only an idle or done child"
        )));
    }
    let Some(tree) = target
        .cwd
        .as_deref()
        .map(|cwd| worktree::linked(shell, cwd))
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
    outside(&tree, caller)?;
    unsaved(shell, &tree.path, pushed)?;
    if let Some(lock) = worktree::lock_reason(shell, &tree.checkout, &tree.path)?
        && lock != AGENT_LOCK
    {
        return Err(refuse(&format!(
            "{} is locked ({lock}); it is not an agent worktree",
            tree.path
        )));
    }
    let workspace = worktree::workspace(shell, &tree.checkout, &tree.path)?;
    if workspace.is_some() && workspace == caller.workspace {
        return Err(refuse(&format!(
            "workspace {} is the caller's own",
            workspace.as_deref().unwrap_or_default()
        )));
    }
    let holders = worktree::holders(&panes, &tree.path, workspace.as_deref());
    for other in holders.iter().filter(|p| p.pane_id != pane) {
        if caller.pane.as_deref() == Some(other.pane_id.as_str()) {
            return Err(refuse(&format!("the caller works in {}", tree.path)));
        }
        if let Some(reason) = worktree::busy(shell, other)? {
            return Err(refuse(&format!(
                "{} also uses {} and is busy ({reason})",
                other.pane_id, tree.path
            )));
        }
    }
    // The target is a holder through its cwd, so removal closes it unless its workspace does.
    let pane_closed = workspace.is_none() || target.workspace_id != workspace;
    let removed = worktree::remove(
        shell,
        ledger,
        &tree,
        workspace.as_deref(),
        &holders,
        caller.workspace.as_deref(),
    )?;
    Ok(Closed {
        pane: pane.into(),
        worktree: Some(Worktree {
            path: tree.path,
            branch: tree.branch,
            workspace,
            branch_deleted: removed.branch_deleted,
            anchor_closed: removed.anchor_closed,
        }),
        pane_closed,
    })
}

/// Refuse a worktree with uncommitted changes, and with `pushed`, one with a commit on no remote
/// ref.
fn unsaved(shell: &dyn crate::shell::Shell, path: &str, pushed: bool) -> Result<()> {
    let git = |args: &[&str]| {
        let mut words = vec!["-C".to_string(), path.to_string()];
        words.extend(args.iter().map(ToString::to_string));
        shell.run("git", &words)
    };
    let status = git(&["status", "--porcelain", "--untracked-files=all"])?;
    if status.status != 0 || !status.stdout.trim().is_empty() {
        return Err(refuse(&format!("{path} has uncommitted changes")));
    }
    if !pushed {
        return Ok(());
    }
    let out = git(&["rev-list", "--count", "HEAD", "--not", "--remotes"])?;
    match out.stdout.trim().parse::<u64>() {
        Ok(0) if out.ok() => Ok(()),
        Ok(n) if out.ok() => Err(refuse(&format!(
            "{path} has {n} commit{} on no remote",
            if n == 1 { "" } else { "s" }
        ))),
        _ => Err(refuse(&format!(
            "cannot count unpushed commits in {path}: {}",
            out.stderr.trim()
        ))),
    }
}

/// Refuse a worktree outside the repository's worktree directory, or one the caller is in.
fn outside(tree: &worktree::Linked, caller: &Caller) -> Result<()> {
    let settings = crate::repository::load(&tree.checkout)?;
    let home = tree.checkout.join(&settings.worktree.dir);
    let path = worktree::resolved(Path::new(&tree.path));
    if !path.starts_with(worktree::resolved(&home)) {
        return Err(refuse(&format!(
            "{} is not under {}",
            tree.path,
            home.display()
        )));
    }
    if caller
        .cwd
        .as_deref()
        .is_some_and(|cwd| worktree::resolved(Path::new(cwd)).starts_with(&path))
    {
        return Err(refuse(&format!(
            "the caller is inside {}; run from the main checkout",
            tree.path
        )));
    }
    Ok(())
}
