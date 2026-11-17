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
pub async fn run(listener: tokio::net::UnixListener, producer: Producer) {
    let stop = producer.stop.clone();
    let app = serve_router().with_state(producer);
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
    use std::io::{Read as _, Write as _};

    /// One HTTP/1.1 exchange over the producer socket, as a Pi session makes it.
    fn exchange(dir: &FsPath, method: &str, path: &str, body: &str) -> (u16, String) {
        let socket = crate::observation::socket_path(dir, SOCKET).unwrap();
        let mut stream = std::os::unix::net::UnixStream::connect(socket).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        let status = reply[9..12].parse().unwrap();
        let body = reply
            .split_once("\r\n\r\n")
            .map(|(_, body)| body.to_owned())
            .unwrap_or_default();
        (status, body)
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
