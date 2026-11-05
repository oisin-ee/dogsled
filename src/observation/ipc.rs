//! Newline-delimited JSON over `observation.sock`. Requests are versioned `current` or
//! `subscribe` operations; responses are complete publications or explicit error objects.

#[cfg(test)]
mod tests;

use super::{Frame, Published, VERSION};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::{oneshot, watch};

pub(crate) const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
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
    activity: Option<super::live_activity::Commands>,
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

    pub fn activity(&mut self, commands: super::live_activity::Commands) {
        self.activity = Some(commands);
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
    activity: Option<super::live_activity::Commands>,
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
                    clients.spawn(async move { let _ = connection(socket, receiver, interests, dirty, details, activity).await; });
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
    mut receiver: watch::Receiver<Option<Arc<Frame>>>,
    interests: super::peers::Interests,
    dirty: Option<crate::service::wake::Dirty>,
    details: super::details::Interests,
    activity: Option<super::live_activity::Commands>,
) -> Result<()> {
    let Ok(Ok(request)) = tokio::time::timeout(TIMEOUT, request(&socket)).await else {
        return failure(&socket, "invalid", "invalid observation request").await;
    };
    if request.version != VERSION {
        return failure(
            &socket,
            "unsupported",
            "unsupported observation protocol version",
        )
        .await;
    }
    if matches!(request.operation, Operation::ActivityCommand) {
        return activity_command(&socket, request, activity, dirty.as_ref()).await;
    }
    if matches!(
        request.operation,
        Operation::Activity | Operation::ActivitySubscribe
    ) {
        return activity_connection(&socket, &mut receiver, request).await;
    }
    let _detail_interest = if matches!(request.operation, Operation::Detail) {
        let Some(pane) = request.pane.as_deref() else {
            return failure(&socket, "invalid", "missing detail pane").await;
        };
        match details.register(pane, dirty.clone()) {
            Ok(interest) => Some(interest),
            Err(_) => return failure(&socket, "invalid", "invalid detail pane").await,
        }
    } else {
        None
    };
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
    let mut sent_detail = None;
    loop {
        if receiver.has_changed().is_err() {
            return failure(&socket, "unavailable", "observation publisher disconnected").await;
        }
        // Release the watch borrow before writing: readers cannot delay publication.
        let frame = receiver.borrow_and_update().clone();
        match frame {
            Some(frame) if matches!(request.operation, Operation::Detail) => {
                let pane = request
                    .pane
                    .as_deref()
                    .ok_or_else(|| Error::Invalid("missing detail pane".into()))?;
                if let Some(payload) = frame.details.get(pane) {
                    let identity = selected_agent(&frame.published, pane)?;
                    let signature = (payload.reference.revision, identity.clone());
                    if sent_detail.as_ref() != Some(&signature) {
                        write_detail(&socket, &frame.published, identity, true, &payload.bytes)
                            .await?;
                        sent_detail = Some(signature);
                    }
                } else if frame
                    .published
                    .views
                    .get("portal")
                    .and_then(|view| view.pointer("/data/agents"))
                    .is_some_and(serde_json::Value::is_array)
                    && selected_agent(&frame.published, pane)?.is_none()
                {
                    let missing = serde_json::to_vec(&super::details::Detail::unavailable(
                        None,
                        "agent unavailable",
                    ))?;
                    write_detail(&socket, &frame.published, None, false, &missing).await?;
                    return Ok(());
                }
            }
            Some(frame) => write(&socket, &frame.bytes).await?,
            None => return failure(&socket, "unavailable", "observation unavailable").await,
        }
        if matches!(request.operation, Operation::Current | Operation::Retry)
            || !updated(&socket, &mut receiver).await?
        {
            return Ok(());
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
    dirty: Option<&crate::service::wake::Dirty>,
) -> Result<()> {
    use portal_types::agents::ActivityCode;
    let result = match (request.pane, request.activity, activity) {
        (Some(pane), Some(command), Some(activity)) => activity.send(pane, command, dirty).await,
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
    receiver: &mut watch::Receiver<Option<Arc<Frame>>>,
    request: Request,
) -> Result<()> {
    let Some(pane) = request.pane else {
        return failure(socket, "invalid_request", "missing activity pane").await;
    };
    if super::live_activity::validate_pane(&pane).is_err() {
        return failure(socket, "invalid_request", "invalid activity pane").await;
    }
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
        let Some(frame) = frame else {
            return failure(socket, "upstream_unavailable", "observation unavailable").await;
        };
        if let Some(bytes) = frame.activity.get(&pane) {
            // Relative validity is a transport budget, not renewed by reading a retained
            // publication. Subtract queue/retention time before handing it to a consumer.
            let mut activity: portal_types::agents::ActivityView = serde_json::from_slice(bytes)?;
            if let portal_types::agents::ActivityDetail::Available { valid_for_ms, .. } =
                &mut activity.detail
            {
                let elapsed = u64::try_from(frame.activity_measured.elapsed().as_millis())
                    .unwrap_or(u64::MAX);
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
        } else if frame
            .published
            .views
            .get("portal")
            .and_then(|view| view.pointer("/data/agents"))
            .is_some_and(serde_json::Value::is_array)
            && selected_agent(&frame.published, &pane)?.is_none()
            && frame
                .published
                .sources
                .local
                .get("terminals")
                .is_some_and(|measured| {
                    crate::ledger::now_ms().saturating_sub(*measured)
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
        if matches!(request.operation, Operation::Activity) || !updated(socket, receiver).await? {
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
            let mut bytes = serde_json::to_vec(&super::peers::Export::of(&frame.published))?;
            bytes.push(b'\n');
            return write(socket, &bytes).await;
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
        .find(|agent| agent["pane"].as_str() == Some(pane))
        .map(|agent| serde_json::from_value(agent.clone()).map_err(Into::into))
        .transpose()
}

/// Serialize only the small envelope; the immutable content bytes are shared by viewers.
async fn write_detail(
    socket: &tokio::net::UnixStream,
    publication: &Published,
    agent: Option<portal_types::agents::AgentView>,
    found: bool,
    content: &[u8],
) -> Result<()> {
    let mut header = serde_json::to_vec(&serde_json::json!({
        "version": VERSION, "epoch": publication.epoch, "revision": publication.revision, "found": found, "agent": agent
    }))?;
    header.pop();
    header.extend_from_slice(b",\"detail\":");
    write(socket, &header).await?;
    write(socket, content).await?;
    write(socket, b"}\n").await
}

/// Observe peer closure even when there are no new publications, releasing its client slot.
async fn updated(
    socket: &tokio::net::UnixStream,
    receiver: &mut watch::Receiver<Option<Arc<Frame>>>,
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
        let path = super::socket_path(&self.state, SOCKET)?;
        UnixStream::connect(path).map_err(|error| {
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) {
                Error::Invalid("observation unavailable: dogsled serve is not running".into())
            } else {
                unavailable(&error)
            }
        })
    }

    fn open(&self, operation: Operation, pane: Option<&str>) -> Result<UnixStream> {
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
            },
        )?;
        socket.write_all(b"\n")?;
        let mut bytes = Vec::new();
        BufReader::new(socket)
            .take(MAX_FRAME_BYTES as u64 + 1)
            .read_until(b'\n', &mut bytes)?;
        if bytes.len() > MAX_FRAME_BYTES || bytes.last() != Some(&b'\n') {
            return Err(Error::Invalid(
                "observation unavailable: invalid peer frame".into(),
            ));
        }
        serde_json::from_slice(&bytes).map_err(Into::into)
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
            .open(Operation::Activity, Some(pane))
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
            .open(Operation::ActivitySubscribe, Some(pane))
            .map_err(|error| activity_transport_error(&error))?;
        let mut reader = BufReader::new(
            socket
                .try_clone()
                .map_err(|error| activity_transport_error(&error))?,
        );
        let publication = read_activity(&mut reader)?;
        Ok(start_subscription(socket, reader, publication))
    }

    pub fn current(&self) -> Result<Published> {
        read(&mut BufReader::new(self.open(Operation::Current, None)?))
    }

    pub fn subscribe(&self) -> Result<Subscription> {
        self.subscription(Operation::Subscribe, None)
    }

    pub fn detail(&self, pane: &str) -> Result<Subscription<super::details::DetailFrame>> {
        self.subscription(Operation::Detail, Some(pane))
    }

    fn subscription<T: Wire>(
        &self,
        operation: Operation,
        pane: Option<&str>,
    ) -> Result<Subscription<T>> {
        let socket = self.open(operation, pane)?;
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
        Reply::Publication(view) if view.valid() && view.version == VERSION => Ok(view),
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
    fn version(&self) -> u32;
    fn epoch(&self) -> &str;
    fn revision(&self) -> u64;
    fn valid(&self) -> bool {
        !self.epoch().is_empty() && self.revision() > 0
    }
}

impl Wire for Published {
    const MAX_BYTES: usize = MAX_FRAME_BYTES;
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
}

fn read_value<T: Wire>(reader: &mut BufReader<UnixStream>) -> Result<T> {
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
    match super::details::Codec::decode::<Reply<T>>(&bytes, T::MAX_BYTES)? {
        Reply::Publication(value) => {
            if value.version() != VERSION {
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
            if value.epoch() != state.publication.epoch()
                || value.revision() <= state.publication.revision()
            {
                Err(Error::Invalid(
                    "observation publication is out of order".into(),
                ))
            } else {
                Ok(value)
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
