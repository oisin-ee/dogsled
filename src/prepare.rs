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
    let Some(id) = request_id else {
        return local_once(shell, ledger, repo, branch, base, None);
    };
    crate::jobs::validate_id(id)?;
    let receipts = ledger.child("preparation-receipts");
    receipts.with_lock(id, || {
        let file = format!("{id}.json");
        let request = (
            main_checkout(shell, repo)?,
            branch.to_owned(),
            base.map(str::to_owned),
        );
        if let Some(receipt) = receipts.read::<PreparationReceipt>(&file)? {
            if receipt.request != request {
                return Err(crate::unit_transport::conflict(
                    "preparation request changed",
                ));
            }
            return Ok(receipt.prepared);
        }
        let prepared = local_once(shell, ledger, repo, branch, base, Some(id))?;
        receipts.write(
            &file,
            &PreparationReceipt {
                request,
                prepared: prepared.clone(),
            },
        )?;
        Ok(prepared)
    })
}

/// A completed preparation is replayed only for the same request and inputs. A lost SSH
/// reply must not create another workspace or reject the branch that this request created.
#[derive(Debug, Serialize, Deserialize)]
struct PreparationReceipt {
    request: (PathBuf, String, Option<String>),
    prepared: Prepared,
}

fn local_once(
    shell: &dyn Shell,
    ledger: &crate::ledger::Ledger,
    repo: &str,
    branch: &str,
    base: Option<&str>,
    request_id: Option<&str>,
) -> Result<Prepared> {
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
    let has_branch = existing.is_some()
        || git(
            &checkout,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .run(shell)?
        .ok();
    if has_branch && base.is_some() {
        return Err(Error::Invalid(format!(
            "branch {branch} exists; omit --base"
        )));
    }
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
    make(
        shell, &checkout, &settings, existing, branch, base, has_branch,
    )
    .or_else(|error| {
        used(ledger, &held, request_id)?;
        Err(error)
    })
}

/// Create or reuse the worktree, run setup in a new one, and open it in Herdr.
fn make(
    shell: &dyn Shell,
    checkout: &Path,
    settings: &crate::repository::Settings,
    existing: Option<worktree::Listed>,
    branch: &str,
    base: Option<&str>,
    has_branch: bool,
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
        let start;
        if has_branch {
            args.extend([path.as_str(), branch]);
        } else {
            start = match base {
                Some(base) => base.to_string(),
                None => default_branch(shell, checkout)?,
            };
            args.extend(["-b", branch, path.as_str(), start.as_str()]);
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
            discard(shell, checkout, &path, (!has_branch).then_some(branch))?;
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

/// The local name of the repository's default branch, the base of a new branch.
fn default_branch(shell: &dyn Shell, checkout: &Path) -> Result<String> {
    let base = crate::integration::base(shell, &checkout.display().to_string())?;
    Ok(base
        .strip_prefix("refs/remotes/origin/")
        .or_else(|| base.strip_prefix("refs/heads/"))
        .unwrap_or(&base)
        .to_string())
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
