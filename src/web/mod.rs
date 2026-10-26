//! The web portal's server: a guarded JSON and SSE API on the loopback
//! address.
//!
//! The board comes from the same read path the sidebar uses: the cached fleet
//! status, the board built from it, and the cached todo sections. It is never
//! probed, and it is rebuilt every two seconds. The API pushes a new payload
//! only when the board changed.

pub mod board;
pub mod guard;
pub mod token;

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use portal_types::BoardView;
use tokio::sync::watch;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::WatchStream;

use crate::config::Policy;
use crate::error::Result;
use crate::ledger::Ledger;
use crate::shell::System;

/// The reader rebuilds the board this often, like the sidebar does.
const REFRESH: Duration = Duration::from_secs(2);
/// The reader waits for the next tick in slices this long.
const TICK: Duration = Duration::from_millis(100);
/// The stream sends a comment this often, so an idle connection stays open.
const KEEP_ALIVE: Duration = Duration::from_secs(15);

/// The state every request shares: the last payload and the served port.
#[derive(Debug, Clone)]
pub struct AppState {
    /// The last board payload, as JSON.
    board: watch::Receiver<String>,
    /// The port the server listens on.
    port: u16,
}

impl AppState {
    /// The current payload.
    fn payload(&self) -> String {
        self.board.borrow().clone()
    }
}

/// Serve the portal API until the process is asked to stop.
///
/// The command deadline does not apply here: a server runs until it is told
/// to stop.
pub fn serve(ledger: &Ledger, policy: &Policy, home: &str, port: u16) -> Result<()> {
    // The token file exists from the first start, so later steps can use it.
    token::token(ledger)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run(ledger, policy, home, port))
}

/// The server's own work: read the board, publish it, and serve it.
async fn run(ledger: &Ledger, policy: &Policy, home: &str, port: u16) -> Result<()> {
    // The server reads without a deadline: it runs until it is told to stop.
    let shell = System {
        budget: crate::budget::Budget::default(),
    };
    // The first payload is empty: a read that fails must not keep the server
    // from starting. The reader replaces it within a tick.
    let (sender, receiver) = watch::channel(empty_payload());
    let state = AppState {
        board: receiver,
        port,
    };
    // The reader runs on a blocking thread, like the sidebar's reader. It reads
    // the cached state and never probes.
    let stop = Arc::new(AtomicBool::new(false));
    let reader = {
        let thread_stop = Arc::clone(&stop);
        let ledger = ledger.clone();
        let policy = policy.clone();
        let home = home.to_owned();
        tokio::task::spawn_blocking(move || {
            reader(
                &shell,
                &ledger,
                &policy,
                &home,
                &sender,
                thread_stop.as_ref(),
            );
        })
    };
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    println!(
        "dogsled web on http://127.0.0.1:{port} (token in {})",
        token::path(ledger).display()
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await?;
    stop.store(true, Ordering::Relaxed);
    let _ = reader.await;
    Ok(())
}

/// The payload the server starts with: an empty board.
fn empty_payload() -> String {
    let view = board::view(
        &crate::board::Board {
            hosts: Vec::new(),
            rows: Vec::new(),
        },
        &[],
        crate::ledger::now_ms(),
    );
    serde_json::to_string(&view).unwrap_or_else(|_| "{}".to_owned())
}

/// The routes and the guards that wrap them.
fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/board", get(board))
        .route("/api/stream", get(stream))
        .layer(axum::middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

/// The current board, as JSON.
async fn board(State(state): State<AppState>) -> Response {
    (
        [(header::CONTENT_TYPE, "application/json")],
        state.payload(),
    )
        .into_response()
}

/// The board, pushed as it changes.
async fn stream(
    State(state): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = std::result::Result<Event, Infallible>>> {
    let stream = WatchStream::new(state.board.clone())
        .map(|payload| Ok(Event::default().event("board").data(payload)));
    Sse::new(stream).keep_alive(KeepAlive::new().interval(KEEP_ALIVE))
}

/// Refuse a request from another host, or from another origin while it
/// changes state.
async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let headers = request.headers().clone();
    if let Some(response) = guard::check(&method, &headers, state.port) {
        return response;
    }
    next.run(request).await
}

/// Rebuild the board every two seconds and publish it when it changed.
///
/// The timestamp moves on every read, so it stays out of the comparison. Only
/// the hosts and the rows decide whether the board changed.
fn reader(
    shell: &System,
    ledger: &Ledger,
    policy: &Policy,
    home: &str,
    sender: &watch::Sender<String>,
    stop: &AtomicBool,
) {
    let mut last = String::new();
    // The fleet is discovered once and kept, like the sidebar's reader keeps
    // it. A failed discovery is tried again on the next tick.
    let mut fleet: Option<crate::hosts::Fleet<'_>> = None;
    while !stop.load(Ordering::Relaxed) {
        if fleet.is_none() {
            fleet = crate::hosts::Fleet::discover(shell, ledger, policy, home.to_owned()).ok();
        }
        if let Some(fleet) = &fleet
            && let Ok(view) = read(fleet)
        {
            // The timestamp moves on every read, so it stays out of the
            // comparison. Only the hosts and the rows decide whether the board
            // changed.
            if let Ok(key) = serde_json::to_string(&(&view.hosts, &view.repos))
                && key != last
            {
                last = key;
                if let Ok(payload) = serde_json::to_string(&view) {
                    sender.send_replace(payload);
                }
            }
        }
        // Wait for the next tick in slices, so a stop is not delayed.
        let until = std::time::Instant::now() + REFRESH;
        while std::time::Instant::now() < until && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(TICK);
        }
    }
}

/// Read the board the way the sidebar does: the cached fleet status, the board
/// built from it, and the cached todo sections.
fn read(fleet: &crate::hosts::Fleet<'_>) -> Result<BoardView> {
    let status = crate::commands::status(fleet)?;
    let now = crate::ledger::now_ms();
    let board = crate::board::build(&status, None, now);
    // A todo cache that cannot be read leaves the todos empty, not the whole
    // API broken.
    let todos = crate::board::cached_todos(fleet.ledger).unwrap_or_default();
    Ok(board::view(&board, &todos, now))
}

/// Stop the server on SIGINT or SIGTERM.
async fn shutdown() {
    let interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt());
    let terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
    match (interrupt, terminate) {
        (Ok(mut interrupt), Ok(mut terminate)) => {
            tokio::select! {
                _ = interrupt.recv() => {}
                _ = terminate.recv() => {}
            }
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

    fn state(payload: &str) -> AppState {
        let (sender, receiver) = watch::channel(payload.to_owned());
        sender.send_replace(payload.to_owned());
        AppState {
            board: receiver,
            port: PORT,
        }
    }

    fn board_json() -> String {
        let view = board::view(
            &crate::board::Board {
                hosts: Vec::new(),
                rows: Vec::new(),
            },
            &[],
            7,
        );
        serde_json::to_string(&view).expect("the board payload")
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
