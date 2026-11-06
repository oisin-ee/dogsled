//! Guarded portal API. One observation subscription supplies board, agents and inbox.
//! Requests select projections; they never observe or derive state.

pub mod activity;
pub mod agents;
pub mod assets;
pub mod board;
pub mod guard;
pub mod inbox;
pub mod oneshot;
mod peer_stream;
pub mod questions;
pub mod reviews;
pub mod store;
#[cfg(test)]
mod test_support;
pub mod token;

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use portal_types::{PortalAvailability, PortalEvent, PortalSnapshot, PortalStatus};
use tokio::sync::watch;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::WatchStream;
use tokio_util::sync::CancellationToken;

use crate::config::Policy;
use crate::error::Result;
use crate::ledger::Ledger;

/// Connection retries do not perform observation.
const RETRY: Duration = Duration::from_secs(2);
/// Bound cancellation checks while waiting for a replacement.
const TICK: Duration = Duration::from_millis(100);
const KEEP_ALIVE: Duration = Duration::from_secs(15);

/// A failed connection retains its last publication, but never marks it current.
#[derive(Debug, Clone)]
pub struct PortalState {
    snapshot: Option<PortalSnapshot>,
    error: Option<String>,
}

impl Default for PortalState {
    fn default() -> Self {
        Self {
            snapshot: None,
            error: Some("observation unavailable".to_owned()),
        }
    }
}

/// What the portal needs to start a lead. One-shot servers have none.
#[derive(Debug, Clone)]
pub struct LeadLaunch {
    /// Reloaded for every lead, so a host policy change needs no portal restart.
    pub config_path: std::path::PathBuf,
    pub home: String,
    pub shell: Arc<dyn crate::shell::Shell>,
}

/// Shared publication and explicit command services.
#[derive(Debug, Clone)]
pub struct AppState {
    portal: watch::Receiver<PortalState>,
    port: u16,
    agents: Arc<std::sync::OnceLock<Option<Arc<crate::herdr_socket::Client>>>>,
    token: guard::Token,
    web: Ledger,
    questions: store::Store,
    stop: CancellationToken,
    leads: Option<Arc<LeadLaunch>>,
    followers: peer_stream::Followers,
}

impl AppState {
    /// Construct command services over the host ledger without reading records.
    #[must_use]
    pub fn new(
        portal: watch::Receiver<PortalState>,
        port: u16,
        agents: Option<Arc<crate::herdr_socket::Client>>,
        token: guard::Token,
        web: Ledger,
    ) -> Self {
        let questions = store::Store::new(web.clone());
        let socket = std::sync::OnceLock::from(agents);
        Self {
            portal,
            port,
            agents: Arc::new(socket),
            token,
            web,
            questions,
            stop: CancellationToken::new(),
            leads: None,
            followers: peer_stream::Followers::default(),
        }
    }

    /// Let this server start leads.
    #[must_use]
    pub fn with_leads(mut self, launch: LeadLaunch) -> Self {
        self.leads = Some(Arc::new(launch));
        self
    }

    /// Hold one publication through projection selection and response construction.
    fn projection<T: serde::Serialize>(
        &self,
        select: impl FnOnce(&PortalSnapshot) -> Option<T>,
    ) -> Response {
        let current = self.portal.borrow();
        if let Some(error) = &current.error {
            return observation_error(error);
        }
        let Some(snapshot) = &current.snapshot else {
            return observation_error("observation unavailable");
        };
        let Some(data) = select(snapshot) else {
            return observation_error("observation unavailable: projection was not published");
        };
        (
            [
                ("x-observation-version", snapshot.version.to_string()),
                ("x-observation-epoch", snapshot.epoch.clone()),
                ("x-observation-revision", snapshot.revision.to_string()),
                ("cache-control", "no-store".to_owned()),
            ],
            axum::Json(data),
        )
            .into_response()
    }

    fn until_shutdown<S: tokio_stream::Stream>(
        &self,
        stream: S,
    ) -> impl tokio_stream::Stream<Item = S::Item> + use<S> {
        futures_util::StreamExt::take_until(stream, self.stop.clone().cancelled_owned())
    }
}

fn observation_error(detail: &str) -> Response {
    (
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        [(header::CACHE_CONTROL, "no-store")],
        axum::Json(serde_json::json!({"error": detail})),
    )
        .into_response()
}

/// Serve until the process is asked to stop, independent of command deadlines.
pub fn serve(
    ledger: &Ledger,
    policy: &Policy,
    home: &str,
    port: Option<u16>,
    config_path: &std::path::Path,
) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let leads = LeadLaunch {
        config_path: config_path.to_path_buf(),
        home: home.to_owned(),
        shell: Arc::new(crate::shell::System::default()),
    };
    runtime.block_on(run_until(
        ledger,
        port.unwrap_or(policy.web.port),
        leads,
        shutdown(),
    ))
}

async fn run_until(
    ledger: &Ledger,
    port: u16,
    leads: LeadLaunch,
    stopping: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let (sender, receiver) = watch::channel(PortalState::default());
    let (listener, state) = listen(ledger, port, receiver).await?;
    let state = state.with_leads(leads);
    let port = state.port;
    let _reader = spawn_reader(ledger.clone(), sender, state.stop.clone());
    println!(
        "dogsled web on http://127.0.0.1:{port} (token in {})",
        token::path(ledger).display()
    );
    Ok(serve_until(listener, state, stopping).await?)
}

async fn serve_until(
    listener: tokio::net::TcpListener,
    state: AppState,
    stopping: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let stop = state.stop.clone();
    let _cancel = stop.clone().drop_guard();
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async move {
            stopping.await;
            stop.cancel();
        })
        .await
}

/// Bind before constructing guards from the actual port. Herdr serves only the
/// explicit commands and the detail routes, which migrate separately.
async fn listen(
    ledger: &Ledger,
    port: u16,
    portal: watch::Receiver<PortalState>,
) -> Result<(tokio::net::TcpListener, AppState)> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let port = listener.local_addr()?.port();
    let mut state = AppState::new(
        portal,
        port,
        None,
        guard::Token::new(token::token(ledger)?),
        ledger.clone(),
    );
    state.agents = Arc::new(std::sync::OnceLock::new());
    Ok((listener, state))
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/board", get(board))
        .route("/api/stream", get(stream))
        .merge(agents::router())
        .merge(activity::router())
        .merge(inbox::routes())
        .merge(questions::routes())
        .merge(reviews::routes())
        .fallback(assets::serve)
        .layer(axum::middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

async fn board(State(state): State<AppState>) -> Response {
    state.projection(|snapshot| snapshot.board.clone())
}

/// One event carries all portal projections from the same publication.
async fn stream(
    State(state): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = std::result::Result<Event, Infallible>>> {
    let stream = WatchStream::new(state.portal.clone()).map(|current| {
        let availability = if current.snapshot.is_some() {
            PortalAvailability::Disconnected
        } else {
            PortalAvailability::Unavailable
        };
        let frame = match (current.snapshot, current.error) {
            (_, Some(error)) => PortalEvent::Status(PortalStatus {
                observation: availability,
                error,
            }),
            (Some(snapshot), None) => PortalEvent::Snapshot(Box::new(snapshot)),
            (None, None) => PortalEvent::Status(PortalStatus {
                observation: PortalAvailability::Unavailable,
                error: "observation unavailable".to_owned(),
            }),
        };
        let event = Event::default()
            .event("portal")
            .json_data(frame)
            .unwrap_or_else(|error| {
                Event::default().event("portal").data(
                    serde_json::json!({
                        "observation": "disconnected", "error": error.to_string(),
                    })
                    .to_string(),
                )
            });
        Ok(event)
    });
    Sse::new(state.until_shutdown(stream)).keep_alive(KeepAlive::new().interval(KEEP_ALIVE))
}

async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    // Host-qualified activity routes retain the producer's bearer-only boundary.
    let segments: Vec<_> = request.uri().path().split('/').collect();
    let activity_write = matches!(
        segments.as_slice(),
        ["", "api", "agents", _, _, "activity"]
            | ["", "api", "agents", _, _, "activity", "producer"]
    ) && matches!(
        *request.method(),
        axum::http::Method::POST | axum::http::Method::PUT
    );
    if activity_write && !guard::bearer_allowed(request.headers(), &state.token) {
        return activity::failure(crate::observation::live_activity::error(
            portal_types::agents::ActivityCode::Unauthorized,
            "activity mutations require bearer authentication",
        ));
    }
    if let Some(response) = guard::check(
        request.method(),
        request.headers(),
        state.port,
        &state.token,
    ) {
        return response;
    }
    next.run(request).await
}

/// One subscription per portal, independent of HTTP subscriber count. A plain thread
/// lets HTTP shutdown finish even during the bounded initial socket handshake.
fn spawn_reader(
    ledger: Ledger,
    sender: watch::Sender<PortalState>,
    stop: CancellationToken,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || reader(&ledger, &sender, &stop))
}

fn reader(ledger: &Ledger, sender: &watch::Sender<PortalState>, stop: &CancellationToken) {
    use crate::observation::{ipc, read};

    let client = ipc::Client::at(&ledger.dir);
    while !stop.is_cancelled() && !sender.is_closed() {
        let result = client.subscribe().and_then(|mut subscription| {
            let mut connection = subscription.current();
            let mut last = None;
            while !stop.is_cancelled() && !sender.is_closed() {
                if let ipc::Status::Disconnected(detail) = connection.status {
                    return Err(crate::error::Error::Invalid(detail));
                }
                if last != Some(connection.publication.revision) {
                    let snapshot = read::view(&connection.publication, "portal")?;
                    sender.send_replace(PortalState {
                        snapshot: Some(snapshot),
                        error: None,
                    });
                    last = Some(connection.publication.revision);
                }
                connection = subscription.changed(TICK);
            }
            Ok(())
        });
        if let Err(error) = result {
            sender.send_modify(|current| current.error = Some(error.to_string()));
        }
        let until = std::time::Instant::now() + RETRY;
        while std::time::Instant::now() < until && !stop.is_cancelled() && !sender.is_closed() {
            std::thread::sleep(TICK);
        }
    }
}

async fn shutdown() {
    let interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt());
    let terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
    match (interrupt, terminate) {
        (Ok(mut interrupt), Ok(mut terminate)) => {
            tokio::select! { _ = interrupt.recv() => {} _ = terminate.recv() => {} }
        }
        (Ok(mut interrupt), Err(_)) => {
            interrupt.recv().await;
        }
        (Err(_), Ok(mut terminate)) => {
            terminate.recv().await;
        }
        (Err(_), Err(_)) => {
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    const PORT: u16 = 7465;

    /// The portal's token, in the state the tests share.
    const TOKEN: &str = "secret";

    fn state(payload: &PortalState) -> AppState {
        let (_sender, receiver) = watch::channel(payload.clone());
        AppState::new(
            receiver,
            PORT,
            None,
            guard::Token::new(TOKEN.to_owned()),
            crate::ledger::Ledger::new(
                std::env::temp_dir().join(format!("dogsled-web-{}", crate::ledger::new_id())),
            )
            .clone(),
        )
    }

    fn board_json() -> PortalState {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        test_support::empty(&ledger)
    }

    /// A request with the given method, host and origin.
    fn request(method: Method, host: &str, origin: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri("/api/board")
            .header(header::HOST, host);
        if let Some(origin) = origin {
            builder = builder.header(header::ORIGIN, origin);
        }
        builder.body(Body::empty()).expect("the request")
    }

    /// A request with the given method, host and bearer token.
    fn request_with_token(method: Method, host: &str, token: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri("/api/board")
            .header(header::HOST, host)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .expect("the request")
    }

    /// One request against a fresh app.
    async fn call(request: Request<Body>) -> Response {
        router(state(&board_json()))
            .oneshot(request)
            .await
            .expect("the router answers")
    }

    #[tokio::test]
    async fn the_board_is_served_for_the_loopback_host() {
        let response = call(request(Method::GET, &format!("127.0.0.1:{PORT}"), None)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("the body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("the payload");
        assert_eq!(value["generated_ms"], 7);
        assert!(value["repos"].is_array());
    }

    #[tokio::test]
    async fn disconnected_portal_retains_snapshot_but_does_not_serve_it_as_current() {
        let publication = board_json();
        let snapshot = publication.snapshot.clone();
        let (sender, receiver) = watch::channel(publication);
        let mut state = state(&PortalState::default());
        state.portal = receiver;
        sender.send_modify(|current| {
            current.error = Some("observation publisher disconnected".to_owned());
        });
        assert_eq!(state.portal.borrow().snapshot, snapshot);
        for path in ["/api/board", "/api/agents", "/api/inbox"] {
            let response = router(state.clone())
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(header::HOST, format!("localhost:{PORT}"))
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            let bytes = axum::body::to_bytes(response.into_body(), 65536)
                .await
                .expect("body");
            assert!(String::from_utf8_lossy(&bytes).contains("observation publisher disconnected"));
        }
    }

    #[tokio::test]
    async fn another_host_is_refused() {
        let response = call(request(Method::GET, "evil.example", None)).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn another_port_is_refused() {
        let response = call(request(Method::GET, "127.0.0.1:8080", None)).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_write_without_the_portal_origin_is_refused() {
        let response = call(request(Method::POST, &format!("127.0.0.1:{PORT}"), None)).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_write_from_another_origin_is_refused() {
        let response = call(request(
            Method::POST,
            &format!("127.0.0.1:{PORT}"),
            Some("http://evil.example"),
        ))
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_write_with_the_portal_origin_reaches_the_route() {
        let response = call(request(
            Method::POST,
            &format!("127.0.0.1:{PORT}"),
            Some(&format!("http://localhost:{PORT}")),
        ))
        .await;
        // The route has no POST, so the guard let the request through and the
        // router refused the method.
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn a_write_with_the_token_and_no_origin_reaches_the_route() {
        let response = call(request_with_token(
            Method::POST,
            &format!("127.0.0.1:{PORT}"),
            TOKEN,
        ))
        .await;
        // The route has no POST, so the guard let the request through and the
        // router refused the method.
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn a_write_with_a_wrong_token_is_refused() {
        let response = call(request_with_token(
            Method::POST,
            &format!("127.0.0.1:{PORT}"),
            "wrong",
        ))
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn an_asset_request_is_answered_with_the_page() {
        let response = call(
            Request::builder()
                .uri("/")
                .header(header::HOST, format!("127.0.0.1:{PORT}"))
                .body(Body::empty())
                .expect("the request"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        // The answer comes from the embedded bundle, not from the router's
        // own not-found, so the asset fallback is mounted.
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("the body");
        assert!(String::from_utf8_lossy(&body).contains("id=\"main\""));
    }

    #[tokio::test]
    async fn an_asset_request_from_another_host_is_refused() {
        let response = call(
            Request::builder()
                .uri("/")
                .header(header::HOST, "evil.example")
                .body(Body::empty())
                .expect("the request"),
        )
        .await;
        // The guard covers the asset fallback too, so the answer is the
        // guard's refusal and not the fallback's 404.
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn the_stream_is_served_for_the_loopback_host() {
        let app = router(state(&board_json()));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/stream")
                    .header(header::HOST, format!("localhost:{PORT}"))
                    .body(Body::empty())
                    .expect("the request"),
            )
            .await
            .expect("the router answers");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_finishes_with_an_open_board_stream() {
        use std::io::{Read as _, Write as _};

        let ledger = Ledger::new(
            std::env::temp_dir().join(format!("dogsled-stop-{}", crate::ledger::new_id())),
        );
        let (_sender, receiver) = watch::channel(board_json());
        let (listener, state) = listen(&ledger, 0, receiver).await.expect("bind the portal");
        let port = state.port;
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            serve_until(listener, state, async {
                let _ = stopping.await;
            })
            .await
        });
        let (opened, ready) = tokio::sync::oneshot::channel();
        let client = tokio::task::spawn_blocking(move || {
            let mut client =
                std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect to the portal");
            client
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("read timeout");
            write!(
                client,
                "GET /api/stream HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
            )
            .expect("request the stream");
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let size = client.read(&mut buffer).expect("read the stream");
                assert!(size > 0, "the stream stays open before shutdown");
                bytes.extend_from_slice(&buffer[..size]);
                if String::from_utf8_lossy(&bytes).contains("event: portal") {
                    break;
                }
            }
            opened.send(()).expect("report the open stream");
            let mut rest = Vec::new();
            client.read_to_end(&mut rest)
        });
        ready.await.expect("the client opened SSE");
        stop.send(()).expect("trigger shutdown");
        let result = tokio::time::timeout(Duration::from_secs(2), server).await;
        assert!(result.is_ok(), "shutdown must not wait for the SSE client");
        result
            .expect("server stopped")
            .expect("server task")
            .expect("serve result");
        client.await.expect("client task").expect("SSE reaches EOF");
        std::fs::remove_dir_all(&ledger.dir).expect("remove portal state");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_does_not_wait_for_a_blocked_publisher_handshake() {
        use std::io::BufRead as _;
        use std::os::unix::net::UnixListener;

        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        std::fs::create_dir_all(&ledger.dir).expect("state directory");
        let path =
            crate::observation::socket_path(&ledger.dir, "observation.sock").expect("socket path");
        let listener = UnixListener::bind(path).expect("stalled publisher");
        let (entered, reading) = tokio::sync::oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let publisher = std::thread::spawn(move || {
            let (socket, _) = listener.accept().expect("subscription connects");
            let mut request = String::new();
            std::io::BufReader::new(&socket)
                .read_line(&mut request)
                .expect("subscription request");
            entered.send(()).expect("report blocked handshake");
            resume.recv().expect("release handshake");
        });
        let (sender, receiver) = watch::channel(PortalState::default());
        let (listener, state) = listen(&ledger, 0, receiver).await.expect("bind portal");
        let reader = spawn_reader(ledger.clone(), sender, state.stop.clone());
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(serve_until(listener, state, async {
            let _ = stopping.await;
        }));
        reading.await.expect("the IPC handshake started");
        stop.send(()).expect("trigger shutdown");
        let result = tokio::time::timeout(Duration::from_secs(2), server).await;
        // Release even on failure, without depending on time to finish the blocked read.
        release.send(()).expect("release publisher");
        publisher.join().expect("publisher stopped");
        reader.join().expect("reader stopped");
        result
            .expect("shutdown does not join the reader")
            .expect("server task")
            .expect("serve result");
        std::fs::remove_dir_all(&ledger.dir).expect("remove portal state");
    }

    #[tokio::test]
    async fn shutdown_ends_the_decisions_stream_without_a_decision() {
        let state = state(&board_json());
        let response = router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/sessions/test-session/decisions")
                    .header(header::HOST, format!("localhost:{PORT}"))
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .body(Body::empty())
                    .expect("the request"),
            )
            .await
            .expect("the decisions response");
        assert_eq!(response.status(), StatusCode::OK);
        state.stop.cancel();
        tokio::time::timeout(
            Duration::from_secs(2),
            axum::body::to_bytes(response.into_body(), 1024),
        )
        .await
        .expect("the decisions stream stops")
        .expect("the body");
    }

    #[tokio::test]
    async fn the_stream_is_refused_for_another_host() {
        let app = router(state(&board_json()));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/stream")
                    .header(header::HOST, "evil.example")
                    .body(Body::empty())
                    .expect("the request"),
            )
            .await
            .expect("the router answers");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
