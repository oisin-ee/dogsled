//! Agent-to-agent delivery of saved files and changes. Delivery stores a transfer and puts one
//! notice in the recipient's inbox. It never applies anything to the recipient's checkout.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::hosts::Fleet;
use crate::inbox::Sender;
use crate::ledger::Ledger;
use crate::shell::Shell;

/// The largest single attachment.
pub(crate) const MAX_ATTACHMENT_BYTES: u64 = 10 * 1024 * 1024;
/// The inbox notice stays far below the inbox text limit, whatever the size of the change.
const SUMMARY_LINES: usize = 40;
const FILE: &str = "delivery.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    /// Named files, read where they are stored. They never enter a checkout.
    Files,
    /// A pinned source and the commit the recipient compares it with.
    Changes,
}

/// The agent a transfer is for. The terminal, not the pane, is its identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Recipient {
    pub(crate) host: String,
    pub(crate) pane: String,
    pub(crate) terminal: String,
    pub(crate) name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Attachment {
    pub(crate) name: String,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Delivery {
    pub(crate) version: u32,
    pub(crate) id: String,
    pub(crate) kind: Kind,
    pub(crate) from: Sender,
    pub(crate) to: Recipient,
    /// For changes: the commit the pinned source is compared with.
    pub(crate) since: Option<String>,
    pub(crate) files: Vec<Attachment>,
    pub(crate) summary: String,
    pub(crate) created_ms: u64,
}

/// What `transfer send` carries.
#[derive(Debug)]
pub(crate) enum Content {
    Files(Vec<PathBuf>),
    Changes {
        since: Option<String>,
        include_worktree: bool,
    },
}

/// A stored transfer and the inbox record that announces it.
#[derive(Debug, Serialize)]
pub(crate) struct Sent {
    pub(crate) id: String,
    pub(crate) kind: Kind,
    pub(crate) to: Recipient,
    pub(crate) summary: String,
    pub(crate) notice: String,
}

impl Sent {
    pub(crate) fn line(&self) -> String {
        format!(
            "Sent transfer {} to {} ({}).\n{}\n",
            self.id,
            self.to.name.as_deref().unwrap_or(&self.to.pane),
            self.to.pane,
            self.summary
        )
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

fn root(ledger: &Ledger) -> PathBuf {
    ledger.dir.join("transfers")
}

fn write(dir: &Path, delivery: &Delivery) -> Result<()> {
    let temporary = dir.join(format!("{FILE}.tmp"));
    fs::write(&temporary, serde_json::to_vec_pretty(delivery)?)?;
    fs::rename(temporary, dir.join(FILE))?;
    Ok(())
}

fn load(ledger: &Ledger, id: &str) -> Result<(PathBuf, Delivery)> {
    let dir = super::directory(ledger, id)?;
    let bytes = fs::read(dir.join(FILE)).map_err(|_| invalid(format!("no transfer {id}")))?;
    let delivery: Delivery = serde_json::from_slice(&bytes)
        .map_err(|_| invalid(format!("transfer {id} has an invalid delivery record")))?;
    if delivery.version != 1 || delivery.id != id {
        return Err(invalid(format!(
            "transfer {id} has an unsupported delivery record"
        )));
    }
    Ok((dir, delivery))
}

/// Copy each named regular file into `dir/files`. A symlink, a directory, a path inside
/// `.git`, a repeated name, or an oversized file refuses the whole transfer.
fn attach(paths: &[PathBuf], dir: &Path) -> Result<Vec<Attachment>> {
    if paths.is_empty() {
        return Err(invalid("name at least one file"));
    }
    let files = dir.join("files");
    fs::create_dir(&files)?;
    let mut attachments: Vec<Attachment> = Vec::new();
    for path in paths {
        let shown = path.display();
        let metadata =
            fs::symlink_metadata(path).map_err(|_| invalid(format!("{shown} cannot be read")))?;
        if !metadata.is_file() {
            return Err(invalid(format!("{shown} is not a regular file")));
        }
        if path.components().any(|part| part.as_os_str() == ".git") {
            return Err(invalid(format!("{shown} is inside a Git directory")));
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| invalid(format!("{shown} has no usable file name")))?
            .to_owned();
        if attachments.iter().any(|attachment| attachment.name == name) {
            return Err(invalid(format!("two files are named {name}")));
        }
        let copy = files.join(&name);
        fs::copy(path, &copy)?;
        // Measure the stored copy: the source may have changed after the first check.
        let stored = super::digest(&copy)?;
        if stored.bytes > MAX_ATTACHMENT_BYTES {
            return Err(invalid(format!(
                "{shown} is larger than {} MiB",
                MAX_ATTACHMENT_BYTES / (1024 * 1024)
            )));
        }
        attachments.push(Attachment {
            name,
            bytes: stored.bytes,
            sha256: stored.sha256,
        });
    }
    Ok(attachments)
}

/// The commit a change transfer is compared with: `since`, else where this checkout left
/// the default branch.
fn since_commit(shell: &dyn Shell, checkout: &Path, since: Option<&str>) -> Result<String> {
    if let Some(revision) = since {
        return super::resolve(shell, checkout, revision)
            .map_err(|_| invalid(format!("--since {revision} does not resolve to a commit")));
    }
    let base = crate::integration::base(shell, &checkout.display().to_string())?;
    super::checked(shell, checkout, &["merge-base", "HEAD", &base])
        .map_err(|_| invalid("no default branch to compare with; pass --since"))
}

fn bounded(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= SUMMARY_LINES {
        return lines.join("\n");
    }
    // The last line of a diffstat is its total.
    let mut kept: Vec<String> = lines
        .iter()
        .take(SUMMARY_LINES - 2)
        .map(ToString::to_string)
        .collect();
    kept.push(format!(
        " ... {} more files",
        lines.len() - SUMMARY_LINES + 1
    ));
    kept.extend(lines.last().map(ToString::to_string));
    kept.join("\n")
}

/// Remove a transfer this call created, with its ref in `checkout`.
fn discard(shell: &dyn Shell, ledger: &Ledger, checkout: &Path, id: &str) {
    if let Ok(manifest) = super::load(ledger, id) {
        super::unpin(shell, checkout, &manifest);
    }
    if let Ok(dir) = super::directory(ledger, id) {
        let _ = fs::remove_dir_all(dir);
    }
}

fn notice(delivery: &Delivery, dir: &Path) -> String {
    let (what, how) = match delivery.kind {
        Kind::Files => (
            "files",
            format!("The files are in {}.", dir.join("files").display()),
        ),
        Kind::Changes => (
            "changes",
            "Nothing was applied to your checkout.".to_owned(),
        ),
    };
    format!(
        "Transfer {id} ({what}).\n{summary}\n{how}\nInspect it with `dogsled transfer show {id}`.",
        id = delivery.id,
        summary = delivery.summary
    )
}

fn store_files(ledger: &Ledger, paths: &[PathBuf], mut delivery: Delivery) -> Result<Delivery> {
    let id = super::random_id()?;
    let destination = super::directory(ledger, &id)?;
    fs::create_dir_all(root(ledger))?;
    let temporary = super::Temporary(destination.with_file_name(format!(".{id}.tmp")));
    fs::create_dir(&temporary.0)?;
    delivery.files = attach(paths, &temporary.0)?;
    delivery.summary = delivery
        .files
        .iter()
        .map(|file| format!(" {} ({} bytes)", file.name, file.bytes))
        .collect::<Vec<_>>()
        .join("\n");
    delivery.id = id;
    write(&temporary.0, &delivery)?;
    fs::rename(&temporary.0, destination)?;
    Ok(delivery)
}

fn store_changes(
    fleet: &Fleet<'_>,
    checkout: &Path,
    since: Option<&str>,
    include_worktree: bool,
    mut delivery: Delivery,
) -> Result<Delivery> {
    let since = since_commit(fleet.shell, checkout, since)?;
    let manifest = super::capture(
        fleet.shell,
        fleet.ledger,
        checkout,
        &super::Options {
            include_worktree,
            base: None,
        },
    )?;
    delivery.id.clone_from(&manifest.id);
    let stored = (|| {
        let stat = super::checked(
            fleet.shell,
            checkout,
            &[
                "diff",
                "--stat",
                "--stat-width=100",
                "--no-color",
                "--no-ext-diff",
                &since,
                manifest.pinned(),
            ],
        )?;
        if stat.is_empty() {
            return Err(invalid(format!(
                "nothing to send: no change since {since}; commit your work or pass --include-worktree"
            )));
        }
        delivery.summary = bounded(&stat);
        delivery.since = Some(since);
        write(&super::directory(fleet.ledger, &manifest.id)?, &delivery)
    })();
    if let Err(error) = stored {
        discard(fleet.shell, fleet.ledger, checkout, &manifest.id);
        return Err(error);
    }
    Ok(delivery)
}

/// Store `content` and announce it to `target`. A failure leaves no transfer behind.
pub(crate) fn send(
    fleet: &Fleet<'_>,
    checkout: &Path,
    target: &str,
    host: Option<&str>,
    content: &Content,
    now: u64,
) -> Result<Sent> {
    let to = crate::inbox::target(fleet, target, host)?;
    if !to.host.is_local() {
        return Err(invalid(format!(
            "{} is on {}; a transfer to another host is not available yet",
            to.agent.pane, to.host.label
        )));
    }
    let terminal = to
        .agent
        .terminal_id
        .clone()
        .ok_or_else(|| invalid("the target has no terminal id"))?;
    let from = crate::inbox::caller(fleet.shell);
    let delivery = Delivery {
        version: 1,
        id: String::new(),
        kind: match content {
            Content::Files(_) => Kind::Files,
            Content::Changes { .. } => Kind::Changes,
        },
        from: from.clone(),
        to: Recipient {
            host: to.host.id.clone(),
            pane: to.agent.pane.clone(),
            terminal,
            name: to.agent.name.clone(),
        },
        since: None,
        files: Vec::new(),
        summary: String::new(),
        created_ms: now,
    };
    let delivery = match content {
        Content::Files(paths) => store_files(fleet.ledger, paths, delivery)?,
        Content::Changes {
            since,
            include_worktree,
        } => store_changes(
            fleet,
            checkout,
            since.as_deref(),
            *include_worktree,
            delivery,
        )?,
    };
    let dir = super::directory(fleet.ledger, &delivery.id)?;
    let text = notice(&delivery, &dir);
    match crate::inbox::send(fleet, &to, from, "transfer", text, now) {
        Ok(record) => Ok(Sent {
            id: delivery.id,
            kind: delivery.kind,
            to: delivery.to,
            summary: delivery.summary,
            notice: record.id,
        }),
        Err(error) => {
            discard(fleet.shell, fleet.ledger, checkout, &delivery.id);
            Err(error)
        }
    }
}

/// One row of `transfer list`.
#[derive(Debug, Serialize)]
pub(crate) struct Line {
    pub(crate) id: String,
    pub(crate) kind: Kind,
    /// `received`, `sent`, or `other` for a caller that is neither end.
    pub(crate) direction: &'static str,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) created_ms: u64,
}

/// The transfers stored on this host that `caller` sent or received. A caller without a
/// terminal, such as a plain shell, sees them all.
pub(crate) fn list(ledger: &Ledger, caller: &Sender) -> Result<Vec<Line>> {
    let entries = match fs::read_dir(root(ledger)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut lines = Vec::new();
    for entry in entries {
        let name = entry?.file_name();
        // A capture without a delivery record, or an unfinished one, is not a transfer yet.
        let Some(Ok((_, delivery))) = name.to_str().map(|id| load(ledger, id)) else {
            continue;
        };
        let direction = match caller.terminal.as_deref() {
            Some(terminal) if terminal == delivery.to.terminal => "received",
            Some(terminal) if delivery.from.terminal.as_deref() == Some(terminal) => "sent",
            Some(_) => continue,
            None => "other",
        };
        lines.push(Line {
            id: delivery.id,
            kind: delivery.kind,
            direction,
            from: delivery
                .from
                .name
                .or(delivery.from.pane)
                .unwrap_or(delivery.from.host),
            to: delivery.to.name.unwrap_or(delivery.to.pane),
            created_ms: delivery.created_ms,
        });
    }
    lines.sort_by(|a, b| (a.created_ms, &a.id).cmp(&(b.created_ms, &b.id)));
    Ok(lines)
}

/// A transfer, the paths of its files, and the manifest of its pinned source.
#[derive(Debug, Serialize)]
pub(crate) struct Shown {
    #[serde(flatten)]
    pub(crate) delivery: Delivery,
    pub(crate) paths: Vec<String>,
    pub(crate) source: Option<super::Manifest>,
}

impl Shown {
    pub(crate) fn text(&self) -> String {
        let mut text = format!(
            "Transfer {} from {}\n{}\n",
            self.delivery.id,
            self.delivery
                .from
                .name
                .as_deref()
                .or(self.delivery.from.pane.as_deref())
                .unwrap_or(&self.delivery.from.host),
            self.delivery.summary
        );
        for path in &self.paths {
            text.push_str(path);
            text.push('\n');
        }
        if let (Some(since), Some(source)) = (&self.delivery.since, &self.source) {
            let _ = writeln!(text, "since {since}\nsource {}", source.pinned());
        }
        text
    }
}

/// Read a transfer and prove that each stored file still has its recorded bytes.
pub(crate) fn show(ledger: &Ledger, id: &str) -> Result<Shown> {
    let (dir, delivery) = load(ledger, id)?;
    let mut paths = Vec::new();
    for file in &delivery.files {
        let path = dir.join("files").join(&file.name);
        let stored = super::digest(&path)?;
        if stored.bytes != file.bytes || stored.sha256 != file.sha256 {
            return Err(invalid(format!(
                "transfer {id}: {} no longer matches its recorded digest",
                file.name
            )));
        }
        paths.push(path.display().to_string());
    }
    let source = match delivery.kind {
        Kind::Files => None,
        Kind::Changes => Some(super::load(ledger, id)?),
    };
    Ok(Shown {
        delivery,
        paths,
        source,
    })
}

#[derive(Debug, Serialize)]
pub(crate) struct Removed {
    pub(crate) id: String,
    pub(crate) removed: bool,
}

/// End a transfer. Only its sender, its recipient, or a caller outside any agent pane may.
/// The ref is removed from `checkout` only when it still names this transfer's source.
pub(crate) fn remove(
    shell: &dyn Shell,
    ledger: &Ledger,
    checkout: &Path,
    caller: &Sender,
    id: &str,
) -> Result<Removed> {
    let (dir, delivery) = load(ledger, id)?;
    let ours = caller.terminal.as_deref().is_some_and(|terminal| {
        terminal == delivery.to.terminal || delivery.from.terminal.as_deref() == Some(terminal)
    });
    if caller.pane.is_some() && !ours {
        return Err(invalid(format!(
            "transfer {id} belongs to another agent; its sender or recipient can drop it"
        )));
    }
    if let Ok(manifest) = super::load(ledger, id) {
        super::unpin(shell, checkout, &manifest);
    }
    let parked = dir.with_file_name(format!(".{id}.dropped"));
    fs::rename(&dir, &parked)?;
    fs::remove_dir_all(parked)?;
    Ok(Removed {
        id: id.into(),
        removed: true,
    })
}
