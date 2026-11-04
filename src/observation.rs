//! Immutable, host-owned observations. Collection and projection belong to the producer;
//! clients receive complete publications without a shell, ledger, or fleet.

pub(crate) mod collect;
pub(crate) mod ipc;
pub(crate) mod peers;
pub(crate) mod peers_runtime;

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::watch;

pub(crate) const VERSION: u32 = 1;

/// Domain payloads are already-derived wire projections. The transport neither interprets
/// foreign paths nor assembles projections. Collectors supply these complete maps together.
pub(crate) type Domains = BTreeMap<String, Value>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Availability {
    Available,
    Unavailable,
    Failed,
    Unsupported,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PeerSource {
    pub epoch: String,
    pub local_revision: u64,
    pub received_ms: u64,
    pub availability: Availability,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Sources {
    pub local: BTreeMap<String, u64>,
    /// Keys are execution-host identifiers, never the receiver-relative `local` alias.
    pub peers: BTreeMap<String, PeerSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Gap {
    pub domain: String,
    pub availability: Availability,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Published {
    pub version: u32,
    pub epoch: String,
    pub revision: u64,
    pub published_ms: u64,
    pub local_revision: u64,
    pub sources: Sources,
    pub local: Domains,
    pub views: Domains,
    pub gaps: Vec<Gap>,
}

/// A frozen input set. An absent domain must have an explicit gap, not an empty success.
#[derive(Debug, Clone)]
pub(crate) struct Publication {
    pub published_ms: u64,
    pub sources: Sources,
    pub local: Domains,
    pub views: Domains,
    pub gaps: Vec<Gap>,
}

#[derive(Debug)]
pub(crate) struct Frame {
    pub published: Arc<Published>,
    /// One newline-delimited frame, serialized before making this revision visible.
    pub bytes: Arc<[u8]>,
}

/// The single producer owns revision assignment. Watch retains only the latest value.
#[derive(Debug)]
pub(crate) struct Publisher {
    epoch: String,
    sender: watch::Sender<Option<Arc<Frame>>>,
}

impl Publisher {
    pub fn new() -> Result<Self> {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes)
            .map_err(|error| Error::Invalid(format!("observation epoch: {error}")))?;
        let epoch = bytes
            .iter()
            .fold(String::with_capacity(32), |mut epoch, byte| {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                epoch.push(char::from(HEX[usize::from(byte >> 4)]));
                epoch.push(char::from(HEX[usize::from(byte & 15)]));
                epoch
            });
        Ok(Self {
            epoch,
            sender: watch::channel(None).0,
        })
    }

    pub fn subscribe(&self) -> watch::Receiver<Option<Arc<Frame>>> {
        self.sender.subscribe()
    }

    pub fn replace(&mut self, input: Publication) -> Result<Arc<Published>> {
        let increment = |value: u64| {
            value.checked_add(1).ok_or_else(|| {
                Error::Invalid("observation revision exhausted; restart the publisher".into())
            })
        };
        let (revision, local_revision) = {
            let previous = self.sender.borrow();
            let previous = previous.as_ref().map(|frame| &frame.published);
            let revision = increment(previous.map_or(0, |value| value.revision))?;
            let local_revision = match previous {
                Some(value)
                    if peers::exportable(&value.local) == peers::exportable(&input.local) =>
                {
                    value.local_revision
                }
                value => increment(value.map_or(0, |value| value.local_revision))?,
            };
            (revision, local_revision)
        };
        let published = Arc::new(Published {
            version: VERSION,
            epoch: self.epoch.clone(),
            revision,
            published_ms: input.published_ms,
            local_revision,
            sources: input.sources,
            local: input.local,
            views: input.views,
            gaps: input.gaps,
        });
        let mut bytes = serde_json::to_vec(&*published)?;
        bytes.push(b'\n');
        if bytes.len() > ipc::MAX_FRAME_BYTES {
            return Err(Error::Invalid("observation publication too large".into()));
        }
        self.sender.send_replace(Some(Arc::new(Frame {
            published: Arc::clone(&published),
            bytes: bytes.into(),
        })));
        Ok(published)
    }
}

/// This adapter runs before configuration loading or fleet discovery.
pub(crate) fn command(command: &crate::cli::ObservationCommand) -> Result<std::process::ExitCode> {
    let client = ipc::Client::at(&crate::ledger::default_dir());
    if matches!(command, crate::cli::ObservationCommand::Current) {
        crate::print(true, &client.current()?, String::new)?;
    } else {
        let mut subscription = client.subscribe()?;
        let mut connection = subscription.current();
        loop {
            if let ipc::Status::Disconnected(detail) = connection.status {
                return Err(Error::Invalid(detail));
            }
            crate::print(true, &*connection.publication, String::new)?;
            let revision = connection.publication.revision;
            loop {
                connection = subscription.changed(std::time::Duration::from_secs(3600));
                if connection.publication.revision != revision
                    || matches!(connection.status, ipc::Status::Disconnected(_))
                {
                    break;
                }
            }
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests;
