//! Apply a received change transfer to working files. The index, HEAD, and every branch stay
//! as they are. A path that differs from the transfer's `since` commit refuses the whole
//! transfer before any write.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::delivery::{self, Kind};
use crate::error::{Error, Result};
use crate::inbox::Sender;
use crate::ledger::Ledger;
use crate::shell::Shell;

const INTENT: &str = "applying.json";
const RECEIPT: &str = "applied.json";
/// How many conflicting paths a refusal names.
const NAMED: usize = 20;

/// What one apply did, saved so that a second apply writes nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Receipt {
    pub(crate) version: u32,
    pub(crate) id: String,
    pub(crate) checkout: String,
    /// The recovery snapshot of the checkout as it was before the first write.
    pub(crate) recovery: String,
    pub(crate) paths: Vec<String>,
    pub(crate) at_ms: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct Applied {
    pub(crate) id: String,
    /// False for `--check`, and for a transfer that changes no file.
    pub(crate) applied: bool,
    /// True when an earlier apply already did the work and this call wrote nothing.
    pub(crate) replayed: bool,
    pub(crate) paths: Vec<String>,
    pub(crate) recovery: Option<String>,
}

impl Applied {
    pub(crate) fn text(&self) -> String {
        let verb = match (self.applied, self.replayed) {
            (_, true) => "was already applied; nothing was written",
            (true, false) => "applied to working files; the index and HEAD are unchanged",
            (false, false) => "applies cleanly; nothing was written",
        };
        let mut text = format!("Transfer {} {verb}.\n", self.id);
        for path in &self.paths {
            text.push_str(path);
            text.push('\n');
        }
        if let Some(recovery) = &self.recovery {
            text.push_str("Recovery snapshot of the earlier state: ");
            text.push_str(recovery);
            text.push('\n');
        }
        text
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

/// One side of a changed path: its Git mode and blob.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Blob {
    mode: String,
    oid: String,
}

#[derive(Debug)]
struct Change {
    path: PathBuf,
    /// The path is absent from the tree compared from.
    added: bool,
    after: Option<Blob>,
}

fn side(mode: &str, oid: &str) -> Option<Blob> {
    (mode != "000000").then(|| Blob {
        mode: mode.into(),
        oid: oid.into(),
    })
}

/// The paths that differ between two trees, with the state of each in `to`.
fn changes(shell: &dyn Shell, checkout: &Path, from: &str, to: &str) -> Result<Vec<Change>> {
    let output = crate::preservation::raw(
        shell,
        checkout,
        &[
            "diff",
            "--raw",
            "-z",
            "--no-renames",
            "--no-abbrev",
            "--no-ext-diff",
            from,
            to,
        ],
    )
    .map_err(|_| invalid("the transfer cannot be compared with this checkout"))?;
    let malformed = || invalid("Git reported a change this command cannot read");
    let mut fields = output
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty());
    let mut found = Vec::new();
    while let Some(header) = fields.next() {
        let header = std::str::from_utf8(header).map_err(|_| malformed())?;
        let parts: Vec<&str> = header.trim_start_matches(':').split(' ').collect();
        let [before_mode, after_mode, _, after_oid, _] = parts[..] else {
            return Err(malformed());
        };
        let path = PathBuf::from(OsStr::from_bytes(fields.next().ok_or_else(malformed)?));
        if before_mode == "160000" || after_mode == "160000" {
            return Err(invalid(format!(
                "{} is a submodule; a transfer cannot apply a submodule change",
                path.display()
            )));
        }
        if !path.components().all(
            |part| matches!(part, Component::Normal(name) if !name.eq_ignore_ascii_case(".git")),
        ) {
            return Err(invalid(format!(
                "{} is not a path inside the checkout",
                path.display()
            )));
        }
        found.push(Change {
            path,
            added: before_mode == "000000",
            after: side(after_mode, after_oid),
        });
    }
    Ok(found)
}

/// Whether anything is at `path`, including a dangling symlink and an ignored file.
fn occupied(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// The touched paths that this checkout cannot take: paths whose content differs from
/// `since`, and paths below something that is not a plain directory.
fn conflicts(
    shell: &dyn Shell,
    root: &Path,
    since: &str,
    worktree_tree: &str,
    touched: &[Change],
) -> Result<Vec<String>> {
    let local: BTreeSet<PathBuf> = changes(shell, root, since, worktree_tree)?
        .into_iter()
        .map(|change| change.path)
        .collect();
    let mut refused = Vec::new();
    for change in touched {
        let blocked_parent = change.path.ancestors().skip(1).any(|parent| {
            !parent.as_os_str().is_empty()
                && fs::symlink_metadata(root.join(parent)).is_ok_and(|found| !found.is_dir())
        });
        // An ignored file is absent from the captured tree, so an added path is also
        // checked where it will be written.
        let hidden = change.added && occupied(&root.join(&change.path));
        if local.contains(&change.path) || blocked_parent || hidden {
            refused.push(change.path.display().to_string());
        }
    }
    Ok(refused)
}

fn write_file(root: &Path, change: &Change, blob: &Blob, bytes: &[u8], id: &str) -> Result<()> {
    let target = root.join(&change.path);
    let mut parent = root.to_path_buf();
    for part in change.path.parent().into_iter().flat_map(Path::components) {
        parent.push(part);
        match fs::symlink_metadata(&parent) {
            Ok(found) if found.is_dir() => {}
            Ok(_) => {
                return Err(invalid(format!(
                    "{} is no longer a directory",
                    parent.display()
                )));
            }
            Err(_) => fs::create_dir(&parent)?,
        }
    }
    // Write beside the target and rename, so a reader never sees a partial file and a
    // symlink at the target is replaced, not followed.
    let name = target
        .file_name()
        .ok_or_else(|| invalid("the changed path has no file name"))?;
    let mut temporary_name = OsStr::new(".dogsled-transfer-").to_os_string();
    temporary_name.push(id);
    temporary_name.push("-");
    temporary_name.push(name);
    let temporary = parent.join(temporary_name);
    if blob.mode == "120000" {
        std::os::unix::fs::symlink(OsStr::from_bytes(bytes), &temporary)?;
    } else {
        fs::write(&temporary, bytes)?;
        let mode = if blob.mode == "100755" { 0o755 } else { 0o644 };
        fs::set_permissions(&temporary, fs::Permissions::from_mode(mode))?;
    }
    fs::rename(temporary, target)?;
    Ok(())
}

/// Remove deleted paths first, so a file can become a directory, then write the rest.
fn write_all(shell: &dyn Shell, root: &Path, touched: &[Change], id: &str) -> Result<()> {
    for change in touched.iter().filter(|change| change.after.is_none()) {
        fs::remove_file(root.join(&change.path))?;
    }
    for change in touched {
        let Some(blob) = &change.after else { continue };
        let bytes = crate::preservation::raw(shell, root, &["cat-file", "blob", &blob.oid])
            .map_err(|_| invalid("a transferred file is missing from this checkout"))?;
        write_file(root, change, blob, &bytes, id)?;
    }
    Ok(())
}

fn save<T: Serialize>(dir: &Path, name: &str, value: &T) -> Result<()> {
    let temporary = dir.join(format!("{name}.tmp"));
    fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    fs::rename(temporary, dir.join(name))?;
    Ok(())
}

fn saved(dir: &Path, name: &str) -> Result<Option<Receipt>> {
    match fs::read(dir.join(name)) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn replay(id: &str, receipt: Receipt) -> Applied {
    Applied {
        id: id.into(),
        applied: true,
        replayed: true,
        paths: receipt.paths,
        recovery: Some(receipt.recovery),
    }
}

/// The checkout as it is now, as a tree of raw bytes. Nothing is pinned.
fn current_tree(shell: &dyn Shell, root: &Path) -> Result<String> {
    crate::preservation::trees(shell, root)
        .map(|trees| trees.worktree_tree)
        .map_err(|error| invalid(format!("this checkout cannot take a transfer now: {error}")))
}

/// What an earlier apply settles: a saved receipt replays, and an unfinished one refuses.
fn earlier(dir: &Path, delivery: &delivery::Delivery, caller: &Sender) -> Result<Option<Applied>> {
    let id = &delivery.id;
    if delivery.kind != Kind::Changes {
        return Err(invalid(format!(
            "transfer {id} holds files, not changes; read them with `dogsled transfer show {id}`"
        )));
    }
    if caller.pane.is_some() && caller.terminal.as_deref() != Some(&delivery.to.terminal) {
        return Err(invalid(format!(
            "transfer {id} was sent to another agent; only its recipient can apply it"
        )));
    }
    if let Some(receipt) = saved(dir, RECEIPT)? {
        return Ok(Some(replay(id, receipt)));
    }
    if let Some(intent) = saved(dir, INTENT)? {
        return Err(invalid(format!(
            "an earlier apply of transfer {id} did not finish in {}; compare it with recovery snapshot {} (`dogsled recovery show {} --repo {}`), then drop the transfer",
            intent.checkout, intent.recovery, intent.recovery, intent.checkout
        )));
    }
    Ok(None)
}

fn refusal(id: &str, since: &str, refused: &[String], total: usize) -> Error {
    let more = refused.len().saturating_sub(NAMED);
    invalid(format!(
        "transfer {id} does not apply: {} of its {total} paths differ here from {since}: {}{}; nothing was written",
        refused.len(),
        refused
            .iter()
            .take(NAMED)
            .cloned()
            .collect::<Vec<_>>()
            .join(", "),
        if more == 0 {
            String::new()
        } else {
            format!(", and {more} more")
        }
    ))
}

/// Apply transfer `id` to the checkout that contains `cwd`. `check` reports and writes no
/// file. The transfer's objects and its ref are imported in both cases.
pub(crate) fn apply(
    shell: &dyn Shell,
    ledger: &Ledger,
    cwd: &Path,
    caller: &Sender,
    id: &str,
    check: bool,
    now: u64,
) -> Result<Applied> {
    let (dir, delivery) = delivery::load(ledger, id)?;
    if let Some(settled) = earlier(&dir, &delivery, caller)? {
        return Ok(settled);
    }
    let root = PathBuf::from(
        super::checked(shell, cwd, &["rev-parse", "--show-toplevel"])
            .map_err(|_| invalid("run `transfer apply` inside the checkout that receives it"))?,
    );
    let source = super::import(shell, ledger, &root, id)?;
    let since = delivery
        .since
        .ok_or_else(|| invalid(format!("transfer {id} records no `since` commit")))?;
    super::resolve(shell, &root, &since)
        .map_err(|_| invalid(format!("this checkout does not have commit {since}")))?;
    let touched = changes(shell, &root, &since, source.pinned())?;
    let paths: Vec<String> = touched
        .iter()
        .map(|change| change.path.display().to_string())
        .collect();
    let before = current_tree(shell, &root)?;
    let refused = conflicts(shell, &root, &since, &before, &touched)?;
    if !refused.is_empty() {
        return Err(refusal(id, &since, &refused, touched.len()));
    }
    if check || touched.is_empty() {
        return Ok(Applied {
            id: id.into(),
            applied: false,
            replayed: false,
            paths,
            recovery: None,
        });
    }
    let snapshot = crate::preservation::snapshot(shell, &root)
        .map_err(|error| invalid(format!("this checkout cannot take a transfer now: {error}")))?;
    if snapshot.worktree_tree != before {
        return Err(invalid(
            "the checkout changed while the transfer was checked; nothing was written",
        ));
    }
    let receipt = Receipt {
        version: 1,
        id: id.into(),
        checkout: root.display().to_string(),
        recovery: snapshot.id,
        paths,
        at_ms: now,
    };
    save(&dir, INTENT, &receipt)?;
    write_all(shell, &root, &touched, id)?;
    save(&dir, RECEIPT, &receipt)?;
    fs::remove_file(dir.join(INTENT))?;
    Ok(Applied {
        id: id.into(),
        applied: true,
        replayed: false,
        paths: receipt.paths,
        recovery: Some(receipt.recovery),
    })
}
