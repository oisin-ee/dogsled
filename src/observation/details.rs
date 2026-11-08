//! Host-local detail observation. Connection leases select work; readers never perform it.

use crate::error::{Error, Result};
use crate::herdr_socket::Client;
use crate::probe::Agent;
use crate::service::wake;
use portal_types::agents::{Conversation, DetailRevisions, Screen};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

/// Match the open-agent refresh cadence in `web/src/agent_view.rs::POLL`.
const DETAIL_TICK: Duration = Duration::from_secs(5);

/// A JSON control byte can become six bytes (`\\u00XX`). The envelope allowance covers
/// session identity, agent metadata and the screen framing, without reserving that memory.
const MAX_JSON_EXPANSION: usize = 6;
const DETAIL_ENVELOPE_BYTES: usize = 64 * 1024;
pub(crate) const CODEC: Codec = Codec::new(crate::activity::CONVERSATION_BYTES as usize);

#[derive(Debug, Clone, Copy)]
pub(crate) struct Codec {
    logical_bytes: usize,
}

impl Codec {
    pub const fn new(logical_bytes: usize) -> Self {
        Self { logical_bytes }
    }
    pub const fn wire_limit(self) -> usize {
        self.logical_bytes * MAX_JSON_EXPANSION + DETAIL_ENVELOPE_BYTES
    }
    pub fn encode<T: Serialize>(self, value: &T) -> Result<Vec<u8>> {
        let bytes = serde_json::to_vec(value)?;
        if bytes.len() > self.wire_limit() {
            return Err(Error::Invalid(
                "agent detail frame exceeds its content bound".into(),
            ));
        }
        Ok(bytes)
    }
    pub fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8], limit: usize) -> Result<T> {
        if bytes.len() > limit {
            return Err(Error::Invalid(
                "agent detail frame exceeds its content bound".into(),
            ));
        }
        Ok(serde_json::from_slice(bytes)?)
    }
}

#[cfg(test)]
#[path = "details_fixture.rs"]
pub(crate) mod fixture;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Session {
    pub terminal: Option<String>,
    pub session: String,
    pub harness: Option<String>,
    pub transcript: Option<String>,
}

impl Session {
    pub(super) fn of(agent: &Agent) -> Option<Self> {
        Some(Self {
            terminal: agent.terminal_id.clone(),
            session: agent
                .agent_session
                .clone()
                .filter(|value| !value.is_empty())?,
            harness: agent.agent.clone(),
            transcript: agent.transcript.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "availability", content = "data", rename_all = "snake_case")]
pub(crate) enum Content<T> {
    Pending,
    Available(T),
    Unavailable(String),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Detail {
    pub session: Option<Session>,
    pub state: String,
    pub conversation: Content<Conversation>,
    pub screen: Content<Screen>,
}

impl Detail {
    pub fn key(&self) -> Result<String> {
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| Error::Invalid("agent session unavailable".into()))?;
        Ok(serde_json::to_string(session)?)
    }

    fn pending(agent: &Agent) -> Self {
        Self {
            session: Session::of(agent),
            state: agent.state.clone(),
            conversation: Content::Pending,
            screen: Content::Pending,
        }
    }

    pub(super) fn unavailable(agent: Option<&Agent>, reason: &str) -> Self {
        Self {
            session: agent.and_then(Session::of),
            state: agent.map_or("unknown", |agent| agent.state.as_str()).into(),
            conversation: Content::Unavailable(reason.into()),
            screen: Content::Unavailable(reason.into()),
        }
    }
}

/// Only references cross the general publication transport.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "availability", content = "gap", rename_all = "snake_case")]
pub(crate) enum ReadState {
    Pending,
    Empty,
    Available,
    Unavailable(String),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Reference {
    pub pane: String,
    pub session: Option<Session>,
    pub revision: u64,
    pub parts: DetailRevisions,
    pub screen_revision: Option<u64>,
    pub state: String,
    pub conversation: ReadState,
    pub screen: ReadState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DetailFrame {
    pub version: u32,
    pub epoch: String,
    pub revision: u64,
    pub found: bool,
    #[serde(default)]
    pub parts: Option<DetailRevisions>,
    pub agent: Option<portal_types::agents::AgentView>,
    pub detail: Detail,
}

#[derive(Debug)]
pub(crate) struct Payload {
    pub reference: Reference,
    pub detail: Detail,
    pub bytes: Arc<[u8]>,
}

impl Payload {
    fn new(
        detail: Detail,
        pane: &str,
        sequence: &AtomicU64,
        previous: Option<&Self>,
    ) -> Result<Self> {
        fn state<T>(content: &Content<T>, empty: impl FnOnce(&T) -> bool) -> ReadState {
            match content {
                Content::Pending => ReadState::Pending,
                Content::Available(value) if empty(value) => ReadState::Empty,
                Content::Available(_) => ReadState::Available,
                Content::Unavailable(gap) => ReadState::Unavailable(gap.clone()),
                Content::Failed(gap) => ReadState::Failed(gap.clone()),
            }
        }
        let revision = sequence
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| Error::Invalid("agent detail revision exhausted".into()))?
            + 1;
        let reference = Reference {
            pane: pane.into(),
            session: detail.session.clone(),
            revision,
            parts: DetailRevisions {
                conversation: previous
                    .filter(|previous| {
                        previous.detail.session == detail.session
                            && previous.detail.conversation == detail.conversation
                    })
                    .map_or(revision, |previous| previous.reference.parts.conversation),
                screen: previous
                    .filter(|previous| {
                        previous.detail.session == detail.session
                            && previous.detail.screen == detail.screen
                    })
                    .map_or(revision, |previous| previous.reference.parts.screen),
            },
            screen_revision: match &detail.screen {
                Content::Available(screen) => Some(screen.revision),
                _ => None,
            },
            state: detail.state.clone(),
            conversation: state(&detail.conversation, |conversation| {
                conversation.turns.is_empty()
            }),
            screen: state(&detail.screen, |screen| screen.text.is_empty()),
        };
        let bytes = CODEC.encode(&detail)?;
        Ok(Self {
            reference,
            detail,
            bytes: bytes.into(),
        })
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Payloads(Interests);

impl Payloads {
    pub fn subscribe(&self, pane: &str) -> Option<watch::Receiver<Option<Arc<Payload>>>> {
        self.0
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .panes
            .get(pane)
            .map(|viewers| viewers.output.subscribe())
    }
}

#[derive(Debug, Clone)]
struct Stop {
    stopped: Arc<AtomicBool>,
    wake: std::sync::mpsc::Sender<Tick>,
}

#[derive(Debug)]
enum Tick {
    Stop,
    #[cfg(test)]
    Manual(std::sync::mpsc::SyncSender<()>),
}

impl Stop {
    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self.wake.send(Tick::Stop);
    }
}

#[derive(Debug)]
struct Viewers {
    count: usize,
    output: watch::Sender<Option<Arc<Payload>>>,
}

impl Default for Viewers {
    fn default() -> Self {
        Self {
            count: 0,
            output: watch::channel(None).0,
        }
    }
}

#[derive(Debug, Default)]
struct Registry {
    panes: BTreeMap<String, Viewers>,
    agents: BTreeMap<String, Agent>,
    sequence: Arc<AtomicU64>,
    workers: BTreeMap<String, Worker>,
    retired: Vec<std::thread::JoinHandle<()>>,
    client: Option<Client>,
}

/// Each connection holds one lease. Workers start from the last serve-owned observation.
#[derive(Debug, Clone, Default)]
pub(crate) struct Interests(Arc<Mutex<Registry>>);

impl Interests {
    pub fn register(
        &self,
        pane: &str,
        dirty: Option<crate::service::wake::Dirty>,
    ) -> Result<Interest> {
        if pane.is_empty() || pane.len() > 256 || pane.chars().any(char::is_control) {
            return Err(Error::Invalid("invalid agent detail interest".into()));
        }
        let mut panes = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let viewers = panes.panes.entry(pane.to_owned()).or_default();
        viewers.count += 1;
        if viewers.count == 1
            && let Some(dirty) = &dirty
        {
            if let Err(error) = panes.select(pane, dirty) {
                panes.panes.remove(pane);
                return Err(error);
            }
            dirty.mark(wake::DETAILS);
        }
        Ok(Interest {
            interests: self.clone(),
            pane: pane.to_owned(),
            dirty,
        })
    }

    #[cfg(test)]
    pub(super) fn panes(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .panes
            .keys()
            .cloned()
            .collect()
    }
}

#[derive(Debug)]
pub(crate) struct Interest {
    interests: Interests,
    pane: String,
    dirty: Option<crate::service::wake::Dirty>,
}

impl Drop for Interest {
    fn drop(&mut self) {
        let mut panes = self
            .interests
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(viewers) = panes.panes.get_mut(&self.pane) {
            viewers.count -= 1;
            if viewers.count == 0 {
                panes.retire(&self.pane);
                panes.panes.remove(&self.pane);
                if let Some(dirty) = &self.dirty {
                    dirty.mark(wake::DETAILS);
                }
            }
        }
    }
}

#[derive(Debug)]
struct Worker {
    session: Session,
    agent: Arc<Mutex<Agent>>,
    stop: Stop,
    thread: std::thread::JoinHandle<()>,
    #[cfg(test)]
    transcript_reads: Arc<std::sync::atomic::AtomicUsize>,
}

/// Slow transcript/socket I/O runs in per-pane workers, never in publication assembly.
#[derive(Debug)]
pub(crate) struct Collector {
    pub interests: Interests,
    pub payloads: Payloads,
    #[cfg(test)]
    pub(super) client: Option<Client>,
}

impl Default for Collector {
    fn default() -> Self {
        let interests = Interests::default();
        Self {
            payloads: Payloads(interests.clone()),
            interests,
            #[cfg(test)]
            client: None,
        }
    }
}

impl Collector {
    pub fn refresh(
        &mut self,
        input: &mut super::Publication,
        dirty: &crate::service::wake::Dirty,
    ) -> Result<()> {
        let mut registry = self
            .interests
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        #[cfg(test)]
        {
            registry.client.clone_from(&self.client);
        }
        let terminals = input
            .local
            .get("terminals")
            .filter(|_| {
                !input
                    .gaps
                    .iter()
                    .any(|gap| gap.domain == "terminals" || gap.domain == "host")
            })
            .map(|value| serde_json::from_value::<crate::probe::Terminals>(value.clone()))
            .transpose()?;
        let mut values = BTreeMap::new();
        registry.agents = terminals
            .into_iter()
            .flat_map(|terminals| terminals.agents)
            .map(|agent| (agent.pane.clone(), agent))
            .collect();
        let panes: Vec<_> = registry.panes.keys().cloned().collect();
        for pane in panes {
            let payload = registry.select(&pane, dirty)?;
            values.insert(pane.clone(), payload.reference.clone());
        }
        registry.retired.retain(|thread| !thread.is_finished());
        input.views.insert(
            "agent_details".into(),
            serde_json::to_value(super::project::View {
                data: Some(values),
                gaps: Vec::new(),
            })?,
        );
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn transcript_reads(&self) -> usize {
        self.interests
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .workers
            .values()
            .map(|worker| worker.transcript_reads.load(Ordering::Acquire))
            .sum()
    }

    #[cfg(test)]
    pub(super) fn tick(&self, pane: &str) -> bool {
        let registry = self
            .interests
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(worker) = registry.workers.get(pane) else {
            return false;
        };
        let (send, receive) = std::sync::mpsc::sync_channel(0);
        worker
            .stop
            .wake
            .send(Tick::Manual(send))
            .expect("manual detail tick");
        receive
            .recv_timeout(Duration::from_secs(3))
            .expect("tick completed");
        true
    }
}

impl Registry {
    fn select(&mut self, pane: &str, dirty: &wake::Dirty) -> Result<Arc<Payload>> {
        let agent = self.agents.get(pane).cloned();
        let session = agent.as_ref().and_then(Session::of);
        if self.workers.get(pane).is_some_and(|worker| {
            Some(&worker.session) != session.as_ref() || worker.stop.stopped.load(Ordering::Acquire)
        }) {
            self.retire(pane);
        }
        let output = self
            .panes
            .get(pane)
            .ok_or_else(|| Error::Invalid("agent detail interest unavailable".into()))?
            .output
            .clone();
        if let (Some(agent), Some(session)) = (agent.as_ref(), session) {
            if let Some(worker) = self.workers.get(pane) {
                *worker
                    .agent
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = agent.clone();
            } else {
                let previous = output.borrow().clone();
                output.send_replace(Some(Arc::new(Payload::new(
                    Detail::pending(agent),
                    pane,
                    &self.sequence,
                    previous.as_deref(),
                )?)));
                self.workers.insert(
                    pane.into(),
                    Worker::start(
                        agent,
                        session,
                        dirty.clone(),
                        output.clone(),
                        self.client.clone(),
                        Arc::clone(&self.sequence),
                    ),
                );
            }
        } else {
            let detail = Detail::unavailable(agent.as_ref(), "agent session unavailable");
            let previous = output.borrow().clone();
            if previous.as_ref().map(|payload| &payload.detail) != Some(&detail) {
                output.send_replace(Some(Arc::new(Payload::new(
                    detail,
                    pane,
                    &self.sequence,
                    previous.as_deref(),
                )?)));
            }
        }
        let payload = output.borrow().clone();
        payload.ok_or_else(|| Error::Invalid("agent detail payload unavailable".into()))
    }

    fn retire(&mut self, pane: &str) {
        if let Some(worker) = self.workers.remove(pane) {
            worker.stop.stop();
            self.retired.push(worker.thread);
        }
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        let mut registry = self
            .interests
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let workers = std::mem::take(&mut registry.workers);
        for (_, worker) in workers {
            worker.stop.stop();
            registry.retired.push(worker.thread);
        }
        let retired = std::mem::take(&mut registry.retired);
        drop(registry);
        for thread in retired {
            let _ = thread.join();
        }
    }
}

impl Worker {
    fn start(
        agent: &Agent,
        session: Session,
        dirty: crate::service::wake::Dirty,
        output: watch::Sender<Option<Arc<Payload>>>,
        client: Option<Client>,
        sequence: Arc<AtomicU64>,
    ) -> Self {
        let current = Arc::new(Mutex::new(agent.clone()));
        let stopped = Arc::new(AtomicBool::new(false));
        let done = Arc::clone(&stopped);
        let source = Arc::clone(&current);
        let (wake, ticks) = std::sync::mpsc::channel();
        #[cfg(test)]
        let transcript_reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        #[cfg(test)]
        let reads = Arc::clone(&transcript_reads);
        let thread = std::thread::spawn(move || {
            let mut capture = Capture::default();
            #[cfg(test)]
            let manual = client.is_some();
            #[cfg(not(test))]
            let manual = false;
            #[cfg(test)]
            let mut acknowledgement: Option<std::sync::mpsc::SyncSender<()>> = None;
            loop {
                if done.load(Ordering::Acquire) {
                    break;
                }
                let agent = source
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                let socket = client.clone().map_or_else(Client::discover, Ok);
                let detail = capture.tick(&agent, socket.as_ref().map_err(ToString::to_string));
                #[cfg(test)]
                reads.store(capture.transcript_reads, Ordering::Release);
                if done.load(Ordering::Acquire) {
                    break;
                }
                let previous = output.borrow().clone();
                if previous
                    .as_ref()
                    .map(|payload: &Arc<Payload>| &payload.detail)
                    != Some(&detail)
                {
                    let payload = Payload::new(detail, &agent.pane, &sequence, previous.as_deref())
                        .or_else(|error| {
                            let mut failed =
                                Detail::unavailable(Some(&agent), "detail serialization failed");
                            failed.conversation = Content::Failed(error.to_string());
                            failed.screen = Content::Failed(error.to_string());
                            Payload::new(failed, &agent.pane, &sequence, previous.as_deref())
                        });
                    match payload {
                        Ok(payload) => {
                            let changed = output.send_if_modified(|value| {
                                if done.load(Ordering::Acquire) {
                                    return false;
                                }
                                *value = Some(Arc::new(payload));
                                true
                            });
                            if changed {
                                dirty.mark(wake::DETAILS);
                            }
                        }
                        Err(error) => eprintln!("agent detail serialization: {error}"),
                    }
                }
                #[cfg(test)]
                if let Some(ack) = acknowledgement.take() {
                    let _ = ack.send(());
                }
                let tick = if manual {
                    ticks.recv().ok()
                } else {
                    match ticks.recv_timeout(DETAIL_TICK) {
                        Ok(tick) => Some(tick),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                };
                match tick {
                    Some(Tick::Stop) => break,
                    #[cfg(test)]
                    Some(Tick::Manual(ack)) => acknowledgement = Some(ack),
                    None => (),
                }
            }
        });
        let stop = Stop { stopped, wake };
        Self {
            session,
            agent: current,
            stop,
            thread,
            #[cfg(test)]
            transcript_reads,
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct Capture {
    transcript: Option<(std::fs::Metadata, Content<Conversation>)>,
    screen: Option<Screen>,
    session: Option<Session>,
    #[cfg(test)]
    pub(super) transcript_reads: usize,
}

impl Capture {
    pub(super) fn tick(
        &mut self,
        agent: &Agent,
        socket: std::result::Result<&Client, String>,
    ) -> Detail {
        let mut detail = Detail::unavailable(Some(agent), "agent session unavailable");
        let Some(session) = detail.session.clone() else {
            return detail;
        };
        if self.session != detail.session {
            self.transcript = None;
            self.screen = None;
            self.session.clone_from(&detail.session);
        }
        detail.conversation = self.conversation(agent, &session);
        detail.screen = match socket {
            Ok(socket) => match read_screen_for_session(socket, agent, &session) {
                Ok(screen) => {
                    let retained = self
                        .screen
                        .as_ref()
                        .filter(|old| old.revision == screen.revision)
                        .unwrap_or(&screen)
                        .clone();
                    self.screen = Some(retained.clone());
                    Content::Available(retained)
                }
                Err(error) => Content::Failed(error.to_string()),
            },
            Err(error) => Content::Unavailable(error),
        };
        detail
    }

    fn conversation(&mut self, agent: &Agent, session: &Session) -> Content<Conversation> {
        let Some(path) = &agent.transcript else {
            return Content::Unavailable("agent transcript unavailable".into());
        };
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => return Content::Failed(error.to_string()),
        };
        if let Some((stamp, content)) = &self.transcript
            && same_file(stamp, &metadata)
        {
            return content.clone();
        }
        #[cfg(test)]
        {
            self.transcript_reads += 1;
        }
        let content = match (|| -> Result<Conversation> {
            let history = crate::activity::try_conversation(
                agent.agent.as_deref().unwrap_or_default(),
                path,
            )?;
            Ok(Conversation {
                pane: agent.pane.clone(),
                session: serde_json::to_string(session)?,
                turns: history.turns,
                truncated: history.truncated,
            })
        })() {
            Ok(conversation) => Content::Available(conversation),
            Err(error) => Content::Failed(error.to_string()),
        };
        self.transcript = Some((metadata, content.clone()));
        content
    }
}

fn same_file(old: &std::fs::Metadata, new: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    old.dev() == new.dev()
        && old.ino() == new.ino()
        && old.len() == new.len()
        && old
            .modified()
            .ok()
            .zip(new.modified().ok())
            .is_some_and(|(old, new)| old == new)
}

fn read_screen_for_session(socket: &Client, agent: &Agent, session: &Session) -> Result<Screen> {
    let matches = |agents: Vec<serde_json::Value>| {
        agents.iter().any(|raw| {
            raw["pane_id"].as_str() == Some(&agent.pane)
                && raw["agent_session"]["value"].as_str() == agent.agent_session.as_deref()
                && raw["terminal_id"].as_str() == agent.terminal_id.as_deref()
        })
    };
    if !matches(socket.agent_list()?) {
        return Err(Error::Invalid("agent session changed".into()));
    }
    let read = socket.pane_read(&agent.pane, "visible", Some(200))?;
    if !matches(socket.agent_list()?) {
        return Err(Error::Invalid("agent session changed".into()));
    }
    Ok(Screen {
        session: serde_json::to_string(session)?,
        pane: agent.pane.clone(),
        text: read["text"]
            .as_str()
            .ok_or_else(|| Error::Invalid("screen text missing".into()))?
            .into(),
        revision: read["revision"]
            .as_u64()
            .ok_or_else(|| Error::Invalid("screen revision missing".into()))?,
    })
}
