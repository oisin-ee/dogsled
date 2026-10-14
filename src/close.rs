//! Close a finished child: the panes and Herdr workspace that hold its worktree, the worktree
//! (through the repository's `remove` hook), and its pane.

use crate::config::Hooks;
use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::worktree::{self, AGENT_LOCK, refuse};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    pub path: String,
    pub branch: String,
    /// The Herdr workspace that showed the worktree, closed before removal.
    pub workspace: Option<String>,
    /// The remove hook deletes the branch only when it is merged.
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

/// Every refusal happens before anything changes. A child tab in the caller's workspace is
/// closed; the caller's own pane, and a worktree the caller works in or shows, are refused.
pub fn local(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    hooks: &Hooks,
    pane: &str,
    caller: &Caller,
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
    let status = shell.run(
        "git",
        &[
            "-C".into(),
            tree.path.clone(),
            "status".into(),
            "--porcelain".into(),
        ],
    )?;
    if status.status != 0 || !status.stdout.trim().is_empty() {
        return Err(refuse(&format!("{} has uncommitted changes", tree.path)));
    }
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
    let branch_deleted =
        worktree::remove(shell, ledger, hooks, &tree, workspace.as_deref(), &holders)?;
    Ok(Closed {
        pane: pane.into(),
        worktree: Some(Worktree {
            path: tree.path,
            branch: tree.branch,
            workspace,
            branch_deleted,
        }),
        pane_closed,
    })
}
