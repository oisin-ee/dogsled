//! Unified actionable inbox, derived from the existing stores and board snapshot.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing::get};
use portal_types::agents::AgentView;
use portal_types::inbox::{InboxItem, InboxKind, InboxView};
use portal_types::questions::Pending;
use portal_types::reviews::Review;
use portal_types::{BoardView, RowView};

use super::AppState;
use crate::error::{Error, Result};

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/inbox", get(list))
}

async fn list(State(state): State<AppState>) -> Response {
    match read(&state).await {
        Ok(view) => Json(view).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": error.to_string(),
            })),
        )
            .into_response(),
    }
}

async fn read(state: &AppState) -> Result<InboxView> {
    let board: BoardView = serde_json::from_str(&state.payload())?;
    let questions = state.questions.list().await?;
    let web = state.web.clone();
    let socket = state.agents.clone();
    let (reviews, agents) = tokio::task::spawn_blocking(move || -> Result<_> {
        let reviews = super::reviews::pending(&web)?;
        let agents = socket
            .as_ref()
            .map(|socket| super::agents::views(socket))
            .transpose()?
            .unwrap_or_default();
        Ok((reviews, agents))
    })
    .await
    .map_err(|error| Error::Invalid(format!("inbox read failed: {error}")))??;
    Ok(view(
        &board,
        &reviews,
        &questions,
        &agents,
        crate::ledger::now_ms(),
    ))
}

fn view(
    board: &BoardView,
    reviews: &[Review],
    questions: &[Pending],
    agents: &[AgentView],
    now: u64,
) -> InboxView {
    let rows: Vec<&RowView> = board.repos.iter().flat_map(|repo| &repo.rows).collect();
    let row_for = |pane: &str| {
        rows.iter()
            .copied()
            .find(|row| row.pane.as_deref() == Some(pane))
    };
    let metadata = |pane: &str| {
        let row = row_for(pane);
        let agent = agents.iter().find(|agent| agent.pane == pane);
        (
            row.map(|row| row.title.clone().unwrap_or_else(|| row.name.clone()))
                .or_else(|| agent.and_then(|agent| agent.name.clone())),
            row.and_then(|row| row.repository.clone())
                .or_else(|| agent.and_then(|agent| agent.repository.clone())),
        )
    };
    let mut items = Vec::new();
    for review in reviews.iter().filter(|review| review.decision.is_none()) {
        let (agent_title, repository) = metadata(&review.pane);
        items.push(InboxItem {
            kind: InboxKind::Review,
            id: review.id.clone(),
            pane: review.pane.clone(),
            title: review.path.clone(),
            agent_title,
            repository,
            age_ms: now.saturating_sub(
                review
                    .revisions
                    .last()
                    .map_or(now, |revision| revision.submitted_ms),
            ),
        });
    }
    for question in questions {
        let (agent_title, repository) = metadata(&question.pane);
        items.push(InboxItem {
            kind: InboxKind::Question,
            id: question.id.clone(),
            pane: question.pane.clone(),
            title: question
                .questions
                .title
                .clone()
                .or_else(|| {
                    question
                        .questions
                        .questions
                        .first()
                        .map(|question| question.question.clone())
                })
                .unwrap_or_else(|| "Question".into()),
            agent_title,
            repository,
            age_ms: now.saturating_sub(question.created_ms),
        });
    }
    for agent in agents {
        let Some(prompt) = &agent.blocked else {
            continue;
        };
        let (agent_title, repository) = metadata(&agent.pane);
        items.push(InboxItem {
            kind: InboxKind::Prompt,
            id: agent.pane.clone(),
            pane: agent.pane.clone(),
            title: prompt.question.clone(),
            agent_title,
            repository,
            age_ms: row_for(&agent.pane).map_or(0, |row| row.age_ms),
        });
    }
    for row in rows.into_iter().filter(|row| row.unseen) {
        items.push(InboxItem {
            kind: InboxKind::UnseenWorker,
            id: row.id.clone(),
            pane: row.pane.clone().unwrap_or_default(),
            title: row.title.clone().unwrap_or_else(|| row.name.clone()),
            agent_title: Some(row.name.clone()),
            repository: row.repository.clone(),
            age_ms: row.age_ms,
        });
    }
    InboxView {
        count: items.len(),
        items,
    }
}

/// Poll the authoritative stores, but emit only actionable changes, not the ticking ages.
pub(super) fn events(
    state: AppState,
) -> impl tokio_stream::Stream<Item = std::result::Result<Event, std::convert::Infallible>> {
    futures_util::stream::unfold(
        (state, String::new(), tokio::time::interval(super::REFRESH)),
        |(state, mut previous, mut interval)| async move {
            loop {
                interval.tick().await;
                let event = match read(&state).await {
                    Ok(view) => {
                        let mut comparison = view.clone();
                        for item in &mut comparison.items {
                            item.age_ms = 0;
                        }
                        let key = serde_json::to_string(&comparison).unwrap_or_default();
                        if key == previous {
                            continue;
                        }
                        previous = key;
                        Event::default().event("inbox").json_data(&view).ok()?
                    }
                    Err(error) => {
                        let key = format!("error: {error}");
                        if key == previous {
                            continue;
                        }
                        previous.clone_from(&key);
                        Event::default().event("inbox_error").data(key)
                    }
                };
                return Some((Ok(event), (state, previous, interval)));
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, header};
    use portal_types::agents::BlockedPrompt;
    use portal_types::questions::AskBody;
    use serde_json::json;
    use tower::ServiceExt;

    fn board() -> Result<BoardView> {
        Ok(serde_json::from_value(json!({
            "hosts": [], "generated_ms": 1,
            "repos": [{"repository": "github.com/owner/repo", "todos": [], "rows": [{
                "group": "running", "kind": "agent", "id": "child", "name": "worker",
                "what": "pi", "host": "local", "pane": "worker-pane", "owner_pane": "owner",
                "title": "Implement inbox", "state": "idle", "activity": "", "age_ms": 10,
                "repository": "github.com/owner/repo", "unseen": true
            }]}]
        }))?)
    }

    fn review() -> Review {
        Review {
            id: "review-one".into(),
            pane: "worker-pane".into(),
            session: "session".into(),
            cwd: "/repo".into(),
            path: "plans/change.md".into(),
            revisions: vec![portal_types::reviews::Revision {
                n: 1,
                content: "Plan".into(),
                submitted_ms: 3,
            }],
            annotations: vec![],
            general: String::new(),
            decision: None,
        }
    }

    #[tokio::test]
    async fn stream_publishes_inbox_changes_and_stops_on_shutdown() -> Result<()> {
        use futures_util::StreamExt as _;
        let ledger = crate::ledger::Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let mut board = board()?;
        board.repos.clear();
        let (_sender, receiver) = tokio::sync::watch::channel(serde_json::to_string(&board)?);
        let state = AppState::new(
            receiver,
            7465,
            None,
            super::super::guard::Token::new("token".into()),
            ledger.child("web"),
        );
        let response = super::super::router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/stream")
                    .header(header::HOST, "localhost:7465")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let mut stream = response.into_body().into_data_stream();
        let next_inbox = |mut stream: axum::body::BodyDataStream| async move {
            loop {
                let bytes = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
                    .await
                    .expect("SSE event deadline")
                    .expect("SSE stays open")
                    .expect("SSE bytes");
                let text = String::from_utf8(bytes.to_vec()).expect("UTF-8 event");
                if text.contains("event: inbox\n") {
                    return (text, stream);
                }
            }
        };
        let (initial, remaining) = next_inbox(stream).await;
        assert!(initial.contains("\"count\":0"));
        stream = remaining;
        let id = state.questions.create(serde_json::from_value(json!({
            "pane": "worker-pane", "session": "session", "cwd": "/repo",
            "questions": {"title": "Choose", "questions": [{"id": "q", "type": "text", "question": "Which?"}]}
        }))?).await?;
        let (changed, remaining) = next_inbox(stream).await;
        assert!(changed.contains("\"count\":1"));
        assert!(changed.contains(&id));
        stream = remaining;
        state.questions.answer(&id, vec![]).await?;
        let (answered, mut stream) = next_inbox(stream).await;
        assert!(answered.contains("\"count\":0"));
        state.stop.cancel();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
                .await
                .expect("shutdown deadline")
                .is_none()
        );
        std::fs::remove_dir_all(ledger.dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn inbox_counts_every_kind_and_excludes_answered_and_decided() -> Result<()> {
        let ledger = crate::ledger::Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let (_sender, receiver) = tokio::sync::watch::channel(serde_json::to_string(&board()?)?);
        let state = AppState::new(
            receiver,
            7465,
            None,
            super::super::guard::Token::new("token".into()),
            ledger.child("web"),
        );
        let mut review = review();
        let save_review = |review: &Review| {
            state.web.child("reviews").write(
                "review-one.json",
                &json!({"review": review, "delivered": false}),
            )
        };
        save_review(&review)?;
        let body: AskBody = serde_json::from_value(json!({
            "pane": "worker-pane", "session": "session", "cwd": "/repo",
            "questions": {"title": "Which option?", "questions": [{"id": "q", "type": "text", "question": "Which?"}]}
        }))?;
        let id = state.questions.create(body).await?;
        let pending = state.questions.list().await?;
        let agent = AgentView {
            pane: "worker-pane".into(),
            name: Some("worker".into()),
            title: None,
            agent: None,
            state: "blocked".into(),
            cwd: None,
            repository: None,
            blocked: Some(BlockedPrompt {
                question: "Allow?".into(),
                options: vec![],
            }),
        };
        let view = view(&board()?, &[review.clone()], &pending, &[agent], 20);
        assert_eq!(view.count, 4);
        assert_eq!(
            view.items.iter().map(|item| item.kind).collect::<Vec<_>>(),
            vec![
                InboxKind::Review,
                InboxKind::Question,
                InboxKind::Prompt,
                InboxKind::UnseenWorker
            ]
        );
        assert!(
            view.items
                .iter()
                .all(|item| item.repository.as_deref() == Some("github.com/owner/repo"))
        );
        let request = || {
            Request::builder()
                .uri("/api/inbox")
                .header(header::HOST, "localhost:7465")
                .body(Body::empty())
                .expect("request")
        };
        let response = super::super::router(state.clone())
            .oneshot(request())
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .expect("body");
        let served: InboxView = serde_json::from_slice(&bytes)?;
        assert_eq!(served.count, 3);
        assert_eq!(served.count, served.items.len());
        state.questions.answer(&id, vec![]).await?;
        review.decision = Some(portal_types::reviews::Decision::Approved { notes: None });
        save_review(&review)?;
        let served = read(&state).await?;
        assert_eq!(served.count, 1);
        assert_eq!(served.items[0].kind, InboxKind::UnseenWorker);
        let response = super::super::router(state)
            .oneshot(
                Request::builder()
                    .uri("/api/inbox")
                    .header(header::HOST, "evil.example")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        std::fs::remove_dir_all(ledger.dir)?;
        Ok(())
    }
}
