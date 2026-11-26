//! Activity is a compact serve publication. HTTP only forwards bounded commands and values.

use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use portal_types::agents::{ActivityCode, ActivityError, PublishActivity, RegisterProducer};

use super::AppState;
use super::agents::{PeerOperation, peer_response};
use crate::observation::live_activity::{self, Command};

/// Host-qualified activity routes. Pi's unqualified producer paths are in `producer`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/agents/{host}/{pane}/activity",
            get(current).put(publish),
        )
        .route(
            "/api/agents/{host}/{pane}/activity/producer",
            post(register),
        )
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
    super::producer::current_activity(state.web.dir.clone(), pane).await
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
    let command = match super::producer::activity_body(request, decode).await {
        Ok(command) => command,
        Err(response) => return *response,
    };
    if host != crate::hosts::LOCAL {
        let operation = match command {
            Command::Register(body) => PeerOperation::Register(body),
            Command::Publish(body) => PeerOperation::Publish(body),
        };
        return peer_response(state, host, pane, operation).await;
    }
    super::producer::activity_command(state.web.dir.clone(), pane, command).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::live_activity::tests::Fixture;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use portal_types::agents::{
        ActivityAck, ActivityDetail, ActivityView, LivePhase, LiveState, ProducerPayload,
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
        use super::super::live::tests as live;
        use portal_types::live::ServerFrame;

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
        let server = live::Server::start(&state).await;
        let mut socket = server.connect().await;
        let address = portal_types::agents::AgentAddress::local("w1:p1");
        live::select(&mut socket, 1, Some(address.clone())).await;
        let view = live::next(&mut socket, |frame| match frame {
            ServerFrame::Activity { id: 1, view } => Some(view),
            _ => None,
        })
        .await;
        assert!(matches!(
            view.detail,
            ActivityDetail::Available {
                state: LiveState {
                    phase: LivePhase::Thinking,
                    ..
                },
                ..
            }
        ));
        live::select(&mut socket, 2, None).await;
        live::select(&mut socket, 3, Some(address)).await;
        let reopened = live::next(&mut socket, |frame| match frame {
            ServerFrame::Activity { id: 3, view } => Some(view),
            _ => None,
        })
        .await;
        assert_eq!(reopened.session, view.session);
        state.stop.cancel();
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
