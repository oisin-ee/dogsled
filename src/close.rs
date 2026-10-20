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
    /// The branch is deleted under the shared loss rule.
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
    /// True when dogsled closed the pane itself, because no workspace close covered it.
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

/// Close context carried over host transport. Every variant uses the same loss rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guard {
    /// Explicit close; inspect local item ownership.
    Clean,
    /// An orphan; no live-owner landing hold.
    Pushed,
    /// A finished reader whose owner is alive.
    Reader,
    /// A finished writer whose owner is alive.
    Writer,
}

impl Guard {
    /// The flags of `host close` for this guard.
    pub fn flags(self) -> &'static [&'static str] {
        match self {
            Self::Clean => &[],
            Self::Pushed => &["--pushed"],
            Self::Reader => &["--landed"],
            Self::Writer => &["--landed", "--writer"],
        }
    }

    pub fn from_flags(pushed: bool, landed: bool, writer: bool) -> Self {
        match (pushed, landed, writer) {
            (_, true, true) => Self::Writer,
            (_, true, false) => Self::Reader,
            (true, false, _) => Self::Pushed,
            _ => Self::Clean,
        }
    }
}

/// Every refusal happens before anything changes. A child tab in the caller's workspace is
/// closed; the caller's own pane, and a worktree the caller works in or shows, are refused.
/// Only an agent worktree (locked `agent:worktree`) is removed; a child in any other checkout
/// loses only its pane.
/// `guard` adds the checks that make an automatic close lose nothing.
pub fn local(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    pane: &str,
    caller: &Caller,
    guard: Guard,
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
    // An explicit recovery may end a stranded shell without removing its checkout or
    // assigning an outcome to the interrupted launch. Automatic cleanup cannot do this.
    if guard == Guard::Clean && crate::launch::close_unfinished(ledger, shell, pane)? {
        return Ok(Closed {
            pane: pane.into(),
            worktree: None,
            pane_closed: true,
        });
    }
    // An automatic close needs the child's own word: idle alone means it waits for input.
    if guard != Guard::Clean
        && target.agent.is_some()
        && !crate::done::finished(ledger, &crate::probe::agent_list(shell)?)?.contains(pane)
    {
        return Err(refuse(&format!(
            "{pane} has not called done; it waits for input"
        )));
    }
    let tree = target
        .cwd
        .as_deref()
        .map(|cwd| worktree::linked(shell, cwd))
        .transpose()?
        .flatten();
    // Dogsled locks every worktree it makes. An unlocked one is someone's own checkout that
    // the child only ran in, such as its owner's: closing the child closes its pane alone.
    let lock = match &tree {
        Some(tree) => worktree::lock_reason(shell, &tree.checkout, &tree.path)?,
        None => None,
    };
    let Some(tree) = tree.filter(|_| lock.is_some()) else {
        shell.run_ok("herdr", &["pane".into(), "close".into(), pane.into()])?;
        return Ok(Closed {
            pane: pane.into(),
            worktree: None,
            pane_closed: true,
        });
    };
    outside(&tree, caller)?;
    let evidence = unsaved(shell, ledger, &tree, caller, guard)?;
    if let Some(lock) = lock
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
        &evidence,
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

/// All close modes use the same loss rule. Automatic owner closes add a landing hold.
fn unsaved(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    tree: &worktree::Linked,
    caller: &Caller,
    guard: Guard,
) -> Result<crate::loss::Evidence> {
    let repo = tree.checkout.display().to_string();
    let evidence = crate::loss::Evidence::fetch(shell, &repo);
    if let Some(warning) = &evidence.warning {
        eprintln!("{warning}");
    }
    let tip = format!("refs/heads/{}", tree.branch);
    let loss = evidence.assess(shell, &tip, Some(&tree.path))?;
    if !loss.loses_nothing() {
        return Err(refuse(&format!("{}: {}", tree.path, loss.detail())));
    }
    crate::cleanup::close_holds(shell, ledger, tree, caller, guard, &loss)?;
    Ok(evidence)
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
