//! Conservative cleanup plans. Every destructive action revalidates local state.
//!
//! One survey serves two scopes. `prune` sees every class and removes what a person confirms.
//! The `serve` sweep sees only entries whose removal loses nothing and applies them unasked.

use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::node::{Node, Snapshot};
use crate::probe::{Agent, Pane};
use crate::shell::Shell;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const DAY: u64 = 86_400_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tree {
    pub repo: String,
    pub path: String,
    pub branch: String,
    #[serde(default)]
    pub head: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<crate::worktree::Identity>,
}

/// How a tree found after the plan relates to the planned tree.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Drift {
    Same,
    /// The same repository, path, and branch at a new commit: live work, so a skip that the next
    /// survey re-plans.
    HeadMoved,
    /// Another repository, path, or branch: another tree, so a failure.
    Identity,
}

impl Tree {
    /// Compare `now`, a fresh listing, with this planned tree. Every recheck uses this, so a new
    /// commit is a skip and an identity change is a failure on each path.
    pub(crate) fn drift(&self, now: &Tree) -> Drift {
        if !crate::worktree::same(&self.repo, &now.repo)
            || !crate::worktree::same(&self.path, &now.path)
            || self.branch != now.branch
            || self.identity != now.identity
        {
            Drift::Identity
        } else if self.head != now.head {
            Drift::HeadMoved
        } else {
            Drift::Same
        }
    }

    /// The skip reason for `Drift::HeadMoved`.
    fn head_moved(&self, subject: &str, now: &str) -> String {
        format!(
            "refuse {subject}: HEAD moved since the survey ({} -> {now})",
            self.head
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub class: String,
    pub id: String,
    pub detail: String,
    pub removable: bool,
    pub tree: Option<Tree>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dirty_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirty_count: Option<usize>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Plan {
    pub entries: Vec<Entry>,
}

/// Who reads a survey. `Prune` is `doctor` and `prune`: every class, with removal after a
/// person confirms it. `Sweep` is the `serve` sweep: only entries whose removal loses nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Prune,
    Sweep,
}

/// A process and its working directory, from the host's process table.
#[derive(Debug, Clone)]
pub struct Proc {
    pub pid: u32,
    /// 0 when the parent is unknown; such a process belongs to no pane.
    pub ppid: u32,
    pub cwd: std::path::PathBuf,
}

/// Every process this user can see, with its resolved working directory, or `None` when the
/// process table cannot be read. Linux reads `/proc/<pid>/cwd`; macOS asks `lsof`, with parents
/// from `ps`. Dogsled's own process is included, so the caller's directory counts as used.
pub fn process_cwds(shell: &dyn Shell) -> Option<Vec<Proc>> {
    let proc = Path::new("/proc");
    if proc.join("self/cwd").exists() {
        let entries = std::fs::read_dir(proc).ok()?;
        return Some(
            entries
                .flatten()
                .filter_map(|e| {
                    let pid = e.file_name().to_str()?.parse::<u32>().ok()?;
                    // Another user's process, or one that already exited, has no readable cwd.
                    let cwd = std::fs::read_link(e.path().join("cwd")).ok()?;
                    let stat = std::fs::read_to_string(e.path().join("stat")).unwrap_or_default();
                    // The command name may contain spaces; the parent follows its last `)`.
                    let parent = stat
                        .rsplit_once(')')
                        .and_then(|(_, rest)| rest.split_whitespace().nth(1)?.parse().ok())
                        .unwrap_or(0);
                    Some(Proc {
                        pid,
                        ppid: parent,
                        cwd,
                    })
                })
                .collect(),
        );
    }
    let out = shell
        .run(
            "lsof",
            &["-a".into(), "-d".into(), "cwd".into(), "-Fpn".into()],
        )
        .ok()?;
    // lsof exits 1 when some processes are unreadable; its output still lists the rest.
    if out.stdout.is_empty() {
        return None;
    }
    let parents: BTreeMap<u32, u32> = shell
        .run("ps", &["-axo".into(), "pid=,ppid=".into()])
        .ok()
        .filter(crate::shell::Output::ok)
        .map(|ps| {
            ps.stdout
                .lines()
                .filter_map(|line| {
                    let mut fields = line.split_whitespace();
                    Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut pid = 0;
    let mut table = Vec::new();
    for line in out.stdout.lines() {
        if let Some(id) = line.strip_prefix('p') {
            pid = id.parse().unwrap_or(0);
        } else if let Some(cwd) = line.strip_prefix('n') {
            table.push(Proc {
                pid,
                ppid: parents.get(&pid).copied().unwrap_or(0),
                cwd: crate::worktree::resolved(Path::new(cwd)),
            });
        }
    }
    Some(table)
}

/// The first process working directory inside `path` that is not one of `owned`, if any.
fn process_in<'a>(
    table: &'a [Proc],
    path: &str,
    owned: &BTreeSet<u32>,
) -> Option<&'a std::path::PathBuf> {
    let root = crate::worktree::resolved(Path::new(path));
    table
        .iter()
        .filter(|p| !owned.contains(&p.pid))
        .map(|p| &p.cwd)
        .find(|cwd| cwd.starts_with(&root))
}

/// The processes that belong to the holder panes, whose own busy check decides for them. An
/// agent pane owns every descendant of its shell, such as the agent and its tool servers. A
/// shell owns only itself: a background job it started still holds the checkout.
fn pane_owned(shell: &dyn Shell, holders: &[Pane], table: &[Proc]) -> Result<BTreeSet<u32>> {
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for p in table {
        children.entry(p.ppid).or_default().push(p.pid);
    }
    let mut owned = BTreeSet::new();
    for pane in holders {
        let Some(info) = crate::probe::process_info(shell, &pane.pane_id)? else {
            continue;
        };
        let Some(root) = info["shell_pid"]
            .as_u64()
            .and_then(|p| u32::try_from(p).ok())
        else {
            continue;
        };
        if pane.agent.is_none() {
            owned.insert(root);
            continue;
        }
        let mut next = vec![root];
        while let Some(pid) = next.pop() {
            if owned.insert(pid) {
                next.extend(children.get(&pid).into_iter().flatten());
            }
        }
    }
    Ok(owned)
}

/// What one survey saw of the host, shared by every worktree's finding.
struct Seen {
    panes: Vec<Pane>,
    workspaces: Vec<Workspace>,
    cwds: Option<Vec<Proc>>,
    evidence: BTreeMap<String, crate::loss::Evidence>,
    /// Agent panes that called `done` and ended that turn. Any other idle agent waits for input.
    done: BTreeSet<String>,
    agents: Vec<Agent>,
    launches: crate::launch::Survey,
}

impl Seen {
    /// Closing the last pane also closes its workspace, potentially the whole group.
    fn pane_group_hold(&self, pane: &str) -> Option<String> {
        pane_group_hold(&self.panes, &self.workspaces, pane)
    }

    fn load(
        node: &Node<'_>,
        cwds: Option<Vec<Proc>>,
        evidence: BTreeMap<String, crate::loss::Evidence>,
    ) -> Result<Self> {
        let (panes, terminals) = pane_snapshot(node.shell)?;
        let agents = crate::probe::agent_list(node.shell)?;
        let done = crate::done::finished(node.ledger, &agents)?;
        let launches = crate::launch::Survey::load(node.ledger, &panes, &terminals, &agents)?;
        Ok(Self {
            panes,
            workspaces: workspaces(node.shell)?,
            cwds,
            evidence,
            done,
            agents,
            launches,
        })
    }
}

pub fn remember(ledger: &Ledger, tree: Tree) -> Result<()> {
    ledger.with_lock("repositories", || {
        let mut repos: BTreeSet<String> = ledger.read("repositories.json")?.unwrap_or_default();
        repos.insert(tree.repo.clone());
        ledger.write("repositories.json", &repos)
    })?;
    ledger.with_lock("worktrees", || {
        let mut trees: Vec<Tree> = ledger.read("worktrees.json")?.unwrap_or_default();
        trees.retain(|t| !crate::worktree::same(&t.path, &tree.path));
        trees.push(tree);
        ledger.write("worktrees.json", &trees)
    })
}

fn git(shell: &dyn Shell, repo: &str, args: &[&str]) -> Result<crate::shell::Output> {
    shell.run(
        "git",
        &[
            vec!["-C".into(), repo.into()],
            args.iter().map(|s| (*s).into()).collect(),
        ]
        .concat(),
    )
}

#[derive(Debug, Deserialize)]
struct Workspace {
    workspace_id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    worktree: Option<WorkspaceTree>,
}

#[derive(Debug, Deserialize)]
struct WorkspaceTree {
    repo_root: String,
    repo_key: String,
    is_linked_worktree: bool,
}

/// Herdr groups open workspace records by `repo_key`, not the Git worktree inventory.
/// A linked workspace closes alone. A primary also closes alone if another primary
/// holds the group; otherwise it requires an explicit group close, which we never do.
fn workspace_group_hold(workspaces: &[Workspace], id: &str) -> Option<String> {
    let tree = workspaces
        .iter()
        .find(|w| w.workspace_id == id)?
        .worktree
        .as_ref()
        .filter(|t| !t.is_linked_worktree)?;
    let others: Vec<_> = workspaces
        .iter()
        .filter(|w| w.workspace_id != id)
        .filter(|w| {
            w.worktree
                .as_ref()
                .is_some_and(|t| t.repo_key == tree.repo_key)
        })
        .collect();
    if others.is_empty()
        || others
            .iter()
            .any(|w| w.worktree.as_ref().is_some_and(|t| !t.is_linked_worktree))
    {
        return None;
    }
    let ids = others
        .iter()
        .map(|w| w.workspace_id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "closing its pane would close linked workspaces {ids}"
    ))
}

/// Shared guard for an explicit close, host handoff, and the sweep's pane removal.
pub fn close_group_hold(shell: &dyn Shell, panes: &[Pane], pane: &str) -> Result<Option<String>> {
    Ok(pane_group_hold(panes, &workspaces(shell)?, pane))
}

fn pane_group_hold(panes: &[Pane], workspaces: &[Workspace], pane: &str) -> Option<String> {
    let workspace = panes
        .iter()
        .find(|p| p.pane_id == pane)?
        .workspace_id
        .as_deref()?;
    if panes
        .iter()
        .filter(|p| p.workspace_id.as_deref() == Some(workspace))
        .count()
        != 1
    {
        return None;
    }
    workspace_group_hold(workspaces, workspace)
}

fn workspaces(shell: &dyn Shell) -> Result<Vec<Workspace>> {
    let out = shell.run_ok("herdr", &["workspace".into(), "list".into()])?;
    let value: serde_json::Value = serde_json::from_str(&out.stdout)?;
    Ok(serde_json::from_value(
        value
            .pointer("/result/workspaces")
            .cloned()
            .ok_or_else(|| Error::Invalid("workspace list has no workspaces".into()))?,
    )?)
}

/// The main checkout of the repository that holds `path`, or `None` outside a repository.
pub(crate) fn main_root(shell: &dyn Shell, path: &str) -> Result<Option<String>> {
    let root = git(
        shell,
        path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    if !root.ok() {
        return Ok(None);
    }
    let root = Path::new(root.stdout.trim());
    Ok(root
        .file_name()
        .filter(|n| *n == ".git")
        .and(root.parent())
        .map(|parent| parent.display().to_string()))
}

/// Only ENOENT is stale. Other errors and dangling symlinks remain reportable.
/// Forgetting a record deletes no data, even if an absent path is on an unmounted volume.
/// Placement remembers the path again if it returns.
fn missing(path: &str) -> bool {
    std::fs::symlink_metadata(path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

/// The `kind` of a worktree entry whose directory remains but that Git can no longer remove.
pub const UNLINKED: &str = "unlinked";

fn unlinked_reason(tree: &Tree, prunable: Option<bool>) -> Option<&'static str> {
    crate::worktree::unlinked_reason(&tree.path, prunable)
}

/// A hold, never a removal: the directory may hold content that no Git check can vouch for.
/// The owner decides; once the directory is gone, the stale-registration rule drops the record.
fn unlinked(tree: &Tree, reason: &str) -> Entry {
    let mut e = entry(
        "worktrees",
        tree.path.clone(),
        format!(
            "held: {reason}; directory remains: {}. Dogsled does not delete it. Check that nothing in it is needed, then move or delete the directory yourself; the next cleanup pass then drops its registration",
            tree.path
        ),
        false,
    );
    e.kind = Some(UNLINKED.into());
    e.tree = Some(tree.clone());
    e
}

/// Registered and configured repositories, and those of live panes, Herdr workspaces, and the
/// caller. Doctor does not scan home or change registrations.
fn repositories(
    node: &Node<'_>,
    panes: &[Pane],
    workspaces: &[Workspace],
    plan: &mut Plan,
) -> Result<BTreeSet<String>> {
    let recorded: Vec<Tree> = node.ledger.read("worktrees.json")?.unwrap_or_default();
    let mut repos: BTreeSet<String> = node.ledger.read("repositories.json")?.unwrap_or_default();
    repos.extend(recorded.iter().map(|t| t.repo.clone()));
    repos.retain(|repo| {
        if !missing(repo) {
            return true;
        }
        plan.entries.push(entry(
            "repositories",
            repo.clone(),
            "missing remembered repository; drops stale entries from repositories.json and worktrees.json".into(),
            true,
        ));
        false
    });
    for tree in &recorded {
        if missing(&tree.path) && !missing(&tree.repo) {
            let mut stale = entry(
                "worktrees",
                tree.path.clone(),
                "missing registered worktree; drops the stale registration".into(),
                true,
            );
            if tree.identity.is_some() {
                stale.tree = Some(tree.clone());
            }
            plan.entries.push(stale);
        }
    }
    for configured in &node.cleanup.repositories {
        let path = configured.strip_prefix("~/").map_or_else(
            || configured.clone(),
            |rest| Path::new(node.home).join(rest).display().to_string(),
        );
        let root = if Path::new(&path).is_dir() {
            match main_root(node.shell, &path) {
                Ok(root) => root,
                Err(error) => {
                    plan.entries.push(entry(
                        "repositories",
                        path.clone(),
                        format!("cleanup.repositories entry is unreadable: {error}"),
                        false,
                    ));
                    continue;
                }
            }
        } else {
            None
        };
        match root {
            Some(root) => {
                repos.insert(root);
            }
            None => plan.entries.push(entry(
                "repositories",
                path,
                "configured in cleanup.repositories but not a Git checkout".into(),
                false,
            )),
        }
    }
    // A discovered Herdr repository is still a warning when its directory has vanished.
    repos.extend(
        workspaces
            .iter()
            .filter_map(|w| w.worktree.as_ref())
            .map(|w| w.repo_root.clone()),
    );
    let cwd = std::env::current_dir()?;
    for path in panes
        .iter()
        .flat_map(|p| [p.cwd.as_deref(), p.foreground_cwd.as_deref()])
        .flatten()
        .chain(
            workspaces
                .iter()
                .filter_map(|w| w.worktree.as_ref())
                .map(|t| t.repo_root.as_str()),
        )
        .chain(cwd.to_str())
    {
        match main_root(node.shell, path) {
            Ok(root) => repos.extend(root),
            Err(error) => plan.entries.push(entry(
                "repositories",
                path.into(),
                format!("discovered repository is unreadable: {error}"),
                false,
            )),
        }
    }
    // One repository reached through a symlink is still one repository.
    Ok(repos
        .into_iter()
        .map(|repo| std::fs::canonicalize(&repo).map_or(repo, |p| p.display().to_string()))
        .collect())
}

/// Linked worktrees, on a branch or detached (an empty branch). The main checkout is never a
/// cleanup candidate.
fn repository_trees(shell: &dyn Shell, repo: &str) -> Result<Vec<Tree>> {
    Ok(listed(shell, repo)?.into_iter().skip(1).collect())
}

/// Every working tree of `repo`, the main checkout first. A detached worktree has no branch.
/// A prunable entry is not a working tree any more: `git worktree remove` refuses it.
fn listed(shell: &dyn Shell, repo: &str) -> Result<Vec<Tree>> {
    Ok(porcelain(shell, repo)?
        .into_iter()
        .filter(|(_, prunable)| !prunable)
        .map(|(tree, _)| tree)
        .collect())
}

/// Every entry of `git worktree list`, with whether Git calls it prunable.
fn porcelain(shell: &dyn Shell, repo: &str) -> Result<Vec<(Tree, bool)>> {
    let mut result = Vec::new();
    let mut prunable = false;
    let output = git(shell, repo, &["worktree", "list", "--porcelain", "-z"])?;
    if !output.ok() {
        return Err(Error::Invalid(format!(
            "cannot list worktrees in {repo}: {}",
            output.stderr.trim()
        )));
    }
    let mut tree = Tree {
        repo: repo.into(),
        path: String::new(),
        branch: String::new(),
        head: String::new(),
        identity: None,
    };
    for field in output.stdout.split('\0') {
        if let Some(value) = field.strip_prefix("worktree ") {
            tree.path = value.into();
        } else if let Some(value) = field.strip_prefix("HEAD ") {
            tree.head = value.into();
        } else if let Some(value) = field.strip_prefix("branch refs/heads/") {
            tree.branch = value.into();
        } else if field == "prunable" || field.starts_with("prunable ") {
            prunable = true;
        } else if field.is_empty() && !tree.path.is_empty() {
            result.push((tree.clone(), std::mem::take(&mut prunable)));
            tree.path.clear();
            tree.branch.clear();
            tree.head.clear();
        }
    }
    Ok(result)
}

fn entry(class: &str, id: String, detail: String, removable: bool) -> Entry {
    Entry {
        class: class.into(),
        id,
        detail,
        removable,
        tree: None,
        kind: None,
        dirty_paths: Vec::new(),
        dirty_count: None,
    }
}

pub fn current_panes(shell: &dyn Shell) -> Result<Vec<Pane>> {
    Ok(pane_snapshot(shell)?.0)
}

/// One `herdr pane list` read: the panes, and each pane's terminal id. Cleanup and the unit
/// stop proof share it.
pub fn pane_snapshot(shell: &dyn Shell) -> Result<(Vec<Pane>, BTreeMap<String, String>)> {
    let out = shell.run_ok("herdr", &["pane".into(), "list".into()])?;
    let value: serde_json::Value = serde_json::from_str(&out.stdout)?;
    let raw = value
        .pointer("/result/panes")
        .cloned()
        .ok_or_else(|| Error::Invalid("pane list has no panes".into()))?;
    let terminals = raw
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            Some((
                p["pane_id"].as_str()?.to_owned(),
                p["terminal_id"].as_str()?.to_owned(),
            ))
        })
        .collect();
    Ok((serde_json::from_value(raw)?, terminals))
}

fn stale(agent: &Agent, now: u64) -> bool {
    matches!(agent.state.as_str(), "idle" | "done")
        && agent
            .state_since_ms
            .is_some_and(|t| now.saturating_sub(t) > DAY)
}

fn disk(node: &Node<'_>, trees: &[Tree], plan: &mut Plan) -> Result<()> {
    let free = crate::probe::disk_free_percent(node.shell, node.home)?;
    if free >= 10.0 {
        return Ok(());
    }
    let paths: BTreeSet<_> = std::iter::once(node.ledger.dir.display().to_string())
        .chain(trees.iter().map(|t| t.path.clone()))
        .collect();
    let mut sizes = Vec::new();
    for path in paths {
        if !Path::new(&path).exists() {
            plan.entries.push(entry(
                "disk",
                path,
                "missing registered path; no disk usage available".into(),
                false,
            ));
            continue;
        }
        let measured = node
            .shell
            .run_ok("du", &["-sk".into(), "--".into(), path.clone()])
            .and_then(|out| {
                out.stdout
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse::<u64>().ok())
                    .ok_or_else(|| Error::Invalid(format!("invalid du output for {path}")))
            });
        match measured {
            Ok(size) => sizes.push((size, path)),
            Err(error) => {
                plan.entries
                    .push(entry("disk", path, format!("unreadable: {error}"), false));
            }
        }
    }
    sizes.sort_by_key(|a| std::cmp::Reverse(a.0));
    let largest = sizes
        .iter()
        .take(5)
        .map(|(size, path)| format!("{size} KiB {path}"))
        .collect::<Vec<_>>()
        .join("; ");
    plan.entries.push(entry(
        "disk",
        node.home.into(),
        format!(
            "{:.1}% used; largest dogsled-owned paths: {largest}",
            100.0 - free
        ),
        false,
    ));
    Ok(())
}

fn processes(shell: &dyn Shell, panes: &[Pane], plan: &mut Plan) -> Result<()> {
    let out = shell.run_ok("ps", &["-axo".into(), "pid=,ppid=,pcpu=,comm=".into()])?;
    let mut rows = BTreeMap::new();
    for line in out.stdout.lines() {
        let mut fields = line.split_whitespace();
        let parsed = fields
            .next()
            .and_then(|s| s.parse::<u32>().ok())
            .zip(fields.next().and_then(|s| s.parse::<u32>().ok()))
            .zip(fields.next().and_then(|s| s.parse::<f64>().ok()));
        let Some(((pid, parent), cpu)) = parsed else {
            return Err(Error::Invalid("invalid ps output".into()));
        };
        rows.insert(pid, (parent, cpu, fields.collect::<Vec<_>>().join(" ")));
    }
    if !rows.values().any(|(_, cpu, _)| *cpu > 100.0) {
        return Ok(());
    }
    let mut roots = BTreeSet::new();
    for pane in panes {
        // A pane that closed since the list has no processes left to own.
        let Some(info) = crate::probe::process_info(shell, &pane.pane_id)? else {
            continue;
        };
        if let Some(pid) = info["shell_pid"].as_u64() {
            roots.insert(pid as u32);
        }
        if let Some(children) = info["foreground_processes"].as_array() {
            for child in children {
                if let Some(pid) = child["pid"].as_u64() {
                    roots.insert(pid as u32);
                }
            }
        }
    }
    for (&pid, (_, cpu, command)) in &rows {
        if *cpu <= 100.0 {
            continue;
        }
        let mut current = pid;
        let mut seen = BTreeSet::new();
        while !roots.contains(&current) && seen.insert(current) {
            let Some((parent, ..)) = rows.get(&current) else {
                break;
            };
            current = *parent;
        }
        if !roots.contains(&current) {
            plan.entries.push(entry(
                "processes",
                pid.to_string(),
                format!("{cpu:.1}% CPU {command}; report only"),
                false,
            ));
        }
    }
    Ok(())
}

pub fn doctor(node: &Node<'_>, now: u64) -> Result<Plan> {
    survey(node, Scope::Prune, now)
}

/// The readable repositories with their worktrees and fresh loss evidence. A missing or
/// unreadable repository becomes a warning, and the survey continues with the others.
fn readable(
    node: &Node<'_>,
    repos: BTreeSet<String>,
    plan: &mut Plan,
) -> (
    BTreeSet<String>,
    Vec<Tree>,
    BTreeMap<String, crate::loss::Evidence>,
) {
    let mut usable = BTreeSet::new();
    let mut trees = Vec::new();
    let mut evidence = BTreeMap::new();
    // An unreadable registry hides no candidate: Git's list still drives removal.
    let recorded: Vec<Tree> = node
        .ledger
        .read("worktrees.json")
        .ok()
        .flatten()
        .unwrap_or_default();
    for repo in repos {
        let discovered =
            crate::worktree::with_lifecycle(node.shell, node.ledger, Path::new(&repo), |guard| {
                let mut found = porcelain(node.shell, &repo)?;
                let mut refused = BTreeSet::new();
                for (tree, prunable) in found.iter_mut().skip(1) {
                    if *prunable || unlinked_reason(tree, Some(false)).is_some() {
                        continue;
                    }
                    let saved = recorded.iter().find(|saved| {
                        crate::worktree::same(&saved.path, &tree.path) && saved.identity.is_some()
                    });
                    let captured = (|| {
                        if let Some(saved) = saved {
                            let linked = guard.observe(node.shell, &tree.path)?;
                            if linked
                                .as_ref()
                                .is_some_and(|now| saved.identity != now.identity)
                            {
                                return Err(crate::worktree::refuse("worktree identity changed"));
                            }
                            Ok(linked)
                        } else {
                            guard.discover(node.shell, &tree.path)
                        }
                    })();
                    match captured {
                        Ok(Some(linked)) => *tree = linked.tree(),
                        Ok(None) => (),
                        Err(error) => {
                            let mut warning = entry(
                                "worktrees",
                                tree.path.clone(),
                                format!("unreadable: {error}"),
                                false,
                            );
                            warning.tree = Some(tree.clone());
                            plan.entries.push(warning);
                            refused.insert(tree.path.clone());
                        }
                    }
                }
                Ok((found, refused))
            });
        match discovered {
            Ok((found, refused)) => {
                for (tree, prunable) in found.iter().skip(1) {
                    if refused.contains(&tree.path) || !Path::new(&tree.path).exists() {
                        continue;
                    }
                    match unlinked_reason(tree, Some(*prunable)) {
                        Some(reason) => plan.entries.push(unlinked(tree, reason)),
                        None => trees.push(tree.clone()),
                    }
                }
                // Dogsled made it, Git forgot it (`git worktree prune`), and its directory stays.
                for tree in recorded.iter().filter(|t| {
                    crate::worktree::same(&t.repo, &repo)
                        && !missing(&t.path)
                        && !found
                            .iter()
                            .any(|(f, _)| crate::worktree::same(&f.path, &t.path))
                }) {
                    if let Some(reason) = unlinked_reason(tree, None) {
                        plan.entries.push(unlinked(tree, reason));
                    }
                }
                let fetched = crate::loss::Evidence::fetch(node.shell, &repo);
                if let Some(warning) = &fetched.warning {
                    plan.entries
                        .push(entry("repositories", repo.clone(), warning.clone(), false));
                }
                evidence.insert(repo.clone(), fetched);
                usable.insert(repo);
            }
            Err(error) => plan.entries.push(entry(
                "repositories",
                repo.clone(),
                format!(
                    "{error}; repository entry {repo}; check {} and {} and cleanup.repositories",
                    node.ledger.dir.join("repositories.json").display(),
                    node.ledger.dir.join("worktrees.json").display()
                ),
                false,
            )),
        }
    }
    (usable, trees, evidence)
}

/// The cleanup plan for `scope`. In `Sweep` scope a worktree that cannot be checked is skipped,
/// so one broken entry does not stop the sweep of the others.
pub fn survey(node: &Node<'_>, scope: Scope, now: u64) -> Result<Plan> {
    // Process directories first: dogsled's own Git calls below run with `-C`, not `cd`.
    let cwds = process_cwds(node.shell);
    let mut seen = Seen::load(node, cwds, BTreeMap::new())?;
    let mut plan = Plan::default();
    let repos = repositories(node, &seen.panes, &seen.workspaces, &mut plan)?;
    let (repos, trees, evidence) = readable(node, repos, &mut plan);
    seen.evidence = evidence;
    let mut closing = Closing::default();
    let found = parallel(&trees, |tree| disposition(node, tree, &seen));
    let mut removed = BTreeSet::new();
    for (tree, found) in trees.iter().zip(found) {
        let found = match found {
            Ok(found) => found,
            Err(_)
                if crate::worktree::with_lifecycle(
                    node.shell,
                    node.ledger,
                    Path::new(&tree.repo),
                    |guard| Ok(gone(guard, node.shell, tree)),
                )
                .unwrap_or(false) =>
            {
                plan.entries.extend(vanished(node, tree));
                continue;
            }
            Err(error) => {
                let mut warning = entry(
                    "worktrees",
                    tree.path.clone(),
                    format!("unreadable: {error}"),
                    false,
                );
                warning.tree = Some(tree.clone());
                plan.entries.push(warning);
                continue;
            }
        };
        if found.removable {
            removed.insert(tree.path.clone());
            closing.workspaces.extend(found.usage.workspace.clone());
            closing
                .panes
                .extend(found.usage.holders.iter().map(|p| p.pane_id.clone()));
        }
        let mut e = entry(
            "worktrees",
            tree.path.clone(),
            found.detail,
            found.removable,
        );
        if found.landed_dirty {
            e.kind = Some("landed_dirty".into());
            e.dirty_count = Some(found.loss.dirty.len());
            e.dirty_paths = found.loss.dirty.into_iter().take(10).collect();
        }
        e.tree = Some(tree.clone());
        plan.entries.push(e);
    }
    for repo in &repos {
        if let Err(error) = branches(node, repo, &seen.evidence[repo], &seen.launches, &mut plan) {
            plan.entries.push(entry(
                "repositories",
                repo.clone(),
                format!("cannot inspect branches: {error}"),
                false,
            ));
        }
    }
    idle(node, now, &seen, &closing, &mut plan)?;
    if scope == Scope::Sweep {
        return Ok(plan);
    }
    targets(node, &repos, &trees, &removed, &seen, &mut plan)?;
    prune_only(node, &seen, now, &mut plan)?;
    Ok(plan)
}

/// The classes the sweep never applies: expired jobs, disk, and processes.
fn prune_only(node: &Node<'_>, seen: &Seen, now: u64, plan: &mut Plan) -> Result<()> {
    for job in crate::jobs::list(node.ledger)? {
        if crate::jobs::terminal(&job) && job.ended_ms.is_some_and(|t| now.saturating_sub(t) > DAY)
        {
            plan.entries.push(entry(
                "jobs",
                job.id.clone(),
                format!(
                    "ended; log {}",
                    crate::jobs::store(node.ledger)
                        .dir
                        .join(format!("{}.log", job.id))
                        .display()
                ),
                true,
            ));
        }
    }
    // Only registered worktrees are dogsled-owned disk paths.
    let owned = node
        .ledger
        .read::<Vec<Tree>>("worktrees.json")?
        .unwrap_or_default();
    disk(node, &owned, plan)?;
    processes(node.shell, &seen.panes, plan)
}

/// The signature that Cargo, and every tool that follows the Cache Directory Tagging
/// Specification, writes at the start of `CACHEDIR.TAG`.
const CACHEDIR_SIGNATURE: &str = "Signature: 8a477f597d28d172789f06886806bc55";

fn cache_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
        && std::fs::read_to_string(path.join("CACHEDIR.TAG"))
            .is_ok_and(|tag| tag.starts_with(CACHEDIR_SIGNATURE))
}

/// Build output: a directory tagged with `CACHEDIR.TAG` directly under a main checkout or a
/// linked worktree, such as Rust's `target/`. Removal loses only what a build makes again, so it
/// is removable when no pane, workspace, or process uses the checkout. Only `prune` removes it.
fn targets(
    node: &Node<'_>,
    repos: &BTreeSet<String>,
    trees: &[Tree],
    removed: &BTreeSet<String>,
    seen: &Seen,
    plan: &mut Plan,
) -> Result<()> {
    let mut checkouts = Vec::new();
    for repo in repos {
        checkouts.extend(listed(node.shell, repo)?.into_iter().take(1));
    }
    // A worktree that this plan removes takes its build output with it.
    checkouts.extend(trees.iter().filter(|t| !removed.contains(&t.path)).cloned());
    // Work in a linked worktree uses that worktree, even when it sits inside the main checkout.
    let linked: Vec<std::path::PathBuf> = trees
        .iter()
        .map(|t| crate::worktree::resolved(Path::new(&t.path)))
        .collect();
    for checkout in checkouts {
        let Ok(children) = std::fs::read_dir(&checkout.path) else {
            continue;
        };
        let mut found: Vec<_> = children
            .flatten()
            .map(|child| child.path())
            .filter(|path| cache_dir(path))
            .collect();
        if found.is_empty() {
            continue;
        }
        found.sort();
        let users = checkout_users(node, &checkout, &linked, seen)?;
        for path in found {
            let mut e = entry(
                "targets",
                path.display().to_string(),
                if users.is_empty() {
                    format!("build output in {}; nothing uses it", checkout.path)
                } else {
                    format!(
                        "build output in {}; refused: {}",
                        checkout.path,
                        users.join("; ")
                    )
                },
                users.is_empty(),
            );
            e.tree = Some(checkout.clone());
            plan.entries.push(e);
        }
    }
    Ok(())
}

/// What uses a checkout: its Herdr workspace, a pane in it, or a process working in it. Work in
/// another linked worktree nested inside it is that worktree's.
fn checkout_users(
    node: &Node<'_>,
    checkout: &Tree,
    linked: &[std::path::PathBuf],
    seen: &Seen,
) -> Result<Vec<String>> {
    let root = crate::worktree::resolved(Path::new(&checkout.path));
    let own = |path: &Path| {
        path.starts_with(&root)
            && !linked
                .iter()
                .any(|tree| *tree != root && tree.starts_with(&root) && path.starts_with(tree))
    };
    let mut users = Vec::new();
    if let Some(workspace) =
        crate::worktree::workspace(node.shell, Path::new(&checkout.repo), &checkout.path)?
    {
        users.push(format!("workspace {workspace} shows it"));
    }
    for pane in &seen.panes {
        if [pane.cwd.as_deref(), pane.foreground_cwd.as_deref()]
            .into_iter()
            .flatten()
            .any(|cwd| own(&crate::worktree::resolved(Path::new(cwd))))
        {
            users.push(format!("{} works in it", pane.pane_id));
        }
    }
    match &seen.cwds {
        None => users.push("the process table is unreadable".into()),
        Some(cwds) => {
            if let Some(cwd) = cwds.iter().map(|p| &p.cwd).find(|cwd| own(cwd)) {
                users.push(format!("a process works in {}", cwd.display()));
            }
        }
    }
    Ok(users)
}

fn remove_target(path: &str) -> Result<String> {
    // A symlink or a directory that lost its tag is not the build output the plan saw.
    if !cache_dir(Path::new(path)) {
        return Err(Error::Invalid(format!(
            "refuse {path}: no longer a tagged cache directory"
        )));
    }
    std::fs::remove_dir_all(path)?;
    Ok(format!("removed build output {path}"))
}

/// `f` over `items` on a few threads, in order. Each item spawns Git processes, which run
/// independently.
fn parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> Result<R> + Sync) -> Vec<Result<R>> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let workers = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .clamp(1, 8);
    let mut results: Vec<(usize, Result<R>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(item) = items.get(i) else {
                            return done;
                        };
                        done.push((i, f(item)));
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
            .collect()
    });
    results.sort_by_key(|(i, _)| *i);
    results.into_iter().map(|(_, r)| r).collect()
}

/// Panes and workspaces that a removable worktree's removal closes.
#[derive(Default)]
struct Closing {
    workspaces: BTreeSet<String>,
    panes: BTreeSet<String>,
}

/// Local refs without a checkout use the same loss rule as checkouts. Keep the default ref.
fn branches(
    node: &Node<'_>,
    repo: &str,
    evidence: &crate::loss::Evidence,
    launches: &crate::launch::Survey,
    plan: &mut Plan,
) -> Result<()> {
    let shell = node.shell;
    let pending = crate::prepare::pending_trees(node.ledger, launches)?;
    // Git keeps a prunable worktree's branch checked out until the entry is pruned.
    let checked_out: BTreeSet<String> = porcelain(shell, repo)?
        .into_iter()
        .map(|(t, _)| t.branch)
        .collect();
    let base = crate::integration::base(shell, repo)?;
    let default = base
        .strip_prefix("refs/remotes/origin/")
        .or_else(|| base.strip_prefix("refs/heads/"))
        .unwrap_or(&base);
    let out = git(
        shell,
        repo,
        &[
            "for-each-ref",
            "--format=%(objectname) %(refname)",
            "refs/heads/",
        ],
    )?;
    if !out.ok() {
        return Err(Error::Invalid(format!(
            "cannot list branches in {repo}: {}",
            out.stderr.trim()
        )));
    }
    let candidates: Vec<(&str, &str, &str)> = out
        .stdout
        .lines()
        .filter_map(|line| {
            let (head, reference) = line.split_once(' ')?;
            let branch = reference.strip_prefix("refs/heads/")?;
            (branch != default && !checked_out.contains(branch))
                .then_some((head, reference, branch))
        })
        .collect();
    let found = parallel(&candidates, |&(_, reference, _)| {
        evidence.assess(shell, reference, None)
    });
    for (&(head, _, branch), found) in candidates.iter().zip(found) {
        let found = found?;
        let handoff = crate::done::list(node.ledger)?.iter().any(|m| {
            m.branch
                .as_ref()
                .is_some_and(|b| crate::worktree::same(&b.repo, repo) && b.name == branch)
        });
        let held = (handoff && !found.landed())
            || pending
                .iter()
                .any(|tree| crate::worktree::same(&tree.repo, repo) && tree.branch == branch);
        let detail = format!(
            "{}{}",
            found.detail(),
            if held {
                "; retained for handoff or pending preparation"
            } else {
                ""
            }
        );
        let removable = found.loses_nothing() && !held;
        let mut e = entry("branches", format!("{repo} {branch}"), detail, removable);
        e.tree = Some(Tree {
            repo: repo.into(),
            path: String::new(),
            branch: branch.into(),
            head: head.into(),
            identity: None,
        });
        plan.entries.push(e);
    }
    Ok(())
}

/// The single shell rule for prune and sweep. Launch ownership is checked before other
/// reasons so unresolved ownership cannot fall through. Obsolete terminals do not own panes.
fn idle_shell(
    node: &Node<'_>,
    pane: &Pane,
    launch: crate::launch::PaneLaunch,
    since: Option<u64>,
    jobs: &[crate::jobs::Job],
    items: &[crate::items::Item],
    now: u64,
) -> Option<String> {
    if !matches!(
        crate::probe::foreground(node.shell, &pane.pane_id),
        Ok(None)
    ) {
        return None;
    }
    match launch {
        crate::launch::PaneLaunch::Final => {
            return Some("final launch; evidence retained".into());
        }
        crate::launch::PaneLaunch::Unresolved => return None,
        _ => (),
    }
    if let Some(job) = jobs
        .iter()
        .find(|j| j.pane == pane.pane_id && j.ended_ms.is_some())
    {
        return Some(format!(
            "job {} ended{}; its log keeps the output",
            job.id,
            job.exit
                .map_or(String::new(), |code| format!(" with exit {code}"))
        ));
    }
    if pane
        .cwd
        .as_deref()
        .is_some_and(|cwd| !Path::new(cwd).exists())
    {
        return Some("shell in a deleted directory".into());
    }
    let own: Vec<_> = items
        .iter()
        .filter(|i| i.host == crate::hosts::LOCAL && i.pane == pane.pane_id)
        .collect();
    if own.iter().any(|i| i.closed_ms.is_none()) {
        return None;
    }
    if own
        .iter()
        .any(|i| i.kind == "agent" && i.closed_ms.is_some())
    {
        return Some("agent item closed".into());
    }
    if launch != crate::launch::PaneLaunch::Unrecorded
        || !own.is_empty()
        || jobs.iter().any(|j| j.pane == pane.pane_id)
    {
        return None;
    }
    since
        .filter(|since| now.saturating_sub(*since) >= crate::watchdog::IDLE_SHELL)
        .map(|since| {
            format!(
                "shell idle for {}",
                crate::watchdog::age(now.saturating_sub(since))
            )
        })
}

/// Both scopes exclude the caller and focused pane. Agent cleanup still requires `done`.
fn idle_panes(node: &Node<'_>, now: u64, seen: &Seen) -> Result<BTreeMap<String, String>> {
    let panes = &seen.panes;
    let agents = &seen.agents;
    let caller = std::env::var("HERDR_PANE_ID").ok();
    let eligible = |pane: &Pane| !pane.focused && caller.as_deref() != Some(pane.pane_id.as_str());
    let mut idle = BTreeMap::new();
    let jobs = crate::jobs::list(node.ledger)?;
    let items = crate::items::list(node.ledger)?;
    let snapshot = node
        .ledger
        .read::<Snapshot>("host.json")?
        .filter(|s| s.fresh(now));
    for pane in panes
        .iter()
        .filter(|p| eligible(p) && p.agent.is_none() && !agents.iter().any(|a| a.pane == p.pane_id))
    {
        let since = snapshot.as_ref().and_then(|s| {
            s.probe
                .panes
                .iter()
                .find(|p| p.pane_id == pane.pane_id && p.idle_shell)
                .and_then(|p| p.idle_since_ms)
        });
        if let Some(reason) = idle_shell(
            node,
            pane,
            seen.launches.pane(&pane.pane_id),
            since,
            &jobs,
            &items,
            now,
        ) {
            idle.insert(pane.pane_id.clone(), reason);
        }
    }
    let Some(snapshot) = snapshot else {
        return Ok(idle);
    };
    // An idle agent that has not called `done` waits for input, however long.
    let done = &seen.done;
    for agent in snapshot.probe.agent_panes.iter().filter(|a| {
        stale(a, now)
            && done.contains(&a.pane)
            && panes.iter().any(|p| p.pane_id == a.pane && eligible(p))
    }) {
        if agents.iter().any(|a| {
            a.pane == agent.pane
                && a.state == agent.state
                && a.transcript == agent.transcript
                && a.transcript_modified == agent.transcript_modified
        }) {
            idle.insert(
                agent.pane.clone(),
                format!(
                    "{} for {}",
                    agent.state,
                    crate::watchdog::age(now.saturating_sub(agent.state_since_ms.unwrap_or(now)))
                ),
            );
        }
    }
    Ok(idle)
}

/// Idle panes, and workspaces whose every pane is idle. A workspace closes as one entry; the
/// caller's workspace and those a worktree removal closes are not entries.
fn idle(node: &Node<'_>, now: u64, seen: &Seen, closing: &Closing, plan: &mut Plan) -> Result<()> {
    let panes = &seen.panes;
    let idle = idle_panes(node, now, seen)?;
    // Final launches close through recovery, never a whole-workspace close that skips locks.
    let launches: BTreeSet<_> = panes
        .iter()
        .filter(|p| seen.launches.pane(&p.pane_id) == crate::launch::PaneLaunch::Final)
        .map(|p| p.pane_id.clone())
        .collect();
    let caller = std::env::var("HERDR_WORKSPACE_ID").ok();
    let mut members: BTreeMap<&str, Vec<&Pane>> = BTreeMap::new();
    for pane in panes {
        if let Some(workspace) = pane.workspace_id.as_deref() {
            members.entry(workspace).or_default().push(pane);
        }
    }
    let whole = |workspace: &str| {
        caller.as_deref() != Some(workspace)
            && !closing.workspaces.contains(workspace)
            && members[workspace].iter().all(|p| {
                idle.contains_key(&p.pane_id)
                    && !closing.panes.contains(&p.pane_id)
                    && !launches.contains(&p.pane_id)
            })
    };
    for (workspace, members) in &members {
        if !whole(workspace) {
            continue;
        }
        let label = seen
            .workspaces
            .iter()
            .find(|w| w.workspace_id == *workspace)
            .map_or("", |w| w.label.as_str());
        let detail = members
            .iter()
            .map(|p| format!("{} {}", p.pane_id, idle[&p.pane_id]))
            .collect::<Vec<_>>()
            .join("; ");
        let hold = workspace_group_hold(&seen.workspaces, workspace);
        plan.entries.push(entry(
            "workspaces",
            (*workspace).into(),
            match &hold {
                Some(reason) => format!("{label}: {detail}; {reason}"),
                None => format!("{label}: {detail}"),
            },
            hold.is_none(),
        ));
    }
    for (pane, detail) in &idle {
        let workspace = panes
            .iter()
            .find(|p| &p.pane_id == pane)
            .and_then(|p| p.workspace_id.as_deref());
        if closing.panes.contains(pane)
            || workspace.is_some_and(|w| closing.workspaces.contains(w) || whole(w))
        {
            continue;
        }
        let hold = seen.pane_group_hold(pane);
        plan.entries.push(entry(
            "panes",
            pane.clone(),
            match &hold {
                Some(reason) => format!("{detail}; {reason}"),
                None => detail.clone(),
            },
            hold.is_none(),
        ));
    }
    Ok(())
}

/// Panes associated with a checkout, and those actively in use. Removal closes idle holders.
struct Usage {
    workspace: Option<String>,
    holders: Vec<Pane>,
    in_use: Vec<String>,
}

fn usage(node: &Node<'_>, tree: &Tree, seen: &Seen) -> Result<Usage> {
    let checkout = Path::new(&tree.repo);
    let workspace = crate::worktree::workspace(node.shell, checkout, &tree.path)?;
    let holders: Vec<Pane> =
        crate::worktree::holders(&seen.panes, &tree.path, workspace.as_deref())
            .into_iter()
            .cloned()
            .collect();
    let mut in_use = Vec::new();
    for pane in &holders {
        if let Some(reason) = crate::worktree::busy(node.shell, pane)? {
            in_use.push(format!("{} is busy ({reason})", pane.pane_id));
        }
    }
    Ok(Usage {
        workspace,
        holders,
        in_use,
    })
}

/// Who applies the holds. Cleanup never takes an open item's checkout; `close` ends the
/// item, and an automatic close by a live owner also waits for landing. Only an explicit close
/// takes a checkout from an agent that has not called `done`.
#[derive(Clone, Copy)]
enum Decider<'a> {
    Cleanup,
    Close { guard: crate::close::Guard<'a> },
}

/// Pane ownership holds: focus, unresolved launches, and agents still waiting for input.
fn pane_holds(usage: &Usage, seen: &Seen, decider: Decider) -> Vec<String> {
    let mut held = usage.in_use.clone();
    for pane in &usage.holders {
        if pane.focused && matches!(decider, Decider::Cleanup) {
            held.push(format!("{} is focused", pane.pane_id));
        }
        if pane.agent.is_none() {
            let launch = seen.launches.pane(&pane.pane_id);
            let blocked = match decider {
                Decider::Cleanup => launch == crate::launch::PaneLaunch::Unresolved,
                Decider::Close {
                    guard: crate::close::Guard::Clean,
                } => false,
                Decider::Close { .. } => matches!(
                    launch,
                    crate::launch::PaneLaunch::Final | crate::launch::PaneLaunch::Unresolved
                ),
            };
            if blocked {
                held.push(format!("{} requires launch recovery", pane.pane_id));
            }
        }
    }
    if !matches!(
        decider,
        Decider::Close {
            guard: crate::close::Guard::Clean
        }
    ) {
        held.extend(
            usage
                .holders
                .iter()
                .filter(|p| p.agent.is_some() && !seen.done.contains(&p.pane_id))
                .filter(|p| {
                    !matches!(decider, Decider::Close {
                    guard: crate::close::Guard::StoppedOrphan { episode }
                } if episode.pane() == p.pane_id)
                })
                .map(|p| format!("{} waits for input; it has not called done", p.pane_id)),
        );
    }
    held
}

/// A shell in the checkout that the host snapshot reports idle for less than the shell rule's
/// age is still in use. `place` marks its preparation used at once, so a worktree it just made
/// holds only such a shell, and removal would race the owner's first commands. A shell with no
/// reported age is not held: that is the existing rule.
fn young_shells(node: &Node<'_>, usage: &Usage) -> Result<Vec<String>> {
    let now = crate::ledger::now_ms();
    let Some(snapshot) = node
        .ledger
        .read::<Snapshot>("host.json")?
        .filter(|s| s.fresh(now))
    else {
        return Ok(Vec::new());
    };
    Ok(usage
        .holders
        .iter()
        .filter(|p| p.agent.is_none())
        .filter(|p| {
            snapshot
                .probe
                .panes
                .iter()
                .find(|s| s.pane_id == p.pane_id && s.idle_shell)
                .and_then(|s| s.idle_since_ms)
                .is_some_and(|since| now.saturating_sub(since) < crate::watchdog::IDLE_SHELL)
        })
        .map(|p| {
            format!(
                "{} is a shell idle for less than {}",
                p.pane_id,
                crate::watchdog::age(crate::watchdog::IDLE_SHELL)
            )
        })
        .collect())
}

/// Holds are independent of loss and identical in both scopes.
fn held(
    node: &Node<'_>,
    tree: &Tree,
    usage: &Usage,
    seen: &Seen,
    loss: &crate::loss::Assessment,
    caller: &crate::close::Caller,
    decider: Decider,
) -> Result<Vec<String>> {
    let mut held = pane_holds(usage, seen, decider);
    // Reload under the final repository-protected check too, not just the survey.
    // Corrupt Job evidence propagates an error and cannot authorize removal.
    held.extend(
        crate::jobs::list(node.ledger)?
            .iter()
            .filter(|job| crate::jobs::holds(job, tree))
            .map(|job| format!("job {} owns this checkout ({})", job.id, job.state)),
    );
    if matches!(decider, Decider::Cleanup) {
        held.extend(young_shells(node, usage)?);
    }
    if usage
        .holders
        .iter()
        .any(|p| caller.pane.as_deref() == Some(&p.pane_id))
        || usage
            .workspace
            .as_ref()
            .is_some_and(|w| caller.workspace.as_ref() == Some(w))
        || caller.cwd.as_deref().is_some_and(|cwd| {
            crate::worktree::resolved(Path::new(cwd))
                .starts_with(crate::worktree::resolved(Path::new(&tree.path)))
        })
    {
        held.push("the caller uses this checkout".into());
    }
    match &seen.cwds {
        None => held.push("the process table is unreadable".into()),
        Some(cwds) => {
            let owned = pane_owned(node.shell, &usage.holders, cwds)?;
            if let Some(cwd) = process_in(cwds, &tree.path, &owned) {
                held.push(format!("a process works in {}", cwd.display()));
            }
        }
    }
    let items: Vec<crate::items::Item> = crate::items::list(node.ledger)?
        .into_iter()
        .filter(|i| i.kind == "agent" && i.closed_ms.is_none())
        .filter(|i| usage.holders.iter().any(|p| p.pane_id == i.pane))
        .collect();
    if matches!(decider, Decider::Cleanup) {
        held.extend(items.iter().map(|i| {
            format!(
                "open item {} for {}; close it with dogsled close",
                i.id, i.pane
            )
        }));
    } else if !loss.landed()
        && matches!(
            decider,
            Decider::Close {
                guard: crate::close::Guard::Clean
            }
        )
    {
        for item in items
            .iter()
            .filter(|i| seen.panes.iter().any(|p| p.pane_id == i.owner_pane))
        {
            held.push(format!("not landed by its owner {}", item.owner_pane));
        }
    }
    if crate::prepare::pending_trees(node.ledger, &seen.launches)?
        .iter()
        .any(|pending| crate::worktree::same(&pending.path, &tree.path))
        || seen
            .launches
            .pending_paths
            .iter()
            .any(|path| crate::worktree::same(path, &tree.path))
    {
        held.push("about to be used by a pending preparation or launch".into());
    }
    if let Some(lock) = crate::worktree::lock_reason(node.shell, Path::new(&tree.repo), &tree.path)?
        && lock != crate::worktree::AGENT_LOCK
    {
        held.push(format!("locked ({lock})"));
    }
    Ok(held)
}

/// Close uses the same holds, including owner context supplied by a remote automatic close.
pub fn close_holds(
    shell: &dyn Shell,
    ledger: &Ledger,
    linked: &crate::worktree::Linked,
    caller: &crate::close::Caller,
    guard: crate::close::Guard,
    loss: &crate::loss::Assessment,
) -> Result<()> {
    let cleanup = crate::config::Cleanup::default();
    let node = Node {
        shell,
        ledger,
        thresholds: crate::config::Thresholds::from(&crate::config::Policy::default()),
        home: "",
        cleanup: &cleanup,
    };
    let tree = Tree {
        repo: linked.checkout.display().to_string(),
        path: linked.path.clone(),
        branch: linked.branch.clone(),
        head: linked.head.clone(),
        identity: linked.identity.clone(),
    };
    let seen = Seen::load(&node, process_cwds(shell), BTreeMap::new())?;
    let usage = usage(&node, &tree, &seen)?;
    let holds = held(
        &node,
        &tree,
        &usage,
        &seen,
        loss,
        caller,
        Decider::Close { guard },
    )?;
    if !holds.is_empty() {
        return Err(crate::worktree::refuse(&format!(
            "{}: held: {}",
            tree.path,
            holds.join("; ")
        )));
    }
    Ok(())
}

struct Disposition {
    detail: String,
    removable: bool,
    usage: Usage,
    loss: crate::loss::Assessment,
    landed_dirty: bool,
}

fn disposition(node: &Node<'_>, tree: &Tree, seen: &Seen) -> Result<Disposition> {
    let usage = usage(node, tree, seen)?;
    let tip = if tree.branch.is_empty() {
        tree.head.clone()
    } else {
        format!("refs/heads/{}", tree.branch)
    };
    let evidence = seen
        .evidence
        .get(&tree.repo)
        .ok_or_else(|| Error::Invalid(format!("{}: missing pass evidence", tree.repo)))?;
    let loss = evidence.assess(node.shell, &tip, Some(&tree.path))?;
    let held = held(
        node,
        tree,
        &usage,
        seen,
        &loss,
        &crate::close::Caller::from_env(),
        Decider::Cleanup,
    )?;
    let landed_dirty = loss.landed() && !loss.dirty.is_empty() && held.is_empty();
    let name = if tree.branch.is_empty() {
        format!("detached at {}", &tree.head[..tree.head.len().min(10)])
    } else {
        format!("branch {}", tree.branch)
    };
    Ok(Disposition {
        detail: format!(
            "{name} {}; {}",
            loss.detail(),
            if held.is_empty() {
                "no holds".into()
            } else {
                format!("held: {}", held.join("; "))
            }
        ),
        removable: loss.loses_nothing() && held.is_empty(),
        usage,
        loss,
        landed_dirty,
    })
}

/// What one planned entry did when no error stopped it.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// The entry was removed, closed, or reported.
    Done(String),
    /// The recheck found that the state changed since the plan: the checkout is now dirty,
    /// held, or not landed, or the pane is now busy. The entry stays. This is the safeguard
    /// working, not a failure. A changed or missing identity, a command error, and unreadable
    /// evidence are errors instead.
    Skipped(String),
}

/// Another actor removed `tree` after this pass listed it: a fresh listing no longer has it and
/// its path is absent. The removal goal holds. A path that Git no longer lists but that still
/// exists is not gone, because it may hold content. A failed listing proves nothing.
fn gone(guard: &crate::worktree::Guard, shell: &dyn Shell, tree: &Tree) -> bool {
    crate::worktree::verified_absent(guard, shell, tree)
}

/// A Git error inside a listed worktree that has since gone is the race, not a failure:
/// `None` then. Any other error stays an error.
fn unless_gone<T>(
    guard: &crate::worktree::Guard,
    shell: &dyn Shell,
    tree: &Tree,
    result: Result<T>,
) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(_) if gone(guard, shell, tree) => Ok(None),
        Err(error) => Err(error),
    }
}

/// The survey's entry for a worktree that went during the survey: the same registration drop
/// that a later survey plans for a missing registered worktree. An unregistered one needs no
/// entry.
fn vanished(node: &Node<'_>, tree: &Tree) -> Option<Entry> {
    let recorded: Vec<Tree> = node
        .ledger
        .read("worktrees.json")
        .ok()
        .flatten()
        .unwrap_or_default();
    recorded
        .into_iter()
        .find(|t| crate::worktree::same(&t.path, &tree.path))
        .map(|_| {
            let mut vanished = entry(
                "worktrees",
                tree.path.clone(),
                "removed during the survey; drops the stale registration".into(),
                true,
            );
            vanished.tree = Some(tree.clone());
            vanished
        })
}

/// The apply-time recheck of a planned worktree.
enum Recheck {
    Removable(Usage, crate::loss::Evidence),
    /// The refusal and its reason, as `refuse <path>: <detail>`.
    Refused(String),
    /// Another actor removed it after the survey: Git no longer lists it and its path is
    /// absent. A path that Git no longer lists but that still exists is refused, because it
    /// may hold content.
    Gone,
}

fn validate_tree(
    guard: &crate::worktree::Guard,
    node: &Node<'_>,
    planned: &Tree,
) -> Result<Recheck> {
    let mut current = repository_trees(node.shell, &planned.repo)?
        .into_iter()
        .find(|t| crate::worktree::same(&t.path, &planned.path));
    if gone(guard, node.shell, planned) {
        return Ok(Recheck::Gone);
    }
    // Git lost it after the survey: the next survey holds it; this pass touches nothing.
    let reason = unlinked_reason(planned, current.as_ref().map(|_| false));
    if let Some(current) = current.as_mut() {
        current.identity = guard
            .observe(node.shell, &current.path)?
            .and_then(|linked| linked.identity);
    }
    let (Some(current), None) = (current, reason) else {
        return Err(Error::Invalid(format!(
            "refuse {}: {}; directory remains",
            planned.path,
            reason.unwrap_or_default()
        )));
    };
    let Some(repo) = unless_gone(
        guard,
        node.shell,
        planned,
        crate::prepare::main_checkout(node.shell, &current.path),
    )?
    else {
        return Ok(Recheck::Gone);
    };
    let drift = planned.drift(&current);
    if planned.identity.is_none()
        || drift == Drift::Identity
        || crate::worktree::resolved(&repo) != crate::worktree::resolved(Path::new(&planned.repo))
        || current.head.is_empty()
    {
        return Err(Error::Invalid(format!(
            "refuse {}: planned worktree identity changed",
            planned.path
        )));
    }
    // A new commit on the same branch is live work, like "not landed": a skip that the next
    // survey re-plans, not an identity change.
    if drift == Drift::HeadMoved {
        return Ok(Recheck::Refused(
            planned.head_moved(&planned.path, &current.head),
        ));
    }
    let evidence = crate::loss::Evidence::fetch(node.shell, &current.repo);
    if let Some(warning) = &evidence.warning {
        eprintln!("{warning}");
    }
    let seen = Seen::load(
        node,
        process_cwds(node.shell),
        BTreeMap::from([(current.repo.clone(), evidence)]),
    )?;
    // Another actor can remove it between this listing and the checks inside it.
    let Some(found) = unless_gone(
        guard,
        node.shell,
        planned,
        disposition(node, &current, &seen),
    )?
    else {
        return Ok(Recheck::Gone);
    };
    if !found.removable {
        return Ok(Recheck::Refused(format!(
            "refuse {}: no longer safe to remove: {}",
            current.path, found.detail
        )));
    }
    let evidence = seen
        .evidence
        .into_values()
        .next()
        .ok_or_else(|| Error::Invalid("missing loss evidence".into()))?;
    Ok(Recheck::Removable(found.usage, evidence))
}

/// Reconcile only the registration of the checkout that this operation captured.
pub fn forget_matching(ledger: &Ledger, tree: &Tree) -> Result<()> {
    ledger.with_lock("worktrees", || {
        let mut recorded: Vec<Tree> = ledger.read("worktrees.json")?.unwrap_or_default();
        recorded.retain(|record| {
            !crate::worktree::same(&record.path, &tree.path)
                || record.identity != tree.identity
                || !crate::worktree::same(&record.repo, &tree.repo)
                || record.branch != tree.branch
        });
        ledger.write("worktrees.json", &recorded)
    })
}

fn remove_tree(guard: &crate::worktree::Guard, node: &Node<'_>, tree: &Tree) -> Result<Outcome> {
    let (held, evidence) = match validate_tree(guard, node, tree)? {
        Recheck::Removable(held, evidence) => (held, evidence),
        Recheck::Refused(reason) => return Ok(Outcome::Skipped(reason)),
        Recheck::Gone => {
            // Another actor removed it after the survey. The goal holds; reconcile the record.
            forget_matching(node.ledger, tree)?;
            return Ok(Outcome::Done(format!(
                "{} was already removed; dropped its registration",
                tree.path
            )));
        }
    };
    let linked = crate::worktree::Linked {
        checkout: tree.repo.clone().into(),
        path: tree.path.clone(),
        branch: tree.branch.clone(),
        head: tree.head.clone(),
        gitdir: tree
            .identity
            .as_ref()
            .map(|identity| identity.gitdir.clone())
            .ok_or_else(|| Error::Invalid("missing administrative identity".into()))?,
        identity: tree.identity.clone(),
    };
    let mut holders = Vec::new();
    for pane in &held.holders {
        let launch = crate::launch::final_pane(node.ledger, node.shell, &pane.pane_id)?;
        match launch {
            crate::launch::PaneLaunch::Final if pane.agent.is_none() => holders.push(pane),
            crate::launch::PaneLaunch::Final | crate::launch::PaneLaunch::Unresolved => {
                return Err(Error::Invalid(format!(
                    "{}: unresolved launch ownership",
                    pane.pane_id
                )));
            }
            _ => holders.push(pane),
        }
    }
    let removed = crate::worktree::remove(
        node.shell,
        node.ledger,
        &linked,
        crate::worktree::Removal {
            guard,
            workspace: held.workspace.as_deref(),
            holders: &holders,
            caller_workspace: std::env::var("HERDR_WORKSPACE_ID").ok().as_deref(),
            evidence: &evidence,
        },
    )?;
    let closed = holders
        .iter()
        .map(|p| p.pane_id.as_str())
        .chain(held.workspace.as_deref())
        .collect::<Vec<_>>();
    Ok(Outcome::Done(format!(
        "removed {}{}{}{}",
        tree.path,
        if closed.is_empty() {
            String::new()
        } else {
            format!(" and closed {}", closed.join(", "))
        },
        if removed.branch_deleted {
            "; branch deleted"
        } else {
            ""
        },
        removed
            .anchor_closed
            .map_or(String::new(), |anchor| format!("; closed anchor {anchor}"))
    )))
}

fn drop_repository(node: &Node<'_>, repo: &str) -> Result<String> {
    node.ledger.with_lock("repositories", || {
        node.ledger.with_lock("worktrees", || {
            if !missing(repo) {
                return Err(Error::Invalid(format!(
                    "refuse {repo}: the repository is no longer missing"
                )));
            }
            let mut repos: BTreeSet<String> =
                node.ledger.read("repositories.json")?.unwrap_or_default();
            let mut trees: Vec<Tree> = node.ledger.read("worktrees.json")?.unwrap_or_default();
            repos.remove(repo);
            // An existing worktree may still hold content even when its main checkout is gone.
            trees.retain(|tree| !crate::worktree::same(&tree.repo, repo) || !missing(&tree.path));
            node.ledger.write("worktrees.json", &trees)?;
            node.ledger.write("repositories.json", &repos)
        })
    })?;
    Ok(format!("dropped stale repository registration {repo}"))
}

fn drop_registration(node: &Node<'_>, path: &str) -> Result<String> {
    node.ledger.with_lock("worktrees", || {
        let mut trees: Vec<Tree> = node.ledger.read("worktrees.json")?.unwrap_or_default();
        if !missing(path) {
            return Err(Error::Invalid(format!(
                "refuse {path}: the worktree is no longer missing"
            )));
        }
        trees.retain(|t| !crate::worktree::same(&t.path, path));
        node.ledger.write("worktrees.json", &trees)
    })?;
    Ok(format!("dropped the stale registration of {path}"))
}

fn remove_job(node: &Node<'_>, id: &str, now: u64) -> Result<String> {
    crate::jobs::validate_id(id)?;
    let store = crate::jobs::store(node.ledger);
    store.with_lock(id, || {
        let job = crate::jobs::get(node.ledger, id)?;
        if !crate::jobs::terminal(&job) || job.ended_ms.is_none_or(|t| now.saturating_sub(t) <= DAY)
        {
            return Err(Error::Invalid(format!("refuse {id}: job is not expired")));
        }
        for extension in ["log", "stop", "json"] {
            match std::fs::remove_file(store.dir.join(format!("{id}.{extension}"))) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        Ok(format!("removed job {id} and log"))
    })
}

/// Delete a branch the plan found integrated or pushed, at the planned commit.
fn delete_branch(shell: &dyn Shell, tree: &Tree) -> Result<Outcome> {
    let reference = format!("refs/heads/{}", tree.branch);
    let evidence = crate::loss::Evidence::fetch(shell, &tree.repo);
    if let Some(warning) = &evidence.warning {
        eprintln!("{warning}");
    }
    let current = git(shell, &tree.repo, &["rev-parse", &reference])?;
    if !current.ok() {
        return Err(Error::Invalid(format!(
            "refuse {}: branch identity changed",
            tree.branch
        )));
    }
    // A new commit on the branch is live work, like "not landed": a skip, not an identity change.
    if current.stdout.trim() != tree.head {
        return Ok(Outcome::Skipped(
            tree.head_moved(&tree.branch, current.stdout.trim()),
        ));
    }
    let loss = evidence.assess(shell, &reference, None)?;
    if !loss.loses_nothing() {
        return Ok(Outcome::Skipped(format!(
            "refuse {}: {}",
            tree.branch,
            loss.detail()
        )));
    }
    let out = git(shell, &tree.repo, &["branch", "-D", &tree.branch])?;
    if !out.ok() {
        return Err(Error::Invalid(format!(
            "cannot delete branch {} in {}: {}",
            tree.branch,
            tree.repo,
            out.stderr.trim()
        )));
    }
    Ok(Outcome::Done(format!(
        "deleted branch {} in {}",
        tree.branch, tree.repo
    )))
}

/// A failed action keeps its target identity for recipient routing.
#[derive(Debug, Serialize, Deserialize)]
pub struct Failure {
    pub entry: Entry,
    pub detail: String,
}

/// What prune did. Each entry is independent, so one failure does not stop the others.
/// `skipped` lists entries whose apply-time recheck refused them because their state changed
/// after the plan; they stay, and they are not failures.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Applied {
    pub results: Vec<String>,
    pub failures: Vec<String>,
    #[serde(default)]
    pub skipped: Vec<String>,
    /// Preserve the target identity for recipient routing; strings remain the status API.
    #[serde(default)]
    pub failure_entries: Vec<Failure>,
    #[serde(default)]
    pub repositories: Vec<Entry>,
    #[serde(default)]
    pub landed_dirty: Vec<Entry>,
    /// Worktrees held because Git can no longer remove them while their directories remain.
    #[serde(default)]
    pub unlinked: Vec<Entry>,
}

impl Applied {
    fn failed(&mut self, entry: &Entry, detail: String) {
        self.failures.push(detail.clone());
        self.failure_entries.push(Failure {
            entry: entry.clone(),
            detail,
        });
    }

    pub fn render(&self) -> String {
        self.results
            .iter()
            .cloned()
            .chain(self.skipped.iter().map(|s| format!("skipped: {s}")))
            .chain(self.failures.iter().map(|f| format!("failed: {f}")))
            .map(|line| line + "\n")
            .collect()
    }
}

pub fn apply(node: &Node<'_>, plan: &Plan, now: u64) -> Result<Applied> {
    let mut applied = Applied::default();
    // Checkout requests carry stale identity. Validate them read-only under the repository
    // mutex before any fresh discovery can bind a generation to a replacement.
    let mut actual = None;
    for requested in &plan.entries {
        let outcome = if requested.class == "worktrees" && requested.tree.is_some() {
            apply_one(node, &Plan::default(), requested, now)
        } else {
            if actual.is_none() {
                actual = Some(doctor(node, now)?);
            }
            apply_one(
                node,
                actual
                    .as_ref()
                    .ok_or_else(|| Error::Invalid("missing cleanup survey".into()))?,
                requested,
                now,
            )
        };
        applied.record(requested, outcome);
    }
    Ok(applied)
}

impl Applied {
    fn record(&mut self, entry: &Entry, outcome: Result<Outcome>) {
        match outcome {
            Ok(Outcome::Done(result)) => self.results.push(result),
            Ok(Outcome::Skipped(reason)) => self.skipped.push(reason),
            Err(e) => self.failed(entry, e.to_string()),
        }
    }
}

/// The classes the sweep applies. Everything else waits for `prune`.
pub const SWEPT: [&str; 5] = [
    "worktrees",
    "branches",
    "workspaces",
    "panes",
    "repositories",
];

/// The `serve` sweep: survey in `Sweep` scope and apply each removable entry with the same
/// checks as `prune`. Each entry is independent, so one failure does not stop the others.
pub fn sweep(node: &Node<'_>, now: u64) -> Result<Applied> {
    let plan = survey(node, Scope::Sweep, now)?;
    Ok(apply_survey(node, &plan, now))
}

/// Apply each removable entry of a sweep survey. Each removal rechecks its entry first, because
/// live work can change a checkout after the survey.
pub(crate) fn apply_survey(node: &Node<'_>, plan: &Plan, now: u64) -> Applied {
    let mut applied = Applied {
        repositories: plan
            .entries
            .iter()
            .filter(|e| e.class == "repositories" && !e.removable)
            .cloned()
            .collect(),
        landed_dirty: plan
            .entries
            .iter()
            .filter(|e| e.kind.as_deref() == Some("landed_dirty"))
            .cloned()
            .collect(),
        unlinked: plan
            .entries
            .iter()
            .filter(|e| e.kind.as_deref() == Some(UNLINKED))
            .cloned()
            .collect(),
        ..Applied::default()
    };
    for entry in plan
        .entries
        .iter()
        .filter(|e| e.removable && SWEPT.contains(&e.class.as_str()))
    {
        applied.record(entry, apply_one(node, plan, entry, now));
    }
    applied
}

fn apply_one(node: &Node<'_>, actual: &Plan, requested: &Entry, now: u64) -> Result<Outcome> {
    if requested.removable
        && requested.class == "worktrees"
        && let Some(tree) = &requested.tree
    {
        return crate::worktree::with_lifecycle(
            node.shell,
            node.ledger,
            Path::new(&tree.repo),
            |guard| remove_tree(guard, node, tree),
        );
    }
    if matches!(requested.class.as_str(), "disk" | "processes") {
        return Ok(Outcome::Done(format!(
            "{} {}: report only",
            requested.class, requested.id
        )));
    }
    // A refused entry stays: the plan shows why, and prune removes only eligible entries.
    if !requested.removable {
        return Ok(Outcome::Done(format!(
            "{} {}: kept",
            requested.class, requested.id
        )));
    }
    let same = |e: &&Entry| e.class == requested.class && e.id == requested.id;
    let Some(current) = actual
        .entries
        .iter()
        .filter(same)
        .find(|e| e.removable)
        .or_else(|| actual.entries.iter().find(same))
    else {
        // The survey lists panes and workspaces only while they are idle: absence means busy.
        if matches!(requested.class.as_str(), "panes" | "workspaces") {
            return Ok(Outcome::Skipped(format!(
                "refuse {} {}: no longer safe to remove: no longer listed as idle",
                requested.class, requested.id
            )));
        }
        return Err(Error::Invalid(format!(
            "refuse {} {}: no longer in the survey",
            requested.class, requested.id
        )));
    };
    let identity = || {
        Error::Invalid(format!(
            "refuse {}: planned worktree identity changed",
            requested.id
        ))
    };
    match (&requested.tree, &current.tree) {
        (None, None) => {}
        (Some(planned), Some(now)) => match planned.drift(now) {
            Drift::Same => {}
            // A new commit after the plan is live work, as in the removal recheck.
            Drift::HeadMoved => {
                return Ok(Outcome::Skipped(
                    planned.head_moved(&requested.id, &now.head),
                ));
            }
            Drift::Identity => return Err(identity()),
        },
        _ => return Err(identity()),
    }
    // The fresh survey refuses it now: its state changed after the plan.
    if !current.removable {
        return Ok(Outcome::Skipped(format!(
            "refuse {} {}: no longer safe to remove: {}",
            requested.class, requested.id, current.detail
        )));
    }
    let candidate = current;
    match candidate.class.as_str() {
        "worktrees" => match &requested.tree {
            Some(_) => Err(Error::Invalid(
                "checkout removal requires repository protection".into(),
            )),
            None => drop_registration(node, &candidate.id).map(Outcome::Done),
        },
        "branches" => match &requested.tree {
            Some(tree) => crate::worktree::with_lifecycle(
                node.shell,
                node.ledger,
                Path::new(&tree.repo),
                |_| delete_branch(node.shell, tree),
            ),
            None => Err(Error::Invalid("a branch entry has no branch".into())),
        },
        "workspaces" => close_workspace(node, &candidate.id, now),
        "panes" => close_pane(node, &candidate.id, now),
        "repositories" => drop_repository(node, &candidate.id).map(Outcome::Done),
        "jobs" => remove_job(node, &candidate.id, now).map(Outcome::Done),
        "targets" => remove_target(&candidate.id).map(Outcome::Done),
        _ => Err(Error::Invalid("unknown cleanup class".into())),
    }
}

/// Re-check a planned workspace close against a fresh survey under the cleanup lock.
fn close_workspace(node: &Node<'_>, id: &str, now: u64) -> Result<Outcome> {
    let seen = Seen::load(node, None, BTreeMap::new())?;
    if let Some(reason) = workspace_group_hold(&seen.workspaces, id) {
        return Ok(Outcome::Skipped(format!("refuse {id}: {reason}")));
    }
    let idle = idle_panes(node, now, &seen)?;
    for pane in seen
        .panes
        .iter()
        .filter(|p| p.workspace_id.as_deref() == Some(id))
    {
        if !idle.contains_key(&pane.pane_id)
            || crate::launch::final_pane(node.ledger, node.shell, &pane.pane_id)?
                == crate::launch::PaneLaunch::Final
        {
            return Ok(Outcome::Skipped(format!(
                "refuse {id}: workspace is no longer idle"
            )));
        }
    }
    match node
        .shell
        .run_ok("herdr", &["workspace".into(), "close".into(), id.into()])
    {
        Ok(_) => (),
        // Herdr closes a workspace with its last pane, so it may be gone already: the goal holds.
        Err(error) if refused_with(&error, "workspace_not_found") => {
            return Ok(Outcome::Done(format!("workspace {id} was already closed")));
        }
        Err(error) => return Err(error),
    }
    Ok(Outcome::Done(format!("closed workspace {id}")))
}

/// Whether Herdr refused a command with this error code.
fn refused_with(error: &Error, code: &str) -> bool {
    let Error::Command { stderr, .. } = error.root() else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(stderr)
        .ok()
        .and_then(|v| v.pointer("/error/code")?.as_str().map(str::to_owned))
        .as_deref()
        == Some(code)
}

/// Re-check a planned pane close against a fresh survey under the cleanup lock.
fn close_pane(node: &Node<'_>, id: &str, now: u64) -> Result<Outcome> {
    let seen = Seen::load(node, None, BTreeMap::new())?;
    if let Some(reason) = seen.pane_group_hold(id) {
        return Ok(Outcome::Skipped(format!("refuse {id}: {reason}")));
    }
    if !idle_panes(node, now, &seen)?.contains_key(id) {
        return Ok(Outcome::Skipped(format!(
            "refuse {id}: pane is no longer idle"
        )));
    }
    match crate::launch::final_pane(node.ledger, node.shell, id)? {
        crate::launch::PaneLaunch::Final
            if crate::launch::close_final(node.ledger, node.shell, id)? =>
        {
            return Ok(Outcome::Done(format!("closed pane {id}")));
        }
        crate::launch::PaneLaunch::Final | crate::launch::PaneLaunch::Unresolved => {
            return Err(Error::Invalid(format!(
                "refuse {id}: unresolved launch ownership"
            )));
        }
        _ => (),
    }
    let agents = crate::probe::agent_list(node.shell)?;
    let idle = if agents.iter().any(|a| a.pane == id) {
        crate::done::finished(node.ledger, &agents)?.contains(id)
    } else {
        crate::probe::foreground(node.shell, id)?.is_none()
    };
    if !idle {
        return Ok(Outcome::Skipped(format!(
            "refuse {id}: pane is busy, or its agent has not called done"
        )));
    }
    node.shell
        .run_ok("herdr", &["pane".into(), "close".into(), id.into()])?;
    Ok(Outcome::Done(format!("closed pane {id}")))
}

pub fn render(plan: &Plan) -> String {
    use std::fmt::Write as _;
    if plan.entries.is_empty() {
        return "No cleanup candidates.\n".into();
    }
    let mut output = String::new();
    for e in &plan.entries {
        let class = match (&e.kind, &e.tree) {
            (Some(kind), Some(tree)) => format!("{} {kind} repo={}", e.class, tree.repo),
            _ => e.class.clone(),
        };
        let _ = writeln!(
            output,
            "{} {}: {} [{}]",
            class,
            e.id,
            e.detail,
            if e.removable {
                "remove"
            } else {
                "report/refuse"
            }
        );
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::Fake;
    use serde_json::json;

    fn planned_branch() -> Tree {
        Tree {
            repo: "/r".into(),
            path: String::new(),
            branch: "feature".into(),
            head: "abc".into(),
            identity: None,
        }
    }

    #[test]
    fn main_checkout_cache_entry_needs_no_linked_administrative_identity() {
        let rig = Rig::new(json!([]));
        let repo = rig.ledger.dir.join("repo");
        let target = repo.join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(
            target.join("CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n",
        )
        .unwrap();
        let mut candidate = entry(
            "targets",
            target.display().to_string(),
            "unused cache".into(),
            true,
        );
        candidate.tree = Some(Tree {
            repo: repo.display().to_string(),
            path: repo.display().to_string(),
            branch: "main".into(),
            head: "abc".into(),
            identity: None,
        });
        let plan = Plan {
            entries: vec![candidate.clone()],
        };
        assert!(matches!(
            apply_one(&rig.node(), &plan, &candidate, 100).unwrap(),
            Outcome::Done(_)
        ));
        assert!(!target.exists());
        assert!(repo.exists());
    }

    #[test]
    fn branch_tip_moved_after_the_plan_is_a_skip() {
        let shell = Fake::new();
        shell.on("git -C /r rev-parse refs/heads/feature", "def\n");
        let outcome = delete_branch(&shell, &planned_branch()).unwrap();
        assert_eq!(
            outcome,
            Outcome::Skipped("refuse feature: HEAD moved since the survey (abc -> def)".into())
        );
    }

    #[test]
    fn branch_missing_after_the_plan_is_an_identity_failure() {
        let shell = Fake::new();
        shell.on_output(
            "git -C /r rev-parse refs/heads/feature",
            crate::shell::Output {
                status: 128,
                stdout: String::new(),
                stderr: "unknown revision".into(),
            },
        );
        let error = delete_branch(&shell, &planned_branch()).unwrap_err();
        assert!(error.to_string().contains("branch identity changed"));
    }

    struct Rig {
        shell: Fake,
        ledger: Ledger,
        cleanup: crate::config::Cleanup,
    }

    impl Rig {
        fn new(workspaces: serde_json::Value) -> Self {
            let mut response = json!({"result":{}});
            response["result"]["workspaces"] = workspaces;
            let rig = Self {
                shell: Fake::new(),
                ledger: Ledger::new(std::env::temp_dir().join(crate::ledger::new_id())),
                cleanup: crate::config::Cleanup::default(),
            };
            rig.shell
                .on("herdr workspace list", &response.to_string())
                .on("herdr agent list", r#"{"result":{"agents":[]}}"#);
            rig.panes(&[("wVR:p1", "wVR", true)]);
            rig
        }

        fn node(&self) -> Node<'_> {
            Node {
                shell: &self.shell,
                ledger: &self.ledger,
                thresholds: crate::config::Thresholds::from(&crate::config::Policy::default()),
                home: "/",
                cleanup: &self.cleanup,
            }
        }

        fn panes(&self, panes: &[(&str, &str, bool)]) {
            let panes: Vec<_> = panes.iter().map(|(id, workspace, idle)| {
                self.shell.on(
                    &format!("herdr pane process-info --pane {id}"),
                    // An idle shell is its own foreground process. A busy pane runs another
                    // program in front of the shell.
                    if *idle { r#"{"result":{"process_info":{"shell_pid":1,"foreground_processes":[{"pid":1,"name":"zsh","argv":["zsh"]}]}}}"# }
                    else { r#"{"result":{"process_info":{"shell_pid":1,"foreground_processes":[{"pid":2,"name":"pi","argv":["pi"]}]}}}"# },
                );
                json!({"pane_id":id,"workspace_id":workspace,"idle_shell":idle,"idle_since_ms":1})
            }).collect();
            self.shell.on(
                "herdr pane list",
                &json!({"result":{"panes":panes}}).to_string(),
            );
            let snapshot = Snapshot {
                sampled_ms: 2_000_000,
                probe: crate::probe::Probe {
                    host: "local".into(),
                    label: "local".into(),
                    load1: 0.0,
                    cores: 1,
                    cpu_ticks: None,
                    busy: None,
                    memory: crate::probe::Memory::default(),
                    disk_free_percent: 90.0,
                    agents: crate::probe::AgentCounts::default(),
                    agent_panes: vec![],
                    reservations: vec![],
                    panes: serde_json::from_value(json!(panes)).unwrap(),
                },
                jobs: vec![],
            };
            self.ledger.write("host.json", &snapshot).unwrap();
        }

        fn plan(&self) -> Plan {
            let node = self.node();
            let seen = Seen::load(&node, None, BTreeMap::new()).unwrap();
            let mut plan = Plan::default();
            idle(&node, 2_000_000, &seen, &Closing::default(), &mut plan).unwrap();
            plan
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.ledger.dir);
        }
    }

    fn workspace(id: &str, linked: bool) -> serde_json::Value {
        json!({"workspace_id":id,"label":"jalgpall","worktree":{
            "repo_root":"/repo","repo_key":"/repo/.git","is_linked_worktree":linked
        }})
    }

    #[test]
    fn missing_remembered_repository_is_removable_but_configuration_is_not() {
        let mut rig = Rig::new(json!([]));
        let missing = rig.ledger.dir.join("missing").display().to_string();
        remember(
            &rig.ledger,
            Tree {
                repo: missing.clone(),
                path: format!("{missing}/tree"),
                branch: "feature".into(),
                head: String::new(),
                identity: None,
            },
        )
        .unwrap();
        rig.cleanup.repositories.push(missing.clone());
        let mut plan = Plan::default();
        let repos = repositories(&rig.node(), &[], &[], &mut plan).unwrap();
        assert!(!repos.contains(&missing));
        let stale = plan
            .entries
            .iter()
            .find(|e| e.class == "repositories" && e.id == missing && e.removable)
            .unwrap();
        assert!(plan.entries.iter().any(|e| e.class == "repositories"
            && e.id == missing
            && !e.removable
            && e.detail.contains("cleanup.repositories")));
        // Surveying must not change registrations; only applying the candidate may do so.
        assert_eq!(
            rig.ledger
                .read::<BTreeSet<String>>("repositories.json")
                .unwrap()
                .unwrap(),
            BTreeSet::from([missing.clone()])
        );
        apply_one(&rig.node(), &plan, stale, 100).unwrap();
        assert!(
            rig.ledger
                .read::<BTreeSet<String>>("repositories.json")
                .unwrap()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            rig.ledger
                .read::<Vec<Tree>>("worktrees.json")
                .unwrap()
                .unwrap(),
            [] as [Tree; 0]
        );
        let mut next = Plan::default();
        repositories(&rig.node(), &[], &[], &mut next).unwrap();
        let configured: Vec<_> = next.entries.iter().filter(|e| e.id == missing).collect();
        assert_eq!(configured.len(), 1);
        assert!(!configured[0].removable);
        assert_eq!(rig.cleanup.repositories, [missing]);
    }

    #[test]
    fn sweep_forgets_missing_registrations_but_keeps_configured_and_discovered_warnings() {
        let mut rig = Rig::new(json!([]));
        rig.panes(&[]);
        let repo = rig
            .ledger
            .dir
            .join("deleted-fixture/repo")
            .display()
            .to_string();
        let tree = Tree {
            repo: repo.clone(),
            path: format!("{repo}/tree"),
            branch: "feature".into(),
            head: String::new(),
            identity: None,
        };
        remember(&rig.ledger, tree).unwrap();
        rig.cleanup.repositories.push(repo.clone());
        rig.shell
            .on("df -Pk /", "FS 1000 500 400 60% /\n")
            .on("ps -axo pid=,ppid=,pcpu=,comm=", "")
            .on_output(
                &format!("git -C {repo} rev-parse --path-format=absolute --git-common-dir"),
                crate::shell::Output {
                    status: 128,
                    stdout: String::new(),
                    stderr: "No such file or directory".into(),
                },
            );
        rig.shell.on(
            "herdr workspace list",
            &json!({"result":{"workspaces":[{"workspace_id":"external","worktree":{
                "repo_root":repo,"repo_key":"external","is_linked_worktree":false
            }}]}})
            .to_string(),
        );
        let planned = doctor(&rig.node(), 100).unwrap();
        assert!(planned.entries.iter().any(|e| e.id == repo && e.removable));
        assert_eq!(
            rig.ledger
                .read::<Vec<Tree>>("worktrees.json")
                .unwrap()
                .unwrap()
                .len(),
            1
        );
        let applied = sweep(&rig.node(), 100).unwrap();
        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        assert!(applied.results.iter().any(|result| result.contains(&repo)));
        let warnings: Vec<_> = applied
            .repositories
            .iter()
            .filter(|e| e.id == repo)
            .collect();
        assert_eq!(warnings.len(), 2);
        assert!(warnings.iter().all(|e| !e.removable));
        assert!(
            warnings
                .iter()
                .any(|e| e.detail.contains("cleanup.repositories"))
        );
        assert_eq!(
            rig.ledger
                .read::<Vec<Tree>>("worktrees.json")
                .unwrap()
                .unwrap(),
            [] as [Tree; 0]
        );
        assert!(
            rig.ledger
                .read::<BTreeSet<String>>("repositories.json")
                .unwrap()
                .unwrap()
                .is_empty()
        );
        assert_eq!(rig.cleanup.repositories, [repo]);
    }

    #[test]
    fn stale_registry_removal_rechecks_absence_and_keeps_other_records() {
        let rig = Rig::new(json!([]));
        let repo = rig.ledger.dir.join("gone").display().to_string();
        let surviving = rig.ledger.dir.join("surviving").display().to_string();
        std::fs::create_dir_all(&surviving).unwrap();
        let live_tree = Tree {
            repo: surviving.clone(),
            path: surviving.clone(),
            branch: "main".into(),
            head: String::new(),
            identity: None,
        };
        remember(&rig.ledger, live_tree.clone()).unwrap();
        remember(
            &rig.ledger,
            Tree {
                repo: repo.clone(),
                path: format!("{repo}/tree"),
                branch: "feature".into(),
                head: String::new(),
                identity: None,
            },
        )
        .unwrap();
        let mut plan = Plan::default();
        repositories(&rig.node(), &[], &[], &mut plan).unwrap();
        let stale = plan.entries.iter().find(|e| e.id == repo).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        assert!(apply_one(&rig.node(), &plan, stale, 100).is_err());
        assert_eq!(
            rig.ledger
                .read::<Vec<Tree>>("worktrees.json")
                .unwrap()
                .unwrap()
                .len(),
            2
        );
        std::fs::remove_dir(&repo).unwrap();
        apply_one(&rig.node(), &plan, stale, 100).unwrap();
        assert_eq!(
            rig.ledger
                .read::<BTreeSet<String>>("repositories.json")
                .unwrap()
                .unwrap(),
            BTreeSet::from([surviving])
        );
        assert_eq!(
            rig.ledger
                .read::<Vec<Tree>>("worktrees.json")
                .unwrap()
                .unwrap(),
            [live_tree]
        );
    }

    #[test]
    fn missing_repository_does_not_forget_an_existing_worktree() {
        let rig = Rig::new(json!([]));
        let path = rig.ledger.dir.join("surviving-tree");
        std::fs::create_dir_all(&path).unwrap();
        let tree = Tree {
            repo: rig.ledger.dir.join("gone-repo").display().to_string(),
            path: path.display().to_string(),
            branch: "feature".into(),
            head: String::new(),
            identity: None,
        };
        remember(&rig.ledger, tree.clone()).unwrap();
        drop_repository(&rig.node(), &tree.repo).unwrap();
        assert!(path.is_dir());
        assert_eq!(
            rig.ledger
                .read::<Vec<Tree>>("worktrees.json")
                .unwrap()
                .unwrap(),
            [tree]
        );
    }

    #[test]
    fn unreadable_and_existing_repository_paths_are_report_only() {
        let rig = Rig::new(json!([]));
        let directory = rig.ledger.dir.join("directory");
        std::fs::create_dir_all(&directory).unwrap();
        let file = rig.ledger.dir.join("file");
        std::fs::write(&file, "not a checkout").unwrap();
        let symlink = rig.ledger.dir.join("dangling");
        std::os::unix::fs::symlink(rig.ledger.dir.join("absent"), &symlink).unwrap();
        // ENOTDIR is not evidence that a registration is safe to forget.
        let paths: BTreeSet<String> = [directory, file.join("child"), file, symlink]
            .into_iter()
            .map(|path| path.display().to_string())
            .collect();
        rig.ledger.write("repositories.json", &paths).unwrap();
        let mut plan = Plan::default();
        let repos = repositories(&rig.node(), &[], &[], &mut plan).unwrap();
        let resolved: BTreeSet<_> = paths
            .iter()
            .map(|path| {
                std::fs::canonicalize(path)
                    .map_or_else(|_| path.clone(), |p| p.display().to_string())
            })
            .collect();
        assert!(resolved.is_subset(&repos));
        readable(&rig.node(), repos, &mut plan);
        for path in &resolved {
            assert!(plan.entries.iter().any(|e| e.id == *path && !e.removable));
            assert!(!plan.entries.iter().any(|e| e.id == *path && e.removable));
        }
        assert_eq!(
            rig.ledger
                .read::<BTreeSet<String>>("repositories.json")
                .unwrap()
                .unwrap(),
            paths
        );
    }

    #[test]
    fn unreadable_parent_keeps_repository_and_worktree_registrations() {
        use std::os::unix::fs::PermissionsExt as _;

        let rig = Rig::new(json!([]));
        let parent = rig.ledger.dir.join("unreadable");
        std::fs::create_dir_all(&parent).unwrap();
        let tree = Tree {
            repo: parent.join("repo").display().to_string(),
            path: parent.join("tree").display().to_string(),
            branch: "feature".into(),
            head: String::new(),
            identity: None,
        };
        remember(&rig.ledger, tree.clone()).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o000)).unwrap();
        let denied = std::fs::symlink_metadata(&tree.repo);
        let mut plan = Plan::default();
        let result = repositories(&rig.node(), &[], &[], &mut plan);
        // Restore before assertions so the fixture can always be removed.
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            denied.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(result.unwrap().contains(&tree.repo));
        assert!(plan.entries.iter().all(|e| !e.removable));
        assert_eq!(
            rig.ledger
                .read::<Vec<Tree>>("worktrees.json")
                .unwrap()
                .unwrap(),
            [tree]
        );
    }

    #[test]
    fn missing_worktree_registration_is_rechecked_without_removing_existing_paths() {
        let rig = Rig::new(json!([]));
        let repo = rig.ledger.dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let tree = Tree {
            repo: repo.display().to_string(),
            path: repo.join("tree").display().to_string(),
            branch: "feature".into(),
            head: String::new(),
            identity: None,
        };
        remember(&rig.ledger, tree.clone()).unwrap();
        let mut plan = Plan::default();
        repositories(&rig.node(), &[], &[], &mut plan).unwrap();
        let stale = plan.entries.iter().find(|e| e.id == tree.path).unwrap();
        assert!(stale.removable);
        std::fs::create_dir(&tree.path).unwrap();
        assert!(apply_one(&rig.node(), &plan, stale, 100).is_err());
        assert_eq!(
            rig.ledger
                .read::<Vec<Tree>>("worktrees.json")
                .unwrap()
                .unwrap(),
            std::slice::from_ref(&tree)
        );
        std::fs::remove_dir(&tree.path).unwrap();
        apply_one(&rig.node(), &plan, stale, 100).unwrap();
        assert_eq!(
            rig.ledger
                .read::<Vec<Tree>>("worktrees.json")
                .unwrap()
                .unwrap(),
            [] as [Tree; 0]
        );
        assert!(repo.is_dir());
    }

    #[test]
    fn planned_worktree_removed_by_another_actor_is_reconciled_not_failed() {
        let rig = Rig::new(json!([]));
        let repo = rig.ledger.dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo = std::fs::canonicalize(repo).unwrap().display().to_string();
        let tree = Tree {
            repo: repo.clone(),
            path: format!("{repo}/.worktrees/tiers-case"),
            branch: "tiers-case".into(),
            head: "abc".into(),
            identity: Some(crate::worktree::Identity {
                common: Path::new(&repo).join(".git"),
                gitdir: Path::new(&repo).join(".git/worktrees/tiers-case"),
                generation: "0123456789abcdef0123456789abcdef".into(),
            }),
        };
        remember(&rig.ledger, tree.clone()).unwrap();
        // The survey planned the removal; then the worktree left Git's list before apply.
        std::fs::create_dir(Path::new(&repo).join(".git")).unwrap();
        rig.shell.on(
            &format!("git -C {repo} rev-parse --path-format=absolute --git-common-dir"),
            &format!("{repo}/.git\n"),
        );
        rig.shell.on(
            &format!("git -C {repo} worktree list --porcelain"),
            &format!("worktree {repo}\n\n"),
        );
        rig.shell.on(
            &format!("git -C {repo} worktree list --porcelain -z"),
            &format!("worktree {repo}\0HEAD abc\0branch refs/heads/main\0\0"),
        );
        let mut planned = entry("worktrees", tree.path.clone(), "no holds".into(), true);
        planned.tree = Some(tree.clone());
        let plan = Plan {
            entries: vec![planned.clone()],
        };
        let registered = || {
            rig.ledger
                .read::<Vec<Tree>>("worktrees.json")
                .unwrap()
                .unwrap()
        };

        // Unlisted but still on disk: it may hold content, so the refusal stays.
        std::fs::create_dir_all(&tree.path).unwrap();
        let refused = apply_one(&rig.node(), &plan, &planned, 100).unwrap_err();
        assert!(
            refused
                .to_string()
                .contains("worktree unregistered by git; directory remains"),
            "{refused}"
        );
        assert_eq!(registered(), std::slice::from_ref(&tree));
        assert!(Path::new(&tree.path).is_dir());

        // Gone from Git and disk: the pass succeeds and forgets the record.
        std::fs::remove_dir(&tree.path).unwrap();
        let Outcome::Done(result) = apply_one(&rig.node(), &plan, &planned, 100).unwrap() else {
            panic!("a vanished worktree is not a skip");
        };
        assert!(result.contains("already removed"), "{result}");
        assert_eq!(registered(), [] as [Tree; 0]);
        assert!(Path::new(&repo).is_dir());
    }

    #[test]
    fn primary_with_open_linked_workspace_is_kept_with_its_idle_pane() {
        let rig = Rig::new(json!([workspace("wVR", false), workspace("wVS", true)]));
        rig.panes(&[("wVR:p1", "wVR", true), ("wVS:p1", "wVS", false)]);
        let plan = rig.plan();
        assert_eq!(plan.entries.len(), 1);
        let primary = &plan.entries[0];
        assert_eq!(
            (&*primary.class, &*primary.id, primary.removable),
            ("workspaces", "wVR", false)
        );
        let rendered = render(&plan);
        assert!(
            rendered.contains("closing its pane would close linked workspaces wVS"),
            "{rendered}"
        );
        assert!(rendered.contains("closing its pane would close linked workspaces"));
        assert!(rendered.contains("[report/refuse]"));
        let calls = rig.shell.calls.lock().unwrap();
        assert_eq!(
            calls
                .iter()
                .filter(|c| *c == "herdr workspace list")
                .count(),
            1
        );
        assert!(
            !calls
                .iter()
                .any(|c| c.starts_with("herdr worktree ") || c.contains(" close "))
        );
    }

    fn gone_workspace_rig() -> Rig {
        let rig = Rig::new(json!([workspace("wVR", false)]));
        rig.shell.on_output(
            "herdr workspace close wVR",
            crate::shell::Output {
                status: 1,
                stdout: String::new(),
                stderr: r#"{"error":{"code":"workspace_not_found","message":"workspace wVR not found"}}"#.into(),
            },
        );
        rig
    }

    #[test]
    fn closing_a_workspace_that_is_already_gone_counts_as_closed() {
        let rig = gone_workspace_rig();
        let outcome = close_workspace(&rig.node(), "wVR", 2_000_000).unwrap();
        assert!(
            matches!(&outcome, Outcome::Done(m) if m.contains("already closed")),
            "{outcome:?}"
        );
    }

    #[test]
    fn another_workspace_close_failure_still_fails() {
        let rig = gone_workspace_rig();
        rig.shell.on_output(
            "herdr workspace close wVR",
            crate::shell::Output {
                status: 1,
                stdout: String::new(),
                stderr: r#"{"error":{"code":"server_unavailable","message":"down"}}"#.into(),
            },
        );
        assert!(close_workspace(&rig.node(), "wVR", 2_000_000).is_err());
    }

    #[test]
    fn primary_without_open_group_is_still_removable() {
        let rig = Rig::new(json!([workspace("wVR", false)]));
        let plan = rig.plan();
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].id, "wVR");
        assert!(plan.entries[0].removable);
    }

    #[test]
    fn linked_workspace_closed_later_in_a_pass_releases_primary_on_next_survey() {
        // wVR failed before the same sweep closed wVS. A checkout remaining on disk
        // without an open workspace does not hold the group on the next survey.
        let rig = Rig::new(json!([workspace("wVR", false), workspace("wVS", true)]));
        rig.panes(&[("wVR:p1", "wVR", true), ("wVS:p1", "wVS", true)]);
        let first = rig.plan();
        assert!(
            !first
                .entries
                .iter()
                .find(|e| e.id == "wVR")
                .unwrap()
                .removable
        );
        assert!(
            first
                .entries
                .iter()
                .find(|e| e.id == "wVS")
                .unwrap()
                .removable
        );
        rig.shell.on(
            "herdr workspace list",
            &json!({"result":{"workspaces":[workspace("wVR", false)]}}).to_string(),
        );
        rig.panes(&[("wVR:p1", "wVR", true)]);
        assert!(rig.plan().entries[0].removable);
    }

    #[test]
    fn duplicate_primary_can_close_without_closing_the_group() {
        let rig = Rig::new(json!([
            workspace("wVR", false),
            workspace("wVS", true),
            workspace("wOther", false)
        ]));
        assert!(rig.plan().entries[0].removable);
    }

    #[test]
    fn group_added_after_plan_is_rechecked_before_workspace_or_last_pane_close() {
        let rig = Rig::new(json!([workspace("wVR", false)]));
        let plan = rig.plan();
        rig.shell.on(
            "herdr workspace list",
            &json!({"result":{"workspaces":[workspace("wVR", false), workspace("wVS", true)]}})
                .to_string(),
        );
        // A hold that appeared after the plan is a skip: the entry stays, and it is no failure.
        let skipped = |outcome: Result<Outcome>| match outcome.unwrap() {
            Outcome::Skipped(reason) => reason,
            Outcome::Done(result) => panic!("not skipped: {result}"),
        };
        assert!(
            skipped(apply_one(&rig.node(), &plan, &plan.entries[0], 2_000_000))
                .contains("closing its pane would close linked workspaces")
        );
        let panes = Plan {
            entries: vec![entry("panes", "wVR:p1".into(), String::new(), true)],
        };
        assert!(
            skipped(apply_one(&rig.node(), &panes, &panes.entries[0], 2_000_000))
                .contains("closing its pane would close linked workspaces")
        );
        assert!(
            !rig.shell
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.contains(" close "))
        );
    }

    #[test]
    fn grouped_primary_can_still_lose_an_idle_pane_when_another_pane_is_busy() {
        let rig = Rig::new(json!([workspace("wVR", false), workspace("wVS", true)]));
        rig.panes(&[("wVR:p1", "wVR", true), ("wVR:p2", "wVR", false)]);
        let plan = rig.plan();
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].class, "panes");
        assert_eq!(plan.entries[0].id, "wVR:p1");
        assert!(plan.entries[0].removable);
    }

    /// Herdr reports the root process of a pane. An idle shell is its own foreground
    /// process; a shell that runs a command is one with a command argument.
    fn root_process(runs_a_command: bool) -> String {
        let process = if runs_a_command {
            r#"{"pid":1,"name":"sh","argv":["sh","-c","sleep 60"]}"#
        } else {
            r#"{"pid":1,"name":"zsh","argv":["zsh"]}"#
        };
        format!(
            r#"{{"result":{{"process_info":{{"shell_pid":1,"foreground_processes":[{process}]}}}}}}"#
        )
    }

    #[test]
    fn a_pane_that_starts_a_command_after_the_plan_is_skipped_not_closed() {
        let rig = Rig::new(json!([]));
        // wVR:p2 runs a program, so the workspace is not idle as a whole and its idle
        // pane wVR:p1 is a removal candidate of its own.
        rig.panes(&[("wVR:p1", "wVR", true), ("wVR:p2", "wVR", false)]);
        rig.shell.on("herdr pane close wVR:p1", r#"{"result":{}}"#);
        let plan = rig.plan();
        let pane = plan
            .entries
            .iter()
            .find(|e| e.class == "panes")
            .expect("the idle pane is a candidate");
        assert_eq!(pane.id, "wVR:p1");
        assert!(pane.removable, "{}", pane.detail);
        // The pane's root process starts a command after the survey.
        rig.shell
            .on("herdr pane process-info --pane wVR:p1", &root_process(true));
        let outcome = apply_one(&rig.node(), &plan, pane, 2_000_000).unwrap();
        match outcome {
            Outcome::Skipped(reason) => {
                assert!(reason.contains("no longer idle"), "{reason}");
            }
            Outcome::Done(result) => panic!("not skipped: {result}"),
        }
        assert!(
            !rig.shell
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.contains("pane close wVR:p1")),
            "the pane was closed"
        );
    }

    #[test]
    fn an_idle_shell_after_the_plan_is_still_closed() {
        let rig = Rig::new(json!([]));
        rig.panes(&[("wVR:p1", "wVR", true), ("wVR:p2", "wVR", false)]);
        rig.shell.on("herdr pane close wVR:p1", r#"{"result":{}}"#);
        let plan = rig.plan();
        let pane = plan
            .entries
            .iter()
            .find(|e| e.class == "panes")
            .expect("the idle pane is a candidate");
        let outcome = apply_one(&rig.node(), &plan, pane, 2_000_000).unwrap();
        assert!(
            matches!(&outcome, Outcome::Done(result) if result.contains("closed pane wVR:p1")),
            "{outcome:?}"
        );
        assert!(
            rig.shell
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.contains("pane close wVR:p1"))
        );
    }
}
