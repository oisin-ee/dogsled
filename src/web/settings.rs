//! Settings routes forward to the host's serve authority, including Pi's request stream.
use super::{
    AppState,
    agents::{PeerOperation, peer_response},
};
use crate::observation::{
    ipc,
    settings::{self, Applied, Command, Failure, Publication},
};
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use portal_types::agents::ProducerEvidence;
use portal_types::models::SettingsChange;
use tokio_stream::StreamExt as _;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/agents/{pane}/settings", put(publish))
        .route("/api/agents/{pane}/settings/requests", get(requests))
        .route(
            "/api/agents/{pane}/settings/requests/{id}/result",
            post(result),
        )
        .route(
            "/api/agents/{host}/{pane}/settings",
            get(current).post(change),
        )
        .layer(axum::extract::DefaultBodyLimit::max(settings::MAX_BODY))
}

pub(super) fn failure(error: Failure) -> Response {
    let status = match error.code.as_str() {
        "invalid_settings" => StatusCode::BAD_REQUEST,
        "unauthorized" => StatusCode::FORBIDDEN,
        "settings_refused" | "session_changed" => StatusCode::CONFLICT,
        "settings_timeout" => StatusCode::GATEWAY_TIMEOUT,
        "settings_busy" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::NOT_FOUND,
    };
    (status, [(header::CACHE_CONTROL, "no-store")], Json(error)).into_response()
}

pub(super) async fn local(state: AppState, pane: String, command: Command) -> Response {
    match tokio::task::spawn_blocking(move || {
        ipc::Client::at(&state.web.dir).settings(&pane, command)
    })
    .await
    {
        Ok(Ok(settings)) => ([(header::CACHE_CONTROL, "no-store")], Json(settings)).into_response(),
        Ok(Err(error)) => failure(error),
        Err(error) => failure(Failure::new("settings_unavailable", &error.to_string())),
    }
}

async fn current(
    State(state): State<AppState>,
    Path((host, pane)): Path<(String, String)>,
) -> Response {
    if host != crate::hosts::LOCAL {
        return peer_response(state, host, pane, PeerOperation::Settings).await;
    }
    local(state, pane, Command::Current).await
}

async fn change(
    State(state): State<AppState>,
    Path((host, pane)): Path<(String, String)>,
    body: Result<Json<SettingsChange>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let change = match body {
        Ok(Json(body)) => body,
        Err(error) => return failure(Failure::new("invalid_settings", &error.body_text())),
    };
    if let Err(error) = settings::validate_change(&change) {
        return failure(error);
    }
    if host != crate::hosts::LOCAL {
        return peer_response(state, host, pane, PeerOperation::ChangeSettings(change)).await;
    }
    local(state, pane, Command::Change(change)).await
}

async fn publish(
    State(state): State<AppState>,
    Path(pane): Path<String>,
    body: Result<Json<Publication>, axum::extract::rejection::JsonRejection>,
) -> Response {
    match body {
        Ok(Json(body)) => local(state, pane, Command::Publish(body)).await,
        Err(error) => failure(Failure::new("invalid_settings", &error.body_text())),
    }
}
async fn result(
    State(state): State<AppState>,
    Path((pane, id)): Path<(String, String)>,
    body: Result<Json<Applied>, axum::extract::rejection::JsonRejection>,
) -> Response {
    match body {
        Ok(Json(result)) => local(state, pane, Command::Result { id, result }).await,
        Err(error) => failure(Failure::new("invalid_settings", &error.body_text())),
    }
}

async fn requests(
    State(state): State<AppState>,
    Path(pane): Path<String>,
    Query(evidence): Query<ProducerEvidence>,
) -> Response {
    let client = ipc::Client::at(&state.web.dir);
    let mut subscription =
        match tokio::task::spawn_blocking(move || client.settings_subscribe(&pane, evidence)).await
        {
            Ok(Ok(subscription)) => subscription,
            Ok(Err(error)) => return failure(error),
            Err(error) => return failure(Failure::new("settings_unavailable", &error.to_string())),
        };
    let (sender, receiver) = tokio::sync::mpsc::channel(64);
    let stop = state.stop.clone();
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
    let mut response = axum::response::sse::Sse::new(state.until_shutdown(stream))
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
    use axum::body::Body;
    use axum::http::{Method, Request};
    use tower::ServiceExt as _;

    fn state(fixture: &Fixture) -> AppState {
        AppState::new(
            tokio::sync::watch::channel(super::super::PortalState::default()).1,
            7465,
            None,
            super::super::guard::Token::new("secret".into()),
            fixture.ledger.clone(),
        )
    }
    fn request(method: Method, path: &str, body: &str, bearer: bool) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, "localhost:7465")
            .header(header::ORIGIN, "http://localhost:7465")
            .header(header::CONTENT_TYPE, "application/json");
        if bearer {
            builder = builder.header(header::AUTHORIZATION, "Bearer secret");
        }
        builder.body(Body::from(body.to_owned())).unwrap()
    }
    async fn json(response: Response) -> serde_json::Value {
        serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), settings::MAX_BODY)
                .await
                .unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn settings_routes_guard_producers_and_validate_changes() {
        let fixture = Fixture::new();
        let app = super::super::router(state(&fixture));
        for (method, path) in [
            (Method::PUT, "/api/agents/w1:p1/settings"),
            (Method::GET, "/api/agents/w1:p1/settings/requests"),
            (
                Method::POST,
                "/api/agents/w1:p1/settings/requests/id/result",
            ),
        ] {
            let response = app
                .clone()
                .oneshot(request(method, path, "{}", false))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(json(response).await["code"], "unauthorized");
        }
        for body in [
            "{}",
            "{\"thinking\":\"wrong\"}",
            "{\"effort\":\"high\"}",
            "null",
            "{\"model\":{\"provider\":\"\",\"id\":\"model\"}}",
        ] {
            let response = app
                .clone()
                .oneshot(request(
                    Method::POST,
                    "/api/agents/local/w1:p1/settings",
                    body,
                    false,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
        let response = app
            .oneshot(request(
                Method::GET,
                "/api/agents/local/w1:p1/settings",
                "",
                false,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(json(response).await["code"], "settings_unavailable");
    }

    #[tokio::test]
    async fn settings_running_server_accepts_native_publication_and_returns_current() {
        let fixture = Fixture::new();
        let state = state(&fixture);
        let stop = state.stop.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(super::super::serve_until(
            listener,
            state,
            stop.clone().cancelled_owned(),
        ));
        let publication = serde_json::to_string(&fixture.publication).unwrap();
        tokio::task::spawn_blocking(move || {
            use std::io::{Read as _, Write as _};
            for (method, path, body) in [("PUT", "/api/agents/w1:p1/settings", publication.as_str()), ("GET", "/api/agents/local/w1:p1/settings", "")] {
                let mut socket = std::net::TcpStream::connect(address).unwrap();
                socket.set_read_timeout(Some(crate::test_support::WAIT_DEADLINE)).unwrap();
                write!(socket, "{method} {path} HTTP/1.1\r\nHost: localhost:7465\r\nAuthorization: Bearer secret\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                let mut response = String::new();
                socket.read_to_string(&mut response).unwrap();
                let (headers, body) = response.split_once("\r\n\r\n").unwrap();
                assert!(headers.starts_with("HTTP/1.1 200"), "{response}");
                assert!(headers.contains("cache-control: no-store"), "{response}");
                assert_eq!(serde_json::from_str::<portal_types::models::AgentSettings>(body).unwrap(), selection());
            }
        }).await.unwrap();
        stop.cancel();
        server.await.unwrap().unwrap();
    }

    #[test]
    fn settings_timeout_maps_to_gateway_timeout() {
        let response = failure(Failure::new(
            "settings_timeout",
            "Pi did not answer in 10 seconds",
        ));
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    }

    #[tokio::test]
    async fn settings_routes_publish_stream_apply_and_refuse() {
        let fixture = Fixture::new();
        let state = state(&fixture);
        let app = super::super::router(state.clone());
        let response = app
            .clone()
            .oneshot(request(
                Method::PUT,
                "/api/agents/w1:p1/settings",
                &serde_json::to_string(&fixture.publication).unwrap(),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let response = app
            .clone()
            .oneshot(request(
                Method::GET,
                "/api/agents/local/w1:p1/settings",
                "",
                false,
            ))
            .await
            .unwrap();
        assert_eq!(
            json(response).await,
            serde_json::to_value(selection()).unwrap()
        );
        let query = format!(
            "/api/agents/w1:p1/settings/requests?session_file={}&session_id=session-a",
            fixture.publication.evidence.session_file
        );
        let response = app
            .clone()
            .oneshot(request(Method::GET, &query, "", true))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        let mut stream = response.into_body().into_data_stream();
        for refusal in [false, true] {
            let request_change = app.clone().oneshot(request(
                Method::POST,
                "/api/agents/local/w1:p1/settings",
                "{\"thinking\":\"low\"}",
                false,
            ));
            let task = tokio::spawn(request_change);
            let mut data = String::new();
            while !data.contains("\n\n") {
                data.push_str(std::str::from_utf8(&stream.next().await.unwrap().unwrap()).unwrap());
            }
            let data = data
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .unwrap();
            let pending: settings::Request = serde_json::from_str(data).unwrap();
            let mut applied = selection();
            applied.thinking = portal_types::models::ThinkingLevel::Low;
            let result = Applied {
                evidence: pending.evidence,
                settings: (!refusal).then_some(applied.clone()),
                error: refusal.then(|| "no credentials for provider".into()),
            };
            let response = app
                .clone()
                .oneshot(request(
                    Method::POST,
                    &format!("/api/agents/w1:p1/settings/requests/{}/result", pending.id),
                    &serde_json::to_string(&result).unwrap(),
                    true,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let response = task.await.unwrap().unwrap();
            assert_eq!(
                response.status(),
                if refusal {
                    StatusCode::CONFLICT
                } else {
                    StatusCode::OK
                }
            );
            if refusal {
                assert_eq!(json(response).await["error"], "no credentials for provider");
            } else {
                assert_eq!(json(response).await, serde_json::to_value(applied).unwrap());
            }
        }
        state.stop.cancel();
        drop(stream);
    }
}
