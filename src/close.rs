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
    /// An orphan handoff; retain its branch.
    Pushed,
    /// A finished reader handoff; retain its branch.
    Reader,
    /// A finished writer handoff; retain its branch.
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
    with_local(shell, ledger, pane, caller, guard, Ok)
}

/// Keep repository protection through the caller's matching-record reconciliation.
pub fn with_local<T>(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    pane: &str,
    caller: &Caller,
    mode: Guard,
    reconcile: impl FnOnce(Closed) -> Result<T>,
) -> Result<T> {
    if caller.pane.as_deref() == Some(pane) {
        return Err(refuse("cannot close the caller pane"));
    }
    let (panes, terminals) = crate::cleanup::pane_snapshot(shell)?;
    let owners = local_owners(ledger, pane)?;
    let target = panes.iter().find(|p| p.pane_id == pane);
    let cwd = target.and_then(|p| p.cwd.as_deref());
    let capture = cwd
        .map(|cwd| worktree::capture_close(shell, cwd))
        .transpose()?
        .flatten();
    let saved = crate::launch::checkout_for_pane(ledger, pane)?;
    let (live, directory) = capture
        .map(|capture| capture.bind_saved(saved.as_ref()))
        .transpose()?
        .unwrap_or((None, None));
    let planned = close_expectation(saved.as_ref(), live.as_ref());
    let Some(planned) = planned else {
        let target = validate_target(shell, &panes, pane, caller)?;
        return reconcile(remove_target(
            shell, ledger, target, &panes, caller, mode, None,
        )?);
    };
    let repo = planned.repo.clone();
    worktree::with_lifecycle(shell, ledger, Path::new(&repo), |lifecycle| {
        if let (Some(saved), Some(live)) = (&saved, &live)
            && !live.matches_checkout(saved)
        {
            return Err(refuse("pane moved to another checkout"));
        }
        let absent = worktree::verified_absent(lifecycle, shell, &planned)
            || (saved.is_none()
                && live.as_ref().is_some_and(|tree| {
                    worktree::verified_absent_unmarked(lifecycle, shell, tree)
                }));
        let planned = if absent {
            planned
        } else {
            establish_close_identity(
                lifecycle,
                shell,
                planned,
                live.as_ref(),
                directory.as_ref(),
                saved.is_none(),
            )?
        };
        if absent || worktree::verified_absent(lifecycle, shell, &planned) {
            validate_absent_target(
                shell,
                ledger,
                pane,
                caller,
                &planned,
                terminals.get(pane).map(String::as_str),
                &owners,
            )?;
            if saved.is_some() {
                crate::launch::validate_checkout_owner(
                    ledger,
                    pane,
                    None,
                    terminals.get(pane).map(String::as_str),
                )?;
            }
            crate::cleanup::forget_matching(ledger, &planned)?;
            return reconcile(Closed {
                pane: pane.into(),
                worktree: Some(Worktree {
                    path: planned.path,
                    branch: planned.branch,
                    workspace: None,
                    branch_deleted: false,
                    anchor_closed: None,
                }),
                pane_closed: false,
            });
        }
        if let Some(live) = &live {
            directory
                .as_ref()
                .ok_or_else(|| refuse("checkout identity is unreadable"))?
                .validate(live)?;
        }
        let panes = crate::cleanup::current_panes(shell)?;
        let target = validate_target(shell, &panes, pane, caller)?;
        if let (Some(live), Some(pin)) = (&live, &directory) {
            pin.validate(live)?;
        }
        reconcile(remove_target(
            shell,
            ledger,
            target,
            &panes,
            caller,
            mode,
            Some((lifecycle, &planned)),
        )?)
    })
}

/// Add only an unmarked checkout's generation; never replace the request's HEAD.
fn establish_close_identity(
    lifecycle: &worktree::Guard,
    shell: &dyn crate::shell::Shell,
    planned: crate::cleanup::Tree,
    live: Option<&worktree::Linked>,
    pin: Option<&worktree::Pin>,
    unsaved: bool,
) -> Result<crate::cleanup::Tree> {
    if unsaved && planned.identity.is_none() {
        return Ok(lifecycle
            .discover_unmarked(
                shell,
                live.ok_or_else(|| refuse("checkout identity is unreadable"))?,
                pin.ok_or_else(|| refuse("checkout identity is unreadable"))?,
            )?
            .tree());
    }
    // Saved identity stays launch-bound; HEAD is the close request's observation.
    if let Some(now) = lifecycle.observe(shell, &planned.path)? {
        match planned.drift(&now.tree()) {
            crate::cleanup::Drift::Identity => return Err(refuse("worktree identity changed")),
            crate::cleanup::Drift::HeadMoved => return Err(refuse("HEAD moved before close")),
            crate::cleanup::Drift::Same => (),
        }
    }
    Ok(planned)
}

/// Retain launch identity, but capture HEAD at the start of this close request.
fn close_expectation(
    saved: Option<&crate::cleanup::Tree>,
    live: Option<&worktree::Linked>,
) -> Option<crate::cleanup::Tree> {
    let mut planned = saved
        .cloned()
        .or_else(|| live.map(worktree::Linked::tree))?;
    if let Some(live) = live {
        planned.head.clone_from(&live.head);
    }
    Some(planned)
}

fn local_owners(ledger: &Ledger, pane: &str) -> Result<Vec<crate::items::Item>> {
    Ok(crate::items::list(ledger)?
        .into_iter()
        .filter(|item| {
            item.host == crate::hosts::LOCAL && item.pane == pane && item.closed_ms.is_none()
        })
        .collect())
}

/// Absence never authorizes reconciliation of a replacement pane or another item.
fn validate_absent_target(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    pane: &str,
    caller: &Caller,
    tree: &crate::cleanup::Tree,
    terminal: Option<&str>,
    owners: &[crate::items::Item],
) -> Result<()> {
    let (panes, terminals) = crate::cleanup::pane_snapshot(shell)?;
    let items = crate::items::list(ledger)?;
    let new_owner = items.iter().any(|item| {
        item.host == crate::hosts::LOCAL
            && item.pane == pane
            && item.closed_ms.is_none()
            && !owners.iter().any(|owner| owner.id == item.id)
    });
    let lost_owner = owners.iter().any(|owner| {
        items
            .iter()
            .find(|item| item.id == owner.id)
            .is_none_or(|item| {
                item.host != owner.host
                    || item.pane != owner.pane
                    || item.owner_pane != owner.owner_pane
                    || item.owner_session != owner.owner_session
                    || item.created_ms != owner.created_ms
            })
    });
    // Only the same captured item becoming closed may leave the open owner set.
    if new_owner || lost_owner {
        return Err(refuse("checkout item ownership changed"));
    }
    if panes.iter().any(|target| target.pane_id == pane) {
        let target = validate_target(shell, &panes, pane, caller)?;
        if terminal.is_none()
            || terminal != terminals.get(pane).map(String::as_str)
            || target.cwd.as_deref().is_none_or(|cwd| {
                !worktree::resolved(Path::new(cwd))
                    .starts_with(worktree::resolved(Path::new(&tree.path)))
            })
        {
            return Err(refuse("removed checkout no longer owns the pane"));
        }
    }
    Ok(())
}

/// Repeat the captured-resource check before entering any close effect.
fn validate_checkout(
    shell: &dyn crate::shell::Shell,
    target: &crate::probe::Pane,
    expected: &crate::cleanup::Tree,
) -> Result<worktree::Linked> {
    let now = worktree::linked_or_detached(shell, &expected.path)?
        .ok_or_else(|| refuse("checkout identity is unreadable"))?;
    if expected.identity.is_none()
        || now.identity != expected.identity
        || now.branch != expected.branch
        || now.path != expected.path
    {
        return Err(refuse("worktree identity changed"));
    }
    if now.head != expected.head {
        return Err(refuse("HEAD moved before close"));
    }
    worktree::validate_pane_checkout(shell, target, &now)?;
    Ok(now)
}

/// Validate pane safety before selecting checkout removal or pane-only closure.
fn validate_target<'a>(
    shell: &dyn crate::shell::Shell,
    panes: &'a [crate::probe::Pane],
    pane: &str,
    caller: &Caller,
) -> Result<&'a crate::probe::Pane> {
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
    if let Some(reason) = crate::cleanup::close_group_hold(shell, panes, pane)? {
        return Err(refuse(&reason));
    }
    Ok(target)
}

fn pane_only(shell: &dyn crate::shell::Shell, pane: &str) -> Result<Closed> {
    shell.run_ok("herdr", &["pane".into(), "close".into(), pane.into()])?;
    Ok(Closed {
        pane: pane.into(),
        worktree: None,
        pane_closed: true,
    })
}

fn remove_target(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    target: &crate::probe::Pane,
    panes: &[crate::probe::Pane],
    caller: &Caller,
    guard: Guard,
    lifecycle: Option<(&worktree::Guard, &crate::cleanup::Tree)>,
) -> Result<Closed> {
    let pane = target.pane_id.as_str();
    let validated = lifecycle
        .map(|(_, expected)| validate_checkout(shell, target, expected))
        .transpose()?;
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
    let tree = match validated {
        Some(tree) => Some(tree),
        None => target
            .cwd
            .as_deref()
            .map(|cwd| worktree::linked_or_detached(shell, cwd))
            .transpose()?
            .flatten(),
    };
    // Dogsled locks every worktree it makes. An unlocked one is someone's own checkout that
    // the child only ran in, such as its owner's: closing the child closes its pane alone.
    let lock = match &tree {
        Some(tree) => worktree::lock_reason(shell, &tree.checkout, &tree.path)?,
        None => None,
    };
    let automatic = guard != Guard::Clean;
    let Some(tree) =
        tree.filter(|_| lock.is_some() && (!automatic || lock.as_deref() == Some(AGENT_LOCK)))
    else {
        return pane_only(shell, pane);
    };
    if lock.as_deref() == Some(AGENT_LOCK) {
        crate::cleanup::remember(ledger, tree.tree())?;
    }
    let workspace = worktree::workspace(shell, &tree.checkout, &tree.path)?;
    let holders = worktree::holders(panes, &tree.path, workspace.as_deref());
    // Another agent, open item, or busy pane still uses the checkout. Even an explicit close
    // releases only its own pane: closing the others would close agents nobody asked to close.
    if shared(shell, ledger, pane, &holders)? {
        return pane_only(shell, pane);
    }
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
    if workspace.is_some() && workspace == caller.workspace {
        return Err(refuse(&format!(
            "workspace {} is the caller's own",
            workspace.as_deref().unwrap_or_default()
        )));
    }
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
        worktree::Removal {
            guard: lifecycle
                .ok_or_else(|| refuse("repository protection missing"))?
                .0,
            workspace: workspace.as_deref(),
            holders: &holders,
            caller_workspace: caller.workspace.as_deref(),
            evidence: &evidence,
        },
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

/// A shared checkout stays, but its finished child can release its own pane.
fn shared(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    pane: &str,
    holders: &[&crate::probe::Pane],
) -> Result<bool> {
    let items = crate::items::list(ledger)?;
    for other in holders.iter().filter(|p| p.pane_id != pane) {
        if other.agent.is_some()
            || items
                .iter()
                .any(|i| i.closed_ms.is_none() && i.pane == other.pane_id)
            || worktree::busy(shell, other)?.is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The same loss rule, with the local branch retained for automatic handoff.
fn unsaved(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    tree: &worktree::Linked,
    caller: &Caller,
    guard: Guard,
) -> Result<crate::loss::Evidence> {
    let repo = tree.checkout.display().to_string();
    // A detached checkout has no branch. No branch is invented for it, so its commits must
    // be held by the default branch or a refreshed remote ref, exactly as for a branch.
    let detached = tree.branch.is_empty();
    let evidence = if guard == Guard::Clean {
        crate::loss::Evidence::fetch(shell, &repo)
    } else {
        let dirty = crate::loss::dirty_paths(shell, &tree.path, false)?;
        if !dirty.is_empty() {
            return Err(refuse(&format!(
                "{}: uncommitted changes: {}",
                tree.path,
                dirty.join(", ")
            )));
        }
        if detached {
            crate::loss::Evidence::fetch(shell, &repo)
        } else {
            crate::loss::Evidence::keeping_branch(&repo, &tree.branch)
        }
    };
    if let Some(warning) = &evidence.warning {
        eprintln!("{warning}");
    }
    let tip = if detached {
        worktree::head_sha(shell, &tree.path)?
    } else {
        format!("refs/heads/{}", tree.branch)
    };
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
