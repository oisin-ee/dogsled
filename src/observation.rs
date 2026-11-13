//! Immutable, host-owned observations. Collection and projection belong to the producer;
//! clients receive the domains that they select from one publication, without a shell,
//! ledger, or fleet.

pub(crate) mod collect;
pub(crate) mod details;
pub(crate) mod diagnostics;
pub(crate) mod display;
pub(crate) mod ipc;
pub(crate) mod live_activity;
mod pages;
pub(crate) mod peers;
pub(crate) mod peers_runtime;
pub(crate) mod project;
pub(crate) mod read;
pub(crate) mod sidebar;
mod socket;
#[cfg(debug_assertions)]
pub(crate) mod stored;
pub(crate) mod timing;
pub(crate) use socket::socket_path;

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tokio::sync::watch;

/// Version 2: a reader selects domains, and a reply can have several frames.
pub(crate) const VERSION: u32 = 2;
/// The gap domain of a pass that failed as a whole.
pub(crate) const FAILED_PASS: &str = "observation";

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

type Members = BTreeMap<String, Arc<pages::Member>>;

#[derive(Debug)]
pub(crate) struct Frame {
    pub published: Arc<Published>,
    /// Each member serialized once, before this revision becomes visible. A reply shares
    /// them, so its size follows the selection, not the publication.
    sources: pages::Member,
    gaps: pages::Member,
    local: Members,
    views: Members,
    /// Host-local detail channels advance independently of the main publication.
    pub details: details::Payloads,
}

impl Frame {
    fn new(
        published: Arc<Published>,
        previous: Option<&Self>,
        details: details::Payloads,
    ) -> Result<Self> {
        let limit = ipc::MAX_FRAME_BYTES;
        Ok(Self {
            sources: pages::Member::new(
                "",
                "sources",
                &serde_json::to_value(&published.sources)?,
                limit,
            )?,
            gaps: pages::Member::new("", "gaps", &serde_json::to_value(&published.gaps)?, limit)?,
            local: Self::members(
                "/local",
                &published.local,
                previous.map(|frame| (&frame.published.local, &frame.local)),
            )?,
            views: Self::members(
                "/views",
                &published.views,
                previous.map(|frame| (&frame.published.views, &frame.views)),
            )?,
            published,
            details,
        })
    }

    /// An unchanged domain keeps its bytes across revisions.
    fn members(at: &str, current: &Domains, old: Option<(&Domains, &Members)>) -> Result<Members> {
        current
            .iter()
            .map(|(name, value)| {
                let kept = old
                    .filter(|(values, _)| values.get(name) == Some(value))
                    .and_then(|(_, members)| members.get(name).cloned());
                let member = match kept {
                    Some(member) => member,
                    None => Arc::new(pages::Member::new(at, name, value, ipc::MAX_FRAME_BYTES)?),
                };
                Ok((name.clone(), member))
            })
            .collect()
    }

    fn chosen<'a>(
        members: &'a Members,
        names: Option<&BTreeSet<String>>,
    ) -> Vec<&'a pages::Member> {
        members
            .iter()
            .filter(|(name, _)| names.is_none_or(|names| names.contains(*name)))
            .map(|(_, member)| &**member)
            .collect()
    }

    /// The frames of one reply: this revision's selected domains with its sources and gaps.
    pub fn reply(&self, select: Option<&ipc::Chosen>) -> Result<Vec<Vec<u8>>> {
        let published = &self.published;
        let scalars = format!(
            "\"version\":{},\"epoch\":{},\"revision\":{},\"published_ms\":{},\"local_revision\":{}",
            published.version,
            Value::from(published.epoch.as_str()),
            published.revision,
            published.published_ms,
            published.local_revision,
        );
        pages::reply(
            &scalars,
            &[
                ("", vec![&self.sources, &self.gaps]),
                (
                    "local",
                    Self::chosen(&self.local, select.map(|chosen| &chosen.local)),
                ),
                (
                    "views",
                    Self::chosen(&self.views, select.map(|chosen| &chosen.views)),
                ),
            ],
            ipc::MAX_FRAME_BYTES,
        )
    }
}

/// The single producer owns revision assignment. Watch retains only the latest value.
#[derive(Debug)]
pub(crate) struct Publisher {
    epoch: String,
    details: details::Payloads,
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
            details: details::Payloads::default(),
            sender: watch::channel(None).0,
        })
    }

    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    pub fn details(&mut self, details: details::Payloads) {
        self.details = details;
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

    /// The input after a pass that failed as a whole: the last domains, and a gap that
    /// says they are no longer refreshed. A reader never sees the failure as current data.
    pub fn failed(&self, published_ms: u64, detail: &str) -> Result<Publication> {
        let gap = Gap {
            domain: FAILED_PASS.into(),
            availability: Availability::Failed,
            detail: detail.into(),
        };
        let mut input = self.sender.borrow().as_ref().map_or_else(
            || Publication {
                published_ms,
                sources: Sources::default(),
                local: Domains::new(),
                views: Domains::new(),
                gaps: Vec::new(),
            },
            |frame| Publication {
                published_ms,
                sources: frame.published.sources.clone(),
                local: frame.published.local.clone(),
                views: frame.published.views.clone(),
                gaps: frame.published.gaps.clone(),
            },
        );
        input.gaps.retain(|gap| gap.domain != FAILED_PASS);
        if let Some(gaps) = input
            .views
            .get_mut("portal")
            .and_then(|view| view.pointer_mut("/data/gaps"))
            .and_then(Value::as_array_mut)
        {
            gaps.retain(|gap| gap["domain"] != FAILED_PASS);
            gaps.push(serde_json::to_value(&gap)?);
        }
        input.gaps.push(gap);
        Ok(input)
    }

    pub fn replace(&mut self, mut input: Publication) -> Result<Arc<Published>> {
        if let Some(previous) = self.sender.borrow().as_ref()
            && previous.published.local == input.local
            && previous.published.sources == input.sources
            && previous.published.gaps == input.gaps
            && same_views(&previous.published.views, &input.views)
        {
            return Ok(Arc::clone(&previous.published));
        }
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
        let previous = self.sender.borrow().clone();
        let frame = Frame::new(
            Arc::clone(&published),
            previous.as_deref(),
            self.details.clone(),
        )?;
        self.sender.send_replace(Some(Arc::new(frame)));
        Ok(published)
    }
}

fn same_views(previous: &Domains, current: &Domains) -> bool {
    previous.len() == current.len()
        && previous.iter().all(|(name, previous)| {
            current.get(name).is_some_and(|current| {
                if name == "portal" {
                    portal_content(previous) == portal_content(current)
                } else {
                    previous == current
                }
            })
        })
}

fn portal_content(view: &Value) -> Value {
    let mut view = view.clone();
    if let Some(snapshot) = view.get_mut("data").and_then(Value::as_object_mut) {
        for field in ["epoch", "revision", "published_ms"] {
            snapshot.remove(field);
        }
    }
    view
}

/// This adapter runs before configuration loading or fleet discovery.
pub(crate) fn command(command: &crate::cli::ObservationCommand) -> Result<std::process::ExitCode> {
    let client = ipc::Client::at(&crate::ledger::default_dir());
    if matches!(command, crate::cli::ObservationCommand::Current) {
        crate::print(true, &client.current(&ipc::Select::All)?, String::new)?;
    } else {
        let mut subscription = client.subscribe(&ipc::Select::All)?;
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
    // Use the same stored producer sources as `state project`, without discovery or lanes.
    let mut reports = Vec::new();
    for host in &fleet.hosts {
        let cached = if host.is_local() {
            crate::hosts::Cached::from(collect::host(
                &fleet.node(),
                crate::host_store::snapshot(fleet.ledger)?,
                true,
                now,
            )?)
        } else {
            crate::host_store::peer(fleet.ledger, &host.id)?.unwrap_or_default()
        };
        reports.push(fleet.cached_report(host, cached, now));
    }
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
    input.local.insert(
        "sidebar_preferences".into(),
        serde_json::to_value(sidebar::preferences(fleet.ledger)?)?,
    );
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
