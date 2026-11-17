//! Pi's producer API: the unqualified paths a Pi session uses to publish its live activity
//! and model settings, and to receive settings changes.
//!
//! `dogsled serve` answers these paths on `producer.sock` in the state directory on every
//! host, so a Pi session reaches its own host's serve whether or not that host runs the
//! portal. The socket has mode 0600, so only the owning user can connect, and it needs no
//! bearer token. The portal (`dogsled web`) keeps the same paths behind its bearer token for
//! Pi sessions started by an earlier release.
//!
//! These paths must stay stable across binary upgrades while Pi sessions keep running.

use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
use std::path::{Path as FsPath, PathBuf};

use axum::extract::{Path, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use portal_types::agents::{ActivityCode, ProducerEvidence, PublishActivity, RegisterProducer};
use tokio_stream::StreamExt as _;
use tokio_util::sync::CancellationToken;

use super::guard::{self, Token};
use super::{activity, settings as routes};
use crate::error::{Error, Result};
use crate::observation::{
    ipc,
    live_activity::{self, Command},
    settings::{self, Applied, Failure, Publication},
};

/// The socket name in the state directory.
pub const SOCKET: &str = "producer.sock";

/// Owner actions carried through the existing peer and observation IPC transports.
/// No producer waits are forwarded: their channels stay in the owning serve process.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "action",
    content = "body",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum Interactive {
    Questions,
    Reviews,
    Answer {
        id: String,
        body: portal_types::questions::AnswerBody,
    },
    Reject {
        id: String,
        body: portal_types::questions::Rejection,
    },
    Show {
        id: String,
    },
    Diff {
        id: String,
        query: Option<String>,
    },
    Annotations {
        id: String,
        body: portal_types::reviews::AnnotationsBody,
    },
    General {
        id: String,
        body: portal_types::reviews::GeneralBody,
    },
    Approve {
        id: String,
        body: portal_types::reviews::ApproveBody,
    },
    RequestChanges {
        id: String,
    },
    Upload {
        bytes: Vec<u8>,
    },
    Attachment {
        name: String,
    },
}

/// Maximum JSON expansion of a 10 MiB image, including the command envelope.
pub(crate) const INTERACTIVE_LIMIT: usize = 40 * 1024 * 1024 + 64 * 1024;

/// One lifetime owner for producer HTTP and owner IPC, including long-poll waiters.
pub(crate) fn interactive_state(dir: &FsPath, stop: CancellationToken) -> super::AppState {
    let mut state = super::AppState::new(
        tokio::sync::watch::channel(super::PortalState::default()).1,
        0,
        None,
        Token::new(String::new()),
        crate::ledger::Ledger::new(dir.to_owned()),
    );
    state.private_producer = true;
    state.stop = stop;
    state
}

/// Execute only inside serve, so answers and decisions wake its producers.
pub(crate) async fn interactive(
    state: super::AppState,
    command: Interactive,
) -> crate::error::Result<super::agents::PeerReply> {
    use super::{questions, reviews};
    let headers = axum::http::HeaderMap::new();
    let binary = matches!(command, Interactive::Attachment { .. });
    let response = match command {
        Interactive::Questions => questions::list(State(state)).await,
        Interactive::Reviews => reviews::list(State(state)).await,
        Interactive::Answer { id, body } => {
            questions::answer(State(state), Path(id), headers, Json(body)).await
        }
        Interactive::Reject { id, body } => {
            questions::reject(State(state), Path(id), headers, Json(body)).await
        }
        Interactive::Show { id } => reviews::show(State(state), Path(id)).await,
        Interactive::Diff { id, query } => {
            reviews::diff(State(state), Path(id), axum::extract::RawQuery(query)).await
        }
        Interactive::Annotations { id, body } => {
            reviews::annotations(State(state), Path(id), Json(body)).await
        }
        Interactive::General { id, body } => {
            reviews::general(State(state), Path(id), Json(body)).await
        }
        Interactive::Approve { id, body } => {
            reviews::approve(State(state), Path(id), Some(Json(body))).await
        }
        Interactive::RequestChanges { id } => {
            reviews::request_changes(State(state), Path(id)).await
        }
        Interactive::Upload { bytes } => {
            reviews::attachment(State(state), axum::body::Body::from(bytes)).await
        }
        Interactive::Attachment { name } => {
            reviews::attachment_file(State(state), Path(name)).await
        }
    };
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), INTERACTIVE_LIMIT)
        .await
        .map_err(|error| Error::Invalid(error.to_string()))?;
    let body = if binary && status == 200 {
        serde_json::json!({"bytes": bytes.to_vec()})
    } else {
        serde_json::from_slice(&bytes)?
    };
    Ok(super::agents::PeerReply { status, body })
}

/// What the producer routes need: the state directory whose serve owns the state, a stop
/// signal for streams, and the bearer token when the portal answers the routes.
#[derive(Debug, Clone)]
pub struct Producer {
    dir: PathBuf,
    stop: CancellationToken,
    bearer: Option<Token>,
}

impl Producer {
    /// The producer routes as `serve` answers them on its private socket.
    #[must_use]
    pub fn serve(dir: &FsPath, stop: CancellationToken) -> Self {
        Self {
            dir: dir.to_owned(),
            stop,
            bearer: None,
        }
    }

    /// The producer routes as the portal answers them, behind its bearer token.
    #[must_use]
    pub(super) fn portal(dir: &FsPath, stop: CancellationToken, token: Token) -> Self {
        Self {
            dir: dir.to_owned(),
            stop,
            bearer: Some(token),
        }
    }
}

pub fn router() -> Router<Producer> {
    Router::new()
        .route(
            "/api/agents/{pane}/activity",
            get(activity_current).put(activity_publish),
        )
        .route(
            "/api/agents/{pane}/activity/producer",
            post(activity_register),
        )
        .route("/api/agents/{pane}/settings", put(settings_publish))
        .route(
            "/api/agents/{pane}/settings/requests",
            get(settings_requests),
        )
        .route(
            "/api/agents/{pane}/settings/requests/{id}/result",
            post(settings_result),
        )
        .layer(axum::extract::DefaultBodyLimit::max(settings::MAX_BODY))
}

/// Host-qualified owner routes. The portal's outer guard checks origin and bearer access.
pub(super) fn owner_routes() -> Router<super::AppState> {
    Router::new()
        .route("/api/hosts/{host}/{*resource}", axum::routing::any(owner))
        .layer(axum::extract::DefaultBodyLimit::max(10 * 1024 * 1024 + 1))
}

async fn owner(
    State(state): State<super::AppState>,
    Path((host, resource)): Path<(String, String)>,
    request: Request,
) -> Response {
    let method = request.method().as_str().to_owned();
    let query = request.uri().query().map(str::to_owned);
    let Ok(bytes) = axum::body::to_bytes(request.into_body(), 10 * 1024 * 1024 + 1).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let command = match owner_command(&method, &resource, query, &bytes) {
        Ok(Some(command)) => command,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": error.to_string()})),
            )
                .into_response();
        }
    };
    let attachment = match &command {
        Interactive::Attachment { name } => Some(name.clone()),
        _ => None,
    };
    let local = host == crate::hosts::LOCAL;
    let question = match &command {
        Interactive::Answer { id, .. } | Interactive::Reject { id, .. } => Some(id.clone()),
        _ => None,
    };
    let decision = matches!(
        command,
        Interactive::Approve { .. } | Interactive::RequestChanges { .. }
    );
    let result = owner_execute(&state, host, command).await;
    if local
        && !state.standalone_interactive
        && let Ok(reply) = &result
        && reply.status == 200
    {
        // Older Pi sessions still wait in this portal process. Wake those as well as
        // serve's producers after serve has durably committed the owner action.
        if let Some(id) = question {
            state.questions.wake(&id);
        }
        if decision {
            super::reviews::notify(&state.web, &reply.body);
        }
    }
    match result {
        Ok(reply) => owner_reply(reply, attachment.as_deref()),
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error":error.to_string()})),
        )
            .into_response(),
    }
}

async fn owner_execute(
    state: &super::AppState,
    host: String,
    command: Interactive,
) -> Result<super::agents::PeerReply> {
    if host == crate::hosts::LOCAL && state.standalone_interactive {
        let mut state = state.clone();
        state.private_producer = true;
        return interactive(state, command).await;
    }
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        if host == crate::hosts::LOCAL {
            ipc::Client::at(&state.web.dir).interactive(command)
        } else {
            super::agents::peer_call(
                &state,
                &host,
                "",
                &super::agents::PeerOperation::Interactive(command),
            )
        }
    })
    .await
    .map_err(|error| Error::Invalid(error.to_string()))?
}

fn owner_command(
    method: &str,
    resource: &str,
    query: Option<String>,
    bytes: &[u8],
) -> serde_json::Result<Option<Interactive>> {
    let parts: Vec<_> = resource.split('/').collect();
    Ok(Some(match (method, parts.as_slice()) {
        ("GET", ["questions"]) => Interactive::Questions,
        ("GET", ["reviews"]) => Interactive::Reviews,
        ("POST", ["questions", id, "answer"]) => Interactive::Answer {
            id: (*id).into(),
            body: serde_json::from_slice(bytes)?,
        },
        ("POST", ["questions", id, "reject"]) => Interactive::Reject {
            id: (*id).into(),
            body: serde_json::from_slice(bytes)?,
        },
        ("GET", ["reviews", id]) => Interactive::Show { id: (*id).into() },
        ("GET", ["reviews", id, "diff"]) => Interactive::Diff {
            id: (*id).into(),
            query,
        },
        ("PUT", ["reviews", id, "annotations"]) => Interactive::Annotations {
            id: (*id).into(),
            body: serde_json::from_slice(bytes)?,
        },
        ("PUT", ["reviews", id, "general"]) => Interactive::General {
            id: (*id).into(),
            body: serde_json::from_slice(bytes)?,
        },
        ("POST", ["reviews", id, "approve"]) => Interactive::Approve {
            id: (*id).into(),
            body: serde_json::from_slice(bytes)?,
        },
        ("POST", ["reviews", id, "request-changes"]) => {
            Interactive::RequestChanges { id: (*id).into() }
        }
        ("POST", ["attachments"]) => Interactive::Upload {
            bytes: bytes.to_vec(),
        },
        ("GET", ["attachments", name]) => Interactive::Attachment {
            name: (*name).into(),
        },
        _ => return Ok(None),
    }))
}

fn owner_reply(reply: super::agents::PeerReply, attachment: Option<&str>) -> Response {
    let status = StatusCode::from_u16(reply.status).unwrap_or(StatusCode::BAD_GATEWAY);
    if let Some(name) = attachment.filter(|_| status == StatusCode::OK) {
        match serde_json::from_value::<Vec<u8>>(reply.body["bytes"].clone()) {
            Ok(bytes) => {
                return (
                    [(header::CONTENT_TYPE, super::reviews::image_type(name))],
                    bytes,
                )
                    .into_response();
            }
            Err(error) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({"error":error.to_string()})),
                )
                    .into_response();
            }
        }
    }
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        Json(reply.body),
    )
        .into_response()
}

/// Bind `producer.sock` in `dir`, replacing a stale socket but never another kind of file.
pub fn bind(dir: &FsPath) -> Result<(PathBuf, std::os::unix::net::UnixListener)> {
    let path = crate::observation::socket_path(dir, SOCKET)?;
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_socket() => std::fs::remove_file(&path)?,
        Ok(_) => return Err(Error::Invalid("producer endpoint is not a socket".into())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    let listener = std::os::unix::net::UnixListener::bind(&path)?;
    let ready = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .and_then(|()| listener.set_nonblocking(true));
    if let Err(error) = ready {
        let _ = std::fs::remove_file(&path);
        return Err(error.into());
    }
    Ok((path, listener))
}

/// The routes serve answers: the unqualified paths, and the activity paths that Pi
/// qualifies with the `local` host.
fn serve_router() -> Router<Producer> {
    router()
        .route(
            "/api/agents/local/{pane}/activity",
            get(activity_current).put(activity_publish),
        )
        .route(
            "/api/agents/local/{pane}/activity/producer",
            post(activity_register),
        )
}

/// Answer the producer routes on `listener` until `producer.stop` is cancelled.
pub async fn run(listener: tokio::net::UnixListener, producer: Producer, state: super::AppState) {
    let stop = producer.stop.clone();
    let app = serve_router()
        .with_state(producer)
        .merge(super::questions::routes().with_state(state.clone()))
        .merge(super::reviews::routes().with_state(state));
    if let Err(error) = axum::serve(listener, app)
        .with_graceful_shutdown(stop.cancelled_owned())
        .await
    {
        eprintln!("dogsled producer endpoint stopped: {error}");
    }
}

fn unauthorized(producer: &Producer, headers: &axum::http::HeaderMap) -> bool {
    producer
        .bearer
        .as_ref()
        .is_some_and(|token| !guard::bearer_allowed(headers, token))
}

/// The current activity of a pane on this host.
pub(super) async fn current_activity(dir: PathBuf, pane: String) -> Response {
    if let Err(error) = live_activity::validate_pane(&pane) {
        return activity::failure(error);
    }
    match tokio::task::spawn_blocking(move || ipc::Client::at(&dir).activity(&pane)).await {
        Ok(Ok(view)) => ([(header::CACHE_CONTROL, "no-store")], Json(view)).into_response(),
        Ok(Err(error)) => activity::failure(error),
        Err(error) => activity::failure(live_activity::error(
            ActivityCode::UpstreamUnavailable,
            &error.to_string(),
        )),
    }
}

/// Read one bounded activity command from a request body.
pub(super) async fn activity_body(
    request: Request,
    decode: fn(&[u8]) -> serde_json::Result<Command>,
) -> std::result::Result<Command, Box<Response>> {
    let mut stream = request.into_body().into_data_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            return Err(Box::new(activity::failure(live_activity::error(
                ActivityCode::InvalidRequest,
                "invalid activity body",
            ))));
        };
        if chunk.len() > live_activity::MAX_BODY.saturating_sub(bytes.len()) {
            return Err(Box::new(
                (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    [(header::CACHE_CONTROL, "no-store")],
                    Json(live_activity::error(
                        ActivityCode::InvalidRequest,
                        "activity body exceeds 64 KiB",
                    )),
                )
                    .into_response(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    decode(&bytes).map_err(|error| {
        Box::new(activity::failure(live_activity::error(
            ActivityCode::InvalidRequest,
            &error.to_string(),
        )))
    })
}

/// Apply one activity command on this host's serve.
pub(super) async fn activity_command(dir: PathBuf, pane: String, command: Command) -> Response {
    match tokio::task::spawn_blocking(move || {
        ipc::Client::at(&dir).activity_command(&pane, command)
    })
    .await
    {
        Ok(Ok(ack)) => ([(header::CACHE_CONTROL, "no-store")], Json(ack)).into_response(),
        Ok(Err(error)) => activity::failure(error),
        Err(error) => activity::failure(live_activity::error(
            ActivityCode::UpstreamUnavailable,
            &error.to_string(),
        )),
    }
}

/// Run one settings command on this host's serve.
pub(super) async fn settings_command(
    dir: PathBuf,
    pane: String,
    command: settings::Command,
) -> Response {
    match tokio::task::spawn_blocking(move || ipc::Client::at(&dir).settings(&pane, command)).await
    {
        Ok(Ok(settings)) => ([(header::CACHE_CONTROL, "no-store")], Json(settings)).into_response(),
        Ok(Err(error)) => routes::failure(error),
        Err(error) => routes::failure(Failure::new("settings_unavailable", &error.to_string())),
    }
}

async fn activity_current(State(producer): State<Producer>, Path(pane): Path<String>) -> Response {
    current_activity(producer.dir, pane).await
}

async fn activity_register(
    State(producer): State<Producer>,
    Path(pane): Path<String>,
    request: Request,
) -> Response {
    activity_mutation(producer, pane, request, |bytes| {
        serde_json::from_slice::<RegisterProducer>(bytes).map(Command::Register)
    })
    .await
}

async fn activity_publish(
    State(producer): State<Producer>,
    Path(pane): Path<String>,
    request: Request,
) -> Response {
    activity_mutation(producer, pane, request, |bytes| {
        serde_json::from_slice::<PublishActivity>(bytes).map(Command::Publish)
    })
    .await
}

async fn activity_mutation(
    producer: Producer,
    pane: String,
    request: Request,
    decode: fn(&[u8]) -> serde_json::Result<Command>,
) -> Response {
    if unauthorized(&producer, request.headers()) {
        return activity::failure(live_activity::error(
            ActivityCode::Unauthorized,
            "activity mutations require bearer authentication",
        ));
    }
    if let Err(error) = live_activity::validate_pane(&pane) {
        return activity::failure(error);
    }
    match activity_body(request, decode).await {
        Ok(command) => activity_command(producer.dir, pane, command).await,
        Err(response) => *response,
    }
}

fn settings_refusal() -> Response {
    routes::failure(Failure::new(
        "unauthorized",
        "Pi settings routes require bearer authentication",
    ))
}

async fn settings_publish(
    State(producer): State<Producer>,
    Path(pane): Path<String>,
    headers: axum::http::HeaderMap,
    body: std::result::Result<Json<Publication>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if unauthorized(&producer, &headers) {
        return settings_refusal();
    }
    match body {
        Ok(Json(body)) => {
            settings_command(producer.dir, pane, settings::Command::Publish(body)).await
        }
        Err(error) => routes::failure(Failure::new("invalid_settings", &error.body_text())),
    }
}

async fn settings_result(
    State(producer): State<Producer>,
    Path((pane, id)): Path<(String, String)>,
    headers: axum::http::HeaderMap,
    body: std::result::Result<Json<Applied>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if unauthorized(&producer, &headers) {
        return settings_refusal();
    }
    match body {
        Ok(Json(result)) => {
            settings_command(producer.dir, pane, settings::Command::Result { id, result }).await
        }
        Err(error) => routes::failure(Failure::new("invalid_settings", &error.body_text())),
    }
}

async fn settings_requests(
    State(producer): State<Producer>,
    Path(pane): Path<String>,
    headers: axum::http::HeaderMap,
    Query(evidence): Query<ProducerEvidence>,
) -> Response {
    if unauthorized(&producer, &headers) {
        return settings_refusal();
    }
    let client = ipc::Client::at(&producer.dir);
    let mut subscription =
        match tokio::task::spawn_blocking(move || client.settings_subscribe(&pane, evidence)).await
        {
            Ok(Ok(subscription)) => subscription,
            Ok(Err(error)) => return routes::failure(error),
            Err(error) => {
                return routes::failure(Failure::new("settings_unavailable", &error.to_string()));
            }
        };
    let (sender, receiver) = tokio::sync::mpsc::channel(64);
    let stop = producer.stop.clone();
    tokio::task::spawn_blocking(move || {
        while !stop.is_cancelled() && !sender.is_closed() {
            match subscription.next() {
                Ok(Some(request)) => {
                    if sender.blocking_send(Some(request)).is_err() {
                        break;
                    }
                }
                Ok(None) => (),
                Err(_) => {
                    let _ = sender.blocking_send(None);
                    break;
                }
            }
        }
    });
    let stream = tokio_stream::wrappers::ReceiverStream::new(receiver).map(|request| {
        let event = match request {
            Some(request) => axum::response::sse::Event::default()
                .event("settings")
                .json_data(request),
            None => axum::response::sse::Event::default()
                .event("error")
                .json_data(Failure::unavailable()),
        };
        Ok::<_, std::convert::Infallible>(event.unwrap_or_else(|_| {
            axum::response::sse::Event::default()
                .event("error")
                .data("invalid settings request")
        }))
    });
    let stream = futures_util::StreamExt::take_until(stream, producer.stop.cancelled_owned());
    let mut response = axum::response::sse::Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::new())
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::settings::tests::{Fixture, selection};
    use std::io::{BufRead as _, Read as _, Write as _};

    /// One HTTP/1.1 exchange over the producer socket, as a Pi session makes it.
    fn exchange(dir: &FsPath, method: &str, path: &str, body: &str) -> (u16, String) {
        exchange_bytes(dir, method, path, body.as_bytes())
    }

    fn exchange_bytes(dir: &FsPath, method: &str, path: &str, body: &[u8]) -> (u16, String) {
        let socket = crate::observation::socket_path(dir, SOCKET).unwrap();
        let mut stream = std::os::unix::net::UnixStream::connect(socket).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        let status = reply[9..12].parse().unwrap();
        let body = reply
            .split_once("\r\n\r\n")
            .map(|(_, body)| body.to_owned())
            .unwrap_or_default();
        (status, body)
    }

    struct InteractiveHost {
        _server: ipc::Server,
        _publisher: crate::observation::Publisher,
        ledger: crate::ledger::Ledger,
    }

    impl InteractiveHost {
        fn new() -> Self {
            let ledger = crate::ledger::Ledger::temporary();
            let publisher = crate::observation::Publisher::new().unwrap();
            let mut server = ipc::Server::new(&ledger.dir).unwrap();
            server.bind(publisher.subscribe()).unwrap();
            Self {
                _server: server,
                _publisher: publisher,
                ledger,
            }
        }
        fn command(&self, command: Interactive) -> super::super::agents::PeerReply {
            ipc::Client::at(&self.ledger.dir)
                .interactive(command)
                .unwrap()
        }
        fn json(&self, method: &str, path: &str, body: &serde_json::Value) -> serde_json::Value {
            let (status, reply) = exchange(&self.ledger.dir, method, path, &body.to_string());
            assert_eq!(status, 200, "{reply}");
            serde_json::from_str(&reply).unwrap()
        }
    }

    fn portal_exchange(
        port: u16,
        method: &str,
        path: &str,
        body: &str,
        origin: &str,
    ) -> (u16, String) {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        write!(stream, "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: {origin}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        (
            reply[9..12].parse().unwrap(),
            reply.split_once("\r\n\r\n").unwrap().1.to_owned(),
        )
    }

    #[tokio::test]
    async fn guarded_portal_owner_routes_forward_to_serve_and_keep_legacy_bearer_checks() {
        let host = InteractiveHost::new();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut publication = super::super::test_support::empty(&host.ledger);
        publication
            .snapshot
            .as_mut()
            .unwrap()
            .board
            .as_mut()
            .unwrap()
            .hosts
            .push(portal_types::HostView {
                label: "offline-host".into(),
                service: "unreachable".into(),
                reachable: false,
                busy: None,
                cores: 0,
                eligible: false,
                reason: Some("connection refused".into()),
                retry_ms: None,
            });
        let state = super::super::AppState::new(
            tokio::sync::watch::channel(publication).1,
            port,
            None,
            Token::new("secret".into()),
            host.ledger.clone(),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, super::super::router(state))
                .await
                .unwrap();
        });
        let dir = host.ledger.dir.clone();
        tokio::task::spawn_blocking(move || {
            let origin = format!("http://127.0.0.1:{port}");
            let (status, inbox) = portal_exchange(port, "GET", "/api/inbox", "", &origin);
            assert_eq!(status, 200);
            let inbox: portal_types::inbox::InboxView = serde_json::from_str(&inbox).unwrap();
            assert_eq!(inbox.gaps[0].host, "offline-host");
            assert_eq!(inbox.gaps[0].error, "connection refused");
            let body = serde_json::json!({"questions":{"questions":[]}}).to_string();
            let (status, _) = portal_exchange(port, "POST", "/api/questions", &body, &origin);
            assert_eq!(
                status, 403,
                "legacy producer creation still requires its token"
            );
            let (status, body) = exchange(&dir, "POST", "/api/questions", &body);
            assert_eq!(status, 200);
            let id = serde_json::from_str::<serde_json::Value>(&body).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned();
            let (status, body) =
                portal_exchange(port, "GET", "/api/hosts/local/questions", "", &origin);
            assert_eq!(status, 200);
            assert!(body.contains(&id));
            let path = format!("/api/hosts/local/questions/{id}/answer");
            assert_eq!(
                portal_exchange(
                    port,
                    "POST",
                    &path,
                    "{\"responses\":[]}",
                    "https://foreign.invalid"
                )
                .0,
                403
            );
            let wait_path = format!("/api/questions/{id}/answer");
            let poll_dir = dir.clone();
            let waiting = std::thread::spawn(move || exchange(&poll_dir, "GET", &wait_path, ""));
            assert_eq!(
                portal_exchange(port, "POST", &path, "{\"responses\":[]}", &origin).0,
                200
            );
            assert_eq!(waiting.join().unwrap().0, 200);
        })
        .await
        .unwrap();
        server.abort();
        let _ = server.await;
    }

    #[test]
    fn peer_relay_mutates_only_the_owning_serve_even_when_panes_collide() {
        let local = InteractiveHost::new();
        let peer = InteractiveHost::new();
        let ask = serde_json::json!({"pane":"same-pane","session":"same-session","questions":{"questions":[]}});
        let local_id = local.json("POST", "/api/questions", &ask)["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let peer_id = peer.json("POST", "/api/questions", &ask)["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let operation = super::super::agents::PeerOperation::Interactive(Interactive::Answer {
            id: peer_id.clone(),
            body: portal_types::questions::AnswerBody {
                responses: Vec::new(),
            },
        });
        let reply = super::super::agents::peer_command(
            String::new(),
            &serde_json::to_string(&operation).unwrap(),
            peer.ledger.clone(),
        )
        .unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(
            exchange(
                &peer.ledger.dir,
                "GET",
                &format!("/api/questions/{peer_id}/answer?wait=0"),
                ""
            )
            .0,
            200
        );
        assert_eq!(
            exchange(
                &local.ledger.dir,
                "GET",
                &format!("/api/questions/{local_id}/answer?wait=0"),
                ""
            )
            .0,
            204
        );
    }

    #[test]
    fn serve_owns_questions_and_ipc_resolves_long_polls_and_withdrawals() {
        let host = InteractiveHost::new();
        let ask = serde_json::json!({"pane":"pane","session":"session","questions":{"questions":[{"id":"q","type":"text","question":"Which?"}]}});
        for rejected in [false, true] {
            let created = host.json("POST", "/api/questions", &ask);
            let id = created["id"].as_str().unwrap().to_owned();
            let pending = host.json("GET", "/api/questions", &serde_json::Value::Null);
            assert_eq!(pending["pending"][0]["id"], id);
            let dir = host.ledger.dir.clone();
            let path = format!("/api/questions/{id}/answer");
            let poll = std::thread::spawn(move || exchange(&dir, "GET", &path, ""));
            let command = if rejected {
                Interactive::Reject {
                    id: id.clone(),
                    body: portal_types::questions::Rejection {
                        reason: "Use the other plan".into(),
                    },
                }
            } else {
                Interactive::Answer {
                    id: id.clone(),
                    body: portal_types::questions::AnswerBody {
                        responses: vec![portal_types::questions::ResponseItem {
                            id: "q".into(),
                            value: serde_json::json!("B"),
                            attachments: None,
                        }],
                    },
                }
            };
            assert_eq!(host.command(command.clone()).status, 200);
            let (status, reply) = poll.join().unwrap();
            assert_eq!(status, 200, "{reply}");
            assert!(reply.contains(if rejected { "Use the other plan" } else { "B" }));
            assert_eq!(host.command(command).status, 409);
            assert_eq!(
                exchange(
                    &host.ledger.dir,
                    "DELETE",
                    &format!("/api/questions/{id}"),
                    ""
                )
                .0,
                409
            );
        }
        let id = host.json("POST", "/api/questions", &ask)["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            exchange(
                &host.ledger.dir,
                "GET",
                &format!("/api/questions/{id}/answer?wait=0"),
                ""
            )
            .0,
            204
        );
        assert_eq!(
            exchange(
                &host.ledger.dir,
                "DELETE",
                &format!("/api/questions/{id}"),
                ""
            )
            .0,
            200
        );
        assert_eq!(
            exchange(
                &host.ledger.dir,
                "GET",
                &format!("/api/questions/{id}/answer?wait=0"),
                ""
            )
            .0,
            410
        );
    }

    #[test]
    fn serve_owns_review_edits_decisions_replay_ack_and_attachments() {
        let host = InteractiveHost::new();
        let submit = serde_json::json!({"pane":"pane","session":"session","cwd":"/repo","path":"plans/one.md","content":"# Plan\\nFirst\\n"});
        for approve in [false, true] {
            let created = host.json("POST", "/api/reviews", &submit);
            let id = created["id"].as_str().unwrap().to_owned();
            let mut revised = submit.clone();
            revised["content"] = serde_json::json!("# Plan\\nSecond\\n");
            assert_eq!(host.json("POST", "/api/reviews", &revised)["revision"], 2);
            assert_eq!(host.command(Interactive::Reviews).status, 200);
            assert_eq!(
                host.command(Interactive::Show { id: id.clone() }).body["revisions"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
            assert_eq!(
                host.command(Interactive::Diff {
                    id: id.clone(),
                    query: Some("from=1&to=2".into())
                })
                .status,
                200
            );
            assert_eq!(
                host.command(Interactive::Annotations {
                    id: id.clone(),
                    body: portal_types::reviews::AnnotationsBody {
                        annotations: Vec::new()
                    }
                })
                .status,
                200
            );
            assert_eq!(
                host.command(Interactive::General {
                    id: id.clone(),
                    body: portal_types::reviews::GeneralBody {
                        general: "Please simplify".into()
                    }
                })
                .status,
                200
            );
            let socket = crate::observation::socket_path(&host.ledger.dir, SOCKET).unwrap();
            let mut stream = std::os::unix::net::UnixStream::connect(socket).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            write!(
                stream,
                "GET /api/sessions/session/decisions HTTP/1.1\r\nHost: localhost\r\n\r\n"
            )
            .unwrap();
            let mut reader = std::io::BufReader::new(stream);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 0 {
                if line == "\r\n" {
                    break;
                }
                line.clear();
            }
            let command = if approve {
                Interactive::Approve {
                    id: id.clone(),
                    body: portal_types::reviews::ApproveBody::default(),
                }
            } else {
                Interactive::RequestChanges { id: id.clone() }
            };
            assert_eq!(host.command(command).status, 200);
            loop {
                line.clear();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line.starts_with("data:") {
                    break;
                }
            }
            assert!(line.contains(&id), "{line}");
            assert!(line.contains(if approve {
                "approved"
            } else {
                "changes_requested"
            }));
            drop(reader);
            assert_eq!(
                host.json(
                    "POST",
                    &format!("/api/decisions/{id}/ack"),
                    &serde_json::Value::Null
                )["delivered"],
                true
            );
        }
    }

    #[test]
    fn serve_uploads_and_fetches_host_owned_attachments() {
        let host = InteractiveHost::new();
        let png = vec![137, 80, 78, 71, 13, 10, 26, 10];
        let (status, uploaded) = exchange_bytes(&host.ledger.dir, "POST", "/api/attachments", &png);
        assert_eq!(status, 200, "{uploaded}");
        let uploaded: serde_json::Value = serde_json::from_str(&uploaded).unwrap();
        let name = uploaded["path"]
            .as_str()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap()
            .to_owned();
        assert_eq!(
            host.command(Interactive::Attachment { name }).body["bytes"],
            serde_json::json!(png)
        );
        assert_eq!(
            host.command(Interactive::Attachment { name: "..".into() })
                .status,
            404
        );
    }

    #[test]
    fn serve_answers_pi_settings_on_its_private_socket_without_the_portal() {
        let fixture = Fixture::new();
        let socket = crate::observation::socket_path(&fixture.ledger.dir, SOCKET).unwrap();
        let mode = std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "only the owning user may connect");
        // No portal runs and no token exists: serve alone accepts the publication.
        assert!(!fixture.ledger.dir.join("web").join("token").exists());
        let body = serde_json::to_string(&fixture.publication).unwrap();
        let (status, reply) = exchange(
            &fixture.ledger.dir,
            "PUT",
            "/api/agents/w1:p1/settings",
            &body,
        );
        assert_eq!(status, 200, "{reply}");
        let current = fixture
            .client
            .settings("w1:p1", settings::Command::Current)
            .unwrap();
        assert_eq!(current, selection());
    }

    #[test]
    fn serve_answers_pi_activity_registration_and_reads_on_its_private_socket() {
        let fixture = crate::observation::live_activity::tests::Fixture::new();
        let body = serde_json::to_string(&fixture.registration).unwrap();
        let (status, reply) = exchange(
            &fixture.ledger.dir,
            "POST",
            "/api/agents/local/w1%3Ap1/activity/producer",
            &body,
        );
        assert_eq!(status, 200, "{reply}");
        let (status, reply) = exchange(
            &fixture.ledger.dir,
            "GET",
            "/api/agents/local/w1%3Ap1/activity",
            "",
        );
        assert_eq!(status, 200, "{reply}");
        let view: portal_types::agents::ActivityView = serde_json::from_str(&reply).unwrap();
        assert_eq!(view.pane, "w1:p1");
    }
}
