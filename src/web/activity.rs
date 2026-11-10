//! Activity is a compact serve publication. HTTP only forwards bounded commands and values.

use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use portal_types::agents::{
    ActivityCode, ActivityError, ActivityView, PublishActivity, RegisterProducer,
};
use tokio_stream::StreamExt as _;

use super::AppState;
use super::agents::{PeerOperation, peer_events, peer_response};
use crate::observation::{
    ipc,
    live_activity::{self, Command},
};

/// Released Pi extensions publish to the unqualified local producer paths.
/// These paths must stay stable across binary upgrades while Pi sessions keep running.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/agents/{pane}/activity",
            get(local_current).put(local_publish),
        )
        .route("/api/agents/{pane}/activity/producer", post(local_register))
        .route(
            "/api/agents/{host}/{pane}/activity",
            get(current).put(publish),
        )
        .route(
            "/api/agents/{host}/{pane}/activity/producer",
            post(register),
        )
        .route("/api/agents/{host}/{pane}/activity/events", get(events))
}

pub(super) fn failure(error: ActivityError) -> Response {
    let status = match error.code {
        ActivityCode::InvalidRequest | ActivityCode::UnsupportedVersion => StatusCode::BAD_REQUEST,
        ActivityCode::Unauthorized => StatusCode::FORBIDDEN,
        ActivityCode::AgentMissing => StatusCode::NOT_FOUND,
        ActivityCode::SessionUnverified
        | ActivityCode::UpstreamUnavailable
        | ActivityCode::Capacity => StatusCode::SERVICE_UNAVAILABLE,
        ActivityCode::UpstreamInvalid => StatusCode::BAD_GATEWAY,
        ActivityCode::SessionChanged
        | ActivityCode::StaleGeneration
        | ActivityCode::SequenceConflict
        | ActivityCode::ProducerClosed => StatusCode::CONFLICT,
    };
    (status, [(header::CACHE_CONTROL, "no-store")], Json(error)).into_response()
}

fn value(view: ActivityView) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Json(view)).into_response()
}

async fn local_current(State(state): State<AppState>, Path(pane): Path<String>) -> Response {
    current(State(state), Path((crate::hosts::LOCAL.into(), pane))).await
}

async fn local_register(
    State(state): State<AppState>,
    Path(pane): Path<String>,
    request: Request,
) -> Response {
    register(
        State(state),
        Path((crate::hosts::LOCAL.into(), pane)),
        request,
    )
    .await
}

async fn local_publish(
    State(state): State<AppState>,
    Path(pane): Path<String>,
    request: Request,
) -> Response {
    publish(
        State(state),
        Path((crate::hosts::LOCAL.into(), pane)),
        request,
    )
    .await
}

async fn current(
    State(state): State<AppState>,
    Path((host, pane)): Path<(String, String)>,
) -> Response {
    if let Err(error) = live_activity::validate_pane(&pane) {
        return failure(error);
    }
    if host != crate::hosts::LOCAL {
        return peer_response(state, host, pane, PeerOperation::Activity).await;
    }
    match tokio::task::spawn_blocking(move || ipc::Client::at(&state.web.dir).activity(&pane)).await
    {
        Ok(Ok(view)) => value(view),
        Ok(Err(error)) => failure(error),
        Err(error) => failure(live_activity::error(
            ActivityCode::UpstreamUnavailable,
            &error.to_string(),
        )),
    }
}

async fn register(
    State(state): State<AppState>,
    Path((host, pane)): Path<(String, String)>,
    request: Request,
) -> Response {
    mutation(state, pane, host, request, |bytes| {
        serde_json::from_slice::<RegisterProducer>(bytes).map(Command::Register)
    })
    .await
}

async fn publish(
    State(state): State<AppState>,
    Path((host, pane)): Path<(String, String)>,
    request: Request,
) -> Response {
    mutation(state, pane, host, request, |bytes| {
        serde_json::from_slice::<PublishActivity>(bytes).map(Command::Publish)
    })
    .await
}

async fn mutation(
    state: AppState,
    pane: String,
    host: String,
    request: Request,
    decode: fn(&[u8]) -> serde_json::Result<Command>,
) -> Response {
    if !super::guard::bearer_allowed(request.headers(), &state.token) {
        return failure(live_activity::error(
            ActivityCode::Unauthorized,
            "activity mutations require bearer authentication",
        ));
    }
    if let Err(error) = live_activity::validate_pane(&pane) {
        return failure(error);
    }
    let mut stream = request.into_body().into_data_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            return failure(live_activity::error(
                ActivityCode::InvalidRequest,
                "invalid activity body",
            ));
        };
        if chunk.len() > live_activity::MAX_BODY.saturating_sub(bytes.len()) {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                [(header::CACHE_CONTROL, "no-store")],
                Json(live_activity::error(
                    ActivityCode::InvalidRequest,
                    "activity body exceeds 64 KiB",
                )),
            )
                .into_response();
        }
        bytes.extend_from_slice(&chunk);
    }
    let command = match decode(&bytes) {
        Ok(command) => command,
        Err(error) => {
            return failure(live_activity::error(
                ActivityCode::InvalidRequest,
                &error.to_string(),
            ));
        }
    };
    if host != crate::hosts::LOCAL {
        let operation = match command {
            Command::Register(body) => PeerOperation::Register(body),
            Command::Publish(body) => PeerOperation::Publish(body),
        };
        return peer_response(state, host, pane, operation).await;
    }
    match tokio::task::spawn_blocking(move || {
        ipc::Client::at(&state.web.dir).activity_command(&pane, command)
    })
    .await
    {
        Ok(Ok(ack)) => ([(header::CACHE_CONTROL, "no-store")], Json(ack)).into_response(),
        Ok(Err(error)) => failure(error),
        Err(error) => failure(live_activity::error(
            ActivityCode::UpstreamUnavailable,
            &error.to_string(),
        )),
    }
}

async fn events(
    State(state): State<AppState>,
    Path((host, pane)): Path<(String, String)>,
) -> Response {
    if let Err(error) = live_activity::validate_pane(&pane) {
        return failure(error);
    }
    if host != crate::hosts::LOCAL {
        return peer_events(state, host, pane, PeerOperation::Activity, "activity").await;
    }
    let client = ipc::Client::at(&state.web.dir);
    let mut subscription =
        match tokio::task::spawn_blocking(move || client.activity_subscribe(&pane)).await {
            Ok(Ok(subscription)) => subscription,
            Ok(Err(error)) => return failure(error),
            Err(error) => {
                return failure(live_activity::error(
                    ActivityCode::UpstreamUnavailable,
                    &error.to_string(),
                ));
            }
        };
    let initial = subscription.current();
    let (sender, receiver) =
        tokio::sync::watch::channel(Some(initial.publication.as_ref().clone()));
    let stop = state.stop.clone();
    tokio::task::spawn_blocking(move || {
        let mut revision = initial.publication.revision;
        while !stop.is_cancelled() && !sender.is_closed() {
            let current = subscription.changed(std::time::Duration::from_millis(100));
            if matches!(current.status, ipc::Status::Disconnected(_)) {
                sender.send_replace(None);
                break;
            }
            if current.publication.revision != revision {
                revision = current.publication.revision;
                sender.send_replace(Some(current.publication.as_ref().clone()));
            }
        }
    });
    let stream = tokio_stream::wrappers::WatchStream::new(receiver).map(|view| {
        let event = match view {
            Some(view) => axum::response::sse::Event::default()
                .event("activity")
                .json_data(view),
            None => axum::response::sse::Event::default()
                .event("error")
                .json_data(live_activity::error(
                    ActivityCode::UpstreamUnavailable,
                    "activity disconnected",
                )),
        };
        Ok::<_, std::convert::Infallible>(event.unwrap_or_else(|_| {
            axum::response::sse::Event::default()
                .event("error")
                .data("invalid activity frame")
        }))
    });
    let mut response = axum::response::sse::Sse::new(state.until_shutdown(stream))
        .keep_alive(
            axum::response::sse::KeepAlive::new().interval(std::time::Duration::from_secs(15)),
        )
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::live_activity::tests::Fixture;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use portal_types::agents::{
        ActivityAck, ActivityDetail, LivePhase, LiveState, ProducerPayload,
    };
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

    fn request(method: Method, path: &str, body: impl Into<Body>, bearer: bool) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, "localhost:7465")
            .header(header::ORIGIN, "http://localhost:7465")
            .header(header::CONTENT_TYPE, "application/json");
        if bearer {
            builder = builder.header(header::AUTHORIZATION, "Bearer secret");
        }
        builder.body(body.into()).unwrap()
    }

    async fn json(response: Response) -> serde_json::Value {
        serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 256 * 1024)
                .await
                .unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn producer_mutations_require_bearer_even_with_matching_origin() {
        let fixture = Fixture::new();
        let app = super::super::router(state(&fixture));
        for (method, path) in [
            (Method::POST, "/api/agents/w1:p1/activity/producer"),
            (Method::PUT, "/api/agents/w1:p1/activity"),
            (Method::POST, "/api/agents/local/w1:p1/activity/producer"),
            (Method::PUT, "/api/agents/local/w1:p1/activity"),
            (Method::POST, "/api/agents/netcup/w1:p1/activity/producer"),
            (Method::PUT, "/api/agents/netcup/w1:p1/activity"),
        ] {
            let response = app
                .clone()
                .oneshot(request(method, path, "{}", false))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert_eq!(json(response).await["code"], "unauthorized");
        }
    }

    #[tokio::test]
    async fn unqualified_get_matches_local_qualified_get_in_the_full_router() {
        let fixture = Fixture::new();
        let app = super::super::router(state(&fixture));
        let mut views = Vec::new();
        for path in [
            "/api/agents/w1%3Ap1/activity",
            "/api/agents/local/w1%3Ap1/activity",
        ] {
            let response = app
                .clone()
                .oneshot(request(Method::GET, path, Body::empty(), false))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            views.push(json(response).await);
        }
        assert_eq!(views[0], views[1]);
    }

    #[tokio::test]
    async fn unqualified_register_and_publish_are_visible_through_qualified_get() {
        let fixture = Fixture::new();
        let app = super::super::router(state(&fixture));
        let response = app
            .clone()
            .oneshot(request(
                Method::POST,
                "/api/agents/w1%3Ap1/activity/producer",
                serde_json::to_vec(&fixture.registration).unwrap(),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let ack: ActivityAck = serde_json::from_value(json(response).await).unwrap();
        let mut update = PublishActivity {
            version: 1,
            epoch: ack.epoch,
            generation: ack.generation,
            producer_id: ack.producer_id,
            session: ack.session,
            sequence: 1,
            payload: ProducerPayload::Snapshot {
                state: LiveState {
                    reasoning: None,
                    phase: LivePhase::Thinking,
                    active_tools: vec![],
                },
            },
        };
        let response = app
            .clone()
            .oneshot(request(
                Method::PUT,
                "/api/agents/w1%3Ap1/activity",
                serde_json::to_vec(&update).unwrap(),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let published: ActivityAck = serde_json::from_value(json(response).await).unwrap();
        assert_eq!(published.sequence, 1);
        // The command acknowledgment can precede its immutable publication.
        let deadline = std::time::Instant::now() + crate::test_support::WAIT_DEADLINE;
        loop {
            let response = app
                .clone()
                .oneshot(request(
                    Method::GET,
                    "/api/agents/local/w1%3Ap1/activity",
                    Body::empty(),
                    false,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let view: ActivityView = serde_json::from_value(json(response).await).unwrap();
            if let ActivityDetail::Available {
                sequence: 1, state, ..
            } = view.detail
            {
                assert_eq!(state.phase, LivePhase::Thinking);
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            tokio::task::yield_now().await;
        }
        update.generation += 1;
        let response = app
            .oneshot(request(
                Method::PUT,
                "/api/agents/w1%3Ap1/activity",
                serde_json::to_vec(&update).unwrap(),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(json(response).await["code"], "stale_generation");
    }

    #[tokio::test]
    async fn actual_routes_register_publish_get_and_stream_complete_values() {
        let fixture = Fixture::new();
        let state = state(&fixture);
        let app = super::super::router(state.clone());
        let response = app
            .clone()
            .oneshot(request(
                Method::POST,
                "/api/agents/local/w1:p1/activity/producer",
                serde_json::to_vec(&fixture.registration).unwrap(),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let ack: ActivityAck = serde_json::from_value(json(response).await).unwrap();
        let update = PublishActivity {
            version: 1,
            epoch: ack.epoch,
            generation: ack.generation,
            producer_id: ack.producer_id,
            session: ack.session,
            sequence: 1,
            payload: ProducerPayload::Snapshot {
                state: LiveState {
                    reasoning: None,
                    phase: LivePhase::Thinking,
                    active_tools: vec![],
                },
            },
        };
        let response = app
            .clone()
            .oneshot(request(
                Method::PUT,
                "/api/agents/local/w1:p1/activity",
                serde_json::to_vec(&update).unwrap(),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        // The command acknowledgment can precede its immutable publication.
        let deadline = std::time::Instant::now() + crate::test_support::WAIT_DEADLINE;
        loop {
            let response = app
                .clone()
                .oneshot(request(
                    Method::GET,
                    "/api/agents/local/w1:p1/activity",
                    Body::empty(),
                    false,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let view: ActivityView = serde_json::from_value(json(response).await).unwrap();
            if matches!(view.detail, ActivityDetail::Available { sequence: 1, .. }) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            tokio::task::yield_now().await;
        }
        let response = app
            .oneshot(request(
                Method::GET,
                "/api/agents/local/w1:p1/activity/events",
                Body::empty(),
                false,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let mut stream = response.into_body().into_data_stream();
        let first = stream.next().await.unwrap().unwrap();
        let first = String::from_utf8(first.to_vec()).unwrap();
        assert!(first.contains("event: activity"), "{first}");
        assert!(first.contains("\"thinking\""), "{first}");
        state.stop.cancel();
        drop(stream);
    }

    #[tokio::test]
    async fn route_rejects_unknown_fields_oversize_and_correlation_conflicts() {
        let fixture = Fixture::new();
        let app = super::super::router(state(&fixture));
        let mut registration = serde_json::to_value(&fixture.registration).unwrap();
        registration["unexpected"] = serde_json::json!(true);
        let response = app
            .clone()
            .oneshot(request(
                Method::POST,
                "/api/agents/local/w1:p1/activity/producer",
                registration.to_string(),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(json(response).await["code"], "invalid_request");
        let response = app
            .clone()
            .oneshot(request(
                Method::PUT,
                "/api/agents/local/w1:p1/activity",
                vec![b' '; 65537],
                true,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let mut registration = fixture.registration.clone();
        registration.version = 2;
        let response = app
            .clone()
            .oneshot(request(
                Method::POST,
                "/api/agents/local/w1:p1/activity/producer",
                serde_json::to_vec(&registration).unwrap(),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(json(response).await["code"], "unsupported_version");
        registration.version = 1;
        registration.expected_generation = 999;
        let response = app
            .oneshot(request(
                Method::POST,
                "/api/agents/local/w1:p1/activity/producer",
                serde_json::to_vec(&registration).unwrap(),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(json(response).await["code"], "stale_generation");
    }
}
