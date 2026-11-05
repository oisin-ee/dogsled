//! Immutable, host-owned observations. Collection and projection belong to the producer;
//! clients receive complete publications without a shell, ledger, or fleet.

pub(crate) mod collect;
pub(crate) mod diagnostics;
pub(crate) mod display;
pub(crate) mod ipc;
pub(crate) mod peers;
pub(crate) mod peers_runtime;
pub(crate) mod project;
pub(crate) mod read;
mod socket;
#[cfg(debug_assertions)]
pub(crate) mod stored;
pub(crate) use socket::socket_path;

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

pub(crate) use portal_types::{ObservationAvailability as Availability, ObservationGap as Gap};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PeerSource {
    pub epoch: String,
    pub local_revision: u64,
    pub received_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retry_tokens: Vec<String>,
    pub availability: Availability,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Sources {
    pub local: BTreeMap<String, u64>,
    /// Keys are execution-host identifiers, never the receiver-relative `local` alias.
    pub peers: BTreeMap<String, PeerSource>,
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

    pub fn current(&self) -> Result<Arc<Published>> {
        self.sender
            .borrow()
            .as_ref()
            .map(|frame| Arc::clone(&frame.published))
            .ok_or_else(|| Error::Invalid("observation unavailable: no publication".into()))
    }

    pub fn replace(&mut self, mut input: Publication) -> Result<Arc<Published>> {
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
        if let Some(portal) = input
            .views
            .get_mut("portal")
            .and_then(|view| view.get_mut("data"))
            && !portal.is_null()
        {
            let mut snapshot: portal_types::PortalSnapshot =
                serde_json::from_value(portal.clone())?;
            snapshot.epoch.clone_from(&self.epoch);
            snapshot.revision = revision;
            snapshot.published_ms = input.published_ms;
            *portal = serde_json::to_value(snapshot)?;
        }
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

/// Test inputs use the real capture and projector, without running action lanes.
#[cfg(test)]
pub(crate) fn captured_actions<'a>(
    fleet: &crate::hosts::Fleet<'a>,
    now: u64,
) -> Result<crate::hosts::ActionFleet<'a>> {
    captured_actions_with_terminals(fleet, now, None)
}

#[cfg(test)]
pub(crate) fn captured_actions_with_terminals<'a>(
    fleet: &crate::hosts::Fleet<'a>,
    now: u64,
    terminals: Option<crate::probe::Terminals>,
) -> Result<crate::hosts::ActionFleet<'a>> {
    let reports = crate::hosts::ObserverFleet(fleet.clone()).status(now)?;
    let status = collect::status(fleet.ledger, reports.clone(), now)?;
    let mut input = Publication {
        published_ms: now,
        sources: Sources {
            local: BTreeMap::from([("status".into(), now)]),
            peers: BTreeMap::new(),
        },
        local: BTreeMap::from([
            ("status".into(), serde_json::to_value(status)?),
            (
                "todos".into(),
                serde_json::to_value(crate::board::cached_todos(fleet.ledger)?)?,
            ),
        ]),
        views: Domains::new(),
        gaps: Vec::new(),
    };
    if let Some(terminals) = terminals {
        input
            .local
            .insert("terminals".into(), serde_json::to_value(terminals)?);
        input.sources.local.insert("terminals".into(), now);
    }
    let local = project::Projector::default().inputs(fleet, &input);
    project::derive(&local, reports, &fleet.policy.into(), &mut input)?;
    let published = Publisher::new()?.replace(input)?;
    crate::hosts::ActionFleet::new(
        fleet.shell,
        fleet.ledger,
        fleet.policy,
        fleet.home.clone(),
        published,
    )
}

#[cfg(test)]
mod tests;
