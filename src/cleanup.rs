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
const HOUR: u64 = 3_600_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tree {
    pub repo: String,
    pub path: String,
    pub branch: String,
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

fn trees(node: &Node<'_>, panes: &[Pane], discover: bool) -> Result<Vec<Tree>> {
    let recorded: Vec<Tree> = node.ledger.read("worktrees.json")?.unwrap_or_default();
    let mut repos: BTreeSet<String> = recorded.iter().map(|t| t.repo.clone()).collect();
    let cwd = std::env::current_dir()?;
    // Discover repositories from live pane directories and the caller. Do not scan home.
    for path in panes
        .iter()
        .flat_map(|p| [p.cwd.as_deref(), p.foreground_cwd.as_deref()])
        .flatten()
        .chain(cwd.to_str().filter(|_| discover))
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
    let mut result = Vec::new();
    for repo in repos {
        let output = git(
            node.shell,
            &repo,
            &["worktree", "list", "--porcelain", "-z"],
        )?;
        if !output.ok() {
            return Err(Error::Invalid(format!(
                "cannot list worktrees in {repo}: {}",
                output.stderr.trim()
            )));
        }
        let mut path = String::new();
        let mut branch = String::new();
        for field in output.stdout.split('\0') {
            if let Some(value) = field.strip_prefix("worktree ") {
                path = value.into();
            } else if let Some(value) = field.strip_prefix("branch refs/heads/") {
                branch = value.into();
            } else if field.is_empty() && !path.is_empty() {
                if path != repo && !branch.is_empty() {
                    result.push(Tree {
                        repo: repo.clone(),
                        path: path.clone(),
                        branch: branch.clone(),
                    });
                }
                path.clear();
                branch.clear();
            }
        }
    }
    Ok(result)
}

fn retired(shell: &dyn Shell, tree: &Tree) -> Result<bool> {
    let branch = format!("refs/heads/{}", tree.branch);
    let exists = git(
        shell,
        &tree.repo,
        &["show-ref", "--verify", "--quiet", &branch],
    )?;
    if exists.status == 1 {
        return Ok(true);
    }
    if !exists.ok() {
        return Err(Error::Invalid(format!(
            "cannot inspect branch {}",
            tree.branch
        )));
    }
    // The repository's remote default branch is authoritative; local main is the fallback.
    let default = git(
        shell,
        &tree.repo,
        &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
    )?;
    let base = if default.ok() {
        default.stdout.trim()
    } else {
        "refs/heads/main"
    };
    let merged = git(
        shell,
        &tree.repo,
        &["merge-base", "--is-ancestor", &branch, base],
    )?;
    match merged.status {
        0 => Ok(true),
        1 => Ok(false),
        _ => Err(Error::Invalid(format!(
            "cannot determine merge status of {} against {base}",
            tree.branch
        ))),
    }
}

fn resolved_path(path: &Path) -> std::path::PathBuf {
    for ancestor in path.ancestors() {
        if let Ok(root) = ancestor.canonicalize() {
            return path
                .strip_prefix(ancestor)
                .map_or(root.clone(), |suffix| root.join(suffix));
        }
    }
    path.to_path_buf()
}

fn live(tree: &Tree, panes: &[Pane]) -> bool {
    panes.iter().any(|p| {
        [p.cwd.as_deref(), p.foreground_cwd.as_deref()]
            .into_iter()
            .flatten()
            .any(|cwd| {
                resolved_path(Path::new(cwd)).starts_with(resolved_path(Path::new(&tree.path)))
            })
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

fn current_panes(shell: &dyn Shell) -> Result<Vec<Pane>> {
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
        let out = node
            .shell
            .run_ok("du", &["-sk".into(), "--".into(), path.clone()])?;
        let size: u64 = out
            .stdout
            .split_whitespace()
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| Error::Invalid(format!("invalid du output for {path}")))?;
        sizes.push((size, path));
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
    let trees = trees(node, &panes, true)?;
    let mut plan = Plan::default();
    for tree in &trees {
        if !live(tree, &panes) && retired(node.shell, tree)? {
            let clean = clean(node.shell, tree)?;
            let mut e = entry(
                "worktrees",
                tree.path.clone(),
                format!(
                    "branch {} merged or deleted; {}",
                    tree.branch,
                    if clean { "clean" } else { "dirty: refused" }
                ),
                clean,
            );
            e.tree = Some(tree.clone());
            plan.entries.push(e);
        }
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
    disk(node, &owned, &mut plan)?;
    processes(node.shell, &panes, &mut plan)?;
    Ok(plan)
}

fn remove_tree(node: &Node<'_>, tree: &Tree) -> Result<String> {
    if live(tree, &current_panes(node.shell)?) {
        return Err(Error::Invalid(format!("refuse {}: live pane", tree.path)));
    }
    if !retired(node.shell, tree)? {
        return Err(Error::Invalid(format!(
            "refuse {}: unmerged branch",
            tree.path
        )));
    }
    if !clean(node.shell, tree)? {
        return Err(Error::Invalid(format!(
            "refuse {}: dirty worktree",
            tree.path
        )));
    }
    let tasks = node.shell.run_ok(
        "mise",
        &[
            "-C".into(),
            tree.repo.clone(),
            "tasks".into(),
            "--json".into(),
        ],
    )?;
    let tasks: serde_json::Value = serde_json::from_str(&tasks.stdout)?;
    let tasks = tasks.as_array().ok_or_else(|| {
        Error::Invalid("mise tasks returned no task array; refusing removal".into())
    })?;
    let has_remove = tasks.iter().any(|t| t["name"] == "agent:worktree-remove");
    // Task discovery can run hooks. Recheck immediately before invoking removal.
    if live(tree, &current_panes(node.shell)?)
        || !clean(node.shell, tree)?
        || !retired(node.shell, tree)?
    {
        return Err(Error::Invalid(format!(
            "refuse {}: worktree changed during planning",
            tree.path
        )));
    }
    let method = if has_remove {
        node.shell.run_setup_ok(
            "mise",
            &[
                "-C".into(),
                tree.repo.clone(),
                "run".into(),
                "-q".into(),
                "agent:worktree-remove".into(),
                "--".into(),
                tree.branch.clone(),
            ],
        )?;
        "agent:worktree-remove (repository task exists)"
    } else {
        node.shell.run_setup_ok(
            "git",
            &[
                "-C".into(),
                tree.repo.clone(),
                "worktree".into(),
                "remove".into(),
                "--".into(),
                tree.path.clone(),
            ],
        )?;
        "git worktree remove (no repository remove task)"
    };
    node.ledger.with_lock("worktrees", || {
        let mut trees: Vec<Tree> = node.ledger.read("worktrees.json")?.unwrap_or_default();
        trees.retain(|t| t.path != tree.path);
        node.ledger.write("worktrees.json", &trees)
    })?;
    Ok(format!("removed {} via {method}", tree.path))
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

pub fn apply(node: &Node<'_>, plan: &Plan, now: u64) -> Result<Vec<String>> {
    let mut results = Vec::new();
    // A remote plan is input, not authority: rebuild the candidate set on this host.
    let actual = doctor(node, now)?;
    for requested in &plan.entries {
        if matches!(requested.class.as_str(), "disk" | "processes") {
            results.push(format!("{} {}: report only", requested.class, requested.id));
            continue;
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
        match candidate.class.as_str() {
            "worktrees" => results.push(remove_tree(
                node,
                candidate
                    .tree
                    .as_ref()
                    .ok_or_else(|| Error::Invalid("missing worktree identity".into()))?,
            )?),
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
                results.push(format!("closed pane {}", candidate.id));
            }
            "jobs" => results.push(remove_job(node, &candidate.id, now)?),
            _ => return Err(Error::Invalid("unknown cleanup class".into())),
        }
    }
    Ok(results)
}

pub fn auto(node: &Node<'_>, now: u64) -> Result<()> {
    node.ledger.with_lock("cleanup", || {
        let last: Option<u64> = node.ledger.read("cleanup.json")?;
        if last.is_some_and(|t| now.saturating_sub(t) < HOUR) {
            return Ok(());
        }
        node.ledger.write("cleanup.json", &now)?;
        // Registered repositories remain discoverable after their last pane closes.
        // Automatic removal never expands to unrelated live-pane repositories.
        for tree in trees(node, &[], false)? {
            if Path::new(&tree.path).exists()
                && !live(&tree, &current_panes(node.shell)?)
                && retired(node.shell, &tree)?
                && clean(node.shell, &tree)?
            {
                eprintln!("wrangle cleanup: {}", remove_tree(node, &tree)?);
            }
        }
        Ok(())
    })
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
