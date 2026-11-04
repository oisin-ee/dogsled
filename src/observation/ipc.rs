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
const MAX_REQUEST_BYTES: usize = 256;
const MAX_CLIENTS: usize = 64;
const TIMEOUT: Duration = Duration::from_secs(5);
const SOCKET: &str = "observation.sock";

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Current,
    Subscribe,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    operation: Operation,
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
enum Reply {
    Publication(Published),
    Failure(Failure),
}

/// Owns the endpoint and joins all transport tasks before releasing its lifetime lock.
#[derive(Debug)]
pub(crate) struct Server {
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    lock: File,
    path: PathBuf,
}

impl Server {
    pub fn bind(state: &Path, receiver: watch::Receiver<Option<Arc<Frame>>>) -> Result<Self> {
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
        let path = state.join(SOCKET);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_socket() => std::fs::remove_file(&path)?,
            Ok(_) => {
                return Err(Error::Invalid(
                    "observation endpoint is not a socket".into(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        let listener = std::os::unix::net::UnixListener::bind(&path)?;
        let setup = || -> Result<_> {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
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
                let _ = std::fs::remove_file(&path);
                return Err(error);
            }
        };
        let (stop, stopped) = oneshot::channel();
        let thread = std::thread::spawn(move || {
            runtime.block_on(serve(listener, receiver, stopped));
        });
        Ok(Self {
            stop: Some(stop),
            thread: Some(thread),
            lock,
            path,
        })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.path);
        // Release copies inherited by a concurrent fork before exec, as Ledger's lock does.
        let _ = self.lock.unlock();
    }
}

async fn serve(
    listener: tokio::net::UnixListener,
    receiver: watch::Receiver<Option<Arc<Frame>>>,
    mut stopped: oneshot::Receiver<()>,
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
                    clients.spawn(async move { let _ = connection(socket, receiver).await; });
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
    loop {
        if receiver.has_changed().is_err() {
            return failure(&socket, "unavailable", "observation publisher disconnected").await;
        }
        // Release the watch borrow before writing: readers cannot delay publication.
        let frame = receiver.borrow_and_update().clone();
        match frame {
            Some(frame) => write(&socket, &frame.bytes).await?,
            None => return failure(&socket, "unavailable", "observation unavailable").await,
        }
        if matches!(request.operation, Operation::Current)
            || !updated(&socket, &mut receiver).await?
        {
            return Ok(());
        }
    }
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
    path: PathBuf,
}

impl Client {
    pub fn at(state: &Path) -> Self {
        Self {
            path: state.join(SOCKET),
        }
    }

    fn open(&self, operation: Operation) -> Result<UnixStream> {
        let mut socket = UnixStream::connect(&self.path).map_err(|error| unavailable(&error))?;
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
            },
        )?;
        socket
            .write_all(b"\n")
            .map_err(|error| unavailable(&error))?;
        Ok(socket)
    }

    pub fn current(&self) -> Result<Published> {
        read(&mut BufReader::new(self.open(Operation::Current)?))
    }

    pub fn subscribe(&self) -> Result<Subscription> {
        let socket = self.open(Operation::Subscribe)?;
        let mut reader = BufReader::new(socket.try_clone()?);
        let publication = Arc::new(read(&mut reader)?);
        socket.set_read_timeout(None)?;
        let shared = Arc::new((
            Mutex::new(Connection {
                publication,
                status: Status::Connected,
                generation: 0,
            }),
            Condvar::new(),
        ));
        let output = Arc::clone(&shared);
        let thread = std::thread::spawn(move || receive(&mut reader, &output));
        Ok(Subscription {
            socket,
            shared,
            thread: Some(thread),
            seen: 0,
        })
    }
}

fn unavailable(error: &std::io::Error) -> Error {
    Error::Invalid(format!("observation unavailable: {error}"))
}

fn read(reader: &mut BufReader<UnixStream>) -> Result<Published> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_FRAME_BYTES as u64 + 1)
        .read_until(b'\n', &mut bytes)
        .map_err(|error| unavailable(&error))?;
    if bytes.is_empty() || bytes.last() != Some(&b'\n') || bytes.len() > MAX_FRAME_BYTES {
        return Err(Error::Invalid(
            "observation unavailable: closed or oversized frame".into(),
        ));
    }
    match serde_json::from_slice::<Reply>(&bytes)? {
        Reply::Publication(value) => {
            if value.version != VERSION {
                return Err(Error::Invalid(
                    "unsupported observation protocol version".into(),
                ));
            }
            if value.epoch.is_empty() || value.revision == 0 || value.local_revision == 0 {
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
pub(crate) struct Connection {
    pub publication: Arc<Published>,
    pub status: Status,
    generation: u64,
}

type Shared = Arc<(Mutex<Connection>, Condvar)>;

#[derive(Debug)]
pub(crate) struct Subscription {
    socket: UnixStream,
    shared: Shared,
    thread: Option<JoinHandle<()>>,
    seen: u64,
}

impl Subscription {
    pub fn current(&self) -> Connection {
        self.shared
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Wait for replacement or disconnect. Timeout returns the retained connection state.
    pub fn changed(&mut self, timeout: Duration) -> Connection {
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

impl Drop for Subscription {
    fn drop(&mut self) {
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn receive(reader: &mut BufReader<UnixStream>, shared: &Shared) {
    loop {
        let result = read(reader);
        let mut state = shared
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = result.and_then(|value| {
            if value.epoch != state.publication.epoch
                || value.revision <= state.publication.revision
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
