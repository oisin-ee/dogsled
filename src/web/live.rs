//! One browser socket multiplexes portal publications and the selected pane's interests.

use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use portal_types::agents::{ActivityView, AgentAddress};
use portal_types::live::{ClientFrame, ServerFrame};
use portal_types::{PortalAvailability, PortalEvent, PortalStatus};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;

use super::agents::{PeerOperation, agent_event};
use super::{AppState, PortalState};
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
                        let Ok(ClientFrame::Open { id, address }) = serde_json::from_str(&text) else {
                            let _ = socket.send(Message::Close(None)).await;
                            return;
                        };
                        // Dropping the previous selection releases both interests and queued frames.
                        selected = None;
                        if let Some(address) = address {
                            if !valid(&address) {
                                let _ = socket.send(Message::Close(None)).await;
                                return;
                            }
                            selected = Some(Selection::new(state, id, &address));
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
    fn new(state: &AppState, id: u64, address: &AgentAddress) -> Self {
        let (sender, frames) = mpsc::channel(2);
        let mut interests = JoinSet::new();
        interests.spawn(follow(
            state.clone(),
            address.clone(),
            sender.clone(),
            PeerOperation::Event,
            |client, pane| client.detail(pane).map_err(|error| error.to_string()),
            agent_event,
            move |result| match result {
                Ok(event) => ServerFrame::Agent {
                    id,
                    event: Box::new(event),
                },
                Err(error) => ServerFrame::AgentUnavailable { id, error },
            },
        ));
        interests.spawn(follow(
            state.clone(),
            address.clone(),
            sender,
            PeerOperation::Activity,
            |client, pane| client.activity_subscribe(pane).map_err(|error| error.error),
            |view: &ActivityView, _| Ok(view.clone()),
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

async fn follow<T, W>(
    state: AppState,
    address: AgentAddress,
    sender: mpsc::Sender<ServerFrame>,
    operation: PeerOperation,
    open: fn(ipc::Client, &str) -> Update<ipc::Subscription<W>>,
    project: fn(&W, &str) -> crate::error::Result<T>,
    frame: impl Fn(Update<T>) -> ServerFrame + Send + Sync,
) where
    T: serde::de::DeserializeOwned + PartialEq + Clone + Send + Sync + 'static,
    W: ipc::Wire + Send + Sync + 'static,
{
    let mut last_error = None;
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
            peer::<T>(
                &state,
                &address,
                &operation,
                &sender,
                &frame,
                &mut last_error,
            )
            .await
        };
        if last_error.as_ref() != Some(&error) {
            last_error = Some(error.clone());
            if sender.send(frame(Err(error))).await.is_err() {
                return;
            }
        }
        // Retry only the failed interest. The portal and the other interest keep flowing.
        tokio::time::sleep(super::RETRY).await;
    }
}

fn local<T, W>(
    state: &AppState,
    pane: String,
    open: fn(ipc::Client, &str) -> Update<ipc::Subscription<W>>,
    project: fn(&W, &str) -> crate::error::Result<T>,
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

async fn peer<T: serde::de::DeserializeOwned>(
    state: &AppState,
    address: &AgentAddress,
    operation: &PeerOperation,
    sender: &mpsc::Sender<ServerFrame>,
    frame: &impl Fn(Update<T>) -> ServerFrame,
    last_error: &mut Option<String>,
) -> String {
    let unavailable = || format!("host {}: agent stream unavailable", address.host);
    let Ok(mut lines) =
        state
            .followers
            .subscribe(state, address.host.clone(), address.pane.clone(), operation)
    else {
        return unavailable();
    };
    let Ok(Some(mut line)) =
        tokio::time::timeout(crate::budget::CALL_LIMIT, current(&mut lines)).await
    else {
        return unavailable();
    };
    loop {
        match line {
            Ok(reply) if reply.status == 200 => {
                let Ok(value) = serde_json::from_value(reply.body) else {
                    return unavailable();
                };
                *last_error = None;
                if sender.send(frame(Ok(value))).await.is_err() {
                    return unavailable();
                }
            }
            Ok(reply) if reply.status < 500 => {
                return reply.body["error"]
                    .as_str()
                    .map_or_else(unavailable, |error| {
                        format!("host {}: {error}", address.host)
                    });
            }
            Ok(_) => return unavailable(),
            Err(error) => return error,
        }
        if lines.changed().await.is_err() {
            return unavailable();
        }
        let Some(next) = lines.borrow_and_update().clone() else {
            return unavailable();
        };
        line = next;
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
        use futures_util::SinkExt as _;
        let text = serde_json::to_string(&ClientFrame::Open { id, address }).unwrap();
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
