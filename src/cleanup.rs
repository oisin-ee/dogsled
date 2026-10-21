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
    cwds: Option<Vec<Proc>>,
    evidence: BTreeMap<String, crate::loss::Evidence>,
    /// Agent panes that called `done` and ended that turn. Any other idle agent waits for input.
    done: BTreeSet<String>,
}

/// The agent panes on this host that are done (`crate::done`).
fn done_panes(shell: &dyn Shell, ledger: &Ledger) -> Result<BTreeSet<String>> {
    crate::done::finished(ledger, &crate::probe::agent_list(shell)?)
}

pub fn remember(ledger: &Ledger, tree: Tree) -> Result<()> {
    ledger.with_lock("repositories", || {
        let mut repos: BTreeSet<String> = ledger.read("repositories.json")?.unwrap_or_default();
        repos.insert(tree.repo.clone());
        ledger.write("repositories.json", &repos)
    })?;
    ledger.with_lock("worktrees", || {
        let mut trees: Vec<Tree> = ledger.read("worktrees.json")?.unwrap_or_default();
        trees.retain(|t| t.path != tree.path);
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

/// Registered and configured repositories, and those of live panes, Herdr workspaces, and the
/// caller. Doctor does not scan home.
fn repositories(
    node: &Node<'_>,
    panes: &[Pane],
    workspaces: &[Workspace],
    plan: &mut Plan,
) -> Result<BTreeSet<String>> {
    let recorded: Vec<Tree> = node.ledger.read("worktrees.json")?.unwrap_or_default();
    let mut repos: BTreeSet<String> = node.ledger.read("repositories.json")?.unwrap_or_default();
    repos.extend(recorded.iter().map(|t| t.repo.clone()));
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

/// Every worktree of `repo`, the main checkout first. A detached worktree has no branch.
fn listed(shell: &dyn Shell, repo: &str) -> Result<Vec<Tree>> {
    let mut result = Vec::new();
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
    };
    for field in output.stdout.split('\0') {
        if let Some(value) = field.strip_prefix("worktree ") {
            tree.path = value.into();
        } else if let Some(value) = field.strip_prefix("HEAD ") {
            tree.head = value.into();
        } else if let Some(value) = field.strip_prefix("branch refs/heads/") {
            tree.branch = value.into();
        } else if field.is_empty() && !tree.path.is_empty() {
            result.push(tree.clone());
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
    let out = shell.run_ok("herdr", &["pane".into(), "list".into()])?;
    let value: serde_json::Value = serde_json::from_str(&out.stdout)?;
    Ok(serde_json::from_value(
        value
            .pointer("/result/panes")
            .cloned()
            .ok_or_else(|| Error::Invalid("pane list has no panes".into()))?,
    )?)
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
    for repo in repos {
        match repository_trees(node.shell, &repo) {
            Ok(found) => {
                trees.extend(
                    found
                        .into_iter()
                        .filter(|tree| Path::new(&tree.path).exists()),
                );
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
    let panes = current_panes(node.shell)?;
    let workspaces = workspaces(node.shell)?;
    let mut plan = Plan::default();
    let repos = repositories(node, &panes, &workspaces, &mut plan)?;
    let (repos, trees, evidence) = readable(node, repos, &mut plan);
    let seen = Seen {
        panes,
        cwds,
        evidence,
        done: done_panes(node.shell, node.ledger)?,
    };
    let mut closing = Closing::default();
    let found = parallel(&trees, |tree| disposition(node, tree, &seen));
    let mut removed = BTreeSet::new();
    for (tree, found) in trees.iter().zip(found) {
        let found = match found {
            Ok(found) => found,
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
        if let Err(error) = branches(node, repo, &seen.evidence[repo], &mut plan) {
            plan.entries.push(entry(
                "repositories",
                repo.clone(),
                format!("cannot inspect branches: {error}"),
                false,
            ));
        }
    }
    idle(node, now, &seen.panes, &workspaces, &closing, &mut plan)?;
    if scope == Scope::Sweep {
        return Ok(plan);
    }
    targets(node, &repos, &trees, &removed, &seen, &mut plan)?;
    prune_only(node, &seen, now, &mut plan)?;
    Ok(plan)
}

/// The classes the sweep never applies: expired jobs, stale registrations, disk, and processes.
fn prune_only(node: &Node<'_>, seen: &Seen, now: u64, plan: &mut Plan) -> Result<()> {
    for job in crate::jobs::list(node.ledger)? {
        if job.ended_ms.is_some_and(|t| now.saturating_sub(t) > DAY) {
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
    for tree in &owned {
        if !Path::new(&tree.path).exists() {
            plan.entries.push(entry(
                "worktrees",
                tree.path.clone(),
                "missing registered worktree; drops the stale registration".into(),
                true,
            ));
        }
    }
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
    plan: &mut Plan,
) -> Result<()> {
    let shell = node.shell;
    let pending = crate::prepare::pending_trees(node.ledger)?;
    let checked_out: BTreeSet<String> =
        listed(shell, repo)?.into_iter().map(|t| t.branch).collect();
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
        let held = pending
            .iter()
            .any(|tree| crate::worktree::same(&tree.repo, repo) && tree.branch == branch);
        let detail = format!(
            "{}{}",
            found.detail(),
            if held {
                "; about to be used by a pending preparation"
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
        });
        plan.entries.push(e);
    }
    Ok(())
}

/// The single shell rule for prune and sweep. Launch ownership is checked before other
/// reasons so a replaced terminal or an unresolved successful start cannot fall through.
fn idle_shell(
    node: &Node<'_>,
    pane: &Pane,
    since: Option<u64>,
    jobs: &[crate::jobs::Job],
    items: &[crate::items::Item],
    now: u64,
) -> Result<Option<String>> {
    if !matches!(
        crate::probe::foreground(node.shell, &pane.pane_id),
        Ok(None)
    ) {
        return Ok(None);
    }
    let launch = crate::launch::final_pane(node.ledger, node.shell, &pane.pane_id)?;
    match launch {
        crate::launch::PaneLaunch::Final => {
            return Ok(Some("final launch; evidence retained".into()));
        }
        crate::launch::PaneLaunch::Unresolved => return Ok(None),
        _ => (),
    }
    if let Some(job) = jobs
        .iter()
        .find(|j| j.pane == pane.pane_id && j.ended_ms.is_some())
    {
        return Ok(Some(format!(
            "job {} ended{}; its log keeps the output",
            job.id,
            job.exit
                .map_or(String::new(), |code| format!(" with exit {code}"))
        )));
    }
    if pane
        .cwd
        .as_deref()
        .is_some_and(|cwd| !Path::new(cwd).exists())
    {
        return Ok(Some("shell in a deleted directory".into()));
    }
    let own: Vec<_> = items
        .iter()
        .filter(|i| i.host == crate::hosts::LOCAL && i.pane == pane.pane_id)
        .collect();
    if own.iter().any(|i| i.closed_ms.is_none()) {
        return Ok(None);
    }
    if own
        .iter()
        .any(|i| i.kind == "agent" && i.closed_ms.is_some())
    {
        return Ok(Some("agent item closed".into()));
    }
    if launch != crate::launch::PaneLaunch::Unrecorded
        || !own.is_empty()
        || jobs.iter().any(|j| j.pane == pane.pane_id)
    {
        return Ok(None);
    }
    Ok(since
        .filter(|since| now.saturating_sub(*since) >= crate::watchdog::IDLE_SHELL)
        .map(|since| {
            format!(
                "shell idle for {}",
                crate::watchdog::age(now.saturating_sub(since))
            )
        }))
}

/// Both scopes exclude the caller and focused pane. Agent cleanup still requires `done`.
fn idle_panes(node: &Node<'_>, now: u64, panes: &[Pane]) -> Result<BTreeMap<String, String>> {
    let caller = std::env::var("HERDR_PANE_ID").ok();
    let eligible = |pane: &Pane| !pane.focused && caller.as_deref() != Some(pane.pane_id.as_str());
    let mut idle = BTreeMap::new();
    let jobs = crate::jobs::list(node.ledger)?;
    let items = crate::items::list(node.ledger)?;
    let agents = crate::probe::agent_list(node.shell)?;
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
        if let Some(reason) = idle_shell(node, pane, since, &jobs, &items, now)? {
            idle.insert(pane.pane_id.clone(), reason);
        }
    }
    let Some(snapshot) = snapshot else {
        return Ok(idle);
    };
    // An idle agent that has not called `done` waits for input, however long.
    let done = crate::done::finished(node.ledger, &agents)?;
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
fn idle(
    node: &Node<'_>,
    now: u64,
    panes: &[Pane],
    workspaces: &[Workspace],
    closing: &Closing,
    plan: &mut Plan,
) -> Result<()> {
    let idle = idle_panes(node, now, panes)?;
    // Final launches close through recovery, never a whole-workspace close that skips locks.
    let mut launches = BTreeSet::new();
    for pane in panes.iter().filter(|p| idle.contains_key(&p.pane_id)) {
        if crate::launch::final_pane(node.ledger, node.shell, &pane.pane_id)?
            == crate::launch::PaneLaunch::Final
        {
            launches.insert(pane.pane_id.clone());
        }
    }
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
        let label = workspaces
            .iter()
            .find(|w| w.workspace_id == *workspace)
            .map_or("", |w| w.label.as_str());
        let detail = members
            .iter()
            .map(|p| format!("{} {}", p.pane_id, idle[&p.pane_id]))
            .collect::<Vec<_>>()
            .join("; ");
        plan.entries.push(entry(
            "workspaces",
            (*workspace).into(),
            format!("{label}: {detail}"),
            true,
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
        plan.entries
            .push(entry("panes", pane.clone(), detail.clone(), true));
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
enum Decider {
    Cleanup,
    Close { guard: crate::close::Guard },
}

/// Pane ownership holds: focus, unresolved launches, and agents still waiting for input.
fn pane_holds(
    node: &Node<'_>,
    usage: &Usage,
    seen: &Seen,
    decider: Decider,
) -> Result<Vec<String>> {
    let mut held = usage.in_use.clone();
    for pane in &usage.holders {
        if pane.focused && matches!(decider, Decider::Cleanup) {
            held.push(format!("{} is focused", pane.pane_id));
        }
        if pane.agent.is_none() {
            let launch = crate::launch::final_pane(node.ledger, node.shell, &pane.pane_id)?;
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
                .map(|p| format!("{} waits for input; it has not called done", p.pane_id)),
        );
    }
    Ok(held)
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
    let mut held = pane_holds(node, usage, seen, decider)?;
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
    } else if !loss.landed() {
        if matches!(
            decider,
            Decider::Close {
                guard: crate::close::Guard::Reader | crate::close::Guard::Writer
            }
        ) {
            held.push("not landed by its owner".into());
        }
        for item in items
            .iter()
            .filter(|i| seen.panes.iter().any(|p| p.pane_id == i.owner_pane))
        {
            held.push(format!("not landed by its owner {}", item.owner_pane));
        }
    }
    if crate::prepare::pending_trees(node.ledger)?
        .iter()
        .any(|pending| crate::worktree::same(&pending.path, &tree.path))
        || crate::launch::pending_paths(node.ledger, &crate::probe::agent_list(node.shell)?)?
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
        head: String::new(),
    };
    let seen = Seen {
        panes: current_panes(shell)?,
        cwds: process_cwds(shell),
        evidence: BTreeMap::new(),
        done: done_panes(shell, ledger)?,
    };
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

fn validate_tree(node: &Node<'_>, planned: &Tree) -> Result<(Usage, crate::loss::Evidence)> {
    let current = repository_trees(node.shell, &planned.repo)?
        .into_iter()
        .find(|t| t.path == planned.path)
        .ok_or_else(|| {
            Error::Invalid(format!(
                "refuse {}: worktree identity disappeared",
                planned.path
            ))
        })?;
    let repo = crate::prepare::main_checkout(node.shell, &current.path)?;
    if current != *planned
        || crate::worktree::resolved(&repo) != crate::worktree::resolved(Path::new(&planned.repo))
        || current.head.is_empty()
    {
        return Err(Error::Invalid(format!(
            "refuse {}: worktree identity changed",
            planned.path
        )));
    }
    let evidence = crate::loss::Evidence::fetch(node.shell, &current.repo);
    if let Some(warning) = &evidence.warning {
        eprintln!("{warning}");
    }
    let seen = Seen {
        cwds: process_cwds(node.shell),
        panes: current_panes(node.shell)?,
        evidence: BTreeMap::from([(current.repo.clone(), evidence)]),
        done: done_panes(node.shell, node.ledger)?,
    };
    let found = disposition(node, &current, &seen)?;
    if !found.removable {
        return Err(Error::Invalid(format!(
            "refuse {}: {}",
            current.path, found.detail
        )));
    }
    let evidence = seen
        .evidence
        .into_values()
        .next()
        .ok_or_else(|| Error::Invalid("missing loss evidence".into()))?;
    Ok((found.usage, evidence))
}

/// Drop a removed worktree's registration. Its repository stays registered.
pub fn forget(ledger: &Ledger, path: &str) -> Result<()> {
    ledger.with_lock("worktrees", || {
        let mut trees: Vec<Tree> = ledger.read("worktrees.json")?.unwrap_or_default();
        trees.retain(|t| t.path != path);
        ledger.write("worktrees.json", &trees)
    })
}

fn remove_tree(node: &Node<'_>, tree: &Tree) -> Result<String> {
    let (held, evidence) = validate_tree(node, tree)?;
    let linked = crate::worktree::Linked {
        checkout: tree.repo.clone().into(),
        path: tree.path.clone(),
        branch: tree.branch.clone(),
    };
    let mut holders = Vec::new();
    for pane in &held.holders {
        let launch = crate::launch::final_pane(node.ledger, node.shell, &pane.pane_id)?;
        match launch {
            crate::launch::PaneLaunch::Final if pane.agent.is_none() => {
                if !crate::launch::close_unfinished(node.ledger, node.shell, &pane.pane_id)? {
                    return Err(Error::Invalid(format!(
                        "{}: launch changed before removal",
                        pane.pane_id
                    )));
                }
            }
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
        held.workspace.as_deref(),
        &holders,
        std::env::var("HERDR_WORKSPACE_ID").ok().as_deref(),
        &evidence,
    )?;
    let closed = holders
        .iter()
        .map(|p| p.pane_id.as_str())
        .chain(held.workspace.as_deref())
        .collect::<Vec<_>>();
    Ok(format!(
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
    ))
}

fn drop_registration(node: &Node<'_>, path: &str) -> Result<String> {
    node.ledger.with_lock("worktrees", || {
        let mut trees: Vec<Tree> = node.ledger.read("worktrees.json")?.unwrap_or_default();
        if Path::new(path).exists() {
            return Err(Error::Invalid(format!(
                "refuse {path}: the worktree exists again"
            )));
        }
        trees.retain(|t| t.path != path);
        node.ledger.write("worktrees.json", &trees)
    })?;
    Ok(format!("dropped the stale registration of {path}"))
}

fn remove_job(node: &Node<'_>, id: &str, now: u64) -> Result<String> {
    crate::jobs::validate_id(id)?;
    let store = crate::jobs::store(node.ledger);
    store.with_lock(id, || {
        let job = crate::jobs::get(node.ledger, id)?;
        if job.ended_ms.is_none_or(|t| now.saturating_sub(t) <= DAY) {
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
fn delete_branch(shell: &dyn Shell, tree: &Tree) -> Result<String> {
    let reference = format!("refs/heads/{}", tree.branch);
    let evidence = crate::loss::Evidence::fetch(shell, &tree.repo);
    if let Some(warning) = &evidence.warning {
        eprintln!("{warning}");
    }
    let current = git(shell, &tree.repo, &["rev-parse", &reference])?;
    if !current.ok() || current.stdout.trim() != tree.head {
        return Err(Error::Invalid(format!(
            "refuse {}: branch identity changed",
            tree.branch
        )));
    }
    let loss = evidence.assess(shell, &reference, None)?;
    if !loss.loses_nothing() {
        return Err(Error::Invalid(format!(
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
    Ok(format!("deleted branch {} in {}", tree.branch, tree.repo))
}

/// What prune did. Each entry is independent, so one failure does not stop the others.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Applied {
    pub results: Vec<String>,
    pub failures: Vec<String>,
    #[serde(default)]
    pub repositories: Vec<Entry>,
    #[serde(default)]
    pub landed_dirty: Vec<Entry>,
}

impl Applied {
    pub fn render(&self) -> String {
        self.results
            .iter()
            .cloned()
            .chain(self.failures.iter().map(|f| format!("failed: {f}")))
            .map(|line| line + "\n")
            .collect()
    }
}

pub fn apply(node: &Node<'_>, plan: &Plan, now: u64) -> Result<Applied> {
    let mut applied = Applied::default();
    // A remote plan is input, not authority: rebuild the candidate set on this host.
    let actual = doctor(node, now)?;
    for requested in &plan.entries {
        match apply_one(node, &actual, requested, now) {
            Ok(result) => applied.results.push(result),
            Err(e) => applied.failures.push(e.to_string()),
        }
    }
    Ok(applied)
}

/// The classes the sweep applies. Everything else waits for `prune`.
pub const SWEPT: [&str; 4] = ["worktrees", "branches", "workspaces", "panes"];

/// The `serve` sweep: survey in `Sweep` scope and apply each removable entry with the same
/// checks as `prune`. Each entry is independent, so one failure does not stop the others.
pub fn sweep(node: &Node<'_>, now: u64) -> Result<Applied> {
    let plan = survey(node, Scope::Sweep, now)?;
    let mut applied = Applied {
        repositories: plan
            .entries
            .iter()
            .filter(|e| e.class == "repositories")
            .cloned()
            .collect(),
        landed_dirty: plan
            .entries
            .iter()
            .filter(|e| e.kind.as_deref() == Some("landed_dirty"))
            .cloned()
            .collect(),
        ..Applied::default()
    };
    for entry in plan
        .entries
        .iter()
        .filter(|e| e.removable && SWEPT.contains(&e.class.as_str()))
    {
        match apply_one(node, &plan, entry, now) {
            Ok(result) => applied.results.push(result),
            Err(e) => applied.failures.push(e.to_string()),
        }
    }
    Ok(applied)
}

fn apply_one(node: &Node<'_>, actual: &Plan, requested: &Entry, now: u64) -> Result<String> {
    if matches!(requested.class.as_str(), "disk" | "processes") {
        return Ok(format!("{} {}: report only", requested.class, requested.id));
    }
    // A refused entry stays: the plan shows why, and prune removes only eligible entries.
    if !requested.removable {
        return Ok(format!("{} {}: kept", requested.class, requested.id));
    }
    let candidate = actual
        .entries
        .iter()
        .find(|e| e.class == requested.class && e.id == requested.id && e.removable)
        .ok_or_else(|| {
            Error::Invalid(format!(
                "refuse {} {}: no longer safe to remove",
                requested.class, requested.id
            ))
        })?;
    if candidate.tree != requested.tree {
        return Err(Error::Invalid(format!(
            "refuse {}: planned worktree identity changed",
            requested.id
        )));
    }
    match candidate.class.as_str() {
        "worktrees" => match &requested.tree {
            Some(tree) => remove_tree(node, tree),
            None => drop_registration(node, &candidate.id),
        },
        "branches" => match &requested.tree {
            Some(tree) => delete_branch(node.shell, tree),
            None => Err(Error::Invalid("a branch entry has no branch".into())),
        },
        "workspaces" => {
            let panes = current_panes(node.shell)?;
            let idle = idle_panes(node, now, &panes)?;
            for pane in panes
                .iter()
                .filter(|p| p.workspace_id.as_deref() == Some(candidate.id.as_str()))
            {
                if !idle.contains_key(&pane.pane_id)
                    || crate::launch::final_pane(node.ledger, node.shell, &pane.pane_id)?
                        == crate::launch::PaneLaunch::Final
                {
                    return Err(Error::Invalid(format!(
                        "refuse {}: workspace is no longer idle",
                        candidate.id
                    )));
                }
            }
            node.shell.run_ok(
                "herdr",
                &["workspace".into(), "close".into(), candidate.id.clone()],
            )?;
            Ok(format!("closed workspace {}", candidate.id))
        }
        "panes" => {
            let panes = current_panes(node.shell)?;
            if !idle_panes(node, now, &panes)?.contains_key(&candidate.id) {
                return Err(Error::Invalid(format!(
                    "refuse {}: pane is no longer idle",
                    candidate.id
                )));
            }
            if crate::launch::close_unfinished(node.ledger, node.shell, &candidate.id)? {
                return Ok(format!("closed pane {}", candidate.id));
            }
            let agents = crate::probe::agent_list(node.shell)?;
            let idle = if agents.iter().any(|a| a.pane == candidate.id) {
                crate::done::finished(node.ledger, &agents)?.contains(&candidate.id)
            } else {
                crate::probe::foreground(node.shell, &candidate.id)?.is_none()
            };
            if !idle {
                return Err(Error::Invalid(format!(
                    "refuse {}: pane is busy, or its agent has not called done",
                    candidate.id
                )));
            }
            node.shell.run_ok(
                "herdr",
                &["pane".into(), "close".into(), candidate.id.clone()],
            )?;
            Ok(format!("closed pane {}", candidate.id))
        }
        "jobs" => remove_job(node, &candidate.id, now),
        "targets" => remove_target(&candidate.id),
        _ => Err(Error::Invalid("unknown cleanup class".into())),
    }
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
