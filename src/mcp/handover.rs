//! The stdio boundary owns unread input and outstanding responses. It never reads ahead
//! of one JSON-RPC line, and drains responses before replacing the executable.

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::sync::{Arc, Mutex};

use rmcp::RoleServer;
use rmcp::model::{
    ClientJsonRpcMessage, ClientRequest, InitializeRequestParams, JsonRpcMessage, RequestId,
    ServerJsonRpcMessage, ServerResult,
};
use rmcp::transport::Transport;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::ledger::Ledger;

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Resume {
    pub peer: Option<InitializeRequestParams>,
    pub pending: Option<String>,
}

#[derive(Default)]
struct State {
    peer: Option<InitializeRequestParams>,
    active: HashSet<RequestId>,
}

pub(super) struct Stdio {
    ledger: Ledger,
    input: Arc<Mutex<File>>,
    reading: Option<tokio::task::JoinHandle<std::io::Result<Option<String>>>>,
    pending: Option<String>,
    state: Arc<Mutex<State>>,
    drained: Arc<tokio::sync::Notify>,
    pub failure: Arc<Mutex<Option<Error>>>,
}

impl Stdio {
    pub fn new(ledger: Ledger, resume: Resume) -> Result<Self> {
        Ok(Self {
            ledger,
            // Unlike std::io::Stdin, File has no userspace input buffer to lose on exec.
            input: Arc::new(Mutex::new(File::open("/dev/stdin")?)),
            reading: None,
            pending: resume.pending,
            state: Arc::new(Mutex::new(State {
                peer: resume.peer,
                ..State::default()
            })),
            drained: Arc::default(),
            failure: Arc::default(),
        })
    }

    async fn next(&mut self) -> Result<Option<ClientJsonRpcMessage>> {
        if self.pending.is_none() {
            // Keep the handle across cancelled receive futures: rmcp selects input and
            // outgoing responses concurrently. Dropping a future must not lose a line.
            let reading = self.reading.get_or_insert_with(|| {
                let input = Arc::clone(&self.input);
                tokio::task::spawn_blocking(move || read_line(&input))
            });
            let result = reading
                .await
                .map_err(|error| Error::Invalid(format!("mcp stdin: {error}")))?;
            self.reading = None;
            let Some(line) = result? else {
                return Ok(None);
            };
            self.pending = Some(line);
        }
        let line = self
            .pending
            .as_deref()
            .ok_or_else(|| Error::Invalid("mcp input disappeared".into()))?;
        let boundary = crate::upgrade::newer(&self.ledger)?;
        if let crate::upgrade::Boundary::Pending(error) = &boundary {
            // Dispatch this input under the store guard; retry at the next input boundary.
            eprintln!("dogsled mcp upgrade: {error}");
        }
        if let crate::upgrade::Boundary::Ready(writer) = boundary {
            // The line is not dispatched yet. Existing requests finish once; only this
            // undispatched line and the negotiated peer state cross the exec boundary.
            loop {
                let notified = self.drained.notified();
                if self.state.lock().map_err(poisoned)?.active.is_empty() {
                    break;
                }
                notified.await;
            }
            let peer = self.state.lock().map_err(poisoned)?.peer.clone();
            let resume = serde_json::to_string(&Resume {
                peer,
                pending: Some(line.into()),
            })?;
            match crate::upgrade::handover(&writer, Some(&resume)) {
                Err(error) if matches!(error.root(), Error::NewerWriter(_)) => {
                    // No input was dispatched or consumed by a failed exec. Let this
                    // request fail under the guard and retry at the next input boundary.
                    eprintln!("dogsled mcp upgrade: {error}");
                }
                result => {
                    result?;
                    return Ok(None);
                }
            }
        }
        let message: ClientJsonRpcMessage = serde_json::from_str(line)?;
        if let JsonRpcMessage::Request(request) = &message {
            let mut state = self.state.lock().map_err(poisoned)?;
            state.active.insert(request.id.clone());
            if let ClientRequest::InitializeRequest(initialize) = &request.request {
                state.peer = Some(initialize.params.clone());
            }
        }
        self.pending = None;
        Ok(Some(message))
    }
}

fn poisoned<T>(error: std::sync::PoisonError<T>) -> Error {
    let message = error.to_string();
    drop(error);
    Error::Invalid(format!("mcp handover state: {message}"))
}

fn read_line(input: &Mutex<File>) -> std::io::Result<Option<String>> {
    let mut input = input
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let mut bytes = Vec::new();
    let mut byte = [0];
    loop {
        match input.read(&mut byte) {
            Ok(0) if bytes.is_empty() => return Ok(None),
            Ok(0) => return Err(std::io::Error::other("incomplete MCP input line")),
            Ok(_) if byte[0] == b'\n' => {
                return String::from_utf8(bytes)
                    .map(Some)
                    .map_err(std::io::Error::other);
            }
            Ok(_) => bytes.push(byte[0]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

impl Transport<RoleServer> for Stdio {
    type Error = std::io::Error;

    fn send(
        &mut self,
        message: ServerJsonRpcMessage,
    ) -> impl Future<Output = std::io::Result<()>> + Send + 'static {
        let state = Arc::clone(&self.state);
        let drained = Arc::clone(&self.drained);
        async move {
            let mut output = std::io::stdout().lock();
            serde_json::to_writer(&mut output, &message)?;
            writeln!(output)?;
            output.flush()?;
            let mut state = state
                .lock()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            match message {
                JsonRpcMessage::Response(response) => {
                    if let ServerResult::InitializeResult(result) = response.result
                        && let Some(peer) = &mut state.peer
                    {
                        peer.protocol_version = result.protocol_version;
                    }
                    state.active.remove(&response.id);
                }
                JsonRpcMessage::Error(error) => {
                    if let Some(id) = error.id {
                        state.active.remove(&id);
                    }
                }
                _ => {}
            }
            drained.notify_one();
            Ok(())
        }
    }

    async fn receive(&mut self) -> Option<ClientJsonRpcMessage> {
        match self.next().await {
            Ok(message) => message,
            Err(error) => {
                if let Ok(mut failure) = self.failure.lock() {
                    *failure = Some(error);
                }
                None
            }
        }
    }

    fn close(&mut self) -> impl Future<Output = std::io::Result<()>> + Send {
        std::future::ready(Ok(()))
    }
}
