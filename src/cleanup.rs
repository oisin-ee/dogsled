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

/// The working directory of every process this user can see, resolved, or `None` when the
/// process table cannot be read. Linux reads `/proc/<pid>/cwd`; macOS asks `lsof`. Wrangle's
/// own process is included, so the caller's directory counts as used.
pub fn process_cwds(shell: &dyn Shell) -> Option<Vec<std::path::PathBuf>> {
    let proc = Path::new("/proc");
    if proc.join("self/cwd").exists() {
        let entries = std::fs::read_dir(proc).ok()?;
        return Some(
            entries
                .flatten()
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))
                })
                // Another user's process, or one that already exited, has no readable cwd.
                .filter_map(|e| std::fs::read_link(e.path().join("cwd")).ok())
                .collect(),
        );
    }
    let out = shell
        .run(
            "lsof",
            &["-a".into(), "-d".into(), "cwd".into(), "-Fn".into()],
        )
        .ok()?;
    // lsof exits 1 when some processes are unreadable; its output still lists the rest.
    if out.stdout.is_empty() {
        return None;
    }
    Some(
        out.stdout
            .lines()
            .filter_map(|line| line.strip_prefix('n'))
            .map(|cwd| crate::worktree::resolved(Path::new(cwd)))
            .collect(),
    )
}

/// The first process working directory inside `path`, if any.
fn process_in<'a>(cwds: &'a [std::path::PathBuf], path: &str) -> Option<&'a std::path::PathBuf> {
    let root = crate::worktree::resolved(Path::new(path));
    cwds.iter().find(|cwd| cwd.starts_with(&root))
}

/// What one survey saw of the host, shared by every worktree's finding.
struct Seen {
    scope: Scope,
    panes: Vec<Pane>,
    stale: BTreeSet<String>,
    cwds: Option<Vec<std::path::PathBuf>>,
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
fn main_root(shell: &dyn Shell, path: &str) -> Result<Option<String>> {
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
            main_root(node.shell, &path)?
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
        repos.extend(main_root(node.shell, path)?);
    }
    // One repository reached through a symlink is still one repository.
    Ok(repos
        .into_iter()
        .map(|repo| std::fs::canonicalize(&repo).map_or(repo, |p| p.display().to_string()))
        .collect())
}

fn trees(shell: &dyn Shell, repos: &BTreeSet<String>) -> Result<Vec<Tree>> {
    let mut result = Vec::new();
    for repo in repos {
        result.extend(repository_trees(shell, repo)?);
    }
    Ok(result)
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

fn count(shell: &dyn Shell, repo: &str, args: &[&str]) -> Result<u64> {
    let out = git(shell, repo, &[&["rev-list", "--count"], args].concat())?;
    if !out.ok() {
        return Err(Error::Invalid(format!(
            "{repo}: cannot count commits: {}",
            out.stderr.trim()
        )));
    }
    out.stdout
        .trim()
        .parse()
        .map_err(|_| Error::Invalid(format!("{repo}: invalid commit count")))
}

/// An age in days past two days, where hours stop being readable.
fn since(ms: u64) -> String {
    if ms > 2 * DAY {
        format!("{}d", ms / DAY)
    } else {
        crate::watchdog::age(ms)
    }
}

fn commits(n: u64) -> String {
    format!("{n} commit{}", if n == 1 { "" } else { "s" })
}

/// Milliseconds since the commit at `reference`.
fn last_commit(shell: &dyn Shell, repo: &str, reference: &str, now: u64) -> Result<u64> {
    let out = git(shell, repo, &["log", "-1", "--format=%ct", reference])?;
    let seconds: u64 = out
        .stdout
        .trim()
        .parse()
        .map_err(|_| Error::Invalid(format!("{repo}: no commit time for {reference}")))?;
    Ok(now.saturating_sub(seconds.saturating_mul(1_000)))
}

/// When Git made a linked worktree, and the commit it started on. Git writes the
/// administrative `commondir` file once, and the first entry of the worktree's HEAD reflog
/// records the starting commit.
struct Origin {
    made_ms: u64,
    start: Option<String>,
}

fn origin(path: &str) -> Option<Origin> {
    let dotgit = std::fs::read_to_string(Path::new(path).join(".git")).ok()?;
    let admin = Path::new(path).join(dotgit.trim().strip_prefix("gitdir: ")?);
    let modified = std::fs::metadata(admin.join("commondir"))
        .ok()?
        .modified()
        .ok()?;
    let start = std::fs::read_to_string(admin.join("logs/HEAD"))
        .ok()
        .and_then(|log| log.split_whitespace().nth(1).map(str::to_string));
    Some(Origin {
        made_ms: modified
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis() as u64,
        start,
    })
}

fn clean(shell: &dyn Shell, tree: &Tree) -> Result<bool> {
    let output = git(
        shell,
        &tree.path,
        &["status", "--porcelain", "--untracked-files=all"],
    )?;
    if !output.ok() {
        return Err(Error::Invalid(format!(
            "cannot check worktree {}: {}",
            tree.path,
            output.stderr.trim()
        )));
    }
    Ok(output.stdout.is_empty())
}

fn entry(class: &str, id: String, detail: String, removable: bool) -> Entry {
    Entry {
        class: class.into(),
        id,
        detail,
        removable,
        tree: None,
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
            "{:.1}% used; largest wrangle-owned paths: {largest}",
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

/// The cleanup plan for `scope`. In `Sweep` scope a worktree that cannot be checked is skipped,
/// so one broken entry does not stop the sweep of the others.
pub fn survey(node: &Node<'_>, scope: Scope, now: u64) -> Result<Plan> {
    // Process directories first: wrangle's own Git calls below run with `-C`, not `cd`.
    let cwds = process_cwds(node.shell);
    let panes = current_panes(node.shell)?;
    let workspaces = workspaces(node.shell)?;
    let mut plan = Plan::default();
    let repos = repositories(node, &panes, &workspaces, &mut plan)?;
    let seen = Seen {
        scope,
        stale: match scope {
            Scope::Prune => stale_agents(node, now)?,
            Scope::Sweep => BTreeSet::new(),
        },
        panes,
        cwds,
    };
    let mut closing = Closing::default();
    let trees: Vec<Tree> = trees(node.shell, &repos)?
        .into_iter()
        .filter(|t| Path::new(&t.path).exists())
        .collect();
    let found = parallel(&trees, |tree| disposition(node, tree, &seen, now));
    let mut removed = BTreeSet::new();
    for (tree, found) in trees.iter().zip(found) {
        let found = match (found, scope) {
            (Ok(Some(found)), _) => found,
            (Ok(None), _) => continue,
            (Err(error), Scope::Sweep) => {
                eprintln!("wrangle sweep: skip {}: {error}", tree.path);
                continue;
            }
            (Err(error), Scope::Prune) => return Err(error),
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
        e.tree = Some(tree.clone());
        plan.entries.push(e);
    }
    for repo in &repos {
        match (branches(node.shell, repo, scope, now, &mut plan), scope) {
            (Ok(()), _) => (),
            (Err(error), Scope::Sweep) => {
                eprintln!("wrangle sweep: skip branches in {repo}: {error}");
            }
            (Err(error), Scope::Prune) => return Err(error),
        }
    }
    idle(
        node,
        scope,
        now,
        &seen.panes,
        &workspaces,
        &closing,
        &mut plan,
    )?;
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
    // Only registered worktrees are wrangle-owned disk paths.
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
            if let Some(cwd) = cwds.iter().find(|cwd| own(cwd)) {
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

/// Local branches without a worktree. Deleting one loses nothing when it is integrated or every
/// commit is on a remote branch. The default branch is kept. The sweep takes only branches whose
/// every commit is on a remote ref, and lists nothing else.
fn branches(shell: &dyn Shell, repo: &str, scope: Scope, now: u64, plan: &mut Plan) -> Result<()> {
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
    let found = parallel(&candidates, |&(_, reference, branch)| {
        // The remote check is one cheap call; the integration search is the costly one.
        let unpushed = count(shell, repo, &[reference, "--not", "--remotes"])?;
        Ok(if unpushed == 0 {
            Some(("every commit is on a remote branch".into(), true))
        } else if scope == Scope::Sweep {
            None
        } else if let Some(integrated) = crate::integration::check(shell, repo, branch)? {
            Some((integrated.to_string(), true))
        } else {
            Some((
                format!(
                    "{} on no remote, the last {} ago",
                    commits(unpushed),
                    since(last_commit(shell, repo, reference, now)?)
                ),
                false,
            ))
        })
    });
    for (&(head, _, branch), found) in candidates.iter().zip(found) {
        let Some((detail, removable)) = found? else {
            continue;
        };
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

/// Panes whose closing loses nothing. At once: a shell at an empty prompt in an ended job's tab,
/// whose output the job log keeps, or in a directory that no longer exists. From a fresh
/// service snapshot: agents idle or done for a day, and shells with no foreground command at
/// any sample for a day. The sweep takes only the immediate rules, and never the caller's pane.
fn idle_panes(
    node: &Node<'_>,
    scope: Scope,
    now: u64,
    panes: &[Pane],
) -> Result<BTreeMap<String, String>> {
    let caller = std::env::var("HERDR_PANE_ID").ok();
    let mut idle = BTreeMap::new();
    let ended: BTreeMap<String, crate::jobs::Job> = crate::jobs::list(node.ledger)?
        .into_iter()
        .filter(|job| job.ended_ms.is_some())
        .map(|job| (job.pane.clone(), job))
        .collect();
    for pane in panes.iter().filter(|p| p.agent.is_none()) {
        if scope == Scope::Sweep && caller.as_deref() == Some(pane.pane_id.as_str()) {
            continue;
        }
        let reason = if let Some(job) = ended.get(&pane.pane_id) {
            format!(
                "job {} ended{}; its log keeps the output",
                job.id,
                job.exit
                    .map_or(String::new(), |code| format!(" with exit {code}"))
            )
        } else if pane
            .cwd
            .as_deref()
            .is_some_and(|cwd| !Path::new(cwd).exists())
        {
            "shell in a deleted directory".to_string()
        } else {
            continue;
        };
        // It is still a shell at an empty prompt.
        if matches!(
            crate::probe::foreground(node.shell, &pane.pane_id),
            Ok(None)
        ) {
            idle.insert(pane.pane_id.clone(), reason);
        }
    }
    if scope == Scope::Sweep {
        return Ok(idle);
    }
    let Some(snapshot) = node
        .ledger
        .read::<Snapshot>("host.json")?
        .filter(|s| s.fresh(now))
    else {
        return Ok(idle);
    };
    let agents = crate::probe::agent_list(node.shell)?;
    for agent in snapshot.probe.agent_panes.iter().filter(|a| stale(a, now)) {
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
    for sampled in &snapshot.probe.panes {
        if idle.contains_key(&sampled.pane_id) {
            continue;
        }
        let Some(since) = sampled
            .idle_since_ms
            .filter(|t| now.saturating_sub(*t) > DAY)
        else {
            continue;
        };
        // It is still a shell at an empty prompt.
        if panes
            .iter()
            .any(|p| p.pane_id == sampled.pane_id && p.agent.is_none())
            && matches!(
                crate::probe::foreground(node.shell, &sampled.pane_id),
                Ok(None)
            )
        {
            idle.insert(
                sampled.pane_id.clone(),
                format!(
                    "shell idle for {}",
                    crate::watchdog::age(now.saturating_sub(since))
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
    scope: Scope,
    now: u64,
    panes: &[Pane],
    workspaces: &[Workspace],
    closing: &Closing,
    plan: &mut Plan,
) -> Result<()> {
    let idle = idle_panes(node, scope, now, panes)?;
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
            && members[workspace]
                .iter()
                .all(|p| idle.contains_key(&p.pane_id) && !closing.panes.contains(&p.pane_id))
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

/// The panes that hold `tree`, and which of them are in use: busy, or an agent that is not
/// idle past a day. Removal closes the holders and the workspace.
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
        } else if pane.agent.is_some() && !seen.stale.contains(&pane.pane_id) {
            in_use.push(format!(
                "{} has an agent that is {}; its owner closes it with `wrangle close`",
                pane.pane_id,
                pane.agent_status.as_deref().unwrap_or("quiet")
            ));
        }
    }
    Ok(Usage {
        workspace,
        holders,
        in_use,
    })
}

/// Why the sweep keeps a worktree that `prune` could remove: the sweep takes only a worktree that
/// nothing holds, not even an idle shell, a workspace, or a process working in it.
fn held(tree: &Tree, usage: &Usage, seen: &Seen) -> Vec<String> {
    if seen.scope == Scope::Prune {
        return Vec::new();
    }
    let mut held: Vec<String> = usage
        .holders
        .iter()
        .map(|p| format!("{} holds it", p.pane_id))
        .chain(
            usage
                .workspace
                .iter()
                .map(|w| format!("workspace {w} shows it")),
        )
        .collect();
    match &seen.cwds {
        None => held.push("the process table is unreadable".into()),
        Some(cwds) => {
            if let Some(cwd) = process_in(cwds, &tree.path) {
                held.push(format!("a process works in {}", cwd.display()));
            }
        }
    }
    held
}

/// What removing `tree` itself would lose: uncommitted changes or a foreign lock.
fn blockers(node: &Node<'_>, tree: &Tree) -> Result<Vec<String>> {
    let mut blockers = Vec::new();
    if !clean(node.shell, tree)? {
        blockers.push("uncommitted changes".to_string());
    }
    if let Some(lock) = crate::worktree::lock_reason(node.shell, Path::new(&tree.repo), &tree.path)?
        && lock != crate::worktree::AGENT_LOCK
    {
        blockers.push(format!("locked ({lock})"));
    }
    Ok(blockers)
}

struct Disposition {
    detail: String,
    removable: bool,
    usage: Usage,
}

/// Doctor's finding for a worktree, or `None` when it is ordinary work. Removable: a worktree
/// whose HEAD has not moved a day after Git made it and whose branch has no commits beyond the
/// base, or an integrated branch, when the worktree
/// is clean and no pane uses it. An unused worktree with unmerged commits is reported. The sweep
/// takes only an integrated branch's worktree, and lists nothing else.
fn disposition(node: &Node<'_>, tree: &Tree, seen: &Seen, now: u64) -> Result<Option<Disposition>> {
    let usage = usage(node, tree, seen)?;
    if tree.branch.is_empty() {
        return detached(node, tree, usage, seen, now).map(Some);
    }
    let base = crate::integration::base(node.shell, &tree.repo)?;
    let branch = format!("refs/heads/{}", tree.branch);
    // A worktree whose HEAD never moved, on a branch with no commits beyond the base, holds no
    // work. Its branch is an ancestor of the base once the base moves, but that is not a merge:
    // it is a new worktree, or an abandoned one after a day. A worktree made on a branch that
    // already had commits keeps them, so it goes through the integration check.
    let untouched = match origin(&tree.path) {
        Some(o) if o.start.as_deref() == Some(tree.head.as_str()) => {
            (count(node.shell, &tree.repo, &[&format!("{base}..{branch}")])? == 0).then_some(o)
        }
        _ => None,
    };
    let (finding, eligible) = if let Some(origin) = untouched {
        // The sweep never takes a worktree for having no commits: that is a person's call.
        if seen.scope == Scope::Sweep {
            return Ok(None);
        }
        let made = now.saturating_sub(origin.made_ms);
        if made <= DAY || !usage.in_use.is_empty() {
            return Ok(None);
        }
        (
            format!(
                "branch {} has no commits since its worktree was made {} ago",
                tree.branch,
                crate::watchdog::age(made)
            ),
            true,
        )
    } else if let Some(integrated) =
        crate::integration::check(node.shell, &tree.repo, &tree.branch)?
    {
        (format!("branch {} {integrated}", tree.branch), true)
    } else if !usage.in_use.is_empty() || seen.scope == Scope::Sweep {
        return Ok(None);
    } else {
        let ahead = count(node.shell, &tree.repo, &[&format!("{base}..{branch}")])?;
        let pushed = match count(node.shell, &tree.repo, &[&branch, "--not", "--remotes"])? {
            0 => "all on a remote branch".to_string(),
            unpushed => format!("{unpushed} on no remote"),
        };
        (
            format!(
                "branch {} has {} not on {}, {pushed}, the last {} ago",
                tree.branch,
                commits(ahead),
                crate::integration::short(&base),
                since(last_commit(node.shell, &tree.repo, &branch, now)?)
            ),
            false,
        )
    };
    let refused: Vec<String> = usage
        .in_use
        .iter()
        .cloned()
        .chain(held(tree, &usage, seen))
        .chain(blockers(node, tree)?)
        .collect();
    Ok(Some(Disposition {
        detail: format!(
            "{finding}; {}",
            if refused.is_empty() {
                "clean and unused".into()
            } else {
                format!("refused: {}", refused.join("; "))
            }
        ),
        removable: eligible && refused.is_empty(),
        usage,
    }))
}

/// A detached worktree has no branch to integrate. Removing it loses nothing when it is clean,
/// unheld, and a remote branch holds its HEAD.
fn detached(
    node: &Node<'_>,
    tree: &Tree,
    usage: Usage,
    seen: &Seen,
    now: u64,
) -> Result<Disposition> {
    let head = &tree.head[..tree.head.len().min(10)];
    let unpushed = count(node.shell, &tree.repo, &[&tree.head, "--not", "--remotes"])?;
    let finding = if unpushed == 0 {
        format!("detached at {head}, which a remote branch holds")
    } else {
        format!(
            "detached at {head} with {} on no remote, the last {} ago",
            commits(unpushed),
            since(last_commit(node.shell, &tree.repo, &tree.head, now)?)
        )
    };
    let refused: Vec<String> = usage
        .in_use
        .iter()
        .cloned()
        .chain(held(tree, &usage, seen))
        .chain(blockers(node, tree)?)
        .collect();
    Ok(Disposition {
        detail: format!(
            "{finding}; {}",
            if refused.is_empty() {
                "clean and unused".into()
            } else {
                format!("refused: {}", refused.join("; "))
            }
        ),
        removable: unpushed == 0 && refused.is_empty(),
        usage,
    })
}

/// Agent panes idle or done for more than a day, from a fresh service snapshot.
fn stale_agents(node: &Node<'_>, now: u64) -> Result<BTreeSet<String>> {
    Ok(node
        .ledger
        .read::<Snapshot>("host.json")?
        .filter(|s| s.fresh(now))
        .map(|s| {
            s.probe
                .agent_panes
                .iter()
                .filter(|a| stale(a, now))
                .map(|a| a.pane.clone())
                .collect()
        })
        .unwrap_or_default())
}

fn validate_tree(node: &Node<'_>, planned: &Tree, scope: Scope, now: u64) -> Result<Usage> {
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
    let seen = Seen {
        scope,
        cwds: process_cwds(node.shell),
        panes: current_panes(node.shell)?,
        stale: match scope {
            Scope::Prune => stale_agents(node, now)?,
            Scope::Sweep => BTreeSet::new(),
        },
    };
    let found = disposition(node, &current, &seen, now)?.ok_or_else(|| {
        Error::Invalid(format!(
            "refuse {}: branch {} is not a cleanup candidate",
            current.path, current.branch
        ))
    })?;
    if !found.removable {
        return Err(Error::Invalid(format!(
            "refuse {}: {}",
            current.path, found.detail
        )));
    }
    Ok(found.usage)
}

/// Drop a removed worktree's registration. Its repository stays registered.
pub fn forget(ledger: &Ledger, path: &str) -> Result<()> {
    ledger.with_lock("worktrees", || {
        let mut trees: Vec<Tree> = ledger.read("worktrees.json")?.unwrap_or_default();
        trees.retain(|t| t.path != path);
        ledger.write("worktrees.json", &trees)
    })
}

fn remove_tree(node: &Node<'_>, tree: &Tree, scope: Scope, now: u64) -> Result<String> {
    let held = validate_tree(node, tree, scope, now)?;
    let linked = crate::worktree::Linked {
        checkout: tree.repo.clone().into(),
        path: tree.path.clone(),
        branch: tree.branch.clone(),
    };
    let holders: Vec<&Pane> = held.holders.iter().collect();
    let removed = crate::worktree::remove(
        node.shell,
        node.ledger,
        &linked,
        held.workspace.as_deref(),
        &holders,
        std::env::var("HERDR_WORKSPACE_ID").ok().as_deref(),
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
        match apply_one(node, Scope::Prune, &actual, requested, now) {
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
    let mut applied = Applied::default();
    let plan = survey(node, Scope::Sweep, now)?;
    for entry in plan
        .entries
        .iter()
        .filter(|e| e.removable && SWEPT.contains(&e.class.as_str()))
    {
        match apply_one(node, Scope::Sweep, &plan, entry, now) {
            Ok(result) => applied.results.push(result),
            Err(e) => applied.failures.push(e.to_string()),
        }
    }
    Ok(applied)
}

fn apply_one(
    node: &Node<'_>,
    scope: Scope,
    actual: &Plan,
    requested: &Entry,
    now: u64,
) -> Result<String> {
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
            Some(tree) => remove_tree(node, tree, scope, now),
            None => drop_registration(node, &candidate.id),
        },
        "branches" => match &requested.tree {
            Some(tree) => delete_branch(node.shell, tree),
            None => Err(Error::Invalid("a branch entry has no branch".into())),
        },
        "workspaces" => {
            node.shell.run_ok(
                "herdr",
                &["workspace".into(), "close".into(), candidate.id.clone()],
            )?;
            Ok(format!("closed workspace {}", candidate.id))
        }
        "panes" => {
            let agents = crate::probe::agent_list(node.shell)?;
            let idle = match agents.iter().find(|a| a.pane == candidate.id) {
                Some(agent) => matches!(agent.state.as_str(), "idle" | "done"),
                None => crate::probe::foreground(node.shell, &candidate.id)?.is_none(),
            };
            if !idle {
                return Err(Error::Invalid(format!(
                    "refuse {}: pane is not idle or done",
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
        let _ = writeln!(
            output,
            "{} {}: {} [{}]",
            e.class,
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
