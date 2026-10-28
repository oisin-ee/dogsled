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

/// Proof that this operation holds the repository-wide lifecycle mutex.
/// The mutex address depends only on the canonical Git common directory.
#[derive(Debug)]
pub struct Guard {
    common: PathBuf,
}

/// A Git administrative directory and its write-once checkout generation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Identity {
    pub common: PathBuf,
    pub gitdir: PathBuf,
    pub generation: String,
}

impl PartialEq for Identity {
    fn eq(&self, other: &Self) -> bool {
        resolved(&self.common) == resolved(&other.common)
            && resolved(&self.gitdir) == resolved(&other.gitdir)
            && self.generation == other.generation
    }
}

impl Eq for Identity {}

const CHECKOUT_ID: &str = "dogsled-checkout-id";

fn generation(gitdir: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(gitdir.join(CHECKOUT_ID)) {
        Ok(text) => {
            let id = text.trim();
            if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(refuse("malformed checkout generation"));
            }
            Ok(Some(id.into()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

impl Guard {
    /// Read an existing checkout without creating its generation marker.
    pub fn observe(&self, shell: &dyn Shell, path: &str) -> Result<Option<Linked>> {
        let Some(tree) = snapshot(shell, path)? else {
            return Ok(None);
        };
        self.check_repository(shell, &tree.checkout)?;
        validate_identity(shell, &tree)?;
        if snapshot(shell, path)?.is_none_or(|now| now.head != tree.head) {
            return Err(refuse("HEAD moved during checkout observation"));
        }
        Ok(Some(tree))
    }

    /// Fresh discovery only. Stale validation must never create a generation marker.
    pub fn discover(&self, shell: &dyn Shell, path: &str) -> Result<Option<Linked>> {
        let Some(mut tree) = snapshot(shell, path)? else {
            return Ok(None);
        };
        self.check_repository(shell, &tree.checkout)?;
        if tree.identity.is_none() {
            tree.identity = Some(self.discover_identity(&tree.gitdir)?);
        }
        validate_identity(shell, &tree)?;
        if snapshot(shell, path)?.is_none_or(|now| now.head != tree.head) {
            return Err(refuse("HEAD moved during checkout discovery"));
        }
        Ok(Some(tree))
    }

    /// Discovery returns only the generation; it never supplies a new HEAD expectation.
    fn discover_identity(&self, gitdir: &Path) -> Result<Identity> {
        let id = if let Some(id) = generation(gitdir)? {
            id
        } else {
            use std::io::Write as _;
            let mut bytes = [0u8; 16];
            getrandom::fill(&mut bytes)
                .map_err(|error| Error::Invalid(format!("checkout generation: {error}")))?;
            let id = bytes
                .iter()
                .fold(String::with_capacity(32), |mut id, byte| {
                    const HEX: &[u8; 16] = b"0123456789abcdef";
                    id.push(char::from(HEX[usize::from(byte >> 4)]));
                    id.push(char::from(HEX[usize::from(byte & 15)]));
                    id
                });
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(gitdir.join(CHECKOUT_ID))
            {
                Ok(mut marker) => {
                    writeln!(marker, "{id}")?;
                    marker.sync_all()?;
                    id
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    generation(gitdir)?.ok_or_else(|| refuse("checkout generation disappeared"))?
                }
                Err(error) => return Err(error.into()),
            }
        };
        Ok(Identity {
            common: self.common.clone(),
            gitdir: gitdir.into(),
            generation: id,
        })
    }

    /// Establish an unmarked checkout only if the pinned directory and the pre-wait
    /// observation still agree. Discovery must not replace the request's HEAD.
    pub fn discover_unmarked(
        &self,
        shell: &dyn Shell,
        before: &Linked,
        pin: &Pin,
    ) -> Result<Linked> {
        pin.validate(before)?;
        let now = linked_or_detached(shell, &before.path)?
            .ok_or_else(|| refuse("checkout identity is unreadable"))?;
        let mut expected = before.tree();
        expected.identity.clone_from(&now.identity);
        if before.identity.is_some()
            || expected.drift(&now.tree()) == crate::cleanup::Drift::Identity
            || resolved(&before.gitdir) != resolved(&now.gitdir)
        {
            return Err(refuse("worktree identity changed"));
        }
        if before.head != now.head {
            return Err(refuse("HEAD moved before close"));
        }
        self.check_repository(shell, &before.checkout)?;
        pin.validate(before)?;
        let mut tree = before.clone();
        tree.identity = Some(match now.identity {
            Some(identity) => identity,
            None => self.discover_identity(&before.gitdir)?,
        });
        pin.validate(before)?;
        validate_identity(shell, &tree)?;
        if snapshot(shell, &before.path)?.is_none_or(|now| now.head != before.head) {
            return Err(refuse("HEAD moved during checkout discovery"));
        }
        pin.validate(before)?;
        Ok(tree)
    }

    pub fn check_repository(&self, shell: &dyn Shell, checkout: &Path) -> Result<()> {
        if common_dir(shell, checkout)?.canonicalize()? != self.common {
            return Err(refuse("repository identity changed"));
        }
        Ok(())
    }
}

pub fn with_lifecycle<T>(
    shell: &dyn Shell,
    ledger: &Ledger,
    checkout: &Path,
    operation: impl FnOnce(&Guard) -> Result<T>,
) -> Result<T> {
    let common = common_dir(shell, checkout)?.canonicalize()?;
    let lock = Ledger {
        dir: common.clone(),
        budget: ledger.budget,
    };
    lock.with_lock("dogsled-lifecycle", || operation(&Guard { common }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Linked {
    /// The main checkout.
    pub checkout: PathBuf,
    pub path: String,
    /// Empty for a detached worktree.
    pub branch: String,
    pub head: String,
    pub gitdir: PathBuf,
    pub identity: Option<Identity>,
}

/// Open handles keep both checkout-root and administrative inodes alive during a wait.
pub struct Pin {
    root: std::fs::File,
    admin: std::fs::File,
}

impl Pin {
    pub(crate) fn validate(&self, tree: &Linked) -> Result<()> {
        use std::os::unix::fs::MetadataExt as _;
        for (file, path) in [
            (&self.root, Path::new(&tree.path)),
            (&self.admin, &tree.gitdir),
        ] {
            let original = file.metadata()?;
            let current = std::fs::metadata(path)?;
            if original.dev() != current.dev() || original.ino() != current.ino() {
                return Err(refuse("worktree identity changed"));
            }
        }
        Ok(())
    }
}

impl Linked {
    pub(crate) fn pin(&self) -> Result<Pin> {
        Ok(Pin {
            root: std::fs::File::open(&self.path)?,
            admin: std::fs::File::open(&self.gitdir)?,
        })
    }

    /// Compare checkout location and administrative identity without recapturing HEAD.
    pub(crate) fn matches_checkout(&self, expected: &crate::cleanup::Tree) -> bool {
        same(&self.path, &expected.path)
            && expected.identity.as_ref().is_none_or(|identity| {
                resolved(&identity.gitdir) == resolved(&self.gitdir)
                    && self.identity.as_ref().is_none_or(|now| now == identity)
            })
    }

    pub fn tree(&self) -> crate::cleanup::Tree {
        crate::cleanup::Tree {
            repo: self.checkout.display().to_string(),
            path: self.path.clone(),
            branch: self.branch.clone(),
            head: self.head.clone(),
            identity: self.identity.clone(),
        }
    }
}

/// One entry of `git worktree list --porcelain`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listed {
    pub path: String,
    pub branch: Option<String>,
    pub lock: Option<String>,
    /// Git calls it prunable: it is not a working tree any more, and `remove` refuses it.
    pub prunable: bool,
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

/// The worktree that contains `cwd`, when it is a linked worktree on a branch. A detached
/// worktree is not one: it has no branch, so it is refused here.
pub fn linked(shell: &dyn Shell, cwd: &str) -> Result<Option<Linked>> {
    let tree = linked_or_detached(shell, cwd)?;
    if let Some(tree) = &tree
        && tree.branch.is_empty()
    {
        return Err(refuse(&format!(
            "{} is a detached worktree; `dogsled prune` removes it once its commit is on a remote",
            tree.path
        )));
    }
    Ok(tree)
}

/// The commit a checkout's HEAD points at.
pub fn head_sha(shell: &dyn Shell, path: &str) -> Result<String> {
    let out = git(shell, Path::new(path), &["rev-parse", "HEAD"])?;
    if !out.ok() {
        return Err(Error::Invalid(format!(
            "{path}: cannot read HEAD: {}",
            out.stderr.trim()
        )));
    }
    let head = out.stdout.trim().to_string();
    if head.is_empty() {
        return Err(Error::Invalid(format!("{path}: HEAD names no commit")));
    }
    Ok(head)
}

/// The linked worktree that contains `cwd`. A detached one has an empty `branch`; no branch is
/// invented for it.
pub fn linked_or_detached(shell: &dyn Shell, cwd: &str) -> Result<Option<Linked>> {
    capture(shell, cwd, true)
}

/// Read-only capture for launch validation, including the main checkout.
pub fn snapshot(shell: &dyn Shell, cwd: &str) -> Result<Option<Linked>> {
    capture(shell, cwd, false)
}

/// The request observation and handles opened against the same directory tuple.
/// Missing locations or handles are absence candidates, never permission to close.
pub(crate) enum CloseCapture {
    Observed {
        tree: Linked,
        pin: Option<Pin>,
    },
    Missing {
        common: PathBuf,
        gitdir: PathBuf,
        path: String,
    },
}

impl CloseCapture {
    /// Bind missing-location evidence to saved identity before exposing the observation.
    pub(crate) fn bind_saved(
        self,
        saved: Option<&crate::cleanup::Tree>,
    ) -> Result<CloseObservation> {
        match self {
            Self::Observed { tree, pin } => Ok(CloseObservation::Live { tree, pin }),
            Self::Missing {
                common,
                gitdir,
                path,
            } => {
                if !saved.is_some_and(|saved| {
                    same(&saved.path, &path)
                        && saved.identity.as_ref().is_some_and(|identity| {
                            resolved(&identity.common) == resolved(&common)
                                && resolved(&identity.gitdir) == resolved(&gitdir)
                        })
                }) {
                    return Err(refuse("checkout identity is unreadable"));
                }
                Ok(CloseObservation::Missing)
            }
        }
    }
}

/// A missing observation can only reconcile verified absence under the lifecycle lock.
pub(crate) enum CloseObservation {
    Live { tree: Linked, pin: Option<Pin> },
    Missing,
}

#[cfg(test)]
std::thread_local! {
    pub(crate) static BEFORE_CLOSE_STAT: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    pub(crate) static BEFORE_CLOSE_PIN: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

fn directory_key(metadata: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt as _;
    (metadata.dev(), metadata.ino())
}

fn missing(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

/// Locate directories without observing HEAD. The observation is bracketed by root/admin
/// stats, and subsequently opened handles must still name that captured tuple.
pub(crate) fn capture_close(shell: &dyn Shell, cwd: &str) -> Result<Option<CloseCapture>> {
    for attempt in 0..2 {
        let out = git(
            shell,
            Path::new(cwd),
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-common-dir",
                "--git-dir",
                "--show-toplevel",
            ],
        )?;
        if !out.ok() {
            return Ok(None);
        }
        let lines: Vec<_> = out.stdout.lines().map(str::trim).collect();
        let [common, admin, root] = lines[..] else {
            return Err(refuse("cannot locate checkout directories"));
        };
        if same(common, admin) {
            return Ok(None);
        }
        let keys = || -> std::io::Result<_> {
            Ok((
                directory_key(&std::fs::metadata(root)?),
                directory_key(&std::fs::metadata(admin)?),
            ))
        };
        #[cfg(test)]
        BEFORE_CLOSE_STAT.with(|hook| {
            if let Some(hook) = hook.borrow_mut().take() {
                hook();
            }
        });
        let before = match keys() {
            Ok(keys) => keys,
            Err(error) if missing(&error) => {
                return Ok(Some(CloseCapture::Missing {
                    common: resolved(Path::new(common)),
                    gitdir: resolved(Path::new(admin)),
                    path: resolved(Path::new(root)).display().to_string(),
                }));
            }
            Err(error) => return Err(error.into()),
        };
        let Some(tree) = linked_or_detached(shell, cwd)? else {
            if attempt == 0 {
                continue;
            }
            return Err(refuse("worktree identity changed during observation"));
        };
        match keys() {
            Ok(after)
                if before == after
                    && same(root, &tree.path)
                    && same(admin, &tree.gitdir.display().to_string()) => {}
            Err(error) if missing(&error) => {
                return Ok(Some(CloseCapture::Observed { tree, pin: None }));
            }
            Err(error) => return Err(error.into()),
            _ if attempt == 0 => continue,
            _ => return Err(refuse("worktree identity changed during observation")),
        }
        #[cfg(test)]
        BEFORE_CLOSE_PIN.with(|hook| {
            if let Some(hook) = hook.borrow_mut().take() {
                hook();
            }
        });
        let pin = match tree.pin() {
            Ok(pin) => pin,
            Err(Error::Io(error)) if missing(&error) => {
                return Ok(Some(CloseCapture::Observed { tree, pin: None }));
            }
            Err(error) => return Err(error),
        };
        if (
            directory_key(&pin.root.metadata()?),
            directory_key(&pin.admin.metadata()?),
        ) != before
        {
            return Err(refuse("worktree identity changed before pinning"));
        }
        match pin.validate(&tree) {
            Ok(()) => (),
            Err(Error::Io(error)) if missing(&error) => {
                return Ok(Some(CloseCapture::Observed { tree, pin: None }));
            }
            Err(error) => return Err(error),
        }
        return Ok(Some(CloseCapture::Observed {
            tree,
            pin: Some(pin),
        }));
    }
    Err(refuse("worktree identity changed during observation"))
}

fn capture(shell: &dyn Shell, cwd: &str, linked_only: bool) -> Result<Option<Linked>> {
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
            "HEAD",
            "--abbrev-ref",
            "HEAD",
        ],
    )?;
    // Administrative identity exists independently of HEAD in an unborn main checkout.
    let unborn;
    let output = if out.ok() {
        &out.stdout
    } else {
        if linked_only {
            return Ok(None);
        }
        let location = git(
            shell,
            Path::new(cwd),
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-common-dir",
                "--git-dir",
                "--show-toplevel",
            ],
        )?;
        if !location.ok() {
            return Ok(None);
        }
        let branch = git(shell, Path::new(cwd), &["symbolic-ref", "--short", "HEAD"])?;
        if !branch.ok() {
            return Err(refuse("cannot read checkout HEAD"));
        }
        let tip = git(
            shell,
            Path::new(cwd),
            &["rev-parse", "--verify", "--quiet", "HEAD"],
        )?;
        let head = tip.ok().then(|| tip.stdout.trim());
        unborn = format!(
            "{}\n{}\n{}\n",
            location.stdout.trim_end(),
            head.unwrap_or_default(),
            branch.stdout.trim()
        );
        &unborn
    };
    let lines: Vec<&str> = output.lines().map(str::trim).collect();
    let [common, dir, top, head, branch] = lines[..] else {
        return Err(Error::Invalid(format!(
            "git rev-parse in {cwd} returned unexpected output"
        )));
    };
    if linked_only && same(common, dir) {
        return Ok(None);
    }
    let common = Path::new(common).canonicalize()?;
    let checkout = if common.file_name().is_some_and(|name| name == ".git") {
        common
            .parent()
            .ok_or_else(|| refuse("Git common directory has no parent"))?
            .to_path_buf()
    } else {
        common.clone()
    };
    let gitdir = Path::new(dir).canonicalize()?;
    let identity = generation(&gitdir)?.map(|generation| Identity {
        common,
        gitdir: gitdir.clone(),
        generation,
    });
    Ok(Some(Linked {
        head: head.into(),
        gitdir,
        identity,
        checkout,
        // Keep Git's checkout spelling for listings and removal commands. Identity
        // comparisons resolve aliases; administrative paths remain canonical.
        path: top.into(),
        branch: if branch == "HEAD" {
            String::new()
        } else {
            branch.into()
        },
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
            } else if line == "prunable" || line.starts_with("prunable ") {
                current.prunable = true;
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

/// Read-only incarnation validation, including after a setup or teardown hook.
pub fn validate_identity(shell: &dyn Shell, tree: &Linked) -> Result<()> {
    let now = snapshot(shell, &tree.path)?;
    if tree.identity.is_none()
        || now.as_ref().is_none_or(|now| {
            now.identity != tree.identity
                || !same(&now.path, &tree.path)
                || now.branch != tree.branch
                || resolved(&now.checkout) != resolved(&tree.checkout)
        })
    {
        return Err(refuse(&format!("{}: worktree identity changed", tree.path)));
    }
    Ok(())
}

/// A pane can move independently of the repository mutex. Its cwd is comparison-only:
/// it must never select a different checkout for removal.
pub(crate) fn validate_pane_checkout(shell: &dyn Shell, pane: &Pane, tree: &Linked) -> Result<()> {
    let current = pane
        .cwd
        .as_deref()
        .map(|cwd| snapshot(shell, cwd))
        .transpose()?
        .flatten();
    if current.as_ref().is_none_or(|now| {
        resolved(&now.gitdir) != resolved(&tree.gitdir) || !now.matches_checkout(&tree.tree())
    }) {
        return Err(refuse("pane moved to another checkout"));
    }
    Ok(())
}

/// Check only cwd-based holders; workspace siblings can legitimately work elsewhere.
fn validate_holder_checkouts(shell: &dyn Shell, tree: &Linked, holders: &[&Pane]) -> Result<()> {
    let panes = crate::cleanup::current_panes(shell)?;
    for original in holders.iter().filter(|pane| {
        pane.cwd
            .as_deref()
            .is_some_and(|cwd| inside(cwd, &tree.path))
    }) {
        if let Some(current) = panes.iter().find(|pane| pane.pane_id == original.pane_id) {
            validate_pane_checkout(shell, current, tree)?;
        }
    }
    Ok(())
}

/// Absence needs a readable original repository, an unfiltered Git listing, and ENOENT
/// for both the checkout and its recorded administrative directory. Symlinks are not absent.
pub fn verified_absent(guard: &Guard, shell: &dyn Shell, tree: &crate::cleanup::Tree) -> bool {
    let Some(identity) = &tree.identity else {
        return false;
    };
    let missing = |path: &Path| {
        std::fs::symlink_metadata(path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    };
    resolved(&identity.common) == resolved(&guard.common)
        && guard.check_repository(shell, Path::new(&tree.repo)).is_ok()
        && raw_paths(shell, Path::new(&tree.repo)).is_ok_and(|listed| {
            !listed.iter().any(|path| {
                same(path, &tree.path)
                    || linked_or_detached(shell, path)
                        .ok()
                        .flatten()
                        .and_then(|live| live.identity)
                        .is_some_and(|live| live == *identity)
            })
        })
        && missing(Path::new(&tree.path))
        && missing(&identity.gitdir)
}

/// An unmarked request can prove disappearance from its captured root/admin locations.
/// This is read-only and cannot bless a replacement with a new generation.
pub(crate) fn verified_absent_unmarked(guard: &Guard, shell: &dyn Shell, tree: &Linked) -> bool {
    tree.identity.is_none()
        && guard.check_repository(shell, &tree.checkout).is_ok()
        && raw_paths(shell, &tree.checkout)
            .is_ok_and(|listed| !listed.iter().any(|path| same(path, &tree.path)))
        && [Path::new(&tree.path), &tree.gitdir]
            .into_iter()
            .all(|path| {
                std::fs::symlink_metadata(path)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            })
}

fn raw_paths(shell: &dyn Shell, checkout: &Path) -> Result<Vec<String>> {
    let out = git(shell, checkout, &["worktree", "list", "--porcelain", "-z"])?;
    if !out.ok() {
        return Err(refuse("cannot list worktrees for absence verification"));
    }
    Ok(out
        .stdout
        .split('\0')
        .filter_map(|field| field.strip_prefix("worktree ").map(str::to_owned))
        .collect())
}

/// Why `git worktree remove` cannot remove `path` although its directory remains: Git no
/// longer lists it, calls it prunable, or its `.git` file is gone. `None` when Git still links
/// it or the directory is absent.
pub fn unlinked(shell: &dyn Shell, checkout: &Path, path: &str) -> Result<Option<&'static str>> {
    if !Path::new(path).exists() {
        return Ok(None);
    }
    let found = list(shell, checkout)?
        .into_iter()
        .find(|w| same(&w.path, path));
    Ok(unlinked_reason(path, found.map(|w| w.prunable)))
}

/// Why Git cannot remove the existing directory `path`, from Git's entry for it: `None` when Git
/// does not list it, otherwise whether Git calls it prunable.
pub fn unlinked_reason(path: &str, prunable: Option<bool>) -> Option<&'static str> {
    match prunable {
        None => Some("worktree unregistered by git"),
        Some(true) => Some("git marks the worktree prunable; git cannot remove it"),
        Some(false)
            if std::fs::symlink_metadata(Path::new(path).join(".git"))
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            Some("worktree has no .git file; git cannot remove it")
        }
        Some(false) => None,
    }
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
#[derive(Clone, Copy)]
pub struct Removal<'a> {
    pub guard: &'a Guard,
    pub workspace: Option<&'a str>,
    pub holders: &'a [&'a Pane],
    pub caller_workspace: Option<&'a str>,
    pub evidence: &'a crate::loss::Evidence,
}

pub fn remove(
    shell: &dyn Shell,
    ledger: &Ledger,
    tree: &Linked,
    removal: Removal<'_>,
) -> Result<Removed> {
    let Removal {
        guard,
        workspace,
        holders,
        caller_workspace,
        evidence,
    } = removal;
    // Git cannot remove it, and its content is unknown: close nothing, unlock nothing.
    if let Some(reason) = unlinked(shell, &tree.checkout, &tree.path)? {
        return Err(refuse(&format!(
            "{}: {reason}; directory remains",
            tree.path
        )));
    }
    guard.check_repository(shell, &tree.checkout)?;
    validate_identity(shell, tree)?;
    validate_holder_checkouts(shell, tree, holders)?;
    let settings = crate::repository::load(&tree.checkout)?;
    teardown(shell, &settings, tree)?;
    if verified_absent(guard, shell, &tree.tree()) {
        crate::cleanup::forget_matching(ledger, &tree.tree())?;
        return Ok(Removed::default());
    }
    validate_identity(shell, tree)?;
    if Path::new(&tree.path).is_dir() {
        // The teardown ran after the callers' loss check and can leave a commit or content
        // that no ref holds. Check the current HEAD and content again before anything closes
        // or is removed.
        recheck_loss(shell, tree, evidence)?;
        evidence.restore_deletions(shell, &tree.path)?;
    }
    validate_holder_checkouts(shell, tree, holders)?;
    close_holders(shell, ledger, workspace, holders)?;
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
        if verified_absent(guard, shell, &tree.tree()) {
            crate::cleanup::forget_matching(ledger, &tree.tree())?;
            return Ok(Removed::default());
        }
        validate_identity(shell, tree)?;
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
    if !verified_absent(guard, shell, &tree.tree()) {
        return Err(Error::Invalid(format!(
            "git worktree remove exited 0 but {} is still a worktree",
            tree.path
        )));
    }
    crate::cleanup::forget_matching(ledger, &tree.tree())?;
    let branch_deleted =
        !evidence.keeps_branch() && !tree.branch.is_empty() && delete_held(shell, tree, evidence)?;
    let anchor_closed = close_anchor(shell, &tree.checkout, caller_workspace)?;
    Ok(Removed {
        branch_deleted,
        anchor_closed,
    })
}

/// Pane and launch recovery effects begin only after teardown and incarnation validation.
fn close_holders(
    shell: &dyn Shell,
    ledger: &Ledger,
    workspace: Option<&str>,
    holders: &[&Pane],
) -> Result<()> {
    let open = crate::cleanup::current_panes(shell)?;
    for pane in holders {
        // Recovery can close a final shell only after teardown and incarnation validation.
        if pane.agent.is_none()
            && matches!(
                crate::launch::final_pane(ledger, shell, &pane.pane_id)?,
                crate::launch::PaneLaunch::Final
            )
        {
            if !crate::launch::close_final(ledger, shell, &pane.pane_id)? {
                return Err(refuse("launch changed before removal"));
            }
            continue;
        }
        let outside = workspace.is_none() || pane.workspace_id.as_deref() != workspace;
        if outside && open.iter().any(|p| p.pane_id == pane.pane_id) {
            shell.run_ok(
                "herdr",
                &["pane".into(), "close".into(), pane.pane_id.clone()],
            )?;
        }
    }
    if let Some(workspace) = workspace {
        shell.run_ok(
            "herdr",
            &["workspace".into(), "close".into(), workspace.into()],
        )?;
    }
    Ok(())
}

/// The loss rule, once more, after the repository's `teardown`. Teardown runs after the
/// callers' check, so it can leave a commit or content that no ref holds. The checkout, its
/// lock, its panes, and its item stay when it does.
fn recheck_loss(shell: &dyn Shell, tree: &Linked, evidence: &crate::loss::Evidence) -> Result<()> {
    // The commit the checkout holds now. The teardown can commit, and can detach HEAD, so the
    // branch tip the callers assessed is not always the tip now.
    let tip = head_sha(shell, &tree.path)?;
    let loss = evidence.assess(shell, &tip, Some(&tree.path))?;
    if !loss.loses_nothing() {
        return Err(refuse(&format!(
            "{}: the teardown left unsaved work ({}); the checkout, its lock, and its panes stay",
            tree.path,
            loss.detail()
        )));
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
    shell.run_ok(
        "herdr",
        &["workspace".into(), "close".into(), anchor.clone()],
    )?;
    std::fs::remove_file(&file)?;
    Ok(Some(anchor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::Output;

    #[derive(Debug, Default)]
    struct RemovalProbe {
        limit: std::sync::Mutex<Option<std::time::Duration>>,
    }

    impl Shell for RemovalProbe {
        fn run(&self, program: &str, args: &[String]) -> Result<Output> {
            if program == "herdr" {
                return Ok(Output {
                    status: 0,
                    stdout: r#"{"result":{"panes":[],"worktrees":[]}}"#.into(),
                    stderr: String::new(),
                });
            }
            let mut command = std::process::Command::new(program);
            for (key, _) in std::env::vars_os() {
                if key.to_string_lossy().starts_with("GIT_") {
                    command.env_remove(key);
                }
            }
            let out = command.args(args).output()?;
            Ok(Output {
                status: out.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            })
        }

        fn run_bounded(
            &self,
            program: &str,
            args: &[String],
            limit: std::time::Duration,
        ) -> Result<Output> {
            if args.get(2).is_some_and(|a| a == "worktree")
                && args.get(3).is_some_and(|a| a == "remove")
            {
                *self.limit.lock().unwrap() = Some(limit);
                return Ok(Output {
                    status: 1,
                    stdout: String::new(),
                    stderr: "stop here".into(),
                });
            }
            self.run(program, args)
        }
    }

    #[test]
    fn removal_gets_the_setup_limit_not_the_routine_limit() {
        let shell = RemovalProbe::default();
        let checkout = std::env::temp_dir().join(crate::ledger::new_id());
        std::fs::create_dir_all(&checkout).unwrap();
        let repo = checkout.display().to_string();
        let path = format!("{repo}/.worktrees/x");
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
            vec!["worktree", "add", "-q", "-b", "x", &path],
        ] {
            git_ok(&shell, &checkout, &args).unwrap();
        }
        let ledger = Ledger::new(checkout.join("state"));
        let evidence = crate::loss::Evidence::default_only(&repo);
        let error = with_lifecycle(&shell, &ledger, &checkout, |guard| {
            let tree = guard.discover(&shell, &path)?.unwrap();
            remove(
                &shell,
                &ledger,
                &tree,
                Removal {
                    guard,
                    workspace: None,
                    holders: &[],
                    caller_workspace: None,
                    evidence: &evidence,
                },
            )
        })
        .unwrap_err();
        assert!(error.to_string().contains("stop here"), "{error}");
        assert_eq!(
            *shell.limit.lock().unwrap(),
            Some(crate::budget::SETUP_LIMIT)
        );
        std::fs::remove_dir_all(checkout).unwrap();
    }
}
