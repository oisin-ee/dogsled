//! Unified actionable inbox, derived from the existing stores and board snapshot.

use axum::extract::State;
use axum::response::Response;
use axum::{Router, routing::get};
use portal_types::agents::AgentView;
use portal_types::inbox::{InboxItem, InboxKind, InboxView};
use portal_types::questions::Pending;
use portal_types::reviews::{Review, plan_title};
use portal_types::{BoardView, RowView};

use super::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/inbox", get(list))
}

async fn list(State(state): State<AppState>) -> Response {
    state.projection(|snapshot| snapshot.inbox.clone())
}

pub(crate) fn view(
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
            revision: review.revisions.len() as u64,
            pane: review.pane.clone(),
            title: review_title(review),
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
            revision: 0,
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
            revision: 0,
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
            revision: 0,
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

fn review_title(review: &Review) -> String {
    let latest = review.revisions.last();
    let title = latest
        .and_then(|revision| plan_title(&revision.content))
        .unwrap_or_else(|| {
            review
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&review.path)
                .to_owned()
        });
    let revision = latest.map_or(0, |revision| revision.n);
    format!("{title}, revision {revision}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{Error, Result};
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
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

    #[test]
    fn inbox_revision_counts_submitted_reviews_and_is_zero_for_questions() -> Result<()> {
        let mut review = review();
        review.revisions.push(portal_types::reviews::Revision {
            n: 3,
            content: "Updated plan".into(),
            submitted_ms: 12,
        });
        let question: Pending = serde_json::from_value(json!({
            "id": "question-one", "pane": "worker-pane", "session": "session",
            "cwd": "/repo", "created_ms": 5,
            "questions": {"questions": [{"id": "q", "type": "text", "question": "Which?"}]}
        }))?;
        let inbox = view(&board()?, &[review], &[question], &[], 20);
        assert_eq!(inbox.items[0].kind, InboxKind::Review);
        assert_eq!(inbox.items[0].revision, 2);
        assert_eq!(inbox.items[1].kind, InboxKind::Question);
        assert_eq!(inbox.items[1].revision, 0);
        Ok(())
    }

    #[test]
    fn review_inbox_title_uses_the_latest_heading_or_file_name() -> Result<()> {
        let mut review = review();
        review.path = "/tmp/work/plans/change.md".to_owned();
        review.revisions[0].content = "# Old title".to_owned();
        review.revisions.push(portal_types::reviews::Revision {
            n: 3,
            content: "# Typed **SQLite** state store".to_owned(),
            submitted_ms: 12,
        });
        for (content, expected) in [
            (
                "# Typed **SQLite** state store",
                "Typed SQLite state store, revision 3",
            ),
            ("A plan without a heading.", "change.md, revision 3"),
            ("#", "change.md, revision 3"),
        ] {
            review
                .revisions
                .last_mut()
                .ok_or_else(|| Error::Invalid("missing test revision".to_owned()))?
                .content = content.to_owned();
            let inbox = view(&board()?, &[review.clone()], &[], &[], 20);
            let item = inbox
                .items
                .iter()
                .find(|item| item.kind == InboxKind::Review)
                .ok_or_else(|| Error::Invalid("missing review inbox item".to_owned()))?;
            assert_eq!(item.title, expected);
            assert_eq!(item.age_ms, 8);
        }
        Ok(())
    }

    #[tokio::test]
    async fn stream_publishes_inbox_changes_and_stops_on_shutdown() -> Result<()> {
        use futures_util::StreamExt as _;
        let ledger = crate::ledger::Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let mut board = board()?;
        board.repos.clear();
        let mut publisher = crate::observation::Publisher::new()?;
        let (sender, receiver) = tokio::sync::watch::channel(super::super::test_support::publish(
            &mut publisher,
            &ledger,
            board.clone(),
            vec![],
        )?);
        let state = AppState::new(
            receiver,
            7465,
            None,
            super::super::guard::Token::new("token".into()),
            ledger.clone(),
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
                if text.contains("event: portal\n") {
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
        sender.send_replace(super::super::test_support::publish(
            &mut publisher,
            &ledger,
            board.clone(),
            vec![],
        )?);
        let (changed, remaining) = next_inbox(stream).await;
        assert!(changed.contains("\"count\":1"));
        assert!(changed.contains(&id));
        stream = remaining;
        state.questions.answer(&id, vec![]).await?;
        sender.send_replace(super::super::test_support::publish(
            &mut publisher,
            &ledger,
            board,
            vec![],
        )?);
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

    async fn served_inbox(state: &AppState) -> Result<InboxView> {
        let response = super::super::router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/inbox")
                    .header(header::HOST, "localhost:7465")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .expect("body");
        Ok(serde_json::from_slice(&bytes)?)
    }

    #[tokio::test]
    async fn inbox_counts_every_kind_and_excludes_answered_and_decided() -> Result<()> {
        let ledger = crate::ledger::Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let mut publisher = crate::observation::Publisher::new()?;
        let (sender, receiver) = tokio::sync::watch::channel(super::super::PortalState::default());
        let state = AppState::new(
            receiver,
            7465,
            None,
            super::super::guard::Token::new("token".into()),
            ledger.clone(),
        );
        let mut review = review();
        let save_review = |review: &Review| {
            crate::store::write(&state.web, |tx| {
                crate::web_store::reviews::put(
                    tx,
                    &super::super::reviews::Record {
                        review: review.clone(),
                        delivered: false,
                    },
                )
            })
        };
        save_review(&review)?;
        let body: AskBody = serde_json::from_value(json!({
            "pane": "worker-pane", "session": "session", "cwd": "/repo",
            "questions": {"title": "Which option?", "questions": [{"id": "q", "type": "text", "question": "Which?"}]}
        }))?;
        let id = state.questions.create(body).await?;
        let pending = state.questions.list().await?;
        let agent = AgentView {
            host: "local".into(),
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
        sender.send_replace(super::super::test_support::publish(
            &mut publisher,
            &ledger,
            board()?,
            vec![],
        )?);
        let served = served_inbox(&state).await?;
        assert_eq!(served.count, 3);
        assert_eq!(served.count, served.items.len());
        state.questions.answer(&id, vec![]).await?;
        review.decision = Some(portal_types::reviews::Decision::Approved { notes: None });
        save_review(&review)?;
        sender.send_replace(super::super::test_support::publish(
            &mut publisher,
            &ledger,
            board()?,
            vec![],
        )?);
        let served = served_inbox(&state).await?;
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
