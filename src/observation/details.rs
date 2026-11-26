//! Host-local detail observation. Connection leases select work; readers never perform it.

use super::live_activity::{ActivityFeed, ActivityFrame};
use crate::error::{Error, Result};
use crate::herdr_socket::Client;
use crate::probe::Agent;
use crate::service::wake;
use notify::{Event, RecursiveMode, Watcher as _};
use portal_types::agents::{Conversation, DetailRevisions, Screen};
use portal_types::files::FilesState;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
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
    pub files: FilesState,
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
            files: FilesState::Pending,
        }
    }

    pub(super) fn unavailable(agent: Option<&Agent>, reason: &str) -> Self {
        Self {
            session: agent.and_then(Session::of),
            state: agent.map_or("unknown", |agent| agent.state.as_str()).into(),
            conversation: Content::Unavailable(reason.into()),
            screen: Content::Unavailable(reason.into()),
            files: FilesState::Unavailable(reason.into()),
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
    pub files: ReadState,
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
                files: previous
                    .filter(|previous| {
                        previous.detail.session == detail.session
                            && previous.detail.files == detail.files
                    })
                    .map_or(revision, |previous| previous.reference.parts.files),
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
            files: match &detail.files {
                FilesState::Pending => ReadState::Pending,
                FilesState::Current(files) if files.entries.is_empty() => ReadState::Empty,
                FilesState::Current(_) => ReadState::Available,
                FilesState::Stale { error, .. } => ReadState::Failed(error.clone()),
                FilesState::NotRepository => ReadState::Unavailable("not a Git repository".into()),
                FilesState::TooMany => ReadState::Failed("too many files".into()),
                FilesState::Unavailable(error) => ReadState::Unavailable(error.clone()),
            },
        };
        let bytes = CODEC.encode(&detail)?;
        Ok(Self {
            reference,
            detail,
            bytes: bytes.into(),
        })
    }
}

impl Payload {
    fn publish(
        detail: Detail,
        agent: &Agent,
        sequence: &AtomicU64,
        output: &watch::Sender<Option<Arc<Self>>>,
        done: &AtomicBool,
        dirty: &wake::Dirty,
    ) {
        let previous = output.borrow().clone();
        if previous
            .as_ref()
            .map(|payload: &Arc<Payload>| &payload.detail)
            != Some(&detail)
        {
            let payload =
                Payload::new(detail, &agent.pane, sequence, previous.as_deref()).or_else(|error| {
                    let mut failed =
                        Detail::unavailable(Some(agent), "detail serialization failed");
                    failed.conversation = Content::Failed(error.to_string());
                    failed.screen = Content::Failed(error.to_string());
                    Payload::new(failed, &agent.pane, sequence, previous.as_deref())
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
    files_wake: std::sync::mpsc::Sender<FilesTick>,
}

#[derive(Debug)]
enum FilesTick {
    Stop,
    #[cfg(test)]
    Manual(std::sync::mpsc::SyncSender<()>),
}

#[derive(Debug)]
enum Tick {
    Stop,
    Files,
    Activity,
    Transcript,
    #[cfg(test)]
    Barrier(std::sync::mpsc::SyncSender<()>),
    #[cfg(test)]
    Manual(std::sync::mpsc::SyncSender<()>),
}

fn next_tick(
    ticks: &std::sync::mpsc::Receiver<Tick>,
    next_capture: std::time::Instant,
    #[cfg(test)] manual: bool,
) -> std::result::Result<Option<Tick>, std::sync::mpsc::RecvError> {
    #[cfg(test)]
    if manual {
        loop {
            match ticks.recv()? {
                Tick::Barrier(ack) => {
                    let _ = ack.send(());
                }
                tick => return Ok(Some(tick)),
            }
        }
    }
    match ticks.recv_timeout(next_capture.saturating_duration_since(std::time::Instant::now())) {
        Ok(tick) => Ok(Some(tick)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(None),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(std::sync::mpsc::RecvError),
    }
}

impl Stop {
    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self.wake.send(Tick::Stop);
        let _ = self.files_wake.send(FilesTick::Stop);
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
    #[cfg(test)]
    files_shell: Option<Arc<dyn crate::shell::Shell>>,
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

    async fn follow_activity(
        &self,
        mut feed: ActivityFeed,
        mut previous: Arc<ActivityFrame>,
        mut stop: tokio::sync::oneshot::Receiver<()>,
    ) {
        loop {
            tokio::select! {
                biased;
                _ = &mut stop => break,
                changed = feed.changed() => {
                    if changed.is_err() {
                        break;
                    }
                }
            }
            let current = feed.borrow_and_update().clone();
            let registry = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (pane, worker) in &registry.workers {
                if previous.changed.get(pane) != current.changed.get(pane) {
                    worker.activity_changed();
                }
            }
            drop(registry);
            previous = current;
        }
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
    files_thread: std::thread::JoinHandle<()>,
    activity_pending: Arc<AtomicBool>,
    #[cfg(test)]
    transcript_reads: Arc<std::sync::atomic::AtomicUsize>,
}

/// Slow transcript/socket I/O runs in per-pane workers, never in publication assembly.
#[derive(Debug)]
pub(crate) struct Collector {
    pub interests: Interests,
    pub payloads: Payloads,
    activity: Option<(
        tokio::sync::oneshot::Sender<()>,
        std::thread::JoinHandle<()>,
    )>,
    #[cfg(test)]
    pub(super) client: Option<Client>,
    #[cfg(test)]
    pub(super) files_shell: Option<Arc<dyn crate::shell::Shell>>,
}

impl Default for Collector {
    fn default() -> Self {
        let interests = Interests::default();
        Self {
            payloads: Payloads(interests.clone()),
            interests,
            activity: None,
            #[cfg(test)]
            client: None,
            #[cfg(test)]
            files_shell: None,
        }
    }
}

impl Collector {
    pub fn with_activity(mut feed: ActivityFeed) -> Result<Self> {
        let mut collector = Self::default();
        let interests = collector.interests.clone();
        let previous = feed.borrow_and_update().clone();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let runtime = tokio::runtime::Builder::new_current_thread().build()?;
        let thread = std::thread::Builder::new()
            .name("detail-activity".into())
            .spawn(move || runtime.block_on(interests.follow_activity(feed, previous, stopped)))?;
        collector.activity = Some((stop, thread));
        Ok(collector)
    }

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
            registry.files_shell.clone_from(&self.files_shell);
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
        let mut index = 0;
        while index < registry.retired.len() {
            if registry.retired[index].is_finished() {
                let _ = registry.retired.swap_remove(index).join();
            } else {
                index += 1;
            }
        }
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
    pub(super) fn files_tick(&self, pane: &str) -> bool {
        let registry = self.interests.0.lock().unwrap();
        let Some(worker) = registry.workers.get(pane) else {
            return false;
        };
        let (send, receive) = std::sync::mpsc::sync_channel(0);
        worker
            .stop
            .files_wake
            .send(FilesTick::Manual(send))
            .unwrap();
        receive
            .recv_timeout(crate::test_support::WAIT_DEADLINE)
            .expect("files tick completed");
        drop(registry);
        self.tick(pane)
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
            .recv_timeout(crate::test_support::WAIT_DEADLINE)
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
                        #[cfg(test)]
                        self.files_shell.clone(),
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
            self.retired.push(worker.files_thread);
        }
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        if let Some((stop, thread)) = self.activity.take() {
            let _ = stop.send(());
            let _ = thread.join();
        }
        let mut registry = self
            .interests
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let workers = std::mem::take(&mut registry.workers);
        for (_, worker) in workers {
            worker.stop.stop();
            registry.retired.push(worker.thread);
            registry.retired.push(worker.files_thread);
        }
        let retired = std::mem::take(&mut registry.retired);
        drop(registry);
        for thread in retired {
            let _ = thread.join();
        }
    }
}

fn watch_transcript(
    path: Option<&str>,
    wake: std::sync::mpsc::Sender<Tick>,
    pending: Arc<AtomicBool>,
) -> Option<notify::RecommendedWatcher> {
    let path = Path::new(path?);
    match register_transcript_watch(path, wake, pending) {
        Ok(watcher) => Some(watcher),
        Err(error) => {
            eprintln!(
                "agent detail transcript watch {}: {error}; using periodic capture",
                path.display()
            );
            None
        }
    }
}

fn register_transcript_watch(
    path: &Path,
    wake: std::sync::mpsc::Sender<Tick>,
    pending: Arc<AtomicBool>,
) -> notify::Result<notify::RecommendedWatcher> {
    let name = path.file_name().ok_or_else(notify::Error::path_not_found)?;
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()?;
    // FSEvents requires a UTF-8 watch path, even when a symlink resolves to a different path.
    if directory.to_str().is_none() {
        return Err(notify::Error::generic("transcript directory is not UTF-8"));
    }
    let target = directory.join(name);
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
        match event {
            Ok(event) => {
                // Ignore our own reads. A rescan hint may have no path.
                if (event.need_rescan()
                    || (!event.kind.is_access() && event.paths.contains(&target)))
                    && !pending.swap(true, Ordering::AcqRel)
                {
                    let _ = wake.send(Tick::Transcript);
                }
            }
            Err(error) => eprintln!(
                "agent detail transcript watch {}: {error}",
                target.display()
            ),
        }
    })?;
    // Watch the directory so replacing the file does not detach the watch from its name.
    watcher.watch(&directory, RecursiveMode::NonRecursive)?;
    Ok(watcher)
}

impl Worker {
    fn activity_changed(&self) {
        // One queued capture is enough even if activity keeps changing during slow I/O.
        if !self.activity_pending.swap(true, Ordering::AcqRel) {
            let _ = self.stop.wake.send(Tick::Activity);
        }
    }

    fn start(
        agent: &Agent,
        session: Session,
        dirty: crate::service::wake::Dirty,
        output: watch::Sender<Option<Arc<Payload>>>,
        client: Option<Client>,
        sequence: Arc<AtomicU64>,
        #[cfg(test)] files_shell: Option<Arc<dyn crate::shell::Shell>>,
    ) -> Self {
        let current = Arc::new(Mutex::new(agent.clone()));
        let stopped = Arc::new(AtomicBool::new(false));
        let done = Arc::clone(&stopped);
        let source = Arc::clone(&current);
        let (wake, ticks) = std::sync::mpsc::channel();
        let activity_pending = Arc::new(AtomicBool::new(false));
        let pending = Arc::clone(&activity_pending);
        let files = FilesWorker::start(
            Arc::clone(&current),
            Arc::clone(&stopped),
            wake.clone(),
            #[cfg(test)]
            files_shell,
        );
        let slot = Arc::clone(&files.slot);
        let transcript_path = session.transcript.clone();
        let transcript_wake = wake.clone();
        let mut detail = Detail::pending(agent);
        #[cfg(test)]
        let transcript_reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        #[cfg(test)]
        let reads = Arc::clone(&transcript_reads);
        let thread = std::thread::spawn(move || {
            let transcript_pending = Arc::new(AtomicBool::new(false));
            // Register before the first read so an append cannot fall between read and watch.
            let _watcher = watch_transcript(
                transcript_path.as_deref(),
                transcript_wake,
                Arc::clone(&transcript_pending),
            );
            let mut capture = Capture::default();
            let mut tick = None;
            let mut next_capture = std::time::Instant::now();
            let manual = cfg!(test) && client.is_some();
            loop {
                if done.load(Ordering::Acquire) || matches!(tick, Some(Tick::Stop)) {
                    break;
                }
                // Clear before capture so a change during the read can queue one more pass.
                match tick {
                    Some(Tick::Transcript) => transcript_pending.store(false, Ordering::Release),
                    Some(Tick::Activity) => pending.store(false, Ordering::Release),
                    _ => (),
                }
                let agent = source
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                let due = !manual && std::time::Instant::now() >= next_capture;
                match tick {
                    Some(Tick::Files) if !due => (),
                    Some(Tick::Transcript) if !due => {
                        if let Some(session) = &detail.session {
                            detail.conversation = capture.conversation(&agent, session);
                        }
                    }
                    _ => {
                        let socket = client.clone().map_or_else(Client::discover, Ok);
                        detail = capture.tick(&agent, socket.as_ref().map_err(ToString::to_string));
                        next_capture = std::time::Instant::now() + DETAIL_TICK;
                    }
                }
                detail.files = FilesRead::state_for(&slot, detail.session.as_ref(), &agent);
                #[cfg(test)]
                reads.store(capture.transcript_reads, Ordering::Release);
                if done.load(Ordering::Acquire) {
                    break;
                }
                Payload::publish(detail.clone(), &agent, &sequence, &output, &done, &dirty);
                #[cfg(test)]
                if let Some(Tick::Manual(ack)) = tick {
                    let _ = ack.send(());
                }
                let Ok(next) = next_tick(
                    &ticks,
                    next_capture,
                    #[cfg(test)]
                    manual,
                ) else {
                    break;
                };
                tick = next;
            }
        });
        let stop = Stop {
            stopped,
            wake,
            files_wake: files.wake,
        };
        Self {
            session,
            agent: current,
            stop,
            thread,
            files_thread: files.thread,
            activity_pending,
            #[cfg(test)]
            transcript_reads,
        }
    }
}

/// An independently cancellable Git reader, with a single bounded latest-result slot.
struct FilesWorker {
    thread: std::thread::JoinHandle<()>,
    wake: std::sync::mpsc::Sender<FilesTick>,
    slot: Arc<Mutex<Option<FilesRead>>>,
}

impl FilesWorker {
    fn start(
        current: Arc<Mutex<Agent>>,
        stopped: Arc<AtomicBool>,
        wake: std::sync::mpsc::Sender<Tick>,
        #[cfg(test)] files_shell: Option<Arc<dyn crate::shell::Shell>>,
    ) -> Self {
        let (files_wake, files_ticks) = std::sync::mpsc::channel();
        let slot = Arc::new(Mutex::new(None::<FilesRead>));
        let files_slot = Arc::clone(&slot);
        let files_source = current;
        let files_done = stopped;
        let notify = wake;
        let files_thread = std::thread::spawn(move || {
            let system = crate::shell::System::default();
            #[cfg(test)]
            let shell = files_shell.as_deref().unwrap_or(&system);
            #[cfg(not(test))]
            let shell = &system;
            #[cfg(test)]
            let mut acknowledgement: Option<std::sync::mpsc::SyncSender<()>> = None;
            while !files_done.load(Ordering::Acquire) {
                let agent = files_source
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                let state = crate::git_status::read(shell, agent.cwd.as_deref(), DETAIL_TICK);
                if files_done.load(Ordering::Acquire) {
                    break;
                }
                let mut slot = files_slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *slot = Some(FilesRead::new(&agent, state, slot.as_ref()));
                drop(slot);
                let _ = notify.send(Tick::Files);
                #[cfg(test)]
                if let Some(ack) = acknowledgement.take() {
                    let _ = ack.send(());
                }
                match files_ticks.recv_timeout(DETAIL_TICK) {
                    Ok(FilesTick::Stop) => break,
                    #[cfg(test)]
                    Ok(FilesTick::Manual(ack)) => acknowledgement = Some(ack),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => (),
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        Self {
            thread: files_thread,
            wake: files_wake,
            slot,
        }
    }
}

/// Session and resolved root fence retained content; cwd rejects superseded requests.
#[derive(Debug)]
struct FilesRead {
    session: Option<Session>,
    cwd: Option<String>,
    root: Option<String>,
    state: FilesState,
}

impl FilesRead {
    fn state_for(
        slot: &Mutex<Option<Self>>,
        session: Option<&Session>,
        agent: &Agent,
    ) -> FilesState {
        slot.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .filter(|read| read.session.as_ref() == session && read.cwd == agent.cwd)
            .map_or(FilesState::Pending, |read| read.state.clone())
    }

    fn new(agent: &Agent, capture: crate::git_status::Capture, previous: Option<&Self>) -> Self {
        let crate::git_status::Capture { root, mut state } = capture;
        let session = Session::of(agent);
        let previous =
            previous.filter(|read| read.session == session && root.is_some() && read.root == root);
        let snapshot = previous.and_then(|read| match &read.state {
            FilesState::Current(files) | FilesState::Stale { files, .. } => Some(files),
            _ => None,
        });
        match (&mut state, snapshot) {
            (FilesState::Current(files), Some(old))
                if files.root == old.root
                    && files.head == old.head
                    && files.entries == old.entries =>
            {
                // Capture time alone is not a content change.
                files.captured_ms = old.captured_ms;
            }
            (FilesState::Unavailable(error), Some(files)) => {
                state = FilesState::Stale {
                    files: files.clone(),
                    error: error.clone(),
                };
            }
            _ => (),
        }
        Self {
            session,
            cwd: agent.cwd.clone(),
            root,
            state,
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct Capture {
    transcript: crate::activity::Incremental,
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
            self.transcript = crate::activity::Incremental::default();
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
        #[cfg(test)]
        let reads = self.transcript.reads;
        let content = match (|| -> Result<Conversation> {
            let history = self
                .transcript
                .advance(agent.agent.as_deref().unwrap_or_default(), path)?;
            Ok(Conversation {
                pane: agent.pane.clone(),
                session: serde_json::to_string(session)?,
                turns: history.turns.clone(),
                truncated: history.truncated,
            })
        })() {
            Ok(conversation) => Content::Available(conversation),
            Err(error) => Content::Failed(error.to_string()),
        };
        #[cfg(test)]
        {
            self.transcript_reads += self.transcript.reads - reads;
        }
        content
    }
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
