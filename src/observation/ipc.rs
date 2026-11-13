//! Newline-delimited JSON over `observation.sock`. Requests are versioned operations that
//! select domains; a response is the selection from one revision, in one or more frames,
//! or an explicit error object.

#[cfg(test)]
mod tests;

use super::{Frame, Published, VERSION};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::{oneshot, watch};

/// The size of one frame, not of one reply. A larger reply has more frames.
pub(crate) const MAX_FRAME_BYTES: usize = 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_CLIENTS: usize = 64;
const TIMEOUT: Duration = Duration::from_secs(5);
const SOCKET: &str = "observation.sock";

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Current,
    Subscribe,
    Peer,
    Retry,
    Detail,
    Activity,
    ActivitySubscribe,
    ActivityCommand,
}

impl Operation {
    const fn name(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Subscribe => "subscribe",
            Self::Peer => "peer",
            Self::Retry => "retry",
            Self::Detail => "detail",
            Self::Activity => "activity",
            Self::ActivitySubscribe => "activity_subscribe",
            Self::ActivityCommand => "activity_command",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    operation: Operation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    peer: Option<super::peers::Request>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    activity: Option<super::live_activity::Command>,
    /// Absent: the whole publication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    select: Option<Chosen>,
}

/// Domain names of one selection. `sources` and `gaps` always travel with them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Chosen {
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub local: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub views: BTreeSet<String>,
}

/// What one reader needs. The cost of a reply follows this, not the publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Select {
    /// For the `observation` command and readers that pass the publication on.
    All,
    Domains(Chosen),
}

impl Select {
    pub fn views<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        Self::Domains(Chosen {
            local: BTreeSet::new(),
            views: names.into_iter().map(str::to_owned).collect(),
        })
    }

    pub fn local<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        Self::Domains(Chosen {
            local: names.into_iter().map(str::to_owned).collect(),
            views: BTreeSet::new(),
        })
    }

    fn chosen(&self) -> Option<Chosen> {
        match self {
            Self::All => None,
            Self::Domains(chosen) => Some(chosen.clone()),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct RetryAck {
    retry_token: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Failure {
    error: FailureDetail,
}

#[derive(Debug, Serialize, Deserialize)]
struct FailureDetail {
    code: String,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Reply<T> {
    Publication(T),
    Failure(Failure),
}

/// Owns publication for the service lifetime, even when its endpoint cannot be bound.
/// Joins all transport tasks before releasing the lifetime lock.
#[derive(Debug)]
pub(crate) struct Server {
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    lock: File,
    path: PathBuf,
    state: PathBuf,
    interests: super::peers::Interests,
    details: super::details::Interests,
    activity: Option<(
        super::live_activity::Commands,
        super::live_activity::ActivityFeed,
    )>,
    dirty: Option<crate::service::wake::Dirty>,
}

impl Server {
    pub fn new(state: &Path) -> Result<Self> {
        std::fs::create_dir_all(state)?;
        let lock = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(state.join("observation.lock"))?;
        lock.try_lock().map_err(|error| {
            Error::Invalid(format!("observation publisher already owned: {error}"))
        })?;
        Ok(Self {
            stop: None,
            thread: None,
            lock,
            path: super::socket_path(state, SOCKET)?,
            state: state.to_owned(),
            interests: super::peers::Interests::default(),
            details: super::details::Interests::default(),
            activity: None,
            dirty: None,
        })
    }

    pub fn branches(
        &mut self,
        interests: super::peers::Interests,
        dirty: crate::service::wake::Dirty,
    ) {
        self.interests = interests;
        self.dirty = Some(dirty);
    }

    pub fn details(&mut self, interests: super::details::Interests) {
        self.details = interests;
    }

    pub fn activity(
        &mut self,
        commands: super::live_activity::Commands,
        feed: super::live_activity::ActivityFeed,
    ) {
        self.activity = Some((commands, feed));
    }

    pub fn bind(&mut self, receiver: watch::Receiver<Option<Arc<Frame>>>) -> Result<()> {
        if self.thread.is_some() {
            return Err(Error::Invalid("observation endpoint already bound".into()));
        }
        self.path = super::socket_path(&self.state, SOCKET)?;
        let path = &self.path;
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_socket() => std::fs::remove_file(path)?,
            Ok(_) => {
                return Err(Error::Invalid(
                    "observation endpoint is not a socket".into(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        let listener = std::os::unix::net::UnixListener::bind(path)?;
        let setup = || -> Result<_> {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            listener.set_nonblocking(true)?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let listener = {
                let _entered = runtime.enter();
                tokio::net::UnixListener::from_std(listener)?
            };
            Ok((runtime, listener))
        };
        let (runtime, listener) = match setup() {
            Ok(ready) => ready,
            Err(error) => {
                let _ = std::fs::remove_file(path);
                return Err(error);
            }
        };
        let (stop, stopped) = oneshot::channel();
        let interests = self.interests.clone();
        let dirty = self.dirty.clone();
        let details = self.details.clone();
        let activity = self.activity.clone();
        let thread = std::thread::spawn(move || {
            runtime.block_on(serve(
                listener, receiver, stopped, interests, dirty, details, activity,
            ));
        });
        self.stop = Some(stop);
        self.thread = Some(thread);
        Ok(())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
            let _ = std::fs::remove_file(&self.path);
        }
        // Release copies inherited by a concurrent fork before exec, as Ledger's lock does.
        let _ = self.lock.unlock();
    }
}

async fn serve(
    listener: tokio::net::UnixListener,
    receiver: watch::Receiver<Option<Arc<Frame>>>,
    mut stopped: oneshot::Receiver<()>,
    interests: super::peers::Interests,
    dirty: Option<crate::service::wake::Dirty>,
    details: super::details::Interests,
    activity: Option<(
        super::live_activity::Commands,
        super::live_activity::ActivityFeed,
    )>,
) {
    let mut clients = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = &mut stopped => break,
            result = clients.join_next(), if !clients.is_empty() => { let _ = result; },
            accepted = listener.accept() => {
                let Ok((socket, _)) = accepted else { break; };
                if clients.len() < MAX_CLIENTS {
                    let receiver = receiver.clone();
                    let interests = interests.clone();
                    let dirty = dirty.clone();
                    let details = details.clone();
                    let activity = activity.clone();
                    clients.spawn(connection(socket, receiver, interests, dirty, details, activity));
                } else {
                    eprintln!("dogsled observation accept: client refused: MAX_CLIENTS ({MAX_CLIENTS}) reached");
                }
            }
        }
    }
    clients.abort_all();
    while clients.join_next().await.is_some() {}
}

async fn request(socket: &tokio::net::UnixStream) -> Result<Request> {
    let mut bytes = Vec::new();
    loop {
        socket.readable().await?;
        let mut byte = [0];
        match socket.try_read(&mut byte) {
            Ok(0) => return Err(Error::Invalid("observation request disconnected".into())),
            Ok(_) if byte[0] == b'\n' => return Ok(serde_json::from_slice(&bytes)?),
            Ok(_) => {
                bytes.push(byte[0]);
                if bytes.len() >= MAX_REQUEST_BYTES {
                    return Err(Error::Invalid("observation request too large".into()));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => (),
            Err(error) => return Err(error.into()),
        }
    }
}

async fn write(socket: &tokio::net::UnixStream, mut bytes: &[u8]) -> Result<()> {
    tokio::time::timeout(TIMEOUT, async {
        while !bytes.is_empty() {
            socket.writable().await?;
            match socket.try_write(bytes) {
                Ok(0) => return Err(std::io::Error::from(std::io::ErrorKind::WriteZero)),
                Ok(count) => bytes = &bytes[count..],
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => (),
                Err(error) => return Err(error),
            }
        }
        Ok::<_, std::io::Error>(())
    })
    .await
    .map_err(|_| Error::Invalid("observation subscriber write timed out".into()))??;
    Ok(())
}

/// Each frame has its own deadline, so a large reply needs progress, not one fast write.
async fn write_frames(socket: &tokio::net::UnixStream, frames: &[Vec<u8>]) -> Result<()> {
    for frame in frames {
        write(socket, frame).await?;
    }
    Ok(())
}

async fn failure(socket: &tokio::net::UnixStream, code: &str, message: &str) -> Result<()> {
    let mut bytes = serde_json::to_vec(&Failure {
        error: FailureDetail {
            code: code.into(),
            message: message.into(),
        },
    })?;
    bytes.push(b'\n');
    write(socket, &bytes).await
}

async fn connection(
    socket: tokio::net::UnixStream,
    receiver: watch::Receiver<Option<Arc<Frame>>>,
    interests: super::peers::Interests,
    dirty: Option<crate::service::wake::Dirty>,
    details: super::details::Interests,
    activity: Option<(
        super::live_activity::Commands,
        super::live_activity::ActivityFeed,
    )>,
) {
    let (operation, result) = match tokio::time::timeout(TIMEOUT, request(&socket)).await {
        Ok(Ok(request)) => (
            request.operation.name(),
            respond(
                socket, receiver, interests, dirty, details, activity, request,
            )
            .await,
        ),
        _ => (
            "request",
            failure(&socket, "invalid", "invalid observation request").await,
        ),
    };
    if let Err(error) = result {
        eprintln!("dogsled observation {operation}: {error}");
    }
}

async fn respond(
    socket: tokio::net::UnixStream,
    mut receiver: watch::Receiver<Option<Arc<Frame>>>,
    interests: super::peers::Interests,
    dirty: Option<crate::service::wake::Dirty>,
    details: super::details::Interests,
    activity: Option<(
        super::live_activity::Commands,
        super::live_activity::ActivityFeed,
    )>,
    request: Request,
) -> Result<()> {
    if request.version != VERSION {
        return failure(
            &socket,
            "unsupported",
            "unsupported observation protocol version",
        )
        .await;
    }
    if matches!(request.operation, Operation::ActivityCommand) {
        return activity_command(&socket, request, activity.map(|(commands, _)| commands)).await;
    }
    if matches!(
        request.operation,
        Operation::Activity | Operation::ActivitySubscribe
    ) {
        return activity_connection(&socket, activity.map(|(_, feed)| feed), request).await;
    }
    if matches!(request.operation, Operation::Detail) {
        let Some(pane) = request.pane.as_deref() else {
            return failure(&socket, "invalid", "missing detail pane").await;
        };
        return detail_connection(&socket, &mut receiver, details, dirty, pane).await;
    }
    if matches!(request.operation, Operation::Retry) {
        let frame = receiver.borrow().clone();
        return retry_command(&socket, frame, request.host, dirty.as_ref()).await;
    }
    if matches!(request.operation, Operation::Peer) {
        let Some(peer) = request.peer else {
            return failure(&socket, "invalid", "missing peer request").await;
        };
        return peer_connection(&socket, &mut receiver, interests, dirty, peer).await;
    }
    loop {
        if receiver.has_changed().is_err() {
            return failure(&socket, "unavailable", "observation publisher disconnected").await;
        }
        // Release the watch borrow before writing: readers cannot delay publication.
        // The reply holds this revision until its last frame: a newer publication cannot
        // mix into it, and a slow reader delays no one else.
        let frame = receiver.borrow_and_update().clone();
        match frame {
            Some(frame) => write_frames(&socket, &frame.reply(request.select.as_ref())?).await?,
            None => return failure(&socket, "unavailable", "observation unavailable").await,
        }
        if matches!(request.operation, Operation::Current | Operation::Retry)
            || !updated(&socket, &mut receiver).await?
        {
            return Ok(());
        }
    }
}

/// A lease starts capture immediately; worker changes wake only this pane's subscribers.
async fn detail_connection(
    socket: &tokio::net::UnixStream,
    receiver: &mut watch::Receiver<Option<Arc<Frame>>>,
    interests: super::details::Interests,
    dirty: Option<crate::service::wake::Dirty>,
    pane: &str,
) -> Result<()> {
    let Ok(_interest) = interests.register(pane, dirty) else {
        return failure(socket, "invalid", "invalid detail pane").await;
    };
    let frame = receiver.borrow_and_update().clone();
    let Some(frame) = frame else {
        return failure(socket, "unavailable", "observation unavailable").await;
    };
    let Some(mut payloads) = frame.details.subscribe(pane) else {
        return failure(socket, "unavailable", "agent detail unavailable").await;
    };
    let mut sent = None;
    loop {
        if receiver.has_changed().is_err() {
            return failure(socket, "unavailable", "observation publisher disconnected").await;
        }
        let frame = receiver.borrow_and_update().clone();
        let Some(frame) = frame else {
            return failure(socket, "unavailable", "observation unavailable").await;
        };
        let identity = selected_agent(&frame.published, pane)?;
        if identity.is_none()
            && frame
                .published
                .views
                .get("portal")
                .and_then(|view| view.pointer("/data/agents"))
                .is_some_and(serde_json::Value::is_array)
        {
            let missing = super::details::CODEC.encode(&super::details::Detail::unavailable(
                None,
                "agent unavailable",
            ))?;
            write_detail(socket, &frame.published, None, false, None, &missing).await?;
            return Ok(());
        }
        let payload = payloads.borrow_and_update().clone();
        // The worker can finish while a new native observation is still being projected.
        // Never pair its session with metadata from a different publication session.
        let terminals = frame
            .published
            .local
            .get("terminals")
            .filter(|_| {
                !frame
                    .published
                    .gaps
                    .iter()
                    .any(|gap| gap.domain == "terminals" || gap.domain == "host")
            })
            .map(|value| serde_json::from_value::<crate::probe::Terminals>(value.clone()))
            .transpose()?;
        let session = terminals
            .as_ref()
            .and_then(|terminals| terminals.agents.iter().find(|agent| agent.pane == pane))
            .and_then(super::details::Session::of);
        if let Some(payload) = payload.filter(|payload| payload.detail.session == session) {
            let signature = (payload.reference.revision, identity.clone());
            if sent.as_ref() != Some(&signature) {
                write_detail(
                    socket,
                    &frame.published,
                    identity,
                    true,
                    Some(&payload.reference.parts),
                    &payload.bytes,
                )
                .await?;
                sent = Some(signature);
            }
        }
        tokio::select! {
            changed = payloads.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
            },
            changed = updated(socket, receiver) => {
                if !changed? {
                    return Ok(());
                }
            },
        }
    }
}

/// Resolve retry intent against one frozen publication before queueing its command.
async fn retry_command(
    socket: &tokio::net::UnixStream,
    frame: Option<Arc<Frame>>,
    host: Option<String>,
    dirty: Option<&crate::service::wake::Dirty>,
) -> Result<()> {
    let (Some(host), Some(dirty)) = (host, dirty) else {
        return failure(socket, "unavailable", "peer observer unavailable").await;
    };
    let Some(frame) = frame else {
        return failure(socket, "unavailable", "observation unavailable").await;
    };
    let reports: Vec<crate::hosts::HostReport> = super::read::view(&frame.published, "hosts")?;
    let Some(selected) = reports
        .into_iter()
        .find(|report| report.host.id == host || report.host.label == host)
    else {
        return failure(socket, "unknown_host", &format!("unknown host {host}")).await;
    };
    let mut bytes = serde_json::to_vec(&RetryAck {
        retry_token: dirty.retry_peer(selected.host.id),
    })?;
    bytes.push(b'\n');
    write(socket, &bytes).await
}

async fn activity_command(
    socket: &tokio::net::UnixStream,
    request: Request,
    activity: Option<super::live_activity::Commands>,
) -> Result<()> {
    use portal_types::agents::ActivityCode;
    let result = match (request.pane, request.activity, activity) {
        (Some(pane), Some(command), Some(activity)) => activity.send(pane, command).await,
        (_, _, None) => Err(super::live_activity::error(
            ActivityCode::UpstreamUnavailable,
            "activity authority unavailable",
        )),
        _ => Err(super::live_activity::error(
            ActivityCode::InvalidRequest,
            "missing activity command",
        )),
    };
    let mut bytes = serde_json::to_vec(&result)?;
    bytes.push(b'\n');
    write(socket, &bytes).await
}

async fn activity_connection(
    socket: &tokio::net::UnixStream,
    feed: Option<super::live_activity::ActivityFeed>,
    request: Request,
) -> Result<()> {
    let Some(pane) = request.pane else {
        return failure(socket, "invalid_request", "missing activity pane").await;
    };
    if super::live_activity::validate_pane(&pane).is_err() {
        return failure(socket, "invalid_request", "invalid activity pane").await;
    }
    let Some(mut receiver) = feed else {
        return failure(
            socket,
            "upstream_unavailable",
            "activity authority unavailable",
        )
        .await;
    };
    loop {
        if receiver.has_changed().is_err() {
            return failure(
                socket,
                "upstream_unavailable",
                "activity publisher disconnected",
            )
            .await;
        }
        let frame = receiver.borrow_and_update().clone();
        if let Some(bytes) = frame.views.get(&pane) {
            // Relative validity is a transport budget, not renewed by reading a retained
            // publication. Subtract queue/retention time before handing it to a consumer.
            let mut activity: portal_types::agents::ActivityView = serde_json::from_slice(bytes)?;
            if let portal_types::agents::ActivityDetail::Available { valid_for_ms, .. } =
                &mut activity.detail
            {
                let elapsed =
                    u64::try_from(frame.measured.elapsed().as_millis()).unwrap_or(u64::MAX);
                *valid_for_ms = valid_for_ms.saturating_sub(elapsed);
                if *valid_for_ms == 0 {
                    activity.detail = portal_types::agents::ActivityDetail::Unavailable {
                        reason: portal_types::agents::UnavailableReason::NativeUnavailable,
                    };
                }
            }
            let mut bytes = serde_json::to_vec(&activity)?;
            bytes.push(b'\n');
            write(socket, &bytes).await?;
        } else if !frame.bound.contains(&pane)
            && frame.bindings_measured_ms.is_some_and(|measured| {
                crate::ledger::now_ms().saturating_sub(measured)
                    < super::live_activity::NATIVE_TTL_MS
            })
        {
            return failure(socket, "agent_missing", "agent is absent").await;
        } else {
            return failure(
                socket,
                "upstream_unavailable",
                "activity binding unavailable",
            )
            .await;
        }
        if matches!(request.operation, Operation::Activity)
            || !updated(socket, &mut receiver).await?
        {
            return Ok(());
        }
    }
}

async fn peer_connection(
    socket: &tokio::net::UnixStream,
    receiver: &mut watch::Receiver<Option<Arc<Frame>>>,
    interests: super::peers::Interests,
    dirty: Option<crate::service::wake::Dirty>,
    peer: super::peers::Request,
) -> Result<()> {
    match interests.register(&peer.interests) {
        Ok(true) => {
            if let Some(dirty) = dirty {
                dirty.mark(crate::service::wake::BRANCHES);
            }
        }
        Ok(false) => (),
        Err(_) => return failure(socket, "invalid", "invalid branch interests").await,
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let frame = receiver.borrow_and_update().clone();
        let Some(frame) = frame else {
            return failure(socket, "unavailable", "observation unavailable").await;
        };
        if !peer.wait
            || peer.epoch.as_ref() != Some(&frame.published.epoch)
            || peer.local_revision != Some(frame.published.local_revision)
            || tokio::time::Instant::now() >= deadline
        {
            let export = serde_json::to_value(super::peers::Export::of(&frame.published))?;
            return write_frames(socket, &super::pages::frames(&export, MAX_FRAME_BYTES)?).await;
        }
        match tokio::time::timeout_at(deadline, updated(socket, receiver)).await {
            Ok(Ok(false)) => return Ok(()),
            Ok(Err(error)) => return Err(error),
            _ => (),
        }
    }
}

fn selected_agent(
    publication: &Published,
    pane: &str,
) -> Result<Option<portal_types::agents::AgentView>> {
    let Some(agents) = publication
        .views
        .get("portal")
        .and_then(|view| view.pointer("/data/agents"))
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(None);
    };
    agents
        .iter()
        .find(|agent| {
            agent["host"].as_str() == Some(crate::hosts::LOCAL)
                && agent["pane"].as_str() == Some(pane)
        })
        .map(|agent| serde_json::from_value(agent.clone()).map_err(Into::into))
        .transpose()
}

/// Serialize only the small envelope; the immutable content bytes are shared by viewers.
async fn write_detail(
    socket: &tokio::net::UnixStream,
    publication: &Published,
    agent: Option<portal_types::agents::AgentView>,
    found: bool,
    parts: Option<&portal_types::agents::DetailRevisions>,
    content: &[u8],
) -> Result<()> {
    let mut header = serde_json::to_vec(&serde_json::json!({
        "version": VERSION, "epoch": publication.epoch, "revision": publication.revision, "found": found, "agent": agent, "parts": parts
    }))?;
    header.pop();
    header.extend_from_slice(b",\"detail\":");
    write(socket, &header).await?;
    write(socket, content).await?;
    write(socket, b"}\n").await
}

/// Observe peer closure even when there are no new publications, releasing its client slot.
async fn updated<T: Send + Sync>(
    socket: &tokio::net::UnixStream,
    receiver: &mut watch::Receiver<T>,
) -> Result<bool> {
    loop {
        tokio::select! {
            changed = receiver.changed() => return Ok(changed.is_ok()),
            readable = socket.readable() => {
                readable?;
                let mut byte = [0];
                match socket.try_read(&mut byte) {
                    Ok(0) => return Ok(false),
                    Ok(_) => return Err(Error::Invalid("unexpected observation subscriber data".into())),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => (),
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }
}

/// This client has only a socket path. It cannot discover, derive, lock, or repair state.
#[derive(Debug, Clone)]
pub(crate) struct Client {
    state: PathBuf,
}

impl Client {
    pub fn at(state: &Path) -> Self {
        Self {
            state: state.to_owned(),
        }
    }

    fn connect(&self) -> Result<UnixStream> {
        let not_running =
            || Error::Invalid("observation unavailable: dogsled serve is not running".into());
        // A long state path resolves its socket through the state directory itself, so a
        // missing state directory is the same fact as a missing socket: nothing serves it.
        let path = super::socket_path(&self.state, SOCKET).map_err(|error| match error {
            Error::Io(io) if io.kind() == std::io::ErrorKind::NotFound => not_running(),
            other => other,
        })?;
        UnixStream::connect(path).map_err(|error| {
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) {
                not_running()
            } else {
                unavailable(&error)
            }
        })
    }

    fn open(
        &self,
        operation: Operation,
        pane: Option<&str>,
        select: Option<Chosen>,
    ) -> Result<UnixStream> {
        let mut socket = self.connect()?;
        socket
            .set_write_timeout(Some(TIMEOUT))
            .map_err(|error| unavailable(&error))?;
        socket
            .set_read_timeout(Some(TIMEOUT))
            .map_err(|error| unavailable(&error))?;
        serde_json::to_writer(
            &mut socket,
            &Request {
                version: VERSION,
                operation,
                peer: None,
                host: None,
                pane: pane.map(str::to_owned),
                activity: None,
                select,
            },
        )?;
        socket
            .write_all(b"\n")
            .map_err(|error| unavailable(&error))?;
        Ok(socket)
    }

    pub fn retry(&self, host: &str) -> Result<String> {
        let mut socket = self.connect()?;
        socket.set_write_timeout(Some(TIMEOUT))?;
        socket.set_read_timeout(Some(TIMEOUT))?;
        serde_json::to_writer(
            &mut socket,
            &Request {
                version: VERSION,
                operation: Operation::Retry,
                peer: None,
                host: Some(host.into()),
                pane: None,
                activity: None,
                select: None,
            },
        )?;
        socket.write_all(b"\n")?;
        let mut line = String::new();
        BufReader::new(socket)
            .take(MAX_FRAME_BYTES as u64 + 1)
            .read_line(&mut line)?;
        if let Ok(failure) = serde_json::from_str::<Failure>(&line) {
            return Err(Error::Invalid(failure.error.message));
        }
        let ack: RetryAck = serde_json::from_str(&line).map_err(|_| {
            Error::Invalid("observation unavailable: invalid retry acknowledgment".into())
        })?;
        Ok(ack.retry_token)
    }

    pub fn peer(&self, request: super::peers::Request) -> Result<super::peers::Export> {
        let mut socket = self.connect()?;
        socket.set_read_timeout(Some(TIMEOUT))?;
        socket.set_write_timeout(Some(TIMEOUT))?;
        serde_json::to_writer(
            &mut socket,
            &Request {
                version: VERSION,
                operation: Operation::Peer,
                peer: Some(request),
                host: None,
                pane: None,
                activity: None,
                select: None,
            },
        )?;
        socket.write_all(b"\n")?;
        let value = super::pages::read(&mut BufReader::new(socket), MAX_FRAME_BYTES)?;
        if let Ok(failure) = Failure::deserialize(&value) {
            return Err(Error::Invalid(format!(
                "observation {}: {}",
                failure.error.code, failure.error.message
            )));
        }
        serde_json::from_value(value).map_err(Into::into)
    }

    pub fn activity_command(
        &self,
        pane: &str,
        command: super::live_activity::Command,
    ) -> std::result::Result<portal_types::agents::ActivityAck, portal_types::agents::ActivityError>
    {
        use portal_types::agents::ActivityCode;
        let exchange = || -> Result<_> {
            let mut socket = self.connect()?;
            socket.set_write_timeout(Some(TIMEOUT))?;
            socket.set_read_timeout(Some(TIMEOUT))?;
            serde_json::to_writer(
                &mut socket,
                &Request {
                    version: VERSION,
                    operation: Operation::ActivityCommand,
                    peer: None,
                    host: None,
                    pane: Some(pane.into()),
                    activity: Some(command),
                    select: None,
                },
            )?;
            socket.write_all(b"\n")?;
            let mut bytes = Vec::new();
            BufReader::new(socket)
                .take(super::live_activity::MAX_BODY as u64 + 1)
                .read_until(b'\n', &mut bytes)?;
            if bytes.len() > super::live_activity::MAX_BODY || bytes.last() != Some(&b'\n') {
                return Err(Error::Invalid("invalid activity acknowledgment".into()));
            }
            Ok(serde_json::from_slice::<std::result::Result<_, _>>(&bytes)?)
        };
        exchange().map_err(|failure| {
            super::live_activity::error(ActivityCode::UpstreamUnavailable, &failure.to_string())
        })?
    }

    pub fn activity(
        &self,
        pane: &str,
    ) -> std::result::Result<portal_types::agents::ActivityView, portal_types::agents::ActivityError>
    {
        let socket = self
            .open(Operation::Activity, Some(pane), None)
            .map_err(|error| activity_transport_error(&error))?;
        read_activity(&mut BufReader::new(socket))
    }

    pub fn activity_subscribe(
        &self,
        pane: &str,
    ) -> std::result::Result<
        Subscription<portal_types::agents::ActivityView>,
        portal_types::agents::ActivityError,
    > {
        let socket = self
            .open(Operation::ActivitySubscribe, Some(pane), None)
            .map_err(|error| activity_transport_error(&error))?;
        let mut reader = BufReader::new(
            socket
                .try_clone()
                .map_err(|error| activity_transport_error(&error))?,
        );
        let publication = read_activity(&mut reader)?;
        Ok(start_subscription(socket, reader, publication))
    }

    pub fn current(&self, select: &Select) -> Result<Published> {
        read(&mut BufReader::new(self.open(
            Operation::Current,
            None,
            select.chosen(),
        )?))
    }

    pub fn subscribe(&self, select: &Select) -> Result<Subscription> {
        self.subscription(Operation::Subscribe, None, select.chosen())
    }

    pub fn detail(&self, pane: &str) -> Result<Subscription<super::details::DetailFrame>> {
        self.subscription(Operation::Detail, Some(pane), None)
    }

    fn subscription<T: Wire>(
        &self,
        operation: Operation,
        pane: Option<&str>,
        select: Option<Chosen>,
    ) -> Result<Subscription<T>> {
        let socket = self.open(operation, pane, select)?;
        let mut reader = BufReader::new(socket.try_clone()?);
        let publication = read_value::<T>(&mut reader)?;
        Ok(start_subscription(socket, reader, publication))
    }
}

fn start_subscription<T: Wire>(
    socket: UnixStream,
    mut reader: BufReader<UnixStream>,
    publication: T,
) -> Subscription<T> {
    let publication = Arc::new(publication);
    // The first publication is authoritative even when the publisher closes after it.
    // Darwin refuses socket options once the peer has shut down (EINVAL); Linux accepts
    // them and the receiver then reads EOF. Both are a disconnect after one publication.
    let (status, receiving) = match socket.set_read_timeout(None) {
        Ok(()) => (Status::Connected, true),
        Err(error) => (Status::Disconnected(error.to_string()), false),
    };
    let shared = Arc::new((
        Mutex::new(Connection {
            publication,
            status,
            generation: 0,
        }),
        Condvar::new(),
    ));
    let output = Arc::clone(&shared);
    let thread = receiving.then(|| std::thread::spawn(move || receive(&mut reader, &output)));
    Subscription {
        socket,
        shared,
        thread,
        seen: 0,
    }
}

fn activity_transport_error(error: &impl std::fmt::Display) -> portal_types::agents::ActivityError {
    super::live_activity::error(
        portal_types::agents::ActivityCode::UpstreamUnavailable,
        &error.to_string(),
    )
}

/// Keep protocol refusal codes typed across the socket boundary, separate from I/O loss.
fn read_activity(
    reader: &mut BufReader<UnixStream>,
) -> std::result::Result<portal_types::agents::ActivityView, portal_types::agents::ActivityError> {
    use portal_types::agents::ActivityCode;
    let invalid =
        |message: &str| super::live_activity::error(ActivityCode::UpstreamInvalid, message);
    let mut bytes = Vec::new();
    reader
        .take(super::live_activity::MAX_FRAME as u64 + 1)
        .read_until(b'\n', &mut bytes)
        .map_err(|error| activity_transport_error(&error))?;
    if bytes.is_empty() {
        return Err(super::live_activity::error(
            ActivityCode::UpstreamUnavailable,
            "activity publisher disconnected",
        ));
    }
    if bytes.last() != Some(&b'\n') || bytes.len() > super::live_activity::MAX_FRAME {
        return Err(invalid("invalid activity frame"));
    }
    let reply: Reply<portal_types::agents::ActivityView> =
        serde_json::from_slice(&bytes).map_err(|_| invalid("invalid activity frame"))?;
    match reply {
        Reply::Publication(view)
            if view.valid() && view.version == super::live_activity::VERSION =>
        {
            Ok(view)
        }
        Reply::Publication(_) => Err(invalid("invalid activity revision or version")),
        Reply::Failure(failure) => {
            let code = match failure.error.code.as_str() {
                "agent_missing" => ActivityCode::AgentMissing,
                "invalid_request" => ActivityCode::InvalidRequest,
                "upstream_unavailable" => ActivityCode::UpstreamUnavailable,
                _ => ActivityCode::UpstreamInvalid,
            };
            Err(super::live_activity::error(code, &failure.error.message))
        }
    }
}

fn unavailable(error: &std::io::Error) -> Error {
    Error::Invalid(format!("observation unavailable: {error}"))
}

fn read(reader: &mut BufReader<UnixStream>) -> Result<Published> {
    read_value(reader)
}

pub(crate) trait Wire:
    serde::de::DeserializeOwned + std::fmt::Debug + Clone + Send + Sync + 'static
{
    const MAX_BYTES: usize;
    /// A framed reply has no size bound; `MAX_BYTES` bounds each of its frames.
    const FRAMED: bool = false;
    const VERSION: u32 = VERSION;
    fn version(&self) -> u32;
    fn epoch(&self) -> &str;
    fn revision(&self) -> u64;
    fn follows(&self, previous: &Self) -> bool {
        self.epoch() == previous.epoch() && self.revision() > previous.revision()
    }
    fn valid(&self) -> bool {
        !self.epoch().is_empty() && self.revision() > 0
    }
}

impl Wire for Published {
    const MAX_BYTES: usize = MAX_FRAME_BYTES;
    const FRAMED: bool = true;
    fn version(&self) -> u32 {
        self.version
    }
    fn epoch(&self) -> &str {
        &self.epoch
    }
    fn revision(&self) -> u64 {
        self.revision
    }
    fn valid(&self) -> bool {
        !self.epoch.is_empty() && self.revision > 0 && self.local_revision > 0
    }
}

impl Wire for portal_types::agents::ActivityView {
    const MAX_BYTES: usize = super::live_activity::MAX_FRAME;
    const VERSION: u32 = super::live_activity::VERSION;
    fn version(&self) -> u32 {
        self.version
    }
    fn epoch(&self) -> &str {
        &self.epoch
    }
    fn revision(&self) -> u64 {
        self.revision
    }
}

impl Wire for super::details::DetailFrame {
    const MAX_BYTES: usize = super::details::CODEC.wire_limit();
    fn version(&self) -> u32 {
        self.version
    }
    fn epoch(&self) -> &str {
        &self.epoch
    }
    fn revision(&self) -> u64 {
        self.revision
    }
    fn follows(&self, previous: &Self) -> bool {
        // This revision stamps the main observation, not the independent detail stream.
        // Several ordered worker captures can share the same metadata publication.
        self.epoch == previous.epoch && self.revision >= previous.revision
    }
}

fn read_value<T: Wire>(reader: &mut BufReader<UnixStream>) -> Result<T> {
    if T::FRAMED {
        let value = super::pages::read(reader, T::MAX_BYTES)?;
        return accept(serde_json::from_value(value)?);
    }
    let mut bytes = Vec::new();
    reader
        .take(T::MAX_BYTES as u64 + 1)
        .read_until(b'\n', &mut bytes)
        .map_err(|error| unavailable(&error))?;
    if bytes.is_empty() || bytes.last() != Some(&b'\n') || bytes.len() > T::MAX_BYTES {
        return Err(Error::Invalid(
            "observation unavailable: closed or oversized frame".into(),
        ));
    }
    accept(super::details::Codec::decode::<Reply<T>>(
        &bytes,
        T::MAX_BYTES,
    )?)
}

fn accept<T: Wire>(reply: Reply<T>) -> Result<T> {
    match reply {
        Reply::Publication(value) => {
            if value.version() != T::VERSION {
                return Err(Error::Invalid(
                    "unsupported observation protocol version".into(),
                ));
            }
            if !value.valid() {
                return Err(Error::Invalid("invalid observation revision".into()));
            }
            Ok(value)
        }
        Reply::Failure(failure) => Err(Error::Invalid(format!(
            "observation {}: {}",
            failure.error.code, failure.error.message
        ))),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Status {
    Connected,
    Disconnected(String),
}

/// A retained publication is not current after its publisher disconnects.
#[derive(Debug, Clone)]
pub(crate) struct Connection<T = Published> {
    pub publication: Arc<T>,
    pub status: Status,
    generation: u64,
}

type Shared<T> = Arc<(Mutex<Connection<T>>, Condvar)>;

#[derive(Debug)]
pub(crate) struct Subscription<T = Published> {
    socket: UnixStream,
    shared: Shared<T>,
    thread: Option<JoinHandle<()>>,
    seen: u64,
}

impl<T: Wire> Subscription<T> {
    pub fn current(&self) -> Connection<T> {
        self.shared
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Wait for replacement or disconnect. Timeout returns the retained connection state.
    pub fn changed(&mut self, timeout: Duration) -> Connection<T> {
        let (mutex, changed) = &*self.shared;
        let state = mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (state, _) = changed
            .wait_timeout_while(state, timeout, |state| {
                state.generation == self.seen && state.status == Status::Connected
            })
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.seen = state.generation;
        state.clone()
    }
}

impl<T> Drop for Subscription<T> {
    fn drop(&mut self) {
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn receive<T: Wire>(reader: &mut BufReader<UnixStream>, shared: &Shared<T>) {
    loop {
        let result = read_value::<T>(reader);
        let mut state = shared
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = result.and_then(|value| {
            if value.follows(&state.publication) {
                Ok(value)
            } else {
                Err(Error::Invalid(
                    "observation publication is out of order".into(),
                ))
            }
        });
        match result {
            Ok(value) => state.publication = Arc::new(value),
            Err(error) => state.status = Status::Disconnected(error.to_string()),
        }
        state.generation = state.generation.wrapping_add(1);
        shared.1.notify_all();
        if matches!(state.status, Status::Disconnected(_)) {
            break;
        }
    }
}
