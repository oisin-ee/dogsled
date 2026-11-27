//! One browser socket multiplexes portal publications and the selected pane's interests.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use portal_types::agents::{ActivityView, AgentAddress, Conversation, DetailSession};
use portal_types::live::{ClientFrame, ConversationCursor, ConversationUpdate, ServerFrame, diff};
use portal_types::{PortalAvailability, PortalEvent, PortalStatus};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;

use super::agents::{PeerAgentEvent, PeerOperation, agent_event, peer_conversation};
use super::{AppState, PortalState};
use crate::observation::details::{Content, DetailFrame};
use crate::observation::{ipc, live_activity};

pub(super) async fn upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    // The ordinary GET guard checks Host, but WebSocket reads do not have CORS protection.
    if headers.contains_key(header::ORIGIN)
        && !super::guard::origin_allowed(
            &Method::POST,
            headers
                .get(header::ORIGIN)
                .and_then(|value| value.to_str().ok()),
            state.port,
        )
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.max_message_size(1024)
        .max_frame_size(1024)
        .on_upgrade(move |socket| async move {
            tokio::select! {
                () = state.stop.cancelled() => {},
                () = serve(&state, socket) => {},
            }
        })
}

fn portal(current: PortalState) -> ServerFrame {
    let event = match (current.snapshot, current.error) {
        (Some(snapshot), None) => PortalEvent::Snapshot(Box::new(snapshot)),
        (snapshot, error) => PortalEvent::Status(PortalStatus {
            observation: if snapshot.is_some() {
                PortalAvailability::Disconnected
            } else {
                PortalAvailability::Unavailable
            },
            error: error.unwrap_or_else(|| "observation unavailable".to_owned()),
        }),
    };
    ServerFrame::Portal { event }
}

async fn serve(state: &AppState, mut socket: WebSocket) {
    use tokio_stream::{StreamExt as _, wrappers::WatchStream};

    let mut portal = WatchStream::new(state.portal.clone()).map(portal);
    let mut inbox = WatchStream::new(state.inbox_live.subscribe(state))
        .filter_map(|result| result.map(|result| ServerFrame::Inbox { result }));
    let mut selected = None::<Selection>;
    let mut heartbeat = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(15),
        Duration::from_secs(15),
    );
    loop {
        let frame = tokio::select! {
            message = socket.recv() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        let Ok(ClientFrame::Open { id, address, conversation }) = serde_json::from_str(&text) else {
                            let _ = socket.send(Message::Close(None)).await;
                            return;
                        };
                        // Dropping the previous selection releases its interests and queued frames.
                        selected = None;
                        if let Some(address) = address {
                            if !valid(&address) {
                                let _ = socket.send(Message::Close(None)).await;
                                return;
                            }
                            selected = Some(Selection::new(state, id, &address, conversation));
                        }
                        continue;
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                    _ => return,
                }
            }
            Some(frame) = portal.next() => frame,
            Some(frame) = inbox.next() => frame,
            Some(frame) = async {
                match &mut selected {
                    Some(selection) => selection.frames.recv().await,
                    None => std::future::pending().await,
                }
            } => frame,
            _ = heartbeat.tick() => {
                if !send(&mut socket, Message::Ping(axum::body::Bytes::new())).await {
                    return;
                }
                continue;
            }
        };
        let Ok(text) = serde_json::to_string(&frame) else {
            return;
        };
        if !send(&mut socket, Message::Text(text.into())).await {
            return;
        }
    }
}

async fn send(socket: &mut WebSocket, message: Message) -> bool {
    matches!(
        tokio::time::timeout(crate::budget::CALL_LIMIT, socket.send(message)).await,
        Ok(Ok(()))
    )
}

fn valid(address: &AgentAddress) -> bool {
    !address.host.is_empty()
        && address.host.len() <= 255
        && !address.host.chars().any(char::is_control)
        && live_activity::validate_pane(&address.pane).is_ok()
}

struct Selection {
    frames: mpsc::Receiver<ServerFrame>,
    // JoinSet aborts these tasks when this pane is deselected or the socket closes.
    _interests: JoinSet<()>,
}

impl Selection {
    fn new(
        state: &AppState,
        id: u64,
        address: &AgentAddress,
        conversation: Option<ConversationCursor>,
    ) -> Self {
        let (sender, frames) = mpsc::channel(2);
        let mut interests = JoinSet::new();
        if address.host == crate::hosts::LOCAL {
            interests.spawn(follow_local_detail(
                state.clone(),
                address.clone(),
                sender.clone(),
                id,
                conversation,
            ));
        } else {
            interests.spawn(follow(
                state.clone(),
                address.clone(),
                sender.clone(),
                PeerOperation::Event,
                |client, pane| client.detail(pane).map_err(|error| error.to_string()),
                |publication, pane| agent_event(publication, pane),
                move |result| match result {
                    Ok(event) => ServerFrame::Agent {
                        id,
                        event: Box::new(event),
                    },
                    Err(error) => ServerFrame::AgentUnavailable { id, error },
                },
            ));
            interests.spawn(follow_remote_conversation(
                state.clone(),
                address.clone(),
                sender.clone(),
                id,
            ));
        }
        interests.spawn(follow(
            state.clone(),
            address.clone(),
            sender,
            PeerOperation::Activity,
            |client, pane| client.activity_subscribe(pane).map_err(|error| error.error),
            |view: &Arc<ActivityView>, _| Ok((**view).clone()),
            move |result| match result {
                Ok(view) => ServerFrame::Activity {
                    id,
                    view: Box::new(view),
                },
                Err(_) => ServerFrame::ActivityUnavailable { id },
            },
        ));
        Self {
            frames,
            _interests: interests,
        }
    }
}

type Update<T> = Result<T, String>;

struct ConversationInterest {
    sender: mpsc::Sender<ServerFrame>,
    id: u64,
    offered: Option<ConversationCursor>,
    sent: Option<(ConversationCursor, Conversation)>,
    error: Option<String>,
}

impl ConversationInterest {
    async fn available(&mut self, cursor: ConversationCursor, snapshot: &Conversation) -> bool {
        if self.sent.as_ref().is_some_and(|(base, _)| base == &cursor) && self.error.is_none() {
            return true;
        }
        let update = if self.offered.take().as_ref() == Some(&cursor) {
            ConversationUpdate::Confirm {
                cursor: cursor.clone(),
            }
        } else if let Some((base, previous)) = &self.sent
            && base != &cursor
            && base.token == cursor.token
            && previous.session == snapshot.session
            && let Some((drop_front, keep)) = diff(&previous.turns, &snapshot.turns)
        {
            ConversationUpdate::Splice {
                base: base.clone(),
                cursor: cursor.clone(),
                drop_front,
                keep,
                turns: snapshot.turns[keep..].to_vec(),
                truncated: snapshot.truncated,
            }
        } else {
            ConversationUpdate::Reset {
                cursor: cursor.clone(),
                session: snapshot.session.clone(),
                turns: snapshot.turns.clone(),
                truncated: snapshot.truncated,
            }
        };
        if !self.send(update).await {
            return false;
        }
        self.sent = Some((cursor, snapshot.clone()));
        self.error = None;
        true
    }

    async fn unavailable(&mut self, error: String) -> bool {
        if self.error.as_ref() == Some(&error) {
            return true;
        }
        self.error = Some(error.clone());
        self.send(ConversationUpdate::Unavailable { error }).await
    }

    async fn send(&self, update: ConversationUpdate) -> bool {
        self.sender
            .send(ServerFrame::Conversation {
                id: self.id,
                update: Box::new(update),
            })
            .await
            .is_ok()
    }
}

fn cursor(host: &str, session: &str, epoch: &str, revision: u64) -> ConversationCursor {
    let mut hash = Sha256::new();
    hash.update(host.as_bytes());
    hash.update([0]);
    hash.update(session.as_bytes());
    hash.update([0]);
    hash.update(epoch.as_bytes());
    let token = hash.finalize()[..16]
        .iter()
        .flat_map(|byte| {
            const HEX: &[u8; 16] = b"0123456789abcdef";
            [
                char::from(HEX[usize::from(byte >> 4)]),
                char::from(HEX[usize::from(byte & 15)]),
            ]
        })
        .collect();
    ConversationCursor { token, revision }
}

/// Hold the publication by reference. The subscription yields a new value only for a new
/// publication, so identity is the comparison; screen-only changes copy no turns.
#[derive(Clone)]
struct Publication(Arc<DetailFrame>);

impl PartialEq for Publication {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// One held detail subscription for a selected agent on this machine. Each publication
/// gives the agent event and the conversation update, so the service sees one interest.
async fn follow_local_detail(
    state: AppState,
    address: AgentAddress,
    sender: mpsc::Sender<ServerFrame>,
    id: u64,
    offered: Option<ConversationCursor>,
) {
    let mut interest = ConversationInterest {
        sender: sender.clone(),
        id,
        offered,
        sent: None,
        error: None,
    };
    let mut last_error = None;
    let mut backoff = Backoff::default();
    loop {
        let mut updates = local(
            &state,
            address.pane.clone(),
            |client, pane| client.detail(pane).map_err(|error| error.to_string()),
            |frame, _| Ok(Publication(Arc::clone(frame))),
        );
        let mut sent_event = None;
        let error = loop {
            match current(&mut updates).await {
                Some(Ok(Publication(frame))) => {
                    let event = match agent_event(&frame, &address.pane) {
                        Ok(event) => event,
                        Err(error) => break error.to_string(),
                    };
                    last_error = None;
                    if sent_event.as_ref() != Some(&event) {
                        sent_event = Some(event.clone());
                        let frame = ServerFrame::Agent {
                            id,
                            event: Box::new(event),
                        };
                        if sender.send(frame).await.is_err() {
                            return;
                        }
                    }
                    if !local_conversation(&mut interest, &address, &frame).await {
                        return;
                    }
                }
                Some(Err(error)) => break error,
                None => break "observation disconnected".to_owned(),
            }
            if updates.changed().await.is_err() {
                break "observation disconnected".to_owned();
            }
        };
        // A delivered value clears the last error, so this attempt worked before it failed.
        if last_error.is_none() {
            backoff.reset();
        }
        if last_error.as_ref() != Some(&error) {
            last_error = Some(error.clone());
            let frame = ServerFrame::AgentUnavailable {
                id,
                error: error.clone(),
            };
            if sender.send(frame).await.is_err() || !interest.unavailable(error).await {
                return;
            }
        }
        tokio::time::sleep(backoff.next_delay()).await;
    }
}

/// The conversation of an agent on another machine. Its events come from the shared held
/// peer stream; each new conversation revision is read once.
async fn follow_remote_conversation(
    state: AppState,
    address: AgentAddress,
    sender: mpsc::Sender<ServerFrame>,
    id: u64,
) {
    // A peer's revisions restart with its service, so a remote cursor is never confirmed.
    let mut interest = ConversationInterest {
        sender,
        id,
        offered: None,
        sent: None,
        error: None,
    };
    let mut backoff = Backoff::default();
    loop {
        let mut connected = false;
        // Peers without an epoch can still supply revisioned updates within one held stream.
        // A new follow must not reuse their cursor after a possible service restart.
        let follow_epoch = crate::ledger::new_id();
        let mut requested = None;
        let error = match PeerUpdates::open(&state, &address, &PeerOperation::Event).await {
            Ok(mut updates) => loop {
                match updates.next::<PeerAgentEvent>().await {
                    Ok(publication) => {
                        connected = true;
                        match remote_conversation(
                            &state,
                            &address,
                            &mut interest,
                            &mut requested,
                            publication,
                            &follow_epoch,
                        )
                        .await
                        {
                            Ok(true) => {}
                            Ok(false) => return,
                            // A failed read is tried again after the wait below, not only
                            // when the agent next writes.
                            Err(error) => break error,
                        }
                    }
                    Err(error) => break error,
                }
            },
            Err(error) => error,
        };
        if connected {
            backoff.reset();
        }
        if !interest.unavailable(error).await {
            return;
        }
        tokio::time::sleep(backoff.next_delay()).await;
    }
}

async fn local_conversation(
    interest: &mut ConversationInterest,
    address: &AgentAddress,
    frame: &DetailFrame,
) -> bool {
    match &frame.detail.conversation {
        Content::Pending => true,
        Content::Failed(error) | Content::Unavailable(error) => {
            interest.unavailable(error.clone()).await
        }
        Content::Available(snapshot) => {
            let Some(parts) = &frame.parts else {
                return interest
                    .unavailable("conversation revision unavailable".into())
                    .await;
            };
            let session = match frame.detail.key() {
                Ok(session) => session,
                Err(error) => return interest.unavailable(error.to_string()).await,
            };
            let cursor = cursor(&address.host, &session, &frame.epoch, parts.conversation);
            interest.available(cursor, snapshot).await
        }
    }
}

async fn remote_conversation(
    state: &AppState,
    address: &AgentAddress,
    interest: &mut ConversationInterest,
    requested: &mut Option<ConversationCursor>,
    publication: PeerAgentEvent,
    follow_epoch: &str,
) -> Update<bool> {
    let event = publication.event;
    if event.pending.is_some_and(|parts| parts.conversation) {
        return Ok(true);
    }
    let (Some(parts), DetailSession::Available(session)) = (event.revisions, event.session) else {
        return Ok(interest
            .unavailable("conversation revision or session unavailable".into())
            .await);
    };
    if event.pane != address.pane {
        return Ok(interest
            .unavailable("agent event belongs to another pane".into())
            .await);
    }
    let cursor = cursor(
        &address.host,
        &session,
        publication.epoch.as_deref().unwrap_or(follow_epoch),
        parts.conversation,
    );
    if requested.as_ref() == Some(&cursor) {
        return Ok(true);
    }
    let snapshot =
        peer_conversation(state.clone(), address.host.clone(), address.pane.clone()).await?;
    if snapshot.session != session || snapshot.pane != address.pane {
        // The session changed between the event and the read; the next event names it.
        return Ok(true);
    }
    *requested = Some(cursor.clone());
    Ok(interest.available(cursor, &snapshot).await)
}

/// The wait before one failed interest is tried again: 1 s, doubling to 30 s, and 1 s again
/// after the interest delivered a value. An unreachable host is not asked every two seconds.
struct Backoff(Duration);

impl Default for Backoff {
    fn default() -> Self {
        Self(Duration::from_secs(1))
    }
}

impl Backoff {
    fn next_delay(&mut self) -> Duration {
        let delay = self.0;
        self.0 = (delay * 2).min(Duration::from_secs(30));
        delay
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

async fn follow<T, W>(
    state: AppState,
    address: AgentAddress,
    sender: mpsc::Sender<ServerFrame>,
    operation: PeerOperation,
    open: fn(ipc::Client, &str) -> Update<ipc::Subscription<W>>,
    project: fn(&Arc<W>, &str) -> crate::error::Result<T>,
    frame: impl Fn(Update<T>) -> ServerFrame + Send + Sync,
) where
    T: serde::de::DeserializeOwned + PartialEq + Clone + Send + Sync + 'static,
    W: ipc::Wire + Send + Sync + 'static,
{
    let mut last_error = None;
    let mut backoff = Backoff::default();
    loop {
        let error = if address.host == crate::hosts::LOCAL {
            let mut updates = local(&state, address.pane.clone(), open, project);
            loop {
                match current(&mut updates).await {
                    Some(Ok(value)) => {
                        last_error = None;
                        if sender.send(frame(Ok(value))).await.is_err() {
                            return;
                        }
                    }
                    Some(Err(error)) => break error,
                    None => break "observation disconnected".to_owned(),
                }
                if updates.changed().await.is_err() {
                    break "observation disconnected".to_owned();
                }
            }
        } else {
            match PeerUpdates::open(&state, &address, &operation).await {
                Ok(mut updates) => loop {
                    match updates.next::<T>().await {
                        Ok(value) => {
                            last_error = None;
                            if sender.send(frame(Ok(value))).await.is_err() {
                                return;
                            }
                        }
                        Err(error) => break error,
                    }
                },
                Err(error) => error,
            }
        };
        // A delivered value clears the last error, so this attempt worked before it failed.
        if last_error.is_none() {
            backoff.reset();
        }
        if last_error.as_ref() != Some(&error) {
            last_error = Some(error.clone());
            if sender.send(frame(Err(error))).await.is_err() {
                return;
            }
        }
        // Retry only the failed interest. The portal and the other interest keep flowing.
        tokio::time::sleep(backoff.next_delay()).await;
    }
}

fn local<T, W>(
    state: &AppState,
    pane: String,
    open: fn(ipc::Client, &str) -> Update<ipc::Subscription<W>>,
    project: fn(&Arc<W>, &str) -> crate::error::Result<T>,
) -> watch::Receiver<Option<Update<T>>>
where
    T: PartialEq + Send + Sync + 'static,
    W: ipc::Wire,
{
    let client = ipc::Client::at(&state.web.dir);
    let (sender, receiver) = watch::channel(None);
    tokio::task::spawn_blocking(move || {
        let mut subscription = match open(client, &pane) {
            Ok(subscription) => subscription,
            Err(error) => {
                sender.send_replace(Some(Err(error)));
                return;
            }
        };
        let mut connection = subscription.current();
        while !sender.is_closed() {
            let next = match connection.status {
                ipc::Status::Disconnected(error) => Err(error),
                ipc::Status::Connected => {
                    project(&connection.publication, &pane).map_err(|error| error.to_string())
                }
            };
            let failed = next.is_err();
            sender.send_if_modified(|previous| {
                if previous.as_ref() == Some(&next) {
                    return false;
                }
                *previous = Some(next);
                true
            });
            if failed {
                break;
            }
            let previous = connection.publication;
            loop {
                connection = subscription.changed(super::TICK);
                if sender.is_closed() {
                    return;
                }
                if !std::sync::Arc::ptr_eq(&previous, &connection.publication)
                    || matches!(connection.status, ipc::Status::Disconnected(_))
                {
                    break;
                }
            }
        }
    });
    receiver
}

async fn current<T: Clone>(updates: &mut watch::Receiver<Option<T>>) -> Option<T> {
    loop {
        if let Some(value) = updates.borrow_and_update().clone() {
            return Some(value);
        }
        updates.changed().await.ok()?;
    }
}

struct PeerUpdates {
    host: String,
    lines: watch::Receiver<Option<super::peer_stream::Line>>,
    first: Option<super::peer_stream::Line>,
}

impl PeerUpdates {
    async fn open(
        state: &AppState,
        address: &AgentAddress,
        operation: &PeerOperation,
    ) -> Update<Self> {
        let unavailable = || format!("host {}: agent stream unavailable", address.host);
        let mut lines = state
            .followers
            .subscribe(state, address.host.clone(), address.pane.clone(), operation)
            .map_err(|_| unavailable())?;
        let Ok(Some(first)) =
            tokio::time::timeout(crate::budget::CALL_LIMIT, current(&mut lines)).await
        else {
            return Err(unavailable());
        };
        Ok(Self {
            host: address.host.clone(),
            lines,
            first: Some(first),
        })
    }

    async fn next<T: serde::de::DeserializeOwned>(&mut self) -> Update<T> {
        let unavailable = || format!("host {}: agent stream unavailable", self.host);
        let line = if let Some(first) = self.first.take() {
            first
        } else {
            self.lines.changed().await.map_err(|_| unavailable())?;
            self.lines
                .borrow_and_update()
                .clone()
                .ok_or_else(unavailable)?
        };
        let reply = line?;
        match reply.status {
            200 => serde_json::from_value(reply.body).map_err(|_| unavailable()),
            ..500 => Err(reply.body["error"]
                .as_str()
                .map_or_else(unavailable, |error| format!("host {}: {error}", self.host))),
            _ => Err(unavailable()),
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use futures_util::StreamExt as _;
    use tokio::net::{TcpListener, TcpStream};
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite};
    use tungstenite::client::IntoClientRequest as _;

    pub(in crate::web) type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

    pub(in crate::web) async fn connect(
        port: u16,
        origin: &str,
    ) -> Result<Socket, tungstenite::Error> {
        let mut request = format!("ws://localhost:{port}/api/live")
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert(header::ORIGIN, origin.parse().unwrap());
        let (socket, response) =
            tokio::time::timeout(crate::test_support::WAIT_DEADLINE, connect_async(request))
                .await
                .expect("upgrade deadline")?;
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        Ok(socket)
    }

    pub(in crate::web) struct Server {
        port: u16,
        task: tokio::task::JoinHandle<std::io::Result<()>>,
    }

    impl Server {
        pub(in crate::web) async fn start(state: &AppState) -> Self {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let mut state = state.clone();
            state.port = port;
            let task = tokio::spawn(super::super::serve_until(
                listener,
                state,
                std::future::pending(),
            ));
            Self { port, task }
        }

        pub(in crate::web) async fn connect(&self) -> Socket {
            connect(self.port, &format!("http://localhost:{}", self.port))
                .await
                .unwrap()
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    pub(in crate::web) async fn select(
        socket: &mut Socket,
        id: u64,
        address: Option<AgentAddress>,
    ) {
        open(socket, id, address, None).await;
    }

    async fn open(
        socket: &mut Socket,
        id: u64,
        address: Option<AgentAddress>,
        conversation: Option<ConversationCursor>,
    ) {
        use futures_util::SinkExt as _;
        let text = serde_json::to_string(&ClientFrame::Open {
            id,
            address,
            conversation,
        })
        .unwrap();
        socket
            .send(tungstenite::Message::Text(text.into()))
            .await
            .unwrap();
    }

    pub(in crate::web) async fn read(socket: &mut Socket) -> Option<ServerFrame> {
        loop {
            match socket.next().await {
                Some(Ok(tungstenite::Message::Text(text))) => {
                    return Some(serde_json::from_str(&text).unwrap());
                }
                Some(Ok(tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_))) => {}
                Some(
                    Ok(tungstenite::Message::Close(_))
                    | Err(tungstenite::Error::Protocol(
                        tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
                    )),
                )
                | None => return None,
                message => panic!("unexpected socket message: {message:?}"),
            }
        }
    }

    pub(in crate::web) async fn next<T>(
        socket: &mut Socket,
        project: impl Fn(ServerFrame) -> Option<T>,
    ) -> T {
        tokio::time::timeout(crate::test_support::WAIT_DEADLINE, async {
            loop {
                if let Some(value) = project(read(socket).await.expect("live socket stays open")) {
                    return value;
                }
            }
        })
        .await
        .expect("live frame deadline")
    }

    pub(in crate::web) async fn agent(socket: &mut Socket) -> portal_types::agents::AgentEvent {
        next(socket, |frame| match frame {
            ServerFrame::Agent { event, .. } => Some(*event),
            _ => None,
        })
        .await
    }

    #[test]
    fn retry_backoff_doubles_caps_and_resets_after_a_value() {
        let mut backoff = Backoff::default();
        let delays: Vec<_> = (0..8).map(|_| backoff.next_delay().as_secs()).collect();
        assert_eq!(delays, [1, 2, 4, 8, 16, 30, 30, 30]);
        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_secs(1));
        assert_eq!(backoff.next_delay(), Duration::from_secs(2));
    }

    #[tokio::test]
    async fn live_socket_resets_splices_and_confirms_conversation_history() {
        use crate::herdr_socket::fake;
        use crate::observation::details::fixture::Fixture;
        use serde_json::json;
        use std::io::Write as _;

        let (_, portal) = watch::channel(PortalState::default());
        let ledger = crate::ledger::Ledger::temporary();
        std::fs::create_dir_all(&ledger.dir).unwrap();
        let path = ledger.dir.join("session.jsonl");
        let line = |id: &str, text: &str| json!({"type":"message","id":id,"message":{"role":"user","content":text}});
        std::fs::write(&path, format!("{}\n", line("one", "first turn"))).unwrap();
        let transcript = path.clone();
        let herdr = fake::Server::start(move |method, _| match method {
            "agent.list" => json!({"result":{"agents":[{
                "pane_id":"w1:p1","agent":"pi","agent_status":"working",
                "agent_session":{"kind":"path","value":transcript}
            }]}}),
            "pane.read" => json!({"result":{"read":{"text":"screen","revision":1}}}),
            other => panic!("unexpected method {other}"),
        });
        let state = AppState::new(
            portal,
            0,
            Some(Arc::new(herdr.client())),
            super::super::guard::Token::new(String::new()),
            ledger,
        );
        let fixture = Fixture::at(herdr.client(), state.web.clone());
        let server = Server::start(&state).await;
        let address = AgentAddress::local("w1:p1");
        let mut socket = server.connect().await;
        select(&mut socket, 1, Some(address.clone())).await;
        let for_id = |id| {
            move |frame| match frame {
                ServerFrame::Conversation {
                    id: selected,
                    update,
                } if selected == id => Some(*update),
                _ => None,
            }
        };
        let ConversationUpdate::Reset {
            cursor: initial,
            session,
            turns,
            truncated,
        } = next(&mut socket, for_id(1)).await
        else {
            panic!("expected a conversation reset");
        };
        assert_ne!(session, "");
        assert!(!truncated);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].id, "one");
        assert_eq!(turns[0].text, "first turn");

        writeln!(
            std::fs::OpenOptions::new().append(true).open(path).unwrap(),
            "{}",
            line("two", "second turn")
        )
        .unwrap();
        assert!(fixture.tick("w1:p1"));
        let ConversationUpdate::Splice {
            base,
            cursor,
            drop_front,
            keep,
            turns: appended,
            truncated,
        } = next(&mut socket, for_id(1)).await
        else {
            panic!("expected a conversation splice");
        };
        assert_eq!(base, initial);
        assert_eq!(cursor.token, base.token);
        assert!(cursor.revision > base.revision);
        assert_eq!((drop_front, keep), (0, turns.len()));
        assert!(!truncated);
        assert_eq!(appended.len(), 1);
        assert_eq!(appended[0].id, "two");
        assert_eq!(appended[0].text, "second turn");

        let mut resumed = server.connect().await;
        open(&mut resumed, 2, Some(address), Some(cursor.clone())).await;
        assert_eq!(
            next(&mut resumed, for_id(2)).await,
            ConversationUpdate::Confirm { cursor }
        );
        state.stop.cancel();
    }

    #[tokio::test]
    async fn live_socket_rejects_cross_origin_upgrades() {
        let (_, portal) = watch::channel(PortalState::default());
        let state = AppState::new(
            portal,
            0,
            None,
            super::super::guard::Token::new(String::new()),
            crate::ledger::Ledger::temporary(),
        );
        let server = Server::start(&state).await;
        let Err(tungstenite::Error::Http(response)) =
            connect(server.port, "https://other.example").await
        else {
            panic!("expected a refused upgrade");
        };
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let mut socket = server.connect().await;
        assert!(matches!(
            read(&mut socket).await,
            Some(ServerFrame::Portal { .. } | ServerFrame::Inbox { .. })
        ));
    }
}
