//! Conservative cleanup plans. Every destructive action revalidates local state.

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

fn trees(node: &Node<'_>, panes: &[Pane]) -> Result<Vec<Tree>> {
    let recorded: Vec<Tree> = node.ledger.read("worktrees.json")?.unwrap_or_default();
    let mut repos: BTreeSet<String> = node.ledger.read("repositories.json")?.unwrap_or_default();
    repos.extend(recorded.iter().map(|t| t.repo.clone()));
    let cwd = std::env::current_dir()?;
    // Discover repositories from live pane directories and the caller. Do not scan home.
    for path in panes
        .iter()
        .flat_map(|p| [p.cwd.as_deref(), p.foreground_cwd.as_deref()])
        .flatten()
        .chain(cwd.to_str())
    {
        let root = git(
            node.shell,
            path,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        if root.ok() {
            let root = Path::new(root.stdout.trim());
            if root.file_name().is_some_and(|n| n == ".git")
                && let Some(parent) = root.parent()
            {
                repos.insert(parent.display().to_string());
            }
        }
    }
    // One repository reached through a symlink is still one repository.
    let repos: BTreeSet<String> = repos
        .into_iter()
        .map(|repo| std::fs::canonicalize(&repo).map_or(repo, |p| p.display().to_string()))
        .collect();
    let mut result = Vec::new();
    for repo in repos {
        result.extend(repository_trees(node.shell, &repo)?);
    }
    Ok(result)
}

fn repository_trees(shell: &dyn Shell, repo: &str) -> Result<Vec<Tree>> {
    let mut result = Vec::new();
    let output = git(shell, repo, &["worktree", "list", "--porcelain", "-z"])?;
    if !output.ok() {
        return Err(Error::Invalid(format!(
            "cannot list worktrees in {repo}: {}",
            output.stderr.trim()
        )));
    }
    let mut path = String::new();
    let mut branch = String::new();
    let mut head = String::new();
    // Git lists the main worktree first. It is never a cleanup candidate.
    let mut main = true;
    for field in output.stdout.split('\0') {
        if let Some(value) = field.strip_prefix("worktree ") {
            path = value.into();
        } else if let Some(value) = field.strip_prefix("HEAD ") {
            head = value.into();
        } else if let Some(value) = field.strip_prefix("branch refs/heads/") {
            branch = value.into();
        } else if field.is_empty() && !path.is_empty() {
            if !main && !branch.is_empty() {
                result.push(Tree {
                    repo: repo.into(),
                    path: path.clone(),
                    branch: branch.clone(),
                    head: head.clone(),
                });
            }
            main = false;
            path.clear();
            branch.clear();
            head.clear();
        }
    }
    Ok(result)
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
        let out = shell.run_ok(
            "herdr",
            &[
                "pane".into(),
                "process-info".into(),
                "--pane".into(),
                pane.pane_id.clone(),
            ],
        )?;
        let value: serde_json::Value = serde_json::from_str(&out.stdout)?;
        let info = value
            .pointer("/result/process_info")
            .ok_or_else(|| Error::Invalid("missing pane process information".into()))?;
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
    let panes = current_panes(node.shell)?;
    let trees = trees(node, &panes)?;
    let mut plan = Plan::default();
    let stale_panes = stale_agents(node, now)?;
    for tree in &trees {
        if !Path::new(&tree.path).exists() {
            continue;
        }
        let Some(integrated) = crate::integration::check(node.shell, &tree.repo, &tree.branch)?
        else {
            continue;
        };
        let held = assess(node, tree, &panes, &stale_panes)?;
        let mut e = entry(
            "worktrees",
            tree.path.clone(),
            format!(
                "branch {} {integrated}; {}",
                tree.branch,
                if held.blockers.is_empty() {
                    "clean and unused".into()
                } else {
                    format!("refused: {}", held.blockers.join("; "))
                }
            ),
            held.blockers.is_empty(),
        );
        e.tree = Some(tree.clone());
        plan.entries.push(e);
    }
    if let Some(snapshot) = node
        .ledger
        .read::<Snapshot>("host.json")?
        .filter(|s| s.fresh(now))
    {
        let agents = crate::probe::agent_list(node.shell)?;
        for agent in snapshot.probe.agent_panes.iter().filter(|a| stale(a, now)) {
            if agents.iter().any(|a| {
                a.pane == agent.pane
                    && a.state == agent.state
                    && a.transcript == agent.transcript
                    && a.transcript_modified == agent.transcript_modified
            }) {
                plan.entries.push(entry(
                    "panes",
                    agent.pane.clone(),
                    format!(
                        "{} for {}",
                        agent.state,
                        crate::watchdog::age(
                            now.saturating_sub(agent.state_since_ms.unwrap_or(now))
                        )
                    ),
                    true,
                ));
            }
        }
    }
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
    disk(node, &owned, &mut plan)?;
    processes(node.shell, &panes, &mut plan)?;
    Ok(plan)
}

/// Why removing `tree` would lose something. Empty means it is clean, unlocked or agent-locked,
/// and every pane that holds it is an idle shell or an agent idle past a day.
struct Held {
    workspace: Option<String>,
    holders: Vec<Pane>,
    blockers: Vec<String>,
}

fn assess(node: &Node<'_>, tree: &Tree, panes: &[Pane], stale: &BTreeSet<String>) -> Result<Held> {
    let mut blockers = Vec::new();
    if !clean(node.shell, tree)? {
        blockers.push("uncommitted changes".to_string());
    }
    let checkout = Path::new(&tree.repo);
    if let Some(lock) = crate::worktree::lock_reason(node.shell, checkout, &tree.path)?
        && lock != crate::worktree::AGENT_LOCK
    {
        blockers.push(format!("locked ({lock})"));
    }
    let workspace = crate::worktree::workspace(node.shell, checkout, &tree.path)?;
    let holders: Vec<Pane> = crate::worktree::holders(panes, &tree.path, workspace.as_deref())
        .into_iter()
        .cloned()
        .collect();
    for pane in &holders {
        if let Some(reason) = crate::worktree::busy(node.shell, pane)? {
            blockers.push(format!("{} is busy ({reason})", pane.pane_id));
        } else if pane.agent.is_some() && !stale.contains(&pane.pane_id) {
            blockers.push(format!(
                "{} has an agent that is {}; its owner closes it with `wrangle close`",
                pane.pane_id,
                pane.agent_status.as_deref().unwrap_or("quiet")
            ));
        }
    }
    Ok(Held {
        workspace,
        holders,
        blockers,
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

fn validate_tree(node: &Node<'_>, planned: &Tree, now: u64) -> Result<Held> {
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
    if crate::integration::check(node.shell, &current.repo, &current.branch)?.is_none() {
        return Err(Error::Invalid(format!(
            "refuse {}: branch {} is not integrated",
            current.path, current.branch
        )));
    }
    let held = assess(
        node,
        &current,
        &current_panes(node.shell)?,
        &stale_agents(node, now)?,
    )?;
    if !held.blockers.is_empty() {
        return Err(Error::Invalid(format!(
            "refuse {}: {}",
            current.path,
            held.blockers.join("; ")
        )));
    }
    Ok(held)
}

/// Drop a removed worktree's registration. Its repository stays registered.
pub fn forget(ledger: &Ledger, path: &str) -> Result<()> {
    ledger.with_lock("worktrees", || {
        let mut trees: Vec<Tree> = ledger.read("worktrees.json")?.unwrap_or_default();
        trees.retain(|t| t.path != path);
        ledger.write("worktrees.json", &trees)
    })
}

fn remove_tree(node: &Node<'_>, tree: &Tree, now: u64) -> Result<String> {
    let held = validate_tree(node, tree, now)?;
    let linked = crate::worktree::Linked {
        checkout: tree.repo.clone().into(),
        path: tree.path.clone(),
        branch: tree.branch.clone(),
    };
    let holders: Vec<&Pane> = held.holders.iter().collect();
    let deleted = crate::worktree::remove(
        node.shell,
        node.ledger,
        node.hooks,
        &linked,
        held.workspace.as_deref(),
        &holders,
    )?;
    let closed = holders
        .iter()
        .map(|p| p.pane_id.as_str())
        .chain(held.workspace.as_deref())
        .collect::<Vec<_>>();
    Ok(format!(
        "removed {}{}{}",
        tree.path,
        if closed.is_empty() {
            String::new()
        } else {
            format!(" and closed {}", closed.join(", "))
        },
        if deleted { "; branch deleted" } else { "" }
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
        match apply_one(node, &actual, requested, now) {
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
            Some(tree) => remove_tree(node, tree, now),
            None => drop_registration(node, &candidate.id),
        },
        "panes" => {
            let agents = crate::probe::agent_list(node.shell)?;
            if !agents
                .iter()
                .any(|a| a.pane == candidate.id && matches!(a.state.as_str(), "idle" | "done"))
            {
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
