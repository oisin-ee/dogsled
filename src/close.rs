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

/// What a worktree must show before it is removed. Uncommitted changes always refuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guard {
    /// `wrangle close`: the owner decided; only uncommitted changes refuse.
    Clean,
    /// An orphan: every commit is on some remote ref.
    Pushed,
    /// A finished reader: no commit outside the default branch, or its branch landed.
    Reader,
    /// A finished writer: its branch landed in the default branch.
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
    let tree = target
        .cwd
        .as_deref()
        .map(|cwd| worktree::linked(shell, cwd))
        .transpose()?
        .flatten();
    // Wrangle locks every worktree it makes. An unlocked one is someone's own checkout that
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
    unsaved(shell, &tree, guard)?;
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

/// Refuse a worktree with uncommitted changes, and whatever else `guard` forbids.
fn unsaved(shell: &dyn crate::shell::Shell, tree: &worktree::Linked, guard: Guard) -> Result<()> {
    let path = tree.path.as_str();
    let git = |args: &[&str]| {
        let mut words = vec!["-C".to_string(), path.to_string()];
        words.extend(args.iter().map(ToString::to_string));
        shell.run("git", &words)
    };
    let status = git(&["status", "--porcelain", "--untracked-files=all"])?;
    if status.status != 0 || !status.stdout.trim().is_empty() {
        return Err(refuse(&format!("{path} has uncommitted changes")));
    }
    match guard {
        Guard::Clean => return Ok(()),
        Guard::Reader | Guard::Writer => return landed(shell, tree, guard),
        Guard::Pushed => (),
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

/// A reader closes with no commit outside the default branch; either role closes once its
/// branch landed there.
fn landed(shell: &dyn crate::shell::Shell, tree: &worktree::Linked, guard: Guard) -> Result<()> {
    let base = crate::integration::base(shell, &tree.path)?;
    let out = shell.run(
        "git",
        &[
            "-C".into(),
            tree.path.clone(),
            "rev-list".into(),
            "--count".into(),
            format!("{base}..HEAD"),
        ],
    )?;
    let ahead: u64 = match out.stdout.trim().parse() {
        Ok(n) if out.ok() => n,
        _ => {
            return Err(refuse(&format!(
                "cannot count commits outside {} in {}: {}",
                crate::integration::short(&base),
                tree.path,
                out.stderr.trim()
            )));
        }
    };
    if ahead == 0 && guard == Guard::Reader {
        return Ok(());
    }
    // A fast-forwarded branch has no commits outside the base, yet it landed.
    if !tree.branch.is_empty()
        && crate::integration::check(shell, &tree.path, &tree.branch)?.is_some()
    {
        return Ok(());
    }
    Err(refuse(&if ahead == 0 {
        format!("{} has no commits to land yet", tree.path)
    } else {
        format!(
            "{} has {ahead} commit{} not in {}",
            tree.path,
            if ahead == 1 { "" } else { "s" },
            crate::integration::short(&base)
        )
    }))
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
