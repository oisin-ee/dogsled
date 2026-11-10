//! Carry a stored transfer to a peer host. The envelope travels on stdin only: one JSON header
//! line, then the bytes of each part in header order. Both ends hold the envelope in memory.

use std::fs;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::delivery::{self, Delivery, Kind, MAX_ATTACHMENT_BYTES};
use crate::backoff::Backoff;
use crate::error::{Error, Result};
use crate::hosts::{Fleet, Host};
use crate::ledger::Ledger;
use crate::shell::Shell;

const MANIFEST: &str = "manifest.json";
const PAYLOAD: &str = "payload.bundle";
const INGRESS: &str = "ingress.json";
const MAX_HEADER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Part {
    name: String,
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    version: u32,
    delivery: Delivery,
    parts: Vec<Part>,
}

/// What the peer saved about one envelope, so that the same envelope replays.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ingress {
    version: u32,
    header_sha256: String,
}

/// Why a peer stored nothing. A fixed code: no text crosses between hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reason {
    /// The envelope is malformed, oversized, or does not match its digests.
    Invalid,
    /// Another envelope already holds this transfer id.
    Conflict,
    /// The recipient's terminal is gone, or it has no inbox reader.
    Undeliverable,
    Storage,
}

impl Reason {
    fn text(self) -> &'static str {
        match self {
            Self::Invalid => "the transfer did not pass the peer's checks",
            Self::Conflict => "the peer already holds another transfer with this id",
            Self::Undeliverable => {
                "the recipient is not running there or has no dogsled inbox reader"
            }
            Self::Storage => "the peer could not store the transfer",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reply {
    Stored { id: String, notice: String },
    Refused { reason: Reason },
}

/// How a push ended for the sender.
#[derive(Debug)]
pub(crate) enum Pushed {
    Stored(String),
    Refused(&'static str),
    /// No valid reply after two tries. The peer may hold the transfer.
    Uncertain,
}

fn hash(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

fn pack(ledger: &Ledger, id: &str) -> Result<Vec<u8>> {
    let (dir, delivery) = delivery::load(ledger, id)?;
    let mut names: Vec<String> = [MANIFEST, PAYLOAD]
        .into_iter()
        .filter(|name| dir.join(name).is_file())
        .map(str::to_owned)
        .collect();
    names.extend(
        delivery
            .files
            .iter()
            .map(|file| format!("files/{}", file.name)),
    );
    let mut parts = Vec::new();
    let mut body = Vec::new();
    for name in names {
        let bytes = fs::read(dir.join(&name))?;
        parts.push(Part {
            name,
            bytes: bytes.len() as u64,
            sha256: hash(&bytes),
        });
        body.extend(bytes);
    }
    let mut envelope = serde_json::to_vec(&Header {
        version: 1,
        delivery,
        parts,
    })?;
    envelope.push(b'\n');
    envelope.extend(body);
    Ok(envelope)
}

/// Send stored transfer `id` to `host`. A peer without the capability gets nothing.
pub(crate) fn push(fleet: &Fleet<'_>, host: &Host, id: &str) -> Result<Pushed> {
    let report: crate::node::SnapshotReport = fleet.remote(host, &["snapshot".into()])?;
    if report.transfer != Some(crate::node::TRANSFER_VERSION) {
        return Err(Error::Invalid(format!(
            "{} runs a dogsled without transfer support; nothing was sent",
            host.label
        )));
    }
    let envelope = pack(fleet.ledger, id)?;
    let args = ["transfer-put".into(), "--input".into(), "-".into()];
    for _ in 0..2 {
        let timeout = fleet
            .shell
            .budget()
            .with_limit("ssh", crate::budget::SETUP_LIMIT)?;
        let reply = fleet
            .shell
            .run_input_bounded("ssh", &fleet.remote_argv(host, &args), &envelope, timeout)
            .ok()
            .filter(crate::shell::Output::ok)
            .and_then(|output| serde_json::from_str::<Reply>(&output.stdout).ok());
        match reply {
            Some(Reply::Stored { id: stored, notice }) if stored == id => {
                return Ok(Pushed::Stored(notice));
            }
            Some(Reply::Refused { reason }) => return Ok(Pushed::Refused(reason.text())),
            Some(Reply::Stored { .. }) | None => {}
        }
    }
    Ok(Pushed::Uncertain)
}

/// A part is the manifest, the bundle, or one attachment directly inside `files/`.
fn attachment(name: &str) -> Option<&str> {
    let file = name.strip_prefix("files/")?;
    let mut parts = Path::new(file).components();
    matches!(
        (parts.next(), parts.next()),
        (Some(Component::Normal(_)), None)
    )
    .then_some(file)
}

/// Check the whole envelope before any write, and return each part with its bytes.
fn checked<'a>(header: &Header, body: &'a [u8]) -> Option<Vec<(&'a [u8], String)>> {
    let delivery = &header.delivery;
    if header.version != 1 || delivery.version != 1 || super::validate_id(&delivery.id).is_err() {
        return None;
    }
    let mut rest = body;
    let mut found = Vec::new();
    let mut attached = Vec::new();
    for part in &header.parts {
        let length = usize::try_from(part.bytes).ok()?;
        let (bytes, tail) = rest.split_at_checked(length)?;
        rest = tail;
        if hash(bytes) != part.sha256 || found.iter().any(|(_, name)| *name == part.name) {
            return None;
        }
        match (delivery.kind, part.name.as_str(), attachment(&part.name)) {
            (Kind::Changes, MANIFEST | PAYLOAD, _) => {}
            (Kind::Files, _, Some(file)) if part.bytes <= MAX_ATTACHMENT_BYTES => {
                attached.push((file, part.bytes, part.sha256.as_str()));
            }
            _ => return None,
        }
        found.push((bytes, part.name.clone()));
    }
    let recorded: Vec<_> = delivery
        .files
        .iter()
        .map(|file| (file.name.as_str(), file.bytes, file.sha256.as_str()))
        .collect();
    let has_manifest = found.iter().any(|(_, name)| name == MANIFEST);
    (rest.is_empty() && attached == recorded && has_manifest == (delivery.kind == Kind::Changes))
        .then_some(found)
}

/// The bundle part must be exactly what the manifest records, or absent with it.
fn manifest_agrees(dir: &Path, id: &str, parts: &[(&[u8], String)]) -> bool {
    let Some((bytes, _)) = parts.iter().find(|(_, name)| name == MANIFEST) else {
        return true;
    };
    let Ok(manifest) = serde_json::from_slice::<super::Manifest>(bytes) else {
        return false;
    };
    let payload = parts.iter().find(|(_, name)| name == PAYLOAD);
    let agrees = match (&manifest.bundle, payload) {
        (None, None) => true,
        (Some(bundle), Some((bytes, _))) => {
            bundle.bytes == bytes.len() as u64 && bundle.sha256 == hash(bytes)
        }
        _ => false,
    };
    agrees && super::validate(&manifest, id).is_ok() && dir.join(MANIFEST).is_file()
}

fn notify(shell: &dyn Shell, ledger: &Ledger, delivery: &Delivery, dir: &Path) -> Result<String> {
    let record = crate::inbox::Record {
        id: format!("transfer-{}", delivery.id),
        terminal: delivery.to.terminal.clone(),
        pane: delivery.to.pane.clone(),
        from: delivery.from.clone(),
        kind: "transfer".into(),
        text: delivery::notice(delivery, dir),
        created_ms: crate::ledger::now_ms(),
        state: crate::inbox::State::Pending,
        lease: Backoff::default(),
        reason: None,
        done_ms: None,
    };
    Ok(crate::inbox::deliver(ledger, shell, record)?.id)
}

fn store(
    ledger: &Ledger,
    header: &Header,
    digest: &str,
    parts: &[(&[u8], String)],
) -> Result<bool> {
    let id = &header.delivery.id;
    let destination = super::directory(ledger, id)?;
    fs::create_dir_all(ledger.dir.join("transfers"))?;
    let temporary = super::Temporary(destination.with_file_name(format!(".{id}.tmp")));
    fs::create_dir(&temporary.0)?;
    fs::create_dir(temporary.0.join("files"))?;
    for (bytes, name) in parts {
        fs::write(temporary.0.join(name), bytes)?;
    }
    if header.delivery.files.is_empty() {
        fs::remove_dir(temporary.0.join("files"))?;
    }
    if !manifest_agrees(&temporary.0, id, parts) {
        return Ok(false);
    }
    delivery::write(&temporary.0, &header.delivery)?;
    fs::write(
        temporary.0.join(INGRESS),
        serde_json::to_vec(&Ingress {
            version: 1,
            header_sha256: digest.into(),
        })?,
    )?;
    fs::rename(&temporary.0, destination)?;
    Ok(true)
}

/// Store one envelope and announce it. The same envelope again returns the same reply and
/// stores nothing new. Any other envelope with that id is a conflict.
pub(crate) fn receive(shell: &dyn Shell, ledger: &Ledger, input: &[u8]) -> Reply {
    let refused = |reason| Reply::Refused { reason };
    let Some(end) = input
        .iter()
        .take(MAX_HEADER_BYTES)
        .position(|byte| *byte == b'\n')
    else {
        return refused(Reason::Invalid);
    };
    let (line, body) = (&input[..end], &input[end + 1..]);
    let Ok(header) = serde_json::from_slice::<Header>(line) else {
        return refused(Reason::Invalid);
    };
    let Some(parts) = checked(&header, body) else {
        return refused(Reason::Invalid);
    };
    let id = header.delivery.id.clone();
    let digest = hash(line);
    let outcome = super::locked(ledger, &id, || {
        let dir = super::directory(ledger, &id)?;
        if dir.exists() {
            let same = fs::read(dir.join(INGRESS))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Ingress>(&bytes).ok())
                .is_some_and(|saved| saved.version == 1 && saved.header_sha256 == digest);
            if !same {
                return Ok(refused(Reason::Conflict));
            }
        } else {
            match store(ledger, &header, &digest, &parts) {
                Ok(true) => {}
                Ok(false) => return Ok(refused(Reason::Invalid)),
                Err(_) => return Ok(refused(Reason::Storage)),
            }
        }
        // The inbox keeps one record per id, so a replay announces nothing twice.
        let Ok(notice) = notify(shell, ledger, &header.delivery, &dir) else {
            let parked = dir.with_file_name(format!(".{id}.dropped"));
            let _ = fs::rename(&dir, &parked).and_then(|()| fs::remove_dir_all(parked));
            return Ok(refused(Reason::Undeliverable));
        };
        Ok(Reply::Stored {
            id: id.clone(),
            notice,
        })
    });
    outcome.unwrap_or_else(|_| refused(Reason::Storage))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::System;
    use std::path::PathBuf;

    const ID: &str = "0123456789abcdef0123456789abcdef";

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> (Self, Ledger) {
            let dir =
                std::env::temp_dir().join(format!("dogsled-peer-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            (Self(dir.clone()), Ledger::new(dir))
        }

        fn stored(&self) -> Vec<String> {
            fs::read_dir(self.0.join("transfers")).map_or_else(
                |_| Vec::new(),
                |entries| {
                    entries
                        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                        .collect()
                },
            )
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn files(body: &[u8]) -> Header {
        Header {
            version: 1,
            delivery: Delivery {
                version: 1,
                id: ID.into(),
                kind: Kind::Files,
                from: crate::inbox::Sender::default(),
                to: delivery::Recipient {
                    host: "peer".into(),
                    pane: "w9:p1".into(),
                    terminal: "term_peer".into(),
                    name: None,
                },
                since: None,
                files: vec![delivery::Attachment {
                    name: "a.txt".into(),
                    bytes: body.len() as u64,
                    sha256: hash(body),
                }],
                summary: String::new(),
                created_ms: 1,
            },
            parts: vec![Part {
                name: "files/a.txt".into(),
                bytes: body.len() as u64,
                sha256: hash(body),
            }],
        }
    }

    fn envelope(header: &Header, body: &[u8]) -> Vec<u8> {
        let mut envelope = serde_json::to_vec(header).unwrap();
        envelope.push(b'\n');
        envelope.extend(body);
        envelope
    }

    fn refuses(name: &str, input: &[u8], reason: Reason) {
        let (scratch, ledger) = Scratch::new(name);
        assert_eq!(
            receive(&System::default(), &ledger, input),
            Reply::Refused { reason },
            "{name}"
        );
        assert_eq!(scratch.stored(), Vec::<String>::new(), "{name}");
    }

    #[test]
    fn an_envelope_that_fails_a_check_stores_nothing() {
        let body = b"hello";
        refuses("no-newline", b"{}", Reason::Invalid);
        refuses("not-json", b"nope\nhello", Reason::Invalid);
        let mut changed = envelope(&files(body), body);
        *changed.last_mut().unwrap() = b'x';
        refuses("changed-byte", &changed, Reason::Invalid);
        let mut extra = envelope(&files(body), body);
        extra.push(b'!');
        refuses("extra-bytes", &extra, Reason::Invalid);
        refuses(
            "short-body",
            &envelope(&files(body), b"hell"),
            Reason::Invalid,
        );
        for name in [
            "files/../x",
            "files/a/b",
            "files/",
            "delivery.json",
            "manifest.json",
        ] {
            let mut header = files(body);
            header.parts[0].name = name.into();
            refuses("part-name", &envelope(&header, body), Reason::Invalid);
        }
        let mut renamed = files(body);
        renamed.delivery.files[0].name = "b.txt".into();
        refuses(
            "unrecorded-file",
            &envelope(&renamed, body),
            Reason::Invalid,
        );
        let mut version = files(body);
        version.version = 2;
        refuses("version", &envelope(&version, body), Reason::Invalid);
        let mut id = files(body);
        id.delivery.id = "../escape".into();
        refuses("id", &envelope(&id, body), Reason::Invalid);
        let mut no_manifest = files(body);
        no_manifest.delivery.kind = Kind::Changes;
        refuses(
            "changes-without-manifest",
            &envelope(&no_manifest, body),
            Reason::Invalid,
        );
        let mut unknown = serde_json::to_value(files(body)).unwrap();
        unknown["extra"] = serde_json::json!(true);
        let mut with_unknown = serde_json::to_vec(&unknown).unwrap();
        with_unknown.extend(b"\nhello");
        refuses("unknown-field", &with_unknown, Reason::Invalid);
        let large = vec![0_u8; usize::try_from(MAX_ATTACHMENT_BYTES).unwrap() + 1];
        refuses(
            "oversized",
            &envelope(&files(&large), &large),
            Reason::Invalid,
        );
    }

    #[test]
    fn another_envelope_with_a_stored_id_is_a_conflict_and_changes_nothing() {
        let (scratch, ledger) = Scratch::new("conflict");
        let dir = scratch.0.join("transfers").join(ID);
        fs::create_dir_all(&dir).unwrap();
        let saved = br#"{"version":1,"header_sha256":"other"}"#;
        fs::write(dir.join(INGRESS), saved).unwrap();
        let body = b"hello";
        assert_eq!(
            receive(&System::default(), &ledger, &envelope(&files(body), body)),
            Reply::Refused {
                reason: Reason::Conflict
            }
        );
        assert_eq!(scratch.stored(), [ID]);
        assert_eq!(fs::read(dir.join(INGRESS)).unwrap(), saved);
        assert!(!dir.join("files").exists());
    }
}
