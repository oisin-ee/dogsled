//! Prepare work in the repository checkout, never in the caller's worktree.

use crate::error::{Error, Result};
use crate::hosts::{Fleet, Host};
use crate::repository::Values;
use crate::shell::Shell;
use crate::worktree::{self, AGENT_LOCK};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prepared {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Pending {
    #[serde(flatten)]
    tree: crate::cleanup::Tree,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
}

pub fn main_checkout(shell: &dyn Shell, repo: &str) -> Result<PathBuf> {
    let out = shell.run_ok(
        "git",
        &[
            "-C".into(),
            repo.into(),
            "rev-parse".into(),
            "--path-format=absolute".into(),
            "--git-common-dir".into(),
        ],
    )?;
    let path = Path::new(out.stdout.trim());
    if path.as_os_str().is_empty() {
        return Err(Error::Invalid("git returned no checkout".into()));
    }
    Ok(if path.file_name().is_some_and(|name| name == ".git") {
        path.parent()
            .ok_or_else(|| Error::Invalid("git returned no checkout parent".into()))?
            .to_path_buf()
    } else {
        path.to_path_buf()
    })
}

pub fn run(
    fleet: &Fleet,
    host: &Host,
    repo: &str,
    branch: &str,
    base: Option<&str>,
    request_id: &str,
) -> Result<Prepared> {
    crate::jobs::validate_id(request_id)?;
    let checkout = main_checkout(fleet.shell, repo)?;
    if host.is_local() {
        let prepared = local(
            fleet.shell,
            fleet.ledger,
            &checkout.display().to_string(),
            branch,
            base,
            Some(request_id),
        )?;
        if let Some(path) = &prepared.path {
            crate::cleanup::remember(
                fleet.ledger,
                crate::cleanup::Tree {
                    repo: checkout.display().to_string(),
                    path: path.clone(),
                    branch: branch.into(),
                    head: String::new(),
                },
            )?;
        }
        return Ok(prepared);
    }
    let home = Path::new(&fleet.home);
    let canonical = home.canonicalize().ok();
    let remote = checkout
        .strip_prefix(home)
        .ok()
        .or_else(|| {
            canonical
                .as_deref()
                .and_then(|home| checkout.strip_prefix(home).ok())
        })
        .unwrap_or(&checkout)
        .display()
        .to_string();
    let mut args = vec![
        "prepare".into(),
        "--repo".into(),
        remote,
        "--branch".into(),
        branch.into(),
        "--request-id".into(),
        request_id.into(),
    ];
    if let Some(base) = base {
        args.extend(["--base".into(), base.into()]);
    }
    fleet.remote_setup(host, &args)
}

/// Create or reuse the branch's linked worktree in the main checkout of `repo`, lock it as an
/// agent worktree, run the repository's `setup` in a new one, and open it as a Herdr workspace
/// linked to the repository. Records the main checkout's workspace as the anchor when the
/// repository had none open.
pub fn local(
    shell: &dyn Shell,
    ledger: &crate::ledger::Ledger,
    repo: &str,
    branch: &str,
    base: Option<&str>,
    request_id: Option<&str>,
) -> Result<Prepared> {
    if let Some(id) = request_id {
        crate::jobs::validate_id(id)?;
    }
    let invalid = branch.is_empty()
        || branch.starts_with('-')
        || !shell
            .run(
                "git",
                &["check-ref-format".into(), "--branch".into(), branch.into()],
            )?
            .ok();
    if invalid {
        return Err(Error::Invalid(format!(
            "{branch} is not a valid branch name"
        )));
    }
    shell
        .run_ok("herdr", &["workspace".into(), "list".into()])
        .map_err(|e| Error::Invalid(format!("no Herdr server is reachable on this host: {e}")))?;
    let checkout = main_checkout(shell, repo)?;
    let settings = crate::repository::load(&checkout)?;
    let existing = worktree::list(shell, &checkout)?
        .into_iter()
        .find(|w| w.branch.as_deref() == Some(branch));
    let has_branch =
        existing.is_some() || has_ref(shell, &checkout, &format!("refs/heads/{branch}"))?;
    let start = start(shell, &checkout, branch, base, has_branch)?;
    // Publish the intended path before creating or reusing it. Cleanup must not race setup
    // or the transport gap before launch has recorded its receipt.
    let pending = crate::cleanup::Tree {
        repo: checkout.display().to_string(),
        path: existing.as_ref().map_or_else(
            || {
                checkout
                    .join(&settings.worktree.dir)
                    .join(branch.replace(['/', '\\'], "-"))
                    .display()
                    .to_string()
            },
            |tree| tree.path.clone(),
        ),
        branch: branch.into(),
        head: String::new(),
    };
    let held = pending.path.clone();
    record(ledger, pending, request_id)?;
    // A failed preparation is final: its admission never resumes it, so it holds nothing.
    // Only an interrupted preparation leaves its hold for inspection.
    make(shell, &checkout, &settings, existing, branch, &start).or_else(|error| {
        used(ledger, &held, request_id)?;
        Err(error)
    })
}

/// Where the branch of a new worktree comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Start {
    /// The local branch exists.
    Local,
    /// Only `origin` has the branch: the local branch starts at this remote-tracking ref and
    /// tracks it.
    Remote(String),
    /// The branch is new: it starts at this base, with no upstream.
    New(String),
}

impl Start {
    /// The local branch that this placement creates, which a failed setup deletes again. A
    /// branch that existed before placement is never deleted.
    fn created<'a>(&self, branch: &'a str) -> Option<&'a str> {
        (*self != Self::Local).then_some(branch)
    }
}

/// Decide where the branch starts. A branch that exists locally or on `origin` is reused and
/// takes no base. A new branch starts at `base`, or at the current tip of the remote default
/// branch, never at a stale local copy of it.
fn start(
    shell: &dyn Shell,
    checkout: &Path,
    branch: &str,
    base: Option<&str>,
    has_branch: bool,
) -> Result<Start> {
    if has_branch {
        return match base {
            Some(_) => Err(Error::Invalid(format!(
                "branch {branch} exists; omit --base"
            ))),
            None => Ok(Start::Local),
        };
    }
    let fetched = fetch(shell, checkout)?;
    let remote = format!("refs/remotes/origin/{branch}");
    if fetched && has_ref(shell, checkout, &remote)? {
        return match base {
            Some(_) => Err(Error::Invalid(format!(
                "branch {branch} exists on origin; omit --base"
            ))),
            None => Ok(Start::Remote(remote)),
        };
    }
    Ok(Start::New(match base {
        Some(base) => base.to_string(),
        None => default_base(shell, checkout, fetched)?,
    }))
}

/// Fetch `origin` so that its branches and default branch are current. `false` means the
/// repository has no `origin`, and only local refs exist.
fn fetch(shell: &dyn Shell, checkout: &Path) -> Result<bool> {
    if !git(checkout, &["remote", "get-url", "origin"])
        .run(shell)?
        .ok()
    {
        return Ok(false);
    }
    git(checkout, &["fetch", "--quiet", "origin"])
        .setup_ok(shell)
        .map_err(|error| {
            Error::Invalid(format!(
                "git fetch origin failed, so the current origin branches are unknown: {error}"
            ))
        })?;
    Ok(true)
}

fn has_ref(shell: &dyn Shell, checkout: &Path, reference: &str) -> Result<bool> {
    Ok(
        git(checkout, &["show-ref", "--verify", "--quiet", reference])
            .run(shell)?
            .ok(),
    )
}

/// Create or reuse the worktree, run setup in a new one, and open it in Herdr.
fn make(
    shell: &dyn Shell,
    checkout: &Path,
    settings: &crate::repository::Settings,
    existing: Option<worktree::Listed>,
    branch: &str,
    start: &Start,
) -> Result<Prepared> {
    let (path, created) = if let Some(found) = existing {
        (reuse(shell, checkout, found)?, false)
    } else {
        let path = checkout
            .join(&settings.worktree.dir)
            .join(branch.replace(['/', '\\'], "-"))
            .display()
            .to_string();
        let mut args = vec!["worktree", "add", "--lock", "--reason", AGENT_LOCK];
        match start {
            Start::Local => args.extend([path.as_str(), branch]),
            Start::Remote(remote) => {
                args.extend(["--track", "-b", branch, path.as_str(), remote.as_str()]);
            }
            Start::New(base) => {
                args.extend(["--no-track", "-b", branch, path.as_str(), base.as_str()]);
            }
        }
        git(checkout, &args).setup_ok(shell)?;
        (path, true)
    };
    if created {
        let values = Values {
            main: checkout,
            path: &path,
            branch,
        };
        if let Err(error) = crate::repository::run(shell, "setup", &settings.worktree.setup, values)
        {
            discard(shell, checkout, &path, start.created(branch))?;
            return Err(Error::Invalid(format!(
                "{error}; the new worktree was removed"
            )));
        }
    }
    open(shell, checkout, &path)
}

fn record(
    ledger: &crate::ledger::Ledger,
    pending: crate::cleanup::Tree,
    request_id: Option<&str>,
) -> Result<()> {
    ledger.with_lock("preparations", || {
        let mut preparations: Vec<Pending> = ledger.read("preparations.json")?.unwrap_or_default();
        preparations.retain(|record| {
            record.tree.path != pending.path || record.request_id.as_deref() != request_id
        });
        preparations.push(Pending {
            tree: pending,
            request_id: request_id.map(str::to_owned),
        });
        ledger.write("preparations.json", &preparations)
    })
}

/// Internal remote preparation applies host-relative checkout routing and remembers cleanup.
pub fn on_host(
    node: &crate::node::Node<'_>,
    repo: &str,
    branch: String,
    base: Option<&str>,
    request_id: Option<&str>,
) -> Result<Prepared> {
    let repo = Path::new(node.home).join(repo).display().to_string();
    let prepared = local(node.shell, node.ledger, &repo, &branch, base, request_id)?;
    if let Some(path) = &prepared.path {
        crate::cleanup::remember(
            node.ledger,
            crate::cleanup::Tree {
                repo,
                path: path.clone(),
                branch,
                head: String::new(),
            },
        )?;
    }
    Ok(prepared)
}

/// Preparations stay pending until the host has confirmed the launch or returned a placement.
/// A record whose launch is final, or whose local preparation failed, has settled and holds
/// nothing, even when the release that should have followed never ran. Interrupted and
/// unresolved preparations keep their hold.
pub fn pending_trees(
    ledger: &crate::ledger::Ledger,
    launches: &crate::launch::Survey,
) -> Result<Vec<crate::cleanup::Tree>> {
    let records: Vec<Pending> = ledger.read("preparations.json")?.unwrap_or_default();
    let mut pending = Vec::new();
    for record in records {
        let id = record.request_id.as_deref();
        let settled = launches.consumed(id, &record.tree.path)
            || id.map_or(Ok(false), |id| crate::work::preparation_failed(ledger, id))?;
        if !settled {
            pending.push(record.tree);
        }
    }
    Ok(pending)
}

/// Release only this preparation owner. Legacy path-only callers cannot clear identified holds.
pub fn used(ledger: &crate::ledger::Ledger, path: &str, request_id: Option<&str>) -> Result<()> {
    if let Some(id) = request_id {
        crate::jobs::validate_id(id)?;
    }
    ledger.with_lock("preparations", || {
        let mut preparations: Vec<Pending> = ledger.read("preparations.json")?.unwrap_or_default();
        preparations.retain(|record| {
            !worktree::same(&record.tree.path, path) || record.request_id.as_deref() != request_id
        });
        ledger.write("preparations.json", &preparations)
    })
}

/// The branch's existing linked worktree, locked as an agent worktree. Another lock, or the
/// main checkout, is someone else's.
fn reuse(shell: &dyn Shell, checkout: &Path, found: worktree::Listed) -> Result<String> {
    if worktree::same(&found.path, &checkout.display().to_string()) {
        return Err(Error::Invalid(format!(
            "branch {} is checked out in the main checkout {}",
            found.branch.unwrap_or_default(),
            found.path
        )));
    }
    match found.lock.as_deref() {
        None => git(
            checkout,
            &["worktree", "lock", "--reason", AGENT_LOCK, &found.path],
        )
        .run_ok(shell)?,
        Some(AGENT_LOCK) => (),
        Some(other) => {
            return Err(Error::Invalid(format!(
                "{} is locked ({other}); it is not an agent worktree",
                found.path
            )));
        }
    }
    Ok(found.path)
}

/// The base of a new branch: the fetched tip of `origin`'s default branch, or the local
/// default branch when the repository has no `origin`. A clone without `origin/HEAD` asks
/// `origin` for its default branch.
fn default_base(shell: &dyn Shell, checkout: &Path, fetched: bool) -> Result<String> {
    let base = crate::integration::base(shell, &checkout.display().to_string())?;
    if !fetched || base.starts_with("refs/remotes/origin/") {
        return Ok(base);
    }
    let out = git(checkout, &["ls-remote", "--symref", "origin", "HEAD"]).run(shell)?;
    let head = out.ok().then(|| {
        out.stdout.lines().find_map(|line| {
            line.strip_prefix("ref: refs/heads/")
                .and_then(|rest| rest.strip_suffix("\tHEAD"))
                .map(|name| format!("refs/remotes/origin/{name}"))
        })
    });
    match head.flatten() {
        Some(remote) if has_ref(shell, checkout, &remote)? => Ok(remote),
        _ => Err(Error::Invalid(format!(
            "cannot find the default branch of origin in {}; pass --base",
            checkout.display()
        ))),
    }
}

/// Remove a worktree that `place` just made and whose setup failed. It holds nothing yet, so
/// the removal is forced through the agent lock, and a branch made for it goes too.
fn discard(shell: &dyn Shell, checkout: &Path, path: &str, branch: Option<&str>) -> Result<()> {
    git(checkout, &["worktree", "remove", "-f", "-f", path]).setup_ok(shell)?;
    if let Some(branch) = branch {
        git(checkout, &["branch", "-D", branch]).run_ok(shell)?;
    }
    Ok(())
}

/// Open `path` as a Herdr workspace without focus, and record the anchor when Herdr opened the
/// main checkout's workspace for it. A workspace that already showed the path may belong to a
/// running child, so that is an error.
fn open(shell: &dyn Shell, checkout: &Path, path: &str) -> Result<Prepared> {
    let main = checkout.display().to_string();
    let before = worktree::workspace(shell, checkout, &main)?;
    let out = shell.run_ok(
        "herdr",
        &[
            "worktree".into(),
            "open".into(),
            "--cwd".into(),
            main.clone(),
            "--path".into(),
            path.into(),
            "--no-focus".into(),
        ],
    )?;
    let opened: serde_json::Value = serde_json::from_str(&out.stdout)?;
    if before.is_none()
        && let Some(anchor) = worktree::workspace(shell, checkout, &main)?
    {
        worktree::record_anchor(shell, checkout, &anchor)?;
    }
    let workspace_id = opened
        .pointer("/result/workspace/workspace_id")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string);
    if opened.pointer("/result/already_open") == Some(&serde_json::Value::Bool(true)) {
        return Err(Error::Invalid(format!(
            "Herdr workspace {} already shows {path}; a child may own it",
            workspace_id.as_deref().unwrap_or("?")
        )));
    }
    let pane_id = opened
        .pointer("/result/root_pane/pane_id")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string);
    if pane_id.is_none() {
        return Err(Error::Parse {
            origin: "herdr worktree open".into(),
            detail: out.stdout.trim().into(),
        });
    }
    let terminal_id = opened
        .pointer("/result/root_pane/terminal_id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Error::Parse {
            origin: "herdr worktree open".into(),
            detail: "created pane has no terminal identity".into(),
        })?
        .to_owned();
    Ok(Prepared {
        pane_id,
        terminal_id: Some(terminal_id),
        workspace_id,
        path: Some(
            opened
                .pointer("/result/worktree/path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(path)
                .to_string(),
        ),
    })
}

/// One Git invocation in `dir`.
struct Git(Vec<String>);

fn git(dir: &Path, args: &[&str]) -> Git {
    Git(["-C".to_string(), dir.display().to_string()]
        .into_iter()
        .chain(args.iter().map(ToString::to_string))
        .collect())
}

impl Git {
    fn run(&self, shell: &dyn Shell) -> Result<crate::shell::Output> {
        shell.run("git", &self.0)
    }

    fn run_ok(&self, shell: &dyn Shell) -> Result<()> {
        shell.run_ok("git", &self.0).map(drop)
    }

    /// Making or removing a checkout can take longer than an ordinary call.
    fn setup_ok(&self, shell: &dyn Shell) -> Result<()> {
        shell.run_setup_ok("git", &self.0).map(drop)
    }
}
