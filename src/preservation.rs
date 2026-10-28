//! Local, non-destructive recovery snapshots. No recovery ref is pushed or expired here.

use std::fmt::Write as _;
use std::fs;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::shell::{Output, Shell};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("recovery refused: {0}")]
    Refused(#[from] Refusal),
    #[error(transparent)]
    Git(#[from] crate::error::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("the index contains unmerged entries")]
    ConflictedIndex,
    #[error("dirty submodule: {0}")]
    DirtySubmodule(PathBuf),
    #[error("nested repository: {0}")]
    NestedRepository(PathBuf),
    #[error("unreadable path: {0}")]
    UnreadablePath(PathBuf),
    #[error("Git cannot store this file type: {0}")]
    UnsupportedFileType(PathBuf),
    #[error("path cannot be transported byte-exactly: {0:?}")]
    UnsupportedPath(Vec<u8>),
    #[error("recovery ref already exists: {0}")]
    RefConflict(String),
    #[error("the live checkout changed during preservation")]
    CheckoutChanged,
    #[error("snapshot verification failed")]
    VerificationFailed,
    #[error("Git is redirected outside the requested checkout")]
    Redirected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Head {
    Branch { name: String, commit: String },
    Detached { commit: String },
    Unborn { branch: String },
}

impl Head {
    fn commit(&self) -> Option<&str> {
        match self {
            Self::Branch { commit, .. } | Self::Detached { commit } => Some(commit),
            Self::Unborn { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub ref_name: String,
    pub commit: String,
    pub head: Head,
    pub index_tree: String,
    pub worktree_tree: String,
    /// Working-tree blobs contain raw, unfiltered bytes. Restore without smudge filters.
    pub raw_worktree_bytes: bool,
    pub created_ms: u64,
    /// Ignored files stay in the live checkout and are explicitly excluded.
    pub ignored_files_captured: bool,
}

/// The commit message is the durable record; both trees are reachable through parents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    id: String,
    ref_name: String,
    head: Head,
    index_tree: String,
    worktree_tree: String,
    raw_worktree_bytes: bool,
    created_ms: u64,
    ignored_files_captured: bool,
}

impl Snapshot {
    fn record(&self) -> Record {
        Record {
            id: self.id.clone(),
            ref_name: self.ref_name.clone(),
            head: self.head.clone(),
            index_tree: self.index_tree.clone(),
            worktree_tree: self.worktree_tree.clone(),
            raw_worktree_bytes: self.raw_worktree_bytes,
            created_ms: self.created_ms,
            ignored_files_captured: self.ignored_files_captured,
        }
    }
}

/// Owns all disposable indexes, outside the checkout. The live index is never opened for writing.
#[derive(Debug)]
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self> {
        let dir = std::env::temp_dir().join(format!("dogsled-recovery-{}", random_id()?));
        fs::DirBuilder::new().mode(0o700).create(&dir)?;
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn random_id() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    }))
}

/// Environment changes belong to the child only, never to this multithreaded process.
fn git(shell: &dyn Shell, checkout: &Path, index: Option<&Path>, args: &[&str]) -> Result<Output> {
    git_input(shell, checkout, index, args, None)
}

fn git_input(
    shell: &dyn Shell,
    checkout: &Path,
    index: Option<&Path>,
    args: &[&str],
    input: Option<&[u8]>,
) -> Result<Output> {
    // Hooks export repository-local routing. The explicit checkout, not the caller's
    // repository or index, owns every Git operation in this module.
    let mut words = Vec::new();
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_PREFIX",
        "GIT_SHALLOW_FILE",
        "GIT_NAMESPACE",
        "GIT_CONFIG",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_GLOBAL",
        "GIT_CONFIG_SYSTEM",
    ] {
        words.extend(["-u".into(), key.into()]);
    }
    for (key, _) in std::env::vars_os() {
        if key.as_encoded_bytes().starts_with(b"GIT_CONFIG_") {
            words.extend(["-u".into(), key.to_string_lossy().into_owned()]);
        }
    }
    words.push("GIT_OPTIONAL_LOCKS=0".into());
    if let Some(index) = index {
        let index = index
            .to_str()
            .ok_or_else(|| Refusal::UnsupportedPath(index.as_os_str().as_bytes().to_vec()))?;
        words.push(format!("GIT_INDEX_FILE={index}"));
    }
    words.extend([
        "git".into(),
        "-C".into(),
        checkout
            .to_str()
            .ok_or_else(|| Refusal::UnsupportedPath(checkout.as_os_str().as_bytes().to_vec()))?
            .into(),
        "-c".into(),
        "core.fsmonitor=false".into(),
    ]);
    words.extend(args.iter().map(ToString::to_string));
    Ok(if let Some(input) = input {
        shell.run_input_bounded("env", &words, input, shell.budget().remaining("git")?)?
    } else {
        shell.run("env", &words)?
    })
}

fn checked(
    shell: &dyn Shell,
    checkout: &Path,
    index: Option<&Path>,
    args: &[&str],
) -> Result<String> {
    let out = git(shell, checkout, index, args)?;
    if !out.ok() {
        return Err(crate::error::Error::Command {
            program: "git".into(),
            args: args.join(" "),
            status: out.status.to_string(),
            stderr: out.stderr,
        }
        .into());
    }
    Ok(out.stdout)
}

fn head(shell: &dyn Shell, checkout: &Path) -> Result<Head> {
    let branch = git(shell, checkout, None, &["symbolic-ref", "-q", "HEAD"])?;
    let commit = git(shell, checkout, None, &["rev-parse", "--verify", "HEAD"])?;
    if branch.ok() {
        let name = branch.stdout.trim().to_owned();
        if commit.ok() {
            Ok(Head::Branch {
                name,
                commit: commit.stdout.trim().into(),
            })
        } else {
            // Only a genuinely absent branch is unborn; broken refs must fail closed.
            let exists = git(
                shell,
                checkout,
                None,
                &["show-ref", "--verify", "--quiet", &name],
            )?;
            if exists.status != 1 {
                return Err(Refusal::VerificationFailed.into());
            }
            Ok(Head::Unborn { branch: name })
        }
    } else if branch.status == 1 && commit.ok() {
        Ok(Head::Detached {
            commit: commit.stdout.trim().into(),
        })
    } else {
        Err(Refusal::VerificationFailed.into())
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Observation {
    head: Head,
    status: String,
    index: Option<Vec<u8>>,
}

fn observe(shell: &dyn Shell, checkout: &Path) -> Result<Observation> {
    let index_path = checked(
        shell,
        checkout,
        None,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    )?;
    let index = match fs::read(index_path.trim()) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err(Refusal::UnreadablePath(PathBuf::from(index_path.trim())).into()),
    };
    Ok(Observation {
        head: head(shell, checkout)?,
        status: checked(
            shell,
            checkout,
            None,
            &[
                "-c",
                "core.quotePath=true",
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
                "--ignore-submodules=none",
            ],
        )?,
        index,
    })
}

fn index_tree(
    shell: &dyn Shell,
    checkout: &Path,
    scratch: &Scratch,
    bytes: Option<&[u8]>,
) -> Result<String> {
    let index = scratch.0.join("staged");
    if let Some(bytes) = bytes {
        fs::write(&index, bytes)?;
    } else {
        checked(shell, checkout, Some(&index), &["read-tree", "--empty"])?;
    }
    Ok(checked(shell, checkout, Some(&index), &["write-tree"])?
        .trim()
        .into())
}

/// Decode Git's core.quotePath=true output without passing filenames through UTF-8.
fn unquote_path(quoted: &str) -> Result<Vec<u8>> {
    let bytes = quoted.as_bytes();
    let unsupported = || Refusal::UnsupportedPath(bytes.to_vec());
    if !bytes.is_ascii() || bytes.contains(&0) {
        return Err(unsupported().into());
    }
    if !bytes.starts_with(b"\"") {
        return Ok(bytes.to_vec());
    }
    let escaped = bytes
        .strip_prefix(b"\"")
        .and_then(|bytes| bytes.strip_suffix(b"\""))
        .ok_or_else(unsupported)?;
    let mut decoded = Vec::new();
    let mut offset = 0;
    while offset < escaped.len() {
        let byte = escaped[offset];
        offset += 1;
        if byte != b'\\' {
            if byte == b'"' {
                return Err(unsupported().into());
            }
            decoded.push(byte);
            continue;
        }
        let byte = *escaped.get(offset).ok_or_else(unsupported)?;
        offset += 1;
        decoded.push(match byte {
            b'a' => 7,
            b'b' => 8,
            b't' => b'\t',
            b'n' => b'\n',
            b'v' => 11,
            b'f' => 12,
            b'r' => b'\r',
            b'"' | b'\\' => byte,
            b'0'..=b'3' => {
                let rest = escaped.get(offset..offset + 2).ok_or_else(unsupported)?;
                if !rest.iter().all(|digit| matches!(digit, b'0'..=b'7')) {
                    return Err(unsupported().into());
                }
                offset += 2;
                (byte - b'0') * 64 + (rest[0] - b'0') * 8 + (rest[1] - b'0')
            }
            _ => return Err(unsupported().into()),
        });
    }
    if decoded.contains(&0) {
        return Err(unsupported().into());
    }
    Ok(decoded)
}

fn removed_path(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

/// Never traverse a replaced directory, particularly a symlink out of the checkout.
fn worktree_metadata(checkout: &Path, path: &Path) -> Result<Option<fs::Metadata>> {
    let mut ancestor = checkout.to_path_buf();
    if let Some(parent) = path.parent() {
        for component in parent.components() {
            ancestor.push(component);
            match fs::symlink_metadata(&ancestor) {
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => return Ok(None),
                Err(error) if removed_path(&error) => return Ok(None),
                Err(error) => return Err(error.into()),
            }
        }
    }
    match fs::symlink_metadata(checkout.join(path)) {
        Ok(meta) => Ok(Some(meta)),
        Err(error) if removed_path(&error) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn worktree_tree(
    shell: &dyn Shell,
    checkout: &Path,
    scratch: &Scratch,
    staged: &str,
) -> Result<String> {
    let index = scratch.0.join("working");
    checked(
        shell,
        checkout,
        Some(&index),
        &["-c", "core.sparseCheckout=false", "read-tree", staged],
    )?;
    let paths = checked(
        shell,
        checkout,
        Some(&index),
        &[
            "-c",
            "core.quotePath=true",
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?;
    let mut removals = String::new();
    let mut entries = String::new();
    for quoted in paths.lines().filter(|path| !path.is_empty()) {
        let bytes = unquote_path(quoted)?;
        let path = Path::new(std::ffi::OsStr::from_bytes(&bytes));
        let absolute = checkout.join(path);
        let meta = worktree_metadata(checkout, path)?;
        let Some(meta) = meta else {
            let _ = writeln!(removals, "0 {}\t{quoted}", "0".repeat(staged.len()));
            continue;
        };
        if meta.is_dir() {
            // Keep clean submodule gitlinks, but remove files replaced by directories.
            if !absolute.join(".git").exists() {
                let _ = writeln!(removals, "0 {}\t{quoted}", "0".repeat(staged.len()));
            }
            continue;
        }
        let (mode, content) = if meta.is_symlink() {
            let target = fs::read_link(&absolute)?;
            ("120000", target.as_os_str().as_bytes().to_vec())
        } else if meta.is_file() {
            (
                if meta.permissions().mode() & 0o111 == 0 {
                    "100644"
                } else {
                    "100755"
                },
                fs::read(&absolute)?,
            )
        } else {
            return Err(Refusal::UnsupportedFileType(absolute).into());
        };
        let args = ["hash-object", "-w", "--no-filters", "--stdin"];
        let out = git_input(shell, checkout, None, &args, Some(&content))?;
        let hash = crate::shell::checked("git", &args.map(String::from), out)?.stdout;
        let _ = writeln!(entries, "{mode} {}\t{quoted}", hash.trim());
    }
    // Remove directory/file conflicts before adding their replacements.
    removals.push_str(&entries);
    let entries = removals;
    let args = ["update-index", "--index-info"];
    let out = git_input(
        shell,
        checkout,
        Some(&index),
        &args,
        Some(entries.as_bytes()),
    )?;
    if !out.ok() {
        return Err(crate::error::Error::Command {
            program: "git".into(),
            args: args.join(" "),
            status: out.status.to_string(),
            stderr: out.stderr,
        }
        .into());
    }
    Ok(checked(shell, checkout, Some(&index), &["write-tree"])?
        .trim()
        .into())
}

/// Verify Git's resolved routing against the checkout's own on-disk metadata.
fn check_routing(shell: &dyn Shell, checkout: &Path) -> Result<()> {
    let root = checkout.canonicalize()?;
    let dot_git = root.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git.canonicalize()?
    } else {
        let content = fs::read_to_string(&dot_git)?;
        let path = content
            .trim()
            .strip_prefix("gitdir: ")
            .ok_or(Refusal::Redirected)?;
        root.join(path).canonicalize()?
    };
    let common_dir = match fs::read_to_string(git_dir.join("commondir")) {
        Ok(path) => git_dir.join(path.trim()).canonicalize()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => git_dir.clone(),
        Err(error) => return Err(error.into()),
    };
    for (arg, expected) in [
        ("--show-toplevel", &root),
        ("--git-dir", &git_dir),
        ("--git-common-dir", &common_dir),
    ] {
        let out = git(
            shell,
            checkout,
            None,
            &["rev-parse", "--path-format=absolute", arg],
        )?;
        if !out.ok() || Path::new(out.stdout.trim()).canonicalize().ok().as_ref() != Some(expected)
        {
            return Err(Refusal::Redirected.into());
        }
    }
    Ok(())
}

/// Reject content Git would omit or cannot faithfully store before capture.
fn preflight(shell: &dyn Shell, checkout: &Path) -> Result<()> {
    check_routing(shell, checkout)?;
    if !checked(shell, checkout, None, &["ls-files", "--unmerged", "-z"])?.is_empty() {
        return Err(Refusal::ConflictedIndex.into());
    }
    let entries = checked(
        shell,
        checkout,
        None,
        &["-c", "core.quotePath=true", "ls-files", "--stage"],
    )?;
    let mut submodules = Vec::new();
    for entry in entries.lines().filter(|entry| entry.starts_with("160000 ")) {
        let Some((info, path)) = entry.split_once('\t') else {
            return Err(Refusal::VerificationFailed.into());
        };
        let bytes = unquote_path(path)?;
        let relative = Path::new(std::ffi::OsStr::from_bytes(&bytes));
        if worktree_metadata(checkout, relative)?.is_none() {
            continue;
        }
        let path = checkout.join(relative);
        if path.join(".git").exists() {
            let current = head(shell, &path)?;
            let oid = info.split_whitespace().nth(1);
            if current.commit() != oid
                || !checked(
                    shell,
                    &path,
                    None,
                    &[
                        "status",
                        "--porcelain=v1",
                        "--untracked-files=all",
                        "--ignore-submodules=none",
                    ],
                )?
                .is_empty()
            {
                return Err(Refusal::DirtySubmodule(path).into());
            }
        } else if path.is_dir() && fs::read_dir(&path)?.next().is_some() {
            return Err(Refusal::DirtySubmodule(path).into());
        }
        submodules.push(path);
    }
    walk(checkout, checkout, &submodules)
}

fn walk(root: &Path, dir: &Path, submodules: &[PathBuf]) -> Result<()> {
    let entries = fs::read_dir(dir).map_err(|_| Refusal::UnreadablePath(dir.into()))?;
    for entry in entries {
        let entry = entry.map_err(|_| Refusal::UnreadablePath(dir.into()))?;
        let path = entry.path();
        if dir == root && entry.file_name() == ".git" {
            continue;
        }
        if submodules.contains(&path) {
            continue;
        }
        if entry.file_name() == ".git" {
            return Err(Refusal::NestedRepository(dir.into()).into());
        }
        let meta =
            fs::symlink_metadata(&path).map_err(|_| Refusal::UnreadablePath(path.clone()))?;
        let kind = meta.file_type();
        if kind.is_symlink() {
            fs::read_link(&path).map_err(|_| Refusal::UnreadablePath(path.clone()))?;
        } else if kind.is_dir() {
            if meta.permissions().mode() & 0o555 == 0 {
                return Err(Refusal::UnreadablePath(path).into());
            }
            walk(root, &path, submodules)?;
        } else if kind.is_file() {
            if meta.permissions().mode() & 0o444 == 0 {
                return Err(Refusal::UnreadablePath(path).into());
            }
            let mut file =
                fs::File::open(&path).map_err(|_| Refusal::UnreadablePath(path.clone()))?;
            // Read through the file: an open alone does not prove that its contents are readable.
            std::io::copy(&mut file, &mut std::io::sink())
                .map_err(|_| Refusal::UnreadablePath(path.clone()))?;
        } else {
            return Err(Refusal::UnsupportedFileType(path).into());
        }
    }
    Ok(())
}

fn commit_tree(
    shell: &dyn Shell,
    checkout: &Path,
    tree: &str,
    parents: &[&str],
    message: &str,
) -> Result<String> {
    let mut args = vec![
        "-c",
        "user.name=Dogsled recovery",
        "-c",
        "user.email=recovery@dogsled.invalid",
        "commit-tree",
        tree,
    ];
    for parent in parents {
        args.extend(["-p", parent]);
    }
    args.extend(["-m", message]);
    Ok(checked(shell, checkout, None, &args)?.trim().into())
}

fn pin(shell: &dyn Shell, checkout: &Path, snapshot: &Snapshot) -> Result<()> {
    let zero = "0".repeat(snapshot.commit.len());
    let out = git(
        shell,
        checkout,
        None,
        &[
            "update-ref",
            "--no-deref",
            &snapshot.ref_name,
            &snapshot.commit,
            &zero,
        ],
    )?;
    if out.ok() {
        return Ok(());
    }
    if git(
        shell,
        checkout,
        None,
        &["show-ref", "--verify", "--quiet", &snapshot.ref_name],
    )?
    .ok()
    {
        return Err(Refusal::RefConflict(snapshot.ref_name.clone()).into());
    }
    Err(crate::error::Error::Invalid(format!("cannot pin recovery ref: {}", out.stderr)).into())
}

/// Capture and pin raw, unfiltered work without clearing anything.
/// Restore must write the captured bytes directly, without Git smudge filters.
/// Failures leave all live content intact.
pub fn snapshot(shell: &dyn Shell, checkout: &Path) -> Result<Snapshot> {
    preflight(shell, checkout)?;
    let before = observe(shell, checkout)?;
    let scratch = Scratch::new()?;
    let staged = index_tree(shell, checkout, &scratch, before.index.as_deref())?;
    let working = worktree_tree(shell, checkout, &scratch, &staged)?;
    let id = random_id()?;
    let mut snapshot = Snapshot {
        ref_name: format!("refs/dogsled/recovery/{id}"),
        id,
        commit: String::new(),
        head: before.head.clone(),
        index_tree: staged,
        worktree_tree: working,
        raw_worktree_bytes: true,
        created_ms: crate::ledger::now_ms(),
        ignored_files_captured: false,
    };
    let parents: Vec<_> = snapshot.head.commit().into_iter().collect();
    let index_commit = commit_tree(
        shell,
        checkout,
        &snapshot.index_tree,
        &parents,
        "Dogsled recovery staged index",
    )?;
    let mut parents = parents;
    parents.push(&index_commit);
    snapshot.commit = commit_tree(
        shell,
        checkout,
        &snapshot.worktree_tree,
        &parents,
        &serde_json::to_string(&snapshot.record())?,
    )?;
    pin(shell, checkout, &snapshot)?;
    verify(shell, checkout, &snapshot)?;
    if before != observe(shell, checkout)? {
        return Err(Refusal::CheckoutChanged.into());
    }
    Ok(snapshot)
}

/// Verify the pinned record, both trees, and the unchanged live checkout.
pub fn verify(shell: &dyn Shell, checkout: &Path, snapshot: &Snapshot) -> Result<()> {
    preflight(shell, checkout)?;
    let before = observe(shell, checkout)?;
    let pinned = checked(
        shell,
        checkout,
        None,
        &["rev-parse", "--verify", &snapshot.ref_name],
    )?;
    if pinned.trim() != snapshot.commit {
        return Err(Refusal::VerificationFailed.into());
    }
    let message = checked(
        shell,
        checkout,
        None,
        &["show", "-s", "--format=%B", &snapshot.commit],
    )?;
    let record: Record = serde_json::from_str(message.trim())?;
    let tree = checked(
        shell,
        checkout,
        None,
        &["rev-parse", &format!("{}^{{tree}}", snapshot.commit)],
    )?;
    let staged_parent = if snapshot.head.commit().is_some() {
        "^2"
    } else {
        "^1"
    };
    let staged = checked(
        shell,
        checkout,
        None,
        &[
            "rev-parse",
            &format!("{}{staged_parent}^{{tree}}", snapshot.commit),
        ],
    )?;
    let scratch = Scratch::new()?;
    let current_index = index_tree(shell, checkout, &scratch, before.index.as_deref())?;
    let current_working = worktree_tree(shell, checkout, &scratch, &current_index)?;
    if record != snapshot.record()
        || !snapshot.raw_worktree_bytes
        || before.head != snapshot.head
        || tree.trim() != snapshot.worktree_tree
        || staged.trim() != snapshot.index_tree
        || current_index != snapshot.index_tree
        || current_working != snapshot.worktree_tree
    {
        return Err(Refusal::VerificationFailed.into());
    }
    if before != observe(shell, checkout)? {
        return Err(Refusal::CheckoutChanged.into());
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::shell::System;
    use std::os::unix::fs::{PermissionsExt, symlink};

    struct Repo {
        scratch: Scratch,
        path: PathBuf,
    }

    impl Repo {
        fn new(committed: bool) -> Self {
            Self::new_in(Scratch::new().unwrap(), committed)
        }

        fn new_in(scratch: Scratch, committed: bool) -> Self {
            let path = scratch.0.join("repo");
            fs::create_dir(&path).unwrap();
            let repo = Self { scratch, path };
            repo.git(&["init", "-b", "main"]);
            repo.git(&["config", "user.name", "Test"]);
            repo.git(&["config", "user.email", "test@example.invalid"]);
            repo.git(&["config", "core.autocrlf", "false"]);
            if committed {
                fs::write(repo.path.join("file"), b"base\n").unwrap();
                repo.git(&["add", "."]);
                repo.git(&["commit", "-m", "base"]);
            }
            repo
        }

        fn git(&self, args: &[&str]) -> String {
            checked(&System::default(), &self.path, None, args).unwrap()
        }

        fn round_trip(&self) -> Snapshot {
            let shell = System::default();
            let before = observe(&shell, &self.path).unwrap();
            let saved = snapshot(&shell, &self.path).unwrap();
            assert_eq!(before, observe(&shell, &self.path).unwrap());
            verify(&shell, &self.path, &saved).unwrap();
            let restored = self.scratch.0.join("restored");
            self.git(&[
                "worktree",
                "add",
                "--detach",
                restored.to_str().unwrap(),
                &saved.commit,
            ]);
            // P2 must restore raw blobs without checkout's smudge/EOL conversion.
            for entry in self
                .git(&["ls-tree", "-r", "-z", &saved.commit])
                .split('\0')
                .filter(|entry| !entry.is_empty())
            {
                let (info, path) = entry.split_once('\t').unwrap();
                if info.starts_with("100") {
                    let oid = info.split_whitespace().nth(2).unwrap();
                    let bytes = self.git(&["cat-file", "blob", oid]);
                    fs::write(restored.join(path), bytes.as_bytes()).unwrap();
                }
            }
            // Restore only inside the NEW checkout: original HEAD, working tree, then staged tree.
            if let Some(head) = saved.head.commit() {
                checked(
                    &shell,
                    &restored,
                    None,
                    &["update-ref", "--no-deref", "HEAD", head],
                )
                .unwrap();
            } else if let Head::Unborn { branch } = &saved.head {
                checked(
                    &shell,
                    &restored,
                    None,
                    &["symbolic-ref", "HEAD", &format!("{branch}-restored")],
                )
                .unwrap();
            }
            checked(&shell, &restored, None, &["read-tree", &saved.index_tree]).unwrap();
            let scratch = Scratch::new().unwrap();
            let restored_observation = observe(&shell, &restored).unwrap();
            assert_eq!(before.status, restored_observation.status);
            assert_eq!(
                saved.index_tree,
                index_tree(
                    &shell,
                    &restored,
                    &scratch,
                    restored_observation.index.as_deref()
                )
                .unwrap()
            );
            assert_eq!(
                saved.worktree_tree,
                worktree_tree(&shell, &restored, &scratch, &saved.index_tree).unwrap()
            );
            let original = tree_files(&self.path);
            let restored_files = tree_files(&restored);
            assert_eq!(original, restored_files);
            saved
        }
    }

    fn tree_files(root: &Path) -> std::collections::BTreeMap<PathBuf, (u32, Vec<u8>)> {
        fn collect(
            root: &Path,
            dir: &Path,
            out: &mut std::collections::BTreeMap<PathBuf, (u32, Vec<u8>)>,
        ) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.file_name().unwrap() == ".git" {
                    continue;
                }
                let meta = fs::symlink_metadata(&path).unwrap();
                if meta.is_dir() {
                    collect(root, &path, out);
                } else {
                    let bytes = if meta.is_symlink() {
                        fs::read_link(&path)
                            .unwrap()
                            .as_os_str()
                            .as_encoded_bytes()
                            .to_vec()
                    } else {
                        fs::read(&path).unwrap()
                    };
                    out.insert(
                        path.strip_prefix(root).unwrap().into(),
                        (meta.permissions().mode(), bytes),
                    );
                }
            }
        }
        let mut out = std::collections::BTreeMap::new();
        collect(root, root, &mut out);
        out
    }

    #[test]
    fn git_quoted_paths_decode_byte_exactly() {
        for (quoted, expected) in [
            (r#""name-\377""#, b"name-\xff".as_slice()),
            (r#""caf\303\251""#, "café".as_bytes()),
            (r#""a\tb\nc\"d\\e""#, b"a\tb\nc\"d\\e".as_slice()),
            ("plain name", b"plain name".as_slice()),
            (r#""\a\b\v\f\r""#, b"\x07\x08\x0b\x0c\r".as_slice()),
        ] {
            assert_eq!(unquote_path(quoted).unwrap(), expected, "{quoted}");
        }
    }

    #[test]
    fn malformed_quoted_paths_refuse_instead_of_changing_names() {
        for quoted in [
            r#""\400""#,
            r#""\37""#,
            r#""\q""#,
            r#""\000""#,
            "\"unterminated",
            "café",
            "bad\0name",
        ] {
            assert!(matches!(
                unquote_path(quoted),
                Err(Error::Refused(Refusal::UnsupportedPath(_)))
            ));
        }
    }

    #[test]
    fn non_utf8_and_accented_names_round_trip_byte_exactly() {
        let repo = Repo::new(true);
        repo.git(&["config", "core.quotePath", "false"]);
        let mut names: Vec<&[u8]> = vec!["café".as_bytes()];
        let non_utf8 = b"name-\xff";
        match fs::write(
            repo.path.join(std::ffi::OsStr::from_bytes(non_utf8)),
            b"exact content\n",
        ) {
            Ok(()) => names.push(non_utf8),
            Err(error) => {
                assert!(
                    !cfg!(target_os = "linux")
                        && (error.kind() == std::io::ErrorKind::InvalidFilename
                            || error.raw_os_error() == Some(nix::errno::Errno::EILSEQ as i32)),
                    "creating invalid-UTF-8 filename: {error}"
                );
                // APFS cannot represent this sub-case. Skip only invalid-UTF-8
                // names, never the accented-name or other byte-exact assertions.
                // Linux must exercise the invalid-UTF-8 round trip below.
                eprintln!("skipping invalid-UTF-8 filename sub-case: {error}");
            }
        }
        for name in &names {
            fs::write(
                repo.path.join(std::ffi::OsStr::from_bytes(name)),
                b"exact content\n",
            )
            .unwrap();
        }
        repo.git(&["add", "."]);
        for name in &names {
            fs::write(
                repo.path.join(std::ffi::OsStr::from_bytes(name)),
                b"working content\n",
            )
            .unwrap();
        }
        let untracked = std::ffi::OsStr::from_bytes(if names.contains(&non_utf8.as_slice()) {
            b"untracked-\xff\t\n\"\\"
        } else {
            b"untracked-\t\n\"\\"
        });
        fs::write(repo.path.join(untracked), b"untracked bytes\0\xff").unwrap();
        let saved = snapshot(&System::default(), &repo.path).unwrap();
        verify(&System::default(), &repo.path, &saved).unwrap();
        let restored = repo.scratch.0.join("restored");
        repo.git(&[
            "worktree",
            "add",
            "--detach",
            restored.to_str().unwrap(),
            &saved.commit,
        ]);
        assert_eq!(tree_files(&repo.path), tree_files(&restored));
        for name in &names {
            assert_eq!(
                fs::read(restored.join(std::ffi::OsStr::from_bytes(name))).unwrap(),
                b"working content\n"
            );
        }
        // A quoted tracked path must also be removable without renaming it.
        for name in &names {
            fs::remove_file(repo.path.join(std::ffi::OsStr::from_bytes(name))).unwrap();
        }
        fs::remove_file(repo.path.join(untracked)).unwrap();
        let removed = snapshot(&System::default(), &repo.path).unwrap();
        assert_eq!(
            repo.git(&["ls-tree", "--name-only", &removed.commit]),
            "file\n"
        );
    }

    #[test]
    fn tracked_directory_replaced_by_regular_file_round_trips() {
        let repo = Repo::new(true);
        fs::create_dir(repo.path.join("dir")).unwrap();
        fs::write(repo.path.join("dir/file"), b"old content").unwrap();
        repo.git(&["add", "."]);
        repo.git(&["commit", "-m", "tracked directory"]);
        fs::remove_dir_all(repo.path.join("dir")).unwrap();
        fs::write(repo.path.join("dir"), b"replacement file").unwrap();
        let saved = repo.round_trip();
        let restored = repo.scratch.0.join("restored");
        assert_eq!(fs::read(restored.join("dir")).unwrap(), b"replacement file");
        assert!(!restored.join("dir/file").exists());
        assert!(
            repo.git(&["ls-tree", &saved.commit, "dir"])
                .starts_with("100644 blob ")
        );
    }

    #[test]
    fn tracked_directory_replaced_by_symlink_never_captures_outside_content() {
        let repo = Repo::new(true);
        fs::create_dir(repo.path.join("dir")).unwrap();
        fs::write(repo.path.join("dir/file"), b"old content").unwrap();
        repo.git(&["add", "."]);
        repo.git(&["commit", "-m", "tracked directory"]);
        let outside = repo.scratch.0.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("file"), b"outside secret").unwrap();
        fs::remove_dir_all(repo.path.join("dir")).unwrap();
        symlink(&outside, repo.path.join("dir")).unwrap();
        let saved = repo.round_trip();
        let entry = repo.git(&["ls-tree", "-r", &saved.commit, "dir"]);
        assert!(entry.starts_with("120000 blob "), "{entry}");
        assert_eq!(entry.lines().count(), 1);
        assert_eq!(repo.git(&["ls-tree", "-r", &saved.commit, "dir/file"]), "");
        assert_eq!(
            fs::read_link(repo.scratch.0.join("restored/dir")).unwrap(),
            outside
        );
        fs::write(outside.join("file"), b"changed outside secret").unwrap();
        verify(&System::default(), &repo.path, &saved).unwrap();
    }

    #[test]
    fn modified_tracked_file() {
        let repo = Repo::new(true);
        fs::write(repo.path.join("file"), b"working\0bytes").unwrap();
        repo.round_trip();
    }

    #[test]
    fn crlf_attributes_preserve_raw_bytes() {
        let repo = Repo::new(true);
        fs::write(repo.path.join(".gitattributes"), b"* text eol=lf\n").unwrap();
        let bytes = b"working\r\nbytes\r\n";
        fs::write(repo.path.join("file"), bytes).unwrap();
        let saved = repo.round_trip();
        assert!(saved.raw_worktree_bytes);
        assert_eq!(
            repo.git(&["cat-file", "blob", &format!("{}:file", saved.commit)])
                .as_bytes(),
            bytes
        );
        fs::write(repo.path.join("file"), b"working\nbytes\n").unwrap();
        assert!(matches!(
            verify(&System::default(), &repo.path, &saved),
            Err(Error::Refused(Refusal::VerificationFailed))
        ));
    }

    #[test]
    fn clean_filter_preserves_raw_bytes() {
        let repo = Repo::new(true);
        repo.git(&["config", "filter.x.clean", "tr a-z A-Z"]);
        fs::write(repo.path.join(".gitattributes"), b"file filter=x\n").unwrap();
        fs::write(repo.path.join("file"), b"staged lowercase\n").unwrap();
        repo.git(&["add", "file"]);
        fs::write(repo.path.join("file"), b"working lowercase\n").unwrap();
        let saved = repo.round_trip();
        assert_eq!(
            repo.git(&["cat-file", "blob", &format!("{}:file", saved.commit)]),
            "working lowercase\n"
        );
        assert_eq!(
            repo.git(&["cat-file", "blob", &format!("{}:file", saved.index_tree)]),
            "STAGED LOWERCASE\n"
        );
    }

    #[test]
    fn autocrlf_preserves_raw_bytes() {
        let repo = Repo::new(true);
        repo.git(&["config", "core.autocrlf", "true"]);
        fs::write(repo.path.join("file"), b"working\r\nbytes\r\n").unwrap();
        let saved = repo.round_trip();
        assert_eq!(
            repo.git(&["cat-file", "blob", &format!("{}:file", saved.commit)])
                .as_bytes(),
            b"working\r\nbytes\r\n"
        );
    }

    #[derive(Debug)]
    struct ConfigInjected {
        other: PathBuf,
    }

    impl ConfigInjected {
        fn args(&self, program: &str, args: &[String]) -> Vec<String> {
            let mut words = vec![
                "GIT_CONFIG_COUNT=1".into(),
                "GIT_CONFIG_KEY_0=core.worktree".into(),
                format!("GIT_CONFIG_VALUE_0={}", self.other.display()),
                program.into(),
            ];
            words.extend_from_slice(args);
            words
        }
    }

    impl Shell for ConfigInjected {
        fn run(&self, program: &str, args: &[String]) -> crate::error::Result<Output> {
            System::default().run("env", &self.args(program, args))
        }

        fn run_input_bounded(
            &self,
            program: &str,
            args: &[String],
            input: &[u8],
            timeout: std::time::Duration,
        ) -> crate::error::Result<Output> {
            System::default().run_input_bounded("env", &self.args(program, args), input, timeout)
        }
    }

    #[test]
    fn config_environment_cannot_redirect_capture() {
        let repo = Repo::new(true);
        let other = Repo::new(true);
        fs::write(repo.path.join("file"), b"requested checkout\n").unwrap();
        fs::write(other.path.join("file"), b"wrong checkout\n").unwrap();
        let shell = ConfigInjected {
            other: other.path.clone(),
        };
        // Prove the injection was removed, even on Git versions that resolve
        // their worktree before consulting environment-injected configuration.
        assert!(
            !git(
                &shell,
                &repo.path,
                None,
                &["config", "--get", "core.worktree"]
            )
            .unwrap()
            .ok()
        );
        let saved = snapshot(&shell, &repo.path).unwrap();
        verify(&shell, &repo.path, &saved).unwrap();
        assert_eq!(
            repo.git(&["cat-file", "blob", &format!("{}:file", saved.commit)]),
            "requested checkout\n"
        );
        assert_eq!(
            fs::read(other.path.join("file")).unwrap(),
            b"wrong checkout\n"
        );
    }

    #[test]
    fn repository_core_worktree_redirect_refuses() {
        let repo = Repo::new(true);
        let other = Repo::new(true);
        repo.git(&["config", "core.worktree", other.path.to_str().unwrap()]);
        assert!(matches!(
            snapshot(&System::default(), &repo.path),
            Err(Error::Refused(Refusal::Redirected))
        ));
        assert_eq!(fs::read(repo.path.join("file")).unwrap(), b"base\n");
        assert_eq!(fs::read(other.path.join("file")).unwrap(), b"base\n");
    }

    #[test]
    fn staged_only_change() {
        let repo = Repo::new(true);
        fs::write(repo.path.join("file"), b"staged").unwrap();
        repo.git(&["add", "file"]);
        repo.round_trip();
    }

    #[test]
    fn staged_and_unstaged_same_file() {
        let repo = Repo::new(true);
        fs::write(repo.path.join("file"), b"staged").unwrap();
        repo.git(&["add", "file"]);
        fs::write(repo.path.join("file"), b"working").unwrap();
        repo.round_trip();
    }

    #[test]
    fn new_untracked_file() {
        let repo = Repo::new(true);
        fs::write(repo.path.join("new\nfile"), b"new").unwrap();
        repo.round_trip();
    }

    #[test]
    fn deleted_file() {
        let repo = Repo::new(true);
        fs::remove_file(repo.path.join("file")).unwrap();
        repo.round_trip();
    }

    #[test]
    fn renamed_file() {
        let repo = Repo::new(true);
        repo.git(&["mv", "file", "renamed"]);
        repo.round_trip();
    }

    #[test]
    fn executable_bit() {
        let repo = Repo::new(true);
        repo.git(&["config", "core.fileMode", "false"]);
        fs::set_permissions(repo.path.join("file"), fs::Permissions::from_mode(0o755)).unwrap();
        repo.round_trip();
    }

    #[test]
    fn symlink_round_trip() {
        let repo = Repo::new(true);
        symlink("missing-target", repo.path.join("link")).unwrap();
        repo.round_trip();
    }

    #[test]
    fn detached_head() {
        let repo = Repo::new(true);
        repo.git(&["checkout", "--detach"]);
        fs::write(repo.path.join("file"), b"detached").unwrap();
        assert!(matches!(repo.round_trip().head, Head::Detached { .. }));
    }

    #[test]
    fn unborn_head() {
        let repo = Repo::new(false);
        fs::write(repo.path.join("file"), b"staged").unwrap();
        repo.git(&["add", "."]);
        fs::write(repo.path.join("file"), b"working").unwrap();
        assert!(matches!(repo.round_trip().head, Head::Unborn { .. }));
    }

    #[test]
    fn empty_unborn_head() {
        let repo = Repo::new(false);
        repo.round_trip();
    }

    #[test]
    fn conflicted_index_refuses_without_mutation() {
        let repo = Repo::new(true);
        repo.git(&["checkout", "-b", "other"]);
        fs::write(repo.path.join("file"), b"other").unwrap();
        repo.git(&["commit", "-am", "other"]);
        repo.git(&["checkout", "main"]);
        fs::write(repo.path.join("file"), b"main").unwrap();
        repo.git(&["commit", "-am", "main"]);
        assert!(
            !git(&System::default(), &repo.path, None, &["merge", "other"])
                .unwrap()
                .ok()
        );
        let before = observe(&System::default(), &repo.path).unwrap();
        assert!(matches!(
            snapshot(&System::default(), &repo.path),
            Err(Error::Refused(Refusal::ConflictedIndex))
        ));
        assert_eq!(before, observe(&System::default(), &repo.path).unwrap());
    }

    #[test]
    fn nested_repository_refuses() {
        let repo = Repo::new(true);
        fs::create_dir(repo.path.join("nested")).unwrap();
        repo.git(&["-C", "nested", "init"]);
        assert!(matches!(
            snapshot(&System::default(), &repo.path),
            Err(Error::Refused(Refusal::NestedRepository(_)))
        ));
    }

    #[test]
    fn unreadable_file_refuses() {
        let repo = Repo::new(true);
        fs::set_permissions(repo.path.join("file"), fs::Permissions::from_mode(0o0)).unwrap();
        let result = snapshot(&System::default(), &repo.path);
        fs::set_permissions(repo.path.join("file"), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            result,
            Err(Error::Refused(Refusal::UnreadablePath(_)))
        ));
    }

    #[test]
    fn unsupported_socket_refuses() {
        // Keep the socket inside the checkout without macOS's long $TMPDIR.
        let scratch = Scratch(Path::new("/tmp").join(format!("ds-{}", random_id().unwrap())));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&scratch.0)
            .unwrap();
        let repo = Repo::new_in(scratch, true);
        let socket = repo.path.join("socket");
        assert!(socket.as_os_str().as_bytes().len() < 100);
        let _socket = std::os::unix::net::UnixListener::bind(socket).unwrap();
        assert!(matches!(
            snapshot(&System::default(), &repo.path),
            Err(Error::Refused(Refusal::UnsupportedFileType(_)))
        ));
    }

    #[test]
    fn unsupported_fifo_refuses() {
        let repo = Repo::new(true);
        let output = System::default()
            .run("mkfifo", &[repo.path.join("fifo").display().to_string()])
            .unwrap();
        assert!(output.ok());
        assert!(matches!(
            snapshot(&System::default(), &repo.path),
            Err(Error::Refused(Refusal::UnsupportedFileType(_)))
        ));
    }

    #[test]
    fn dirty_submodule_refuses() {
        let source = Repo::new(true);
        let repo = Repo::new(true);
        repo.git(&[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            source.path.to_str().unwrap(),
            "sub",
        ]);
        repo.git(&["commit", "-am", "submodule"]);
        fs::write(repo.path.join("sub/file"), b"dirty").unwrap();
        assert!(matches!(
            snapshot(&System::default(), &repo.path),
            Err(Error::Refused(Refusal::DirtySubmodule(_)))
        ));
    }

    #[test]
    fn cas_conflict_does_not_clobber() {
        let repo = Repo::new(true);
        let mut saved = snapshot(&System::default(), &repo.path).unwrap();
        let original = saved.commit.clone();
        saved.commit = repo.git(&["rev-parse", "HEAD"]).trim().into();
        assert!(matches!(
            pin(&System::default(), &repo.path, &saved),
            Err(Error::Refused(Refusal::RefConflict(_)))
        ));
        assert_eq!(repo.git(&["rev-parse", &saved.ref_name]).trim(), original);
    }

    #[test]
    fn ignored_files_are_excluded_and_recorded() {
        let repo = Repo::new(true);
        fs::write(repo.path.join(".gitignore"), b"ignored\n").unwrap();
        fs::write(repo.path.join("ignored"), b"private").unwrap();
        let saved = snapshot(&System::default(), &repo.path).unwrap();
        assert!(!saved.ignored_files_captured);
        assert!(
            !repo
                .git(&["ls-tree", "-r", &saved.commit])
                .contains("ignored")
        );
        assert_eq!(fs::read(repo.path.join("ignored")).unwrap(), b"private");
    }

    #[derive(Debug, Clone, Copy)]
    enum Injection {
        FailPin,
        ChangeWorking,
        ChangeHead,
        ChangeIndex,
    }

    #[derive(Debug)]
    struct Injected {
        path: PathBuf,
        injection: Injection,
    }

    impl Shell for Injected {
        fn run_input_bounded(
            &self,
            program: &str,
            args: &[String],
            input: &[u8],
            timeout: std::time::Duration,
        ) -> crate::error::Result<Output> {
            System::default().run_input_bounded(program, args, input, timeout)
        }

        fn run(&self, program: &str, args: &[String]) -> crate::error::Result<Output> {
            let pinning = args.iter().any(|arg| arg == "update-ref");
            if pinning && matches!(self.injection, Injection::FailPin) {
                return Ok(Output {
                    status: 1,
                    stdout: String::new(),
                    stderr: "injected write failure".into(),
                });
            }
            let out = System::default().run(program, args)?;
            if pinning && out.ok() {
                match self.injection {
                    Injection::ChangeWorking => fs::write(self.path.join("file"), b"concurrent")?,
                    Injection::ChangeHead => {
                        checked(
                            &System::default(),
                            &self.path,
                            None,
                            &["symbolic-ref", "HEAD", "refs/heads/concurrent"],
                        )
                        .unwrap();
                    }
                    Injection::ChangeIndex => {
                        checked(
                            &System::default(),
                            &self.path,
                            None,
                            &["update-index", "--assume-unchanged", "file"],
                        )
                        .unwrap();
                    }
                    Injection::FailPin => {}
                }
            }
            Ok(out)
        }
    }

    #[test]
    fn failed_write_leaves_live_content_and_index_byte_identical() {
        let repo = Repo::new(true);
        fs::write(repo.path.join("file"), b"staged").unwrap();
        repo.git(&["add", "file"]);
        fs::write(repo.path.join("file"), b"working").unwrap();
        fs::write(repo.path.join("untracked"), b"untracked").unwrap();
        let before = observe(&System::default(), &repo.path).unwrap();
        let files = tree_files(&repo.path);
        let shell = Injected {
            path: repo.path.clone(),
            injection: Injection::FailPin,
        };
        assert!(snapshot(&shell, &repo.path).is_err());
        assert_eq!(before, observe(&System::default(), &repo.path).unwrap());
        assert_eq!(files, tree_files(&repo.path));
        assert_eq!(repo.git(&["for-each-ref", "refs/dogsled/recovery/"]), "");
    }

    #[test]
    fn concurrent_checkout_changes_refuse_without_clearing() {
        for injection in [
            Injection::ChangeWorking,
            Injection::ChangeHead,
            Injection::ChangeIndex,
        ] {
            let repo = Repo::new(true);
            let shell = Injected {
                path: repo.path.clone(),
                injection,
            };
            assert!(snapshot(&shell, &repo.path).is_err(), "{injection:?}");
            assert!(repo.path.join("file").is_file());
            assert_ne!(repo.git(&["for-each-ref", "refs/dogsled/recovery/"]), "");
        }
    }

    #[test]
    fn unreadable_directory_refuses() {
        let repo = Repo::new(true);
        let dir = repo.path.join("unreadable");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o0)).unwrap();
        let result = snapshot(&System::default(), &repo.path);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            result,
            Err(Error::Refused(Refusal::UnreadablePath(_)))
        ));
    }

    #[test]
    fn clean_submodule_is_captured_as_a_gitlink() {
        let source = Repo::new(true);
        let repo = Repo::new(true);
        repo.git(&[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            source.path.to_str().unwrap(),
            "sub",
        ]);
        let saved = snapshot(&System::default(), &repo.path).unwrap();
        assert!(
            repo.git(&["ls-tree", &saved.commit, "sub"])
                .starts_with("160000 commit ")
        );
        verify(&System::default(), &repo.path, &saved).unwrap();
    }

    #[test]
    fn verify_detects_working_and_index_changes() {
        let repo = Repo::new(true);
        let saved = snapshot(&System::default(), &repo.path).unwrap();
        fs::write(repo.path.join("file"), b"later").unwrap();
        assert!(verify(&System::default(), &repo.path, &saved).is_err());
        repo.git(&["add", "file"]);
        assert!(verify(&System::default(), &repo.path, &saved).is_err());
    }
}
