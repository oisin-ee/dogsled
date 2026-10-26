//! The portal's questions API: the forms an agent asks, and the answers the
//! owner sends back.
//!
//! The contract is the one the Pi extension uses: the agent posts a form and
//! gets an id, the owner answers it in the portal, and the agent long-polls
//! for the answers. The answers stay in the store until the question is
//! withdrawn, so a poll that never reaches the agent costs one more poll and
//! never an answer.

use std::time::Duration;

use axum::extract::{Path as RoutePath, RawQuery, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use portal_types::questions::{AnswerBody, AskBody, PendingId, PendingList};
use serde_json::json;

use crate::error::Error;
use crate::web::AppState;
use crate::web::store::{Answered, Poll};

/// The time a long poll waits for the answers.
const POLL_LIMIT: Duration = Duration::from_secs(60);

/// The routes of the questions API.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/questions", get(list).post(create))
        .route("/api/questions/{id}/answer", get(answer_wait).post(answer))
        .route("/api/questions/{id}", axum::routing::delete(withdraw))
}

/// A new question. The agent that asks it gets the id it waits on.
async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AskBody>,
) -> Response {
    if !crate::web::guard::bearer_allowed(&headers, &state.token) {
        return refused("the request needs the portal token");
    }
    match state.questions.create(body).await {
        Ok(id) => Json(PendingId { id }).into_response(),
        Err(error) => failed(&error),
    }
}

/// The questions that wait for an answer, oldest first.
async fn list(State(state): State<AppState>) -> Response {
    match state.questions.list().await {
        Ok(pending) => Json(PendingList { pending }).into_response(),
        Err(error) => failed(&error),
    }
}

/// The answers of a question, when the owner gave them.
///
/// The request waits for the answers, up to the poll limit. It answers 204
/// when the time ends, 200 with the answers when they come, and 410 when the
/// question was withdrawn.
async fn answer_wait(
    State(state): State<AppState>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    if !crate::web::guard::bearer_allowed(&headers, &state.token) {
        return refused("the request needs the portal token");
    }
    let limit = match poll_limit(query.as_deref()) {
        Ok(limit) => limit,
        Err(detail) => return bad_request(&detail),
    };
    match state.questions.wait(&id, limit).await {
        Ok(Poll::Answered(responses)) => Json(responses).into_response(),
        Ok(Poll::Timeout) => StatusCode::NO_CONTENT.into_response(),
        Ok(Poll::Gone) => gone(),
        Err(error) => failed(&error),
    }
}

/// The owner's answers of a question.
async fn answer(
    State(state): State<AppState>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
    Json(body): Json<AnswerBody>,
) -> Response {
    if !origin_allowed(&headers, state.port) {
        return refused("the request needs the portal origin");
    }
    match state.questions.answer(&id, body.responses).await {
        Ok(Answered::Saved) => Json(json!({ "id": id, "answered": true })).into_response(),
        Ok(Answered::Already) => conflict("the question is already answered"),
        Ok(Answered::Gone) => gone(),
        Err(error) => failed(&error),
    }
}

/// Withdraw a question. The agent that waits for it gets 410.
async fn withdraw(
    State(state): State<AppState>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
) -> Response {
    if !crate::web::guard::bearer_allowed(&headers, &state.token) {
        return refused("the request needs the portal token");
    }
    match state.questions.withdraw(&id).await {
        Ok(true) => Json(json!({ "id": id, "withdrawn": true })).into_response(),
        Ok(false) => gone(),
        Err(error) => failed(&error),
    }
}

/// Whether the request comes from the portal's own page.
fn origin_allowed(headers: &HeaderMap, port: u16) -> bool {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    crate::web::guard::origin_allowed(&axum::http::Method::POST, origin, port)
}

/// The time the long poll waits, from the `wait` query. It never waits longer
/// than the poll limit.
fn poll_limit(query: Option<&str>) -> std::result::Result<Duration, String> {
    let Some(query) = query else {
        return Ok(POLL_LIMIT);
    };
    for part in query.split('&') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        if key != "wait" {
            continue;
        }
        let seconds: u64 = value
            .parse()
            .map_err(|_| "the wait must be a number of seconds".to_owned())?;
        return Ok(Duration::from_secs(seconds.min(POLL_LIMIT.as_secs())));
    }
    Ok(POLL_LIMIT)
}

/// The request was refused.
fn refused(detail: &str) -> Response {
    (StatusCode::FORBIDDEN, Json(json!({ "error": detail }))).into_response()
}

/// The request was not understood.
fn bad_request(detail: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": detail }))).into_response()
}

/// The question already holds answers.
fn conflict(detail: &str) -> Response {
    (StatusCode::CONFLICT, Json(json!({ "error": detail }))).into_response()
}

/// The question is not there.
fn gone() -> Response {
    (
        StatusCode::GONE,
        Json(json!({ "error": "the question is not there" })),
    )
        .into_response()
}

/// The portal could not read or write its own state.
fn failed(error: &Error) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": error.to_string() })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::Method;
    use axum::middleware::Next;
    use tower::ServiceExt;

    /// The port the tests ask for.
    const PORT: u16 = 7465;

    /// The portal's token.
    const TOKEN: &str = "secret";

    /// A state over a fresh directory.
    fn fresh_state() -> AppState {
        let (_sender, receiver) = tokio::sync::watch::channel(String::new());
        AppState::new(
            receiver,
            PORT,
            None,
            crate::web::guard::Token::new(TOKEN.to_owned()),
            crate::ledger::Ledger::new(
                std::env::temp_dir()
                    .join(format!("dogsled-questions-api-{}", crate::ledger::new_id())),
            )
            .child("web"),
        )
    }

    /// The same guard the server uses.
    async fn test_guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
        let method = request.method().clone();
        let headers = request.headers().clone();
        if let Some(response) =
            crate::web::guard::check(&method, &headers, state.port, &state.token)
        {
            return response;
        }
        next.run(request).await
    }

    /// One request against the questions API.
    async fn call(state: &AppState, request: Request<Body>) -> Response {
        let app = Router::new()
            .merge(routes())
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                test_guard,
            ))
            .with_state(state.clone());
        app.oneshot(request).await.expect("the router answers")
    }

    /// A request with the portal's origin.
    fn request(method: Method, path: &str, body: Option<&str>) -> Request<Body> {
        let builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, format!("127.0.0.1:{PORT}"))
            .header(header::ORIGIN, format!("http://127.0.0.1:{PORT}"));
        let builder = match body {
            Some(body) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_owned())),
            None => builder.body(Body::empty()),
        };
        builder.expect("the request")
    }

    /// A request with the portal's token.
    fn token_request(method: Method, path: &str, body: Option<&str>) -> Request<Body> {
        let request = request(method, path, body);
        let (mut parts, body) = request.into_parts();
        parts.headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {TOKEN}").parse().expect("a header"),
        );
        Request::from_parts(parts, body)
    }

    /// The body of a response, as JSON.
    async fn json_body(response: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("the body");
        serde_json::from_slice(&bytes).expect("the payload")
    }

    /// The form a test asks.
    fn ask_body() -> String {
        json!({
            "questions": {"title": "Choice", "questions": [
                {"id": "q1", "type": "single", "question": "Which one?", "options": ["A", "B"]}
            ]},
            "pane": "%1",
            "session": "session-1",
            "cwd": "/tmp/work",
        })
        .to_string()
    }

    /// Post a question and return its id.
    async fn ask(state: &AppState) -> String {
        let response = call(
            state,
            token_request(Method::POST, "/api/questions", Some(&ask_body())),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        json_body(response).await["id"]
            .as_str()
            .expect("the question id")
            .to_owned()
    }

    #[tokio::test]
    async fn a_question_asked_with_the_token_is_answered_through_the_origin() {
        let state = fresh_state();
        let id = ask(&state).await;
        // The pending question is listed for the portal.
        let response = call(&state, request(Method::GET, "/api/questions", None)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["pending"].as_array().expect("the list").len(), 1);
        assert_eq!(body["pending"][0]["id"], serde_json::json!(id));
        // The long poll waits for the answers.
        let waiting = call(
            &state,
            token_request(
                Method::GET,
                &format!("/api/questions/{id}/answer?wait=2"),
                None,
            ),
        );
        // The owner answers.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let response = call(
            &state,
            request(
                Method::POST,
                &format!("/api/questions/{id}/answer"),
                Some(r#"{"responses":[{"id":"q1","value":"B"}]}"#),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        // The long poll gets the answers.
        let response = waiting.await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["id"], serde_json::json!(id));
        assert_eq!(body["responses"][0]["value"], serde_json::json!("B"));
    }

    #[tokio::test]
    async fn a_question_without_a_token_or_an_origin_is_refused() {
        let state = fresh_state();
        let response = call(
            &state,
            Request::builder()
                .method(Method::POST)
                .uri("/api/questions")
                .header(header::HOST, format!("127.0.0.1:{PORT}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(ask_body()))
                .expect("the request"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_question_waiting_for_an_answer_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("dogsled-restart-{}", crate::ledger::new_id()));
        let web = crate::ledger::Ledger::new(dir.clone()).child("web");
        let id = {
            let (_sender, receiver) = tokio::sync::watch::channel(String::new());
            let state = AppState::new(
                receiver,
                PORT,
                None,
                crate::web::guard::Token::new(TOKEN.to_owned()),
                web.clone(),
            );
            ask(&state).await
        };
        // A new state over the same directory holds the question that waits.
        let (_sender, receiver) = tokio::sync::watch::channel(String::new());
        let state = AppState::new(
            receiver,
            PORT,
            None,
            crate::web::guard::Token::new(TOKEN.to_owned()),
            web,
        );
        let response = call(&state, request(Method::GET, "/api/questions", None)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["pending"][0]["id"], serde_json::json!(id));
    }

    #[tokio::test]
    async fn a_withdrawn_question_wakes_the_waiting_poll_with_gone() {
        let state = fresh_state();
        let id = ask(&state).await;
        let waiting = call(
            &state,
            token_request(
                Method::GET,
                &format!("/api/questions/{id}/answer?wait=5"),
                None,
            ),
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let response = call(
            &state,
            token_request(Method::DELETE, &format!("/api/questions/{id}"), None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = waiting.await;
        assert_eq!(response.status(), StatusCode::GONE);
    }

    #[tokio::test]
    async fn a_long_poll_that_times_out_answers_no_content() {
        let state = fresh_state();
        let id = ask(&state).await;
        let response = call(
            &state,
            token_request(
                Method::GET,
                &format!("/api/questions/{id}/answer?wait=0"),
                None,
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn an_answer_without_the_portal_origin_is_refused() {
        let state = fresh_state();
        let id = ask(&state).await;
        let response = call(
            &state,
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/questions/{id}/answer"))
                .header(header::HOST, format!("127.0.0.1:{PORT}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"responses":[]}"#))
                .expect("the request"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
