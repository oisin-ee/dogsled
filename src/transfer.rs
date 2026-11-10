//! Pinned source capture and bundle import. Neither operation materializes a checkout.

use std::fmt::Write as _;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::ledger::Ledger;
use crate::shell::{Output, Shell};

pub(crate) mod apply;
pub(crate) mod delivery;

pub(crate) type Result<T> = std::result::Result<T, Refusal>;

/// Fixed diagnostics: Git stderr and remote URLs never cross this boundary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum Refusal {
    #[error("transfer id is invalid")]
    InvalidId,
    #[error("transfer manifest is invalid or unsupported")]
    InvalidManifest,
    #[error("a base revision cannot include working-tree changes")]
    InvalidOptions,
    #[error("source revision does not resolve to a commit")]
    MissingCommit,
    #[error("source index contains unmerged entries")]
    ConflictedIndex,
    #[error("source checkout changed during capture")]
    CheckoutChanged,
    #[error("source checkout cannot be captured safely")]
    UnsupportedSource,
    #[error("transfer repository identity does not match")]
    RepositoryMismatch,
    #[error("transfer object format does not match")]
    ObjectFormatMismatch,
    #[error("transfer payload does not match its recorded digest and size")]
    PayloadMismatch,
    #[error("transfer bundle prerequisites are unavailable")]
    BundlePrerequisites,
    #[error("transfer ref already names another object")]
    RefConflict,
    #[error("transfer objects failed verification")]
    VerificationFailed,
    #[error("transfer Git operation failed")]
    Git,
    #[error("transfer storage operation failed")]
    Storage,
}

impl From<Refusal> for crate::error::Error {
    fn from(refusal: Refusal) -> Self {
        Self::Invalid(refusal.to_string())
    }
}

#[derive(Debug, Default)]
pub(crate) struct Options {
    pub(crate) include_worktree: bool,
    pub(crate) base: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub(crate) version: u32,
    pub(crate) id: String,
    pub(crate) repository: String,
    pub(crate) object_format: String,
    pub(crate) commit: String,
    pub(crate) snapshot: Option<Snapshot>,
    pub(crate) ref_name: String,
    pub(crate) bundle: Option<Bundle>,
    pub(crate) created_ms: u64,
}

impl Manifest {
    pub(crate) fn pinned(&self) -> &str {
        self.snapshot
            .as_ref()
            .map_or(&self.commit, |snapshot| &snapshot.commit)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Snapshot {
    pub(crate) commit: String,
    pub(crate) index_tree: String,
    pub(crate) worktree_tree: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Bundle {
    pub(crate) sha256: String,
    pub(crate) bytes: u64,
}

fn validate_id(id: &str) -> Result<()> {
    crate::jobs::validate_id(id).map_err(|_| Refusal::InvalidId)?;
    if !hex(id, 32) {
        return Err(Refusal::InvalidId);
    }
    Ok(())
}

fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn random_id() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| Refusal::Storage)?;
    let mut id = String::new();
    for byte in bytes {
        write!(id, "{byte:02x}").map_err(|_| Refusal::Storage)?;
    }
    Ok(id)
}

fn directory(ledger: &Ledger, id: &str) -> Result<PathBuf> {
    validate_id(id)?;
    Ok(ledger.dir.join("transfers").join(id))
}

fn validate(manifest: &Manifest, id: &str) -> Result<()> {
    validate_id(id)?;
    validate_id(&manifest.id)?;
    let length = match manifest.object_format.as_str() {
        "sha1" => 40,
        "sha256" => 64,
        _ => return Err(Refusal::InvalidManifest),
    };
    if manifest.version != 1
        || manifest.id != id
        || manifest.repository.is_empty()
        || manifest.ref_name != format!("refs/dogsled/transfer/{id}")
        || !hex(&manifest.commit, length)
        || manifest
            .bundle
            .as_ref()
            .is_some_and(|bundle| !hex(&bundle.sha256, 64) || bundle.bytes == 0)
        || manifest.snapshot.as_ref().is_some_and(|snapshot| {
            !hex(&snapshot.commit, length)
                || !hex(&snapshot.index_tree, length)
                || !hex(&snapshot.worktree_tree, length)
        })
    {
        return Err(Refusal::InvalidManifest);
    }
    Ok(())
}

pub(crate) fn load(ledger: &Ledger, id: &str) -> Result<Manifest> {
    let bytes =
        fs::read(directory(ledger, id)?.join("manifest.json")).map_err(|_| Refusal::Storage)?;
    let manifest = serde_json::from_slice(&bytes).map_err(|_| Refusal::InvalidManifest)?;
    validate(&manifest, id)?;
    Ok(manifest)
}

fn git(shell: &dyn Shell, checkout: &Path, args: &[&str]) -> Result<Output> {
    let words = crate::preservation::git_words(checkout, None, args)
        .map_err(|_| Refusal::UnsupportedSource)?;
    shell.run("env", &words).map_err(|_| Refusal::Git)
}

fn checked(shell: &dyn Shell, checkout: &Path, args: &[&str]) -> Result<String> {
    let output = git(shell, checkout, args)?;
    if !output.ok() {
        return Err(Refusal::Git);
    }
    Ok(output.stdout.trim().into())
}

fn repository(shell: &dyn Shell, checkout: &Path) -> Result<String> {
    let origin = checked(shell, checkout, &["remote", "get-url", "origin"])?;
    Ok(crate::checkout::identity(&origin).unwrap_or(origin))
}

fn object_format(shell: &dyn Shell, checkout: &Path) -> Result<String> {
    checked(shell, checkout, &["rev-parse", "--show-object-format"])
}

fn resolve(shell: &dyn Shell, checkout: &Path, revision: &str) -> Result<String> {
    let output = git(
        shell,
        checkout,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{revision}^{{commit}}"),
        ],
    )?;
    if !output.ok() {
        return Err(Refusal::MissingCommit);
    }
    Ok(output.stdout.trim().into())
}

fn source_error(error: &crate::preservation::Error) -> Refusal {
    match error {
        crate::preservation::Error::Refused(crate::preservation::Refusal::ConflictedIndex) => {
            Refusal::ConflictedIndex
        }
        crate::preservation::Error::Refused(crate::preservation::Refusal::CheckoutChanged) => {
            Refusal::CheckoutChanged
        }
        _ => Refusal::UnsupportedSource,
    }
}

fn snapshot(shell: &dyn Shell, checkout: &Path) -> Result<Snapshot> {
    let trees =
        crate::preservation::trees(shell, checkout).map_err(|error| source_error(&error))?;
    let head = trees.head.commit().ok_or(Refusal::MissingCommit)?;
    let index_commit = crate::preservation::commit_tree(
        shell,
        checkout,
        &trees.index_tree,
        &[head],
        "Dogsled transfer staged index",
    )
    .map_err(|error| source_error(&error))?;
    let commit = crate::preservation::commit_tree(
        shell,
        checkout,
        &trees.worktree_tree,
        &[head, &index_commit],
        "Dogsled transfer working tree",
    )
    .map_err(|error| source_error(&error))?;
    Ok(Snapshot {
        commit,
        index_tree: trees.index_tree,
        worktree_tree: trees.worktree_tree,
    })
}

fn existing(shell: &dyn Shell, checkout: &Path, manifest: &Manifest) -> Result<bool> {
    let output = git(
        shell,
        checkout,
        &["rev-parse", "--verify", "--quiet", &manifest.ref_name],
    )?;
    if output.ok() {
        if output.stdout.trim() != manifest.pinned() {
            return Err(Refusal::RefConflict);
        }
        return Ok(true);
    }
    if output.status != 1 {
        return Err(Refusal::Git);
    }
    Ok(false)
}

/// Returns true only when this call created the ref.
fn pin(shell: &dyn Shell, checkout: &Path, manifest: &Manifest) -> Result<bool> {
    if existing(shell, checkout, manifest)? {
        return Ok(false);
    }
    let output = git(
        shell,
        checkout,
        &[
            "update-ref",
            "--no-deref",
            &manifest.ref_name,
            manifest.pinned(),
            &"0".repeat(manifest.pinned().len()),
        ],
    )?;
    if output.ok() {
        return Ok(true);
    }
    if !existing(shell, checkout, manifest)? {
        return Err(Refusal::Git);
    }
    Ok(false)
}

fn unpin(shell: &dyn Shell, checkout: &Path, manifest: &Manifest) {
    let _ = git(
        shell,
        checkout,
        &[
            "update-ref",
            "--no-deref",
            "-d",
            &manifest.ref_name,
            manifest.pinned(),
        ],
    );
}

fn digest(path: &Path) -> Result<Bundle> {
    let mut file = fs::File::open(path).map_err(|_| Refusal::Storage)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut bytes = 0;
    loop {
        let count = file.read(&mut buffer).map_err(|_| Refusal::Storage)?;
        if count == 0 {
            break;
        }
        hash.update(buffer.get(..count).ok_or(Refusal::Storage)?);
        bytes += count as u64;
    }
    let mut sha256 = String::new();
    for byte in hash.finalize() {
        write!(sha256, "{byte:02x}").map_err(|_| Refusal::Storage)?;
    }
    Ok(Bundle { sha256, bytes })
}

/// The lock closure keeps typed transfer failures separate from ledger failures.
fn locked<T>(ledger: &Ledger, id: &str, action: impl FnOnce() -> Result<T>) -> Result<T> {
    validate_id(id)?;
    ledger
        .with_lock(&format!("transfer-{id}"), || Ok(action()))
        .map_err(|_| Refusal::Storage)?
}

struct Temporary(PathBuf);

impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn capture(
    shell: &dyn Shell,
    ledger: &Ledger,
    checkout: &Path,
    options: &Options,
) -> Result<Manifest> {
    if options.include_worktree && options.base.is_some() {
        return Err(Refusal::InvalidOptions);
    }
    let source = resolve(shell, checkout, options.base.as_deref().unwrap_or("HEAD"))?;
    let snapshot = options
        .include_worktree
        .then(|| snapshot(shell, checkout))
        .transpose()?;
    let id = random_id()?;
    let mut manifest = Manifest {
        version: 1,
        repository: repository(shell, checkout)?,
        object_format: object_format(shell, checkout)?,
        commit: source,
        snapshot,
        ref_name: format!("refs/dogsled/transfer/{id}"),
        id: id.clone(),
        bundle: None,
        created_ms: crate::ledger::now_ms(),
    };
    validate(&manifest, &id)?;
    locked(ledger, &id, || {
        let destination = directory(ledger, &id)?;
        let temporary = destination.with_file_name(format!(".{id}.tmp"));
        fs::create_dir_all(ledger.dir.join("transfers")).map_err(|_| Refusal::Storage)?;
        fs::create_dir(&temporary).map_err(|_| Refusal::Storage)?;
        let temporary = Temporary(temporary);
        let created = pin(shell, checkout, &manifest)?;
        let result = publish(shell, checkout, &mut manifest, &temporary.0, &destination);
        if result.is_err() && created {
            unpin(shell, checkout, &manifest);
        }
        result?;
        Ok(manifest)
    })
}

fn publish(
    shell: &dyn Shell,
    checkout: &Path,
    manifest: &mut Manifest,
    temporary: &Path,
    destination: &Path,
) -> Result<()> {
    let payload = temporary.join("payload.bundle.tmp");
    let output = git(
        shell,
        checkout,
        &[
            "bundle",
            "create",
            payload.to_str().ok_or(Refusal::Storage)?,
            &manifest.ref_name,
            "--not",
            "--remotes=origin",
        ],
    )?;
    if output.ok() {
        // Prove the produced bundle before publishing its manifest.
        checked(
            shell,
            checkout,
            &[
                "bundle",
                "verify",
                payload.to_str().ok_or(Refusal::Storage)?,
            ],
        )?;
        manifest.bundle = Some(digest(&payload)?);
        fs::rename(&payload, temporary.join("payload.bundle")).map_err(|_| Refusal::Storage)?;
    } else {
        let containing = checked(
            shell,
            checkout,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "--contains",
                manifest.pinned(),
                "refs/remotes/origin/",
            ],
        )?;
        if containing.is_empty() {
            return Err(Refusal::Git);
        }
    }
    let bytes = serde_json::to_vec_pretty(&manifest).map_err(|_| Refusal::InvalidManifest)?;
    let file = temporary.join("manifest.json.tmp");
    fs::write(&file, bytes).map_err(|_| Refusal::Storage)?;
    fs::rename(file, temporary.join("manifest.json")).map_err(|_| Refusal::Storage)?;
    fs::rename(temporary, destination).map_err(|_| Refusal::Storage)?;
    Ok(())
}

fn fetch_origin(shell: &dyn Shell, checkout: &Path) -> Result<()> {
    // Only remote-tracking refs may move, even with a custom origin fetch mapping.
    checked(
        shell,
        checkout,
        &[
            "fetch",
            "--no-tags",
            "--no-write-fetch-head",
            "--refmap=",
            "origin",
            "+refs/heads/*:refs/remotes/origin/*",
        ],
    )?;
    Ok(())
}

fn verify_objects(shell: &dyn Shell, checkout: &Path, manifest: &Manifest) -> Result<()> {
    if resolve(shell, checkout, &manifest.ref_name)? != manifest.pinned() {
        return Err(Refusal::VerificationFailed);
    }
    if let Some(snapshot) = &manifest.snapshot {
        let parent = resolve(shell, checkout, &format!("{}^1", manifest.pinned()))
            .map_err(|_| Refusal::VerificationFailed)?;
        if parent != manifest.commit {
            return Err(Refusal::VerificationFailed);
        }
        for tree in [&snapshot.index_tree, &snapshot.worktree_tree] {
            if !git(
                shell,
                checkout,
                &["cat-file", "-e", &format!("{tree}^{{tree}}")],
            )?
            .ok()
            {
                return Err(Refusal::VerificationFailed);
            }
        }
        let working = checked(
            shell,
            checkout,
            &["rev-parse", &format!("{}^{{tree}}", manifest.pinned())],
        )?;
        let staged = checked(
            shell,
            checkout,
            &["rev-parse", &format!("{}^2^{{tree}}", manifest.pinned())],
        )?;
        if working != snapshot.worktree_tree || staged != snapshot.index_tree {
            return Err(Refusal::VerificationFailed);
        }
    }
    Ok(())
}

pub(crate) fn import(
    shell: &dyn Shell,
    ledger: &Ledger,
    checkout: &Path,
    id: &str,
) -> Result<Manifest> {
    locked(ledger, id, || {
        let manifest = load(ledger, id)?;
        if repository(shell, checkout)? != manifest.repository {
            return Err(Refusal::RepositoryMismatch);
        }
        if object_format(shell, checkout)? != manifest.object_format {
            return Err(Refusal::ObjectFormatMismatch);
        }
        let payload = directory(ledger, id)?.join("payload.bundle");
        if let Some(bundle) = &manifest.bundle
            && digest(&payload)? != *bundle
        {
            return Err(Refusal::PayloadMismatch);
        }
        if existing(shell, checkout, &manifest)? {
            verify_objects(shell, checkout, &manifest)?;
            return Ok(manifest);
        }
        if manifest.bundle.is_some() {
            let payload = payload.to_str().ok_or(Refusal::Storage)?;
            if !git(shell, checkout, &["bundle", "verify", payload])?.ok() {
                fetch_origin(shell, checkout)?;
                if !git(shell, checkout, &["bundle", "verify", payload])?.ok() {
                    return Err(Refusal::BundlePrerequisites);
                }
            }
            let advertised = checked(
                shell,
                checkout,
                &["bundle", "list-heads", payload, &manifest.ref_name],
            )?;
            if advertised != format!("{} {}", manifest.pinned(), manifest.ref_name) {
                return Err(Refusal::VerificationFailed);
            }
            // Import objects without updating a ref; pin owns ref creation below.
            checked(
                shell,
                checkout,
                &[
                    "fetch",
                    "--no-tags",
                    "--no-write-fetch-head",
                    payload,
                    &manifest.ref_name,
                ],
            )?;
        } else if resolve(shell, checkout, manifest.pinned()).is_err() {
            fetch_origin(shell, checkout)?;
            resolve(shell, checkout, manifest.pinned())?;
        }
        let created = pin(shell, checkout, &manifest)?;
        let result = verify_objects(shell, checkout, &manifest);
        if result.is_err() && created {
            unpin(shell, checkout, &manifest);
        }
        result?;
        Ok(manifest)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::System;
    use std::collections::BTreeMap;
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::{PermissionsExt as _, symlink};
    use std::sync::Mutex;

    struct Repo {
        root: Temporary,
        origin: PathBuf,
        a: PathBuf,
        b: PathBuf,
        ledger: Ledger,
    }

    impl Repo {
        fn new() -> Self {
            Self::with_format("sha1")
        }

        fn with_format(format: &str) -> Self {
            let root = Temporary(
                std::env::temp_dir().join(format!("dogsled-transfer-{}", random_id().unwrap())),
            );
            fs::create_dir(&root.0).unwrap();
            let origin = root.0.join("origin.git");
            let a = root.0.join("a");
            let b = root.0.join("b");
            let ledger = Ledger::new(root.0.join("state"));
            run(
                &root.0,
                &[
                    "init",
                    "--bare",
                    "-b",
                    "main",
                    &format!("--object-format={format}"),
                    origin.to_str().unwrap(),
                ],
            );
            run(
                &root.0,
                &["clone", origin.to_str().unwrap(), a.to_str().unwrap()],
            );
            configure(&a);
            for path in ["file", "deleted", "renamed", "binary"] {
                fs::write(a.join(path), format!("base {path}\n")).unwrap();
            }
            fs::write(a.join(".gitignore"), b"ignored\n").unwrap();
            run(&a, &["add", "."]);
            run(&a, &["commit", "-m", "base"]);
            run(&a, &["push", "origin", "main"]);
            run(
                &root.0,
                &["clone", origin.to_str().unwrap(), b.to_str().unwrap()],
            );
            configure(&b);
            Self {
                root,
                origin,
                a,
                b,
                ledger,
            }
        }

        fn commit(&self, message: &str) -> String {
            fs::write(self.a.join("file"), message).unwrap();
            run(&self.a, &["add", "file"]);
            run(&self.a, &["commit", "-m", message]);
            run(&self.a, &["rev-parse", "HEAD"])
        }

        fn capture(&self, include_worktree: bool) -> Manifest {
            capture(
                &System::default(),
                &self.ledger,
                &self.a,
                &Options {
                    include_worktree,
                    base: None,
                },
            )
            .unwrap()
        }

        fn import(&self, shell: &dyn Shell, id: &str) -> Result<Manifest> {
            import(shell, &self.ledger, &self.b, id)
        }

        fn absent(&self, manifest: &Manifest) {
            assert!(
                !git(
                    &System::default(),
                    &self.b,
                    &["show-ref", "--verify", "--quiet", &manifest.ref_name]
                )
                .unwrap()
                .ok()
            );
        }
    }

    fn configure(path: &Path) {
        for (key, value) in [
            ("user.name", "Test"),
            ("user.email", "test@example.invalid"),
            ("core.autocrlf", "false"),
            ("core.filemode", "true"),
            ("core.symlinks", "true"),
        ] {
            run(path, &["config", key, value]);
        }
    }

    fn run(checkout: &Path, args: &[&str]) -> String {
        checked(&System::default(), checkout, args).unwrap()
    }

    fn raw(checkout: &Path, args: &[&str]) -> Vec<u8> {
        let mut words = vec![
            "GIT_OPTIONAL_LOCKS=0".into(),
            "git".into(),
            "-C".into(),
            checkout.to_str().unwrap().into(),
        ];
        words.extend(args.iter().map(ToString::to_string));
        let output = System::default()
            .run_bytes_bounded(
                "env",
                &words,
                std::time::Duration::from_secs(30),
                usize::MAX,
            )
            .unwrap();
        assert_eq!(output.status, 0);
        output.stdout
    }

    #[derive(Debug, PartialEq, Eq)]
    struct State {
        head: String,
        branch: String,
        index_tree: String,
        index: Vec<u8>,
        status: Vec<u8>,
        files: BTreeMap<PathBuf, (u32, Vec<u8>)>,
    }

    fn files(root: &Path, relative: &Path, entries: &mut BTreeMap<PathBuf, (u32, Vec<u8>)>) {
        for entry in fs::read_dir(root.join(relative)).unwrap() {
            let entry = entry.unwrap();
            if relative.as_os_str().is_empty() && entry.file_name() == ".git" {
                continue;
            }
            let path = relative.join(entry.file_name());
            let meta = fs::symlink_metadata(root.join(&path)).unwrap();
            if meta.is_dir() {
                files(root, &path, entries);
            } else {
                let bytes = if meta.is_symlink() {
                    fs::read_link(root.join(&path))
                        .unwrap()
                        .as_os_str()
                        .as_bytes()
                        .to_vec()
                } else {
                    fs::read(root.join(&path)).unwrap()
                };
                entries.insert(path, (meta.permissions().mode(), bytes));
            }
        }
    }

    fn state(checkout: &Path) -> State {
        let head = run(checkout, &["rev-parse", "HEAD"]);
        let branch = run(checkout, &["symbolic-ref", "HEAD"]);
        let index_tree = run(checkout, &["write-tree"]);
        let index_path = run(
            checkout,
            &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        );
        let index = fs::read(index_path).unwrap();
        let status = raw(checkout, &["status", "--porcelain=v2", "-z"]);
        let mut entries = BTreeMap::new();
        files(checkout, Path::new(""), &mut entries);
        State {
            head,
            branch,
            index_tree,
            index,
            status,
            files: entries,
        }
    }

    #[derive(Debug, Default)]
    struct Recording {
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl Shell for Recording {
        fn run(&self, program: &str, args: &[String]) -> crate::error::Result<Output> {
            self.calls.lock().unwrap().push(args.to_vec());
            System::default().run(program, args)
        }
    }

    impl Recording {
        fn origin_fetches(&self) -> usize {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|args| {
                    args.iter().any(|arg| arg == "fetch") && args.iter().any(|arg| arg == "origin")
                })
                .count()
        }
    }

    #[test]
    fn committed_only_preserves_both_checkouts_and_excludes_dirty_content() {
        let repo = Repo::new();
        let commit = repo.commit("unpushed");
        fs::write(repo.a.join("file"), b"not transported").unwrap();
        fs::write(repo.a.join("untracked"), b"not transported").unwrap();
        fs::write(repo.b.join("file"), b"destination staged").unwrap();
        run(&repo.b, &["add", "file"]);
        fs::write(repo.b.join("file"), b"destination unstaged").unwrap();
        fs::write(repo.b.join("local-only"), b"destination untracked").unwrap();
        let source = state(&repo.a);
        let destination = state(&repo.b);
        let manifest = repo.capture(false);
        assert_eq!(manifest.commit, commit);
        assert_eq!(manifest.snapshot, None);
        assert_eq!(run(&repo.a, &["rev-parse", &manifest.ref_name]), commit);
        assert!(manifest.bundle.is_some());
        assert_eq!(manifest, load(&repo.ledger, &manifest.id).unwrap());
        let shell = Recording::default();
        assert_eq!(repo.import(&shell, &manifest.id).unwrap(), manifest);
        assert_eq!(shell.origin_fetches(), 0);
        assert_eq!(run(&repo.b, &["rev-parse", &manifest.ref_name]), commit);
        assert_eq!(state(&repo.a), source);
        assert_eq!(state(&repo.b), destination);
        assert_eq!(
            raw(&repo.b, &["show", &format!("{}:file", manifest.ref_name)]),
            b"unpushed"
        );
        assert!(
            !git(
                &System::default(),
                &repo.b,
                &[
                    "cat-file",
                    "-e",
                    &format!("{}:untracked", manifest.ref_name)
                ]
            )
            .unwrap()
            .ok()
        );
        assert_eq!(
            run(&repo.a, &["for-each-ref", "refs/dogsled/recovery/"]),
            ""
        );
    }

    #[test]
    fn working_tree_transports_raw_entries_without_changing_source_or_destination() {
        let repo = Repo::new();
        fs::write(repo.a.join("file"), b"staged\n").unwrap();
        run(&repo.a, &["add", "file"]);
        fs::write(repo.a.join("file"), b"unstaged\n").unwrap();
        fs::write(repo.a.join("binary"), [0, 255, 128, 1, 0]).unwrap();
        fs::remove_file(repo.a.join("deleted")).unwrap();
        run(&repo.a, &["mv", "renamed", "new-name"]);
        fs::write(repo.a.join("untracked"), b"untracked\0bytes").unwrap();
        fs::write(repo.a.join("executable"), b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(repo.a.join("executable"), fs::Permissions::from_mode(0o755)).unwrap();
        symlink("new-name", repo.a.join("symlink")).unwrap();
        fs::write(repo.a.join("ignored"), b"secret").unwrap();
        let source = state(&repo.a);
        let destination = state(&repo.b);
        let manifest = repo.capture(true);
        let snapshot = manifest.snapshot.as_ref().unwrap();
        assert_eq!(snapshot.index_tree, source.index_tree);
        assert_eq!(manifest.commit, source.head);
        assert_ne!(snapshot.commit, manifest.commit);
        assert_eq!(
            run(&repo.a, &["rev-parse", &manifest.ref_name]),
            snapshot.commit
        );
        assert_eq!(
            run(&repo.a, &["rev-parse", &format!("{}^1", snapshot.commit)]),
            source.head
        );
        assert_eq!(state(&repo.a), source);
        repo.import(&System::default(), &manifest.id).unwrap();
        assert_eq!(
            run(&repo.b, &["rev-parse", &manifest.ref_name]),
            snapshot.commit
        );
        assert_eq!(
            run(&repo.b, &["rev-parse", &format!("{}^1", snapshot.commit)]),
            source.head
        );
        assert_eq!(state(&repo.b), destination);
        assert_eq!(
            run(&repo.a, &["ls-tree", "-r", &snapshot.worktree_tree]),
            run(&repo.b, &["ls-tree", "-r", &snapshot.worktree_tree])
        );
        assert_eq!(
            run(&repo.a, &["ls-tree", "-r", &snapshot.index_tree]),
            run(&repo.b, &["ls-tree", "-r", &snapshot.index_tree])
        );
        for tree in [&snapshot.index_tree, &snapshot.worktree_tree] {
            let entries = run(&repo.a, &["ls-tree", "-r", tree]);
            for entry in entries.lines() {
                let oid = entry.split_whitespace().nth(2).unwrap();
                assert_eq!(
                    raw(&repo.a, &["cat-file", "blob", oid]),
                    raw(&repo.b, &["cat-file", "blob", oid])
                );
            }
        }
        let entries = run(&repo.b, &["ls-tree", "-r", &snapshot.worktree_tree]);
        assert!(!entries.contains("\tignored"));
        assert!(!entries.contains("\tdeleted"));
        assert!(!entries.contains("\trenamed"));
        assert!(entries.contains("\tnew-name"));
        assert!(
            entries
                .lines()
                .any(|line| line.starts_with("100755 ") && line.ends_with("\texecutable"))
        );
        assert!(
            entries
                .lines()
                .any(|line| line.starts_with("120000 ") && line.ends_with("\tsymlink"))
        );
        for (path, bytes) in [
            ("file", b"unstaged\n".as_slice()),
            ("binary", &[0, 255, 128, 1, 0]),
            ("untracked", b"untracked\0bytes"),
            ("symlink", b"new-name"),
        ] {
            assert_eq!(
                raw(
                    &repo.b,
                    &["show", &format!("{}:{path}", snapshot.worktree_tree)]
                ),
                bytes
            );
        }
        assert_eq!(
            raw(&repo.b, &["show", &format!("{}:file", snapshot.index_tree)]),
            b"staged\n"
        );
        assert_eq!(state(&repo.a), source);
    }

    #[test]
    fn origin_commit_uses_no_bundle_and_fetches_once_when_missing() {
        let repo = Repo::new();
        let commit = repo.commit("published");
        run(&repo.a, &["push", "origin", "main"]);
        let manifest = repo.capture(false);
        assert_eq!(manifest.commit, commit);
        assert_eq!(manifest.bundle, None);
        assert!(
            !directory(&repo.ledger, &manifest.id)
                .unwrap()
                .join("payload.bundle")
                .exists()
        );
        assert_eq!(
            resolve(&System::default(), &repo.b, &commit),
            Err(Refusal::MissingCommit)
        );
        // A user-supplied fetch mapping must not turn import into a branch update.
        run(
            &repo.b,
            &[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/heads/*",
            ],
        );
        let before = state(&repo.b);
        let shell = Recording::default();
        repo.import(&shell, &manifest.id).unwrap();
        assert_eq!(shell.origin_fetches(), 1);
        assert_eq!(state(&repo.b), before);
    }

    #[test]
    fn git_words_reuse_preservation_isolation() {
        let repo = Repo::new();
        let shell = Recording::default();
        let args = ["rev-parse", "HEAD"];
        git(&shell, &repo.a, &args).unwrap();
        assert_eq!(
            *shell.calls.lock().unwrap(),
            vec![crate::preservation::git_words(&repo.a, None, &args).unwrap()]
        );
    }

    #[test]
    fn altered_committed_source_refuses_without_creating_a_ref() {
        let repo = Repo::new();
        repo.commit("unpushed");
        let mut manifest = repo.capture(false);
        manifest.commit = run(&repo.b, &["rev-parse", "HEAD"]);
        let path = directory(&repo.ledger, &manifest.id)
            .unwrap()
            .join("manifest.json");
        fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert_eq!(
            repo.import(&System::default(), &manifest.id),
            Err(Refusal::VerificationFailed)
        );
        repo.absent(&manifest);
    }

    #[test]
    fn altered_source_commit_refuses_and_removes_only_a_new_ref() {
        for preexisting in [false, true] {
            let repo = Repo::new();
            repo.commit("unpushed");
            let mut manifest = repo.capture(true);
            if preexisting {
                repo.import(&System::default(), &manifest.id).unwrap();
            }
            manifest.commit = run(&repo.b, &["rev-parse", "HEAD"]);
            let path = directory(&repo.ledger, &manifest.id)
                .unwrap()
                .join("manifest.json");
            fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
            assert_eq!(
                repo.import(&System::default(), &manifest.id),
                Err(Refusal::VerificationFailed)
            );
            if preexisting {
                assert_eq!(
                    run(&repo.b, &["rev-parse", &manifest.ref_name]),
                    manifest.pinned()
                );
            } else {
                repo.absent(&manifest);
            }
        }
    }

    #[derive(Debug)]
    struct BundleFailure;

    impl Shell for BundleFailure {
        fn run(&self, program: &str, args: &[String]) -> crate::error::Result<Output> {
            if args.windows(2).any(|pair| pair == ["bundle", "create"]) {
                return Ok(Output {
                    status: 1,
                    stdout: String::new(),
                    stderr: "bundle creation failed".into(),
                });
            }
            System::default().run(program, args)
        }
    }

    #[test]
    fn failed_bundle_capture_removes_its_ref_and_temporary_directory() {
        let repo = Repo::new();
        repo.commit("unpushed");
        assert_eq!(
            capture(&BundleFailure, &repo.ledger, &repo.a, &Options::default()),
            Err(Refusal::Git)
        );
        assert_eq!(
            run(&repo.a, &["for-each-ref", "refs/dogsled/transfer/"]),
            ""
        );
        assert_eq!(
            fs::read_dir(repo.ledger.dir.join("transfers"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn altered_payload_refuses_without_creating_a_ref() {
        let repo = Repo::new();
        repo.commit("unpushed");
        let manifest = repo.capture(false);
        let path = directory(&repo.ledger, &manifest.id)
            .unwrap()
            .join("payload.bundle");
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] ^= 1;
        fs::write(path, bytes).unwrap();
        let before = state(&repo.b);
        assert_eq!(
            repo.import(&System::default(), &manifest.id),
            Err(Refusal::PayloadMismatch)
        );
        repo.absent(&manifest);
        assert_eq!(state(&repo.b), before);
    }

    #[test]
    fn stale_prerequisites_fetch_once_then_import_and_unreachable_origin_refuses() {
        for reachable in [true, false] {
            let repo = Repo::new();
            repo.commit("new prerequisite");
            run(&repo.a, &["push", "origin", "main"]);
            repo.commit("unpushed on new prerequisite");
            let manifest = repo.capture(false);
            assert!(manifest.bundle.is_some());
            if !reachable {
                fs::rename(&repo.origin, repo.root.0.join("offline.git")).unwrap();
            }
            let before = state(&repo.b);
            let shell = Recording::default();
            let result = repo.import(&shell, &manifest.id);
            assert_eq!(shell.origin_fetches(), 1);
            if reachable {
                assert_eq!(result.unwrap(), manifest);
            } else {
                assert_eq!(result, Err(Refusal::Git));
                repo.absent(&manifest);
            }
            assert_eq!(state(&repo.b), before);
        }
    }

    #[test]
    fn repository_mismatch_refuses_and_repeated_import_and_conflicting_ref_are_safe() {
        let repo = Repo::new();
        repo.commit("unpushed");
        let manifest = repo.capture(false);
        run(
            &repo.b,
            &[
                "remote",
                "set-url",
                "origin",
                "https://secret.invalid/other.git",
            ],
        );
        let shell = Recording::default();
        assert_eq!(
            repo.import(&shell, &manifest.id),
            Err(Refusal::RepositoryMismatch)
        );
        assert_eq!(shell.origin_fetches(), 0);
        repo.absent(&manifest);
        run(
            &repo.b,
            &["remote", "set-url", "origin", repo.origin.to_str().unwrap()],
        );
        repo.import(&shell, &manifest.id).unwrap();
        let before = state(&repo.b);
        let refs = run(&repo.b, &["show-ref"]);
        repo.import(&shell, &manifest.id).unwrap();
        assert_eq!(run(&repo.b, &["show-ref"]), refs);
        assert_eq!(state(&repo.b), before);
        assert_eq!(shell.origin_fetches(), 0);
        run(&repo.b, &["update-ref", &manifest.ref_name, "HEAD"]);
        let refs = run(&repo.b, &["show-ref"]);
        assert_eq!(repo.import(&shell, &manifest.id), Err(Refusal::RefConflict));
        assert_eq!(run(&repo.b, &["show-ref"]), refs);
        assert_eq!(state(&repo.b), before);
    }

    #[test]
    fn base_selection_and_invalid_worktree_options_are_explicit() {
        let repo = Repo::new();
        let base = run(&repo.a, &["rev-parse", "HEAD"]);
        repo.commit("later");
        let options = Options {
            include_worktree: true,
            base: Some(base.clone()),
        };
        assert_eq!(
            capture(&System::default(), &repo.ledger, &repo.a, &options),
            Err(Refusal::InvalidOptions)
        );
        let manifest = capture(
            &System::default(),
            &repo.ledger,
            &repo.a,
            &Options {
                include_worktree: false,
                base: Some(base.clone()),
            },
        )
        .unwrap();
        assert_eq!(manifest.commit, base);
        repo.import(&System::default(), &manifest.id).unwrap();
        let invalid = Options {
            include_worktree: false,
            base: Some("--all".into()),
        };
        assert_eq!(
            capture(&System::default(), &repo.ledger, &repo.a, &invalid),
            Err(Refusal::MissingCommit)
        );
    }

    #[test]
    fn unmerged_index_and_unborn_head_refuse_without_pinning() {
        let repo = Repo::new();
        run(&repo.a, &["switch", "-c", "other"]);
        repo.commit("other change");
        run(&repo.a, &["switch", "main"]);
        repo.commit("main change");
        assert!(
            !git(&System::default(), &repo.a, &["merge", "other"])
                .unwrap()
                .ok()
        );
        let before = raw(&repo.a, &["status", "--porcelain=v2", "-z"]);
        let index = fs::read(repo.a.join(".git/index")).unwrap();
        assert_eq!(
            capture(
                &System::default(),
                &repo.ledger,
                &repo.a,
                &Options {
                    include_worktree: true,
                    base: None
                }
            ),
            Err(Refusal::ConflictedIndex)
        );
        assert_eq!(raw(&repo.a, &["status", "--porcelain=v2", "-z"]), before);
        assert_eq!(fs::read(repo.a.join(".git/index")).unwrap(), index);
        assert_eq!(
            run(&repo.a, &["for-each-ref", "refs/dogsled/transfer/"]),
            ""
        );
        let unborn = repo.root.0.join("unborn");
        fs::create_dir(&unborn).unwrap();
        run(&unborn, &["init", "-b", "main"]);
        assert_eq!(
            capture(
                &System::default(),
                &repo.ledger,
                &unborn,
                &Options::default()
            ),
            Err(Refusal::MissingCommit)
        );
    }

    #[test]
    fn manifests_reject_unsafe_ids_unknown_fields_and_unsupported_versions() {
        let repo = Repo::new();
        let manifest = repo.capture(false);
        for id in ["../escape", "", "-option", "AABB", "/absolute"] {
            assert_eq!(load(&repo.ledger, id), Err(Refusal::InvalidId));
            assert_eq!(repo.import(&System::default(), id), Err(Refusal::InvalidId));
        }
        let path = directory(&repo.ledger, &manifest.id)
            .unwrap()
            .join("manifest.json");
        let original = serde_json::to_value(&manifest).unwrap();
        for field in ["version", "unknown", "id", "ref_name"] {
            let mut value = original.clone();
            value[field] = match field {
                "version" => serde_json::json!(2),
                "id" => serde_json::json!("../escape"),
                "ref_name" => serde_json::json!("refs/heads/main"),
                _ => serde_json::json!("https://secret.invalid/value"),
            };
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            let error = load(&repo.ledger, &manifest.id).unwrap_err();
            assert!(matches!(
                error,
                Refusal::InvalidManifest | Refusal::InvalidId
            ));
            assert!(!error.to_string().contains("secret.invalid"));
        }
    }

    #[test]
    fn sha256_bundle_round_trip_and_object_format_mismatch() {
        let repo = Repo::with_format("sha256");
        repo.commit("sha256 unpushed");
        let manifest = repo.capture(false);
        assert_eq!(manifest.object_format, "sha256");
        assert_eq!(manifest.commit.len(), 64);
        repo.import(&System::default(), &manifest.id).unwrap();
        let other = Repo::new();
        run(
            &other.b,
            &["remote", "set-url", "origin", repo.origin.to_str().unwrap()],
        );
        assert_eq!(
            import(&System::default(), &repo.ledger, &other.b, &manifest.id),
            Err(Refusal::ObjectFormatMismatch)
        );
    }
}
