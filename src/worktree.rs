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

fn git_words(dir: &Path, args: &[&str]) -> Vec<String> {
    let mut words = vec!["-C".to_string(), dir.display().to_string()];
    words.extend(args.iter().map(ToString::to_string));
    words
}

fn git(shell: &dyn Shell, dir: &Path, args: &[&str]) -> Result<crate::shell::Output> {
    shell.run("git", &git_words(dir, args))
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

/// Where a new tab for work in a directory belongs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Home {
    /// Not in a repository: the caller's workspace.
    Outside,
    /// The Herdr workspace that shows the repository.
    Shown(String),
    /// A repository that no Herdr workspace shows.
    Unshown,
}

/// The workspace that shows the repository at `cwd`: the workspace of the worktree that
/// contains `cwd`, else the main checkout's.
pub fn home(shell: &dyn Shell, cwd: &str) -> Result<Home> {
    let out = git(shell, Path::new(cwd), &["rev-parse", "--show-toplevel"])?;
    if !out.ok() {
        return Ok(Home::Outside);
    }
    let top = out.stdout.trim();
    let listed = herdr_worktrees(shell, Path::new(cwd))?;
    let open = |w: &serde_json::Value| w["open_workspace_id"].as_str().map(ToString::to_string);
    let own = listed
        .iter()
        .find(|w| w["path"].as_str().is_some_and(|p| same(p, top)))
        .and_then(open);
    let main = || {
        listed
            .iter()
            .find(|w| w["is_linked_worktree"] == false)
            .and_then(open)
    };
    Ok(own.or_else(main).map_or(Home::Unshown, Home::Shown))
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct CloseWorkspace {
    workspace_id: String,
    #[serde(default)]
    focused: bool,
    #[serde(default)]
    pane_count: usize,
    worktree: Option<CloseTree>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct CloseTree {
    repo_key: String,
    repo_root: String,
    #[serde(default)]
    checkout_path: String,
    is_linked_worktree: bool,
}

fn close_workspaces(shell: &dyn Shell) -> Result<Vec<CloseWorkspace>> {
    let out = shell.run_ok("herdr", &["workspace".into(), "list".into()])?;
    let value: serde_json::Value = serde_json::from_str(&out.stdout)?;
    Ok(serde_json::from_value(
        value
            .pointer("/result/workspaces")
            .cloned()
            .ok_or_else(|| Error::Invalid("Herdr workspace list has no workspaces".into()))?,
    )?)
}

fn close_group(workspaces: &[CloseWorkspace], id: &str) -> Vec<CloseWorkspace> {
    let Some(tree) = workspaces
        .iter()
        .find(|w| w.workspace_id == id)
        .and_then(|w| w.worktree.as_ref())
        .filter(|t| !t.is_linked_worktree)
    else {
        return Vec::new();
    };
    let mut members: Vec<_> = workspaces
        .iter()
        .filter(|w| {
            w.worktree
                .as_ref()
                .is_some_and(|t| t.repo_key == tree.repo_key)
        })
        .cloned()
        .collect();
    // With another primary, Herdr closes this primary alone; never request group intent.
    if members.len() < 2
        || members
            .iter()
            .filter(|w| w.worktree.as_ref().is_some_and(|t| !t.is_linked_worktree))
            .count()
            != 1
    {
        return Vec::new();
    }
    members.sort_by(|a, b| a.workspace_id.cmp(&b.workspace_id));
    members
}

/// Close a pane without allowing an implicit last-pane close to take unchecked workspaces.
/// The caller has already authorized this pane; every additional agent must have called done.
pub fn close_pane(
    shell: &dyn Shell,
    ledger: &Ledger,
    pane: &str,
    caller: &crate::close::Caller,
) -> Result<()> {
    let panes = crate::cleanup::current_panes(shell)?;
    let target = panes
        .iter()
        .find(|p| p.pane_id == pane)
        .ok_or_else(|| refuse(&format!("pane {pane} no longer exists")))?;
    if let Some(workspace) = target.workspace_id.as_deref()
        && panes
            .iter()
            .filter(|p| p.workspace_id.as_deref() == Some(workspace))
            .count()
            == 1
        && confirm_group(shell, ledger, workspace, Some(pane), caller, &panes)?
    {
        return Ok(());
    }
    shell.run_ok("herdr", &["pane".into(), "close".into(), pane.into()])?;
    Ok(())
}

/// Workspace close and last-pane close share the same group preflight and confirmation.
pub fn close_workspace(
    shell: &dyn Shell,
    ledger: &Ledger,
    workspace: &str,
    caller: &crate::close::Caller,
) -> Result<()> {
    let panes = crate::cleanup::current_panes(shell)?;
    if !confirm_group(shell, ledger, workspace, None, caller, &panes)? {
        shell.run_ok(
            "herdr",
            &["workspace".into(), "close".into(), workspace.into()],
        )?;
    }
    Ok(())
}

fn confirm_group(
    shell: &dyn Shell,
    ledger: &Ledger,
    workspace: &str,
    authorized_pane: Option<&str>,
    caller: &crate::close::Caller,
    panes: &[Pane],
) -> Result<bool> {
    let members = close_group(&close_workspaces(shell)?, workspace);
    if members.is_empty() {
        return Ok(false);
    }
    let ids = members
        .iter()
        .map(|w| w.workspace_id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let identities = group_panes(shell, &members)?;
    if identities.len()
        != panes
            .iter()
            .filter(|p| {
                members
                    .iter()
                    .any(|w| p.workspace_id.as_deref() == Some(&w.workspace_id))
            })
            .count()
        || identities.iter().any(|(pane, _)| !panes.contains(pane))
    {
        return Err(refuse(&format!(
            "worktree group [{ids}]: panes changed before preflight"
        )));
    }
    check_group(
        shell,
        ledger,
        workspace,
        authorized_pane,
        caller,
        panes,
        &members,
    )
    .map_err(|error| refuse(&format!("worktree group [{ids}]: {error}")))?;
    // Herdr has no conditional group-close request. Narrow the inspection window and
    // refuse changed membership, pane activity, focus, or provenance before confirmation.
    if close_group(&close_workspaces(shell)?, workspace) != members
        || group_panes(shell, &members)? != identities
    {
        return Err(refuse(&format!(
            "worktree group [{ids}]: membership or panes changed during preflight"
        )));
    }
    if identities.iter().any(|(_, terminal)| terminal.is_none()) {
        return Err(refuse(&format!(
            "worktree group [{ids}]: missing terminal identity"
        )));
    }
    shell.herdr_request(&serde_json::json!({"id":"dogsled-close-group","method":"workspace.close","params":{"workspace_id":workspace,"close_group":true}}))?;
    Ok(true)
}

fn group_panes(
    shell: &dyn Shell,
    members: &[CloseWorkspace],
) -> Result<Vec<(Pane, Option<String>)>> {
    let (panes, terminals) = crate::cleanup::pane_snapshot(shell)?;
    let mut result: Vec<_> = panes
        .into_iter()
        .filter(|p| {
            members
                .iter()
                .any(|w| p.workspace_id.as_deref() == Some(&w.workspace_id))
        })
        .map(|p| {
            let terminal = terminals.get(&p.pane_id).cloned();
            (p, terminal)
        })
        .collect();
    result.sort_by(|a, b| a.0.pane_id.cmp(&b.0.pane_id));
    Ok(result)
}

fn check_group(
    shell: &dyn Shell,
    ledger: &Ledger,
    workspace: &str,
    authorized_pane: Option<&str>,
    caller: &crate::close::Caller,
    panes: &[Pane],
    members: &[CloseWorkspace],
) -> Result<()> {
    for member in members {
        let holders: Vec<_> = panes
            .iter()
            .filter(|p| p.workspace_id.as_deref() == Some(&member.workspace_id))
            .collect();
        if member.focused
            || caller.workspace.as_deref() == Some(&member.workspace_id)
            || holders.len() != member.pane_count
            || holders.is_empty()
        {
            return Err(refuse(&format!(
                "{} is focused, is the caller's workspace, or has incomplete pane information",
                member.workspace_id
            )));
        }
        for pane in holders {
            if pane.focused || caller.pane.as_deref() == Some(&pane.pane_id) {
                return Err(refuse(&format!(
                    "{} is focused or belongs to the caller",
                    pane.pane_id
                )));
            }
            if let Some(reason) = busy(shell, pane)? {
                return Err(refuse(&format!("{} is busy ({reason})", pane.pane_id)));
            }
        }
    }
    let agents = crate::probe::agent_list(shell)?;
    let done = crate::done::finished(ledger, &agents)?;
    for pane in panes.iter().filter(|p| {
        members
            .iter()
            .any(|w| p.workspace_id.as_deref() == Some(&w.workspace_id))
    }) {
        let agent = agents.iter().find(|a| a.pane == pane.pane_id);
        if let Some(agent) = agent
            && !matches!(agent.state.as_str(), "idle" | "done")
        {
            return Err(refuse(&format!(
                "{} is busy ({})",
                pane.pane_id, agent.state
            )));
        }
        if (pane.agent.is_some() || agent.is_some())
            && authorized_pane != Some(pane.pane_id.as_str())
            && !done.contains(&pane.pane_id)
        {
            return Err(refuse(&format!("{} has not called done", pane.pane_id)));
        }
        if matches!(
            crate::launch::final_pane(ledger, shell, &pane.pane_id)?,
            crate::launch::PaneLaunch::Final | crate::launch::PaneLaunch::Unresolved
        ) {
            return Err(refuse(&format!(
                "{} requires launch recovery",
                pane.pane_id
            )));
        }
    }
    check_group_trees(shell, ledger, workspace, authorized_pane, caller, members)
}

fn check_group_trees(
    shell: &dyn Shell,
    ledger: &Ledger,
    workspace: &str,
    authorized_pane: Option<&str>,
    caller: &crate::close::Caller,
    members: &[CloseWorkspace],
) -> Result<()> {
    let primary = members
        .iter()
        .find_map(|w| w.worktree.as_ref().filter(|t| !t.is_linked_worktree))
        .ok_or_else(|| refuse("group has no primary"))?;
    let inventory = list(shell, Path::new(&primary.repo_root))?;
    let evidence = crate::loss::Evidence::fetch(shell, &primary.repo_root);
    if let Some(warning) = &evidence.warning {
        eprintln!("{warning}");
    }
    for member in members {
        let provenance = member
            .worktree
            .as_ref()
            .ok_or_else(|| refuse("group has no provenance"))?;
        let listed = inventory
            .iter()
            .find(|t| same(&t.path, &provenance.checkout_path))
            .ok_or_else(|| {
                refuse(&format!(
                    "{}: checkout is absent from Git inventory",
                    member.workspace_id
                ))
            })?;
        let branch = listed
            .branch
            .as_deref()
            .ok_or_else(|| refuse(&format!("{}: detached checkout", member.workspace_id)))?;
        let tree = Linked {
            checkout: primary.repo_root.clone().into(),
            path: listed.path.clone(),
            branch: branch.into(),
        };
        let loss = evidence.assess(shell, &format!("refs/heads/{branch}"), Some(&tree.path))?;
        // Group confirmation closes workspaces, not checkouts: never repair or force dirt.
        if !crate::loss::dirty_paths(shell, &tree.path, false)?.is_empty() || !loss.loses_nothing()
        {
            return Err(refuse(&format!(
                "{} ({}): dirty or unheld checkout: {}",
                member.workspace_id,
                tree.path,
                loss.detail()
            )));
        }
        let guard = if member.workspace_id == workspace && authorized_pane.is_some() {
            crate::close::Guard::Clean
        } else {
            crate::close::Guard::Pushed
        };
        crate::cleanup::close_holds(shell, ledger, &tree, caller, guard, &loss)?;
    }
    Ok(())
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
    let caller = crate::close::Caller {
        workspace: caller_workspace.map(str::to_owned),
        ..crate::close::Caller::from_env()
    };
    check_removal_groups(shell, ledger, workspace, holders, &caller)?;
    let settings = crate::repository::load(&tree.checkout)?;
    teardown(shell, &settings, tree)?;
    if Path::new(&tree.path).is_dir() {
        evidence.restore_deletions(shell, &tree.path)?;
    }
    let open = crate::cleanup::current_panes(shell)?;
    for pane in holders {
        let outside = workspace.is_none() || pane.workspace_id.as_deref() != workspace;
        if outside && open.iter().any(|p| p.pane_id == pane.pane_id) {
            close_pane(shell, ledger, &pane.pane_id, &caller)?;
        }
    }
    if let Some(workspace) = workspace {
        close_workspace(shell, ledger, workspace, &caller)?;
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
    // Removing ignored build output can take as long as creating the checkout.
    let out = shell.run_setup(
        "git",
        &git_words(&tree.checkout, &["worktree", "remove", &tree.path]),
    )?;
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
    let anchor_closed = close_anchor(shell, ledger, &tree.checkout, caller_workspace)?;
    Ok(Removed {
        branch_deleted,
        anchor_closed,
    })
}

/// Validate affected groups before teardown or the first holder closes. A partial workspace
/// removal cannot close a group, so it must not inherit the group's additional restrictions.
fn check_removal_groups(
    shell: &dyn Shell,
    ledger: &Ledger,
    workspace: Option<&str>,
    holders: &[&Pane],
    caller: &crate::close::Caller,
) -> Result<()> {
    if workspace.is_none() && holders.is_empty() {
        return Ok(());
    }
    let open = crate::cleanup::current_panes(shell)?;
    let workspaces = close_workspaces(shell)?;
    let ids: std::collections::BTreeSet<_> = workspace
        .into_iter()
        .chain(holders.iter().filter_map(|p| p.workspace_id.as_deref()))
        .collect();
    for id in ids {
        if workspace != Some(id)
            && open.iter().any(|p| {
                p.workspace_id.as_deref() == Some(id)
                    && !holders.iter().any(|holder| holder.pane_id == p.pane_id)
            })
        {
            continue;
        }
        let members = close_group(&workspaces, id);
        if !members.is_empty() {
            check_group(shell, ledger, id, None, caller, &open, &members)?;
        }
    }
    Ok(())
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
    ledger: &Ledger,
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
    close_workspace(
        shell,
        ledger,
        &anchor,
        &crate::close::Caller {
            workspace: caller_workspace.map(str::to_owned),
            ..crate::close::Caller::from_env()
        },
    )?;
    std::fs::remove_file(&file)?;
    Ok(Some(anchor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::{Fake, Output};

    #[test]
    fn last_pane_close_refuses_a_busy_linked_workspace_before_closing_anything() {
        let shell = Fake::new();
        shell.on(
            "herdr pane list",
            r#"{"result":{"panes":[
                {"pane_id":"w1:p1","workspace_id":"w1"},
                {"pane_id":"w2:p1","workspace_id":"w2","agent":"pi","agent_status":"working"}
            ]}}"#,
        );
        shell.on("herdr pane process-info --pane w1:p1", r#"{"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":10,"name":"sh"}]}}}"#);
        shell.on(
            "herdr workspace list",
            r#"{"result":{"workspaces":[
                {"workspace_id":"w1","pane_count":1,"worktree":{"repo_key":"repo","repo_root":"/repo","checkout_path":"/repo","is_linked_worktree":false}},
                {"workspace_id":"w2","pane_count":1,"worktree":{"repo_key":"repo","repo_root":"/repo","checkout_path":"/repo/.worktrees/child","is_linked_worktree":true}}
            ]}}"#,
        );
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let error =
            close_pane(&shell, &ledger, "w1:p1", &crate::close::Caller::default()).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("w2"), "{message}");
        assert!(message.contains("busy"), "{message}");
        assert!(
            shell
                .calls
                .lock()
                .unwrap()
                .iter()
                .all(|call| { !call.contains("pane close") && !call.contains("workspace close") }),
            "a blocked group must not receive a close request"
        );
    }

    #[test]
    fn removal_gets_the_setup_limit_not_the_routine_limit() {
        let shell = Fake::new();
        let checkout = std::env::temp_dir().join(crate::ledger::new_id());
        let repo = checkout.display().to_string();
        let path = format!("{repo}/.worktrees/x");
        shell.on("herdr pane list", r#"{"result":{"panes":[]}}"#);
        shell.on(
            &format!("git -C {repo} worktree list --porcelain"),
            &format!("worktree {repo}\n\nworktree {path}\nbranch refs/heads/x\n"),
        );
        let removal = format!("git -C {repo} worktree remove {path}");
        shell.on_output(
            &removal,
            Output {
                status: 1,
                stdout: String::new(),
                stderr: "stop here".into(),
            },
        );
        let tree = Linked {
            checkout,
            path,
            branch: "x".into(),
        };
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let evidence = crate::loss::Evidence::default_only(&repo);
        let error = remove(&shell, &ledger, &tree, None, &[], None, &evidence).unwrap_err();
        assert!(error.to_string().contains("stop here"), "{error}");
        let limits = shell.limits.lock().unwrap();
        let limit = limits
            .iter()
            .find(|(call, _)| *call == removal)
            .map(|(_, l)| *l);
        assert_eq!(limit, Some(crate::budget::SETUP_LIMIT));
    }
}
