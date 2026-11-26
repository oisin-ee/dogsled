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
    let current = state.portal.borrow().clone();
    let inbox = match projected(&state, &current).await {
        Ok(inbox) => inbox,
        Err(error) => return super::observation_error(&error),
    };
    AppState::project(&current, |_| Some(inbox))
}

type Projection = Result<InboxView, String>;
type Sender = tokio::sync::watch::Sender<Option<Projection>>;

/// One inbox projection per web server, not one fan-out to peers per open tab.
#[derive(Debug, Clone, Default)]
pub(super) struct LiveInbox(std::sync::Arc<std::sync::Mutex<Option<Sender>>>);

impl LiveInbox {
    pub(super) fn subscribe(
        &self,
        state: &AppState,
    ) -> tokio::sync::watch::Receiver<Option<Projection>> {
        let mut cached = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(sender) = cached.as_ref().filter(|sender| sender.receiver_count() > 0) {
            return sender.subscribe();
        }
        let (sender, receiver) = tokio::sync::watch::channel(None);
        *cached = Some(sender.clone());
        let state = state.clone();
        tokio::spawn(async move {
            let mut portal = state.portal.clone();
            loop {
                let current = portal.borrow_and_update().clone();
                let result = tokio::select! {
                    () = state.stop.cancelled() => break,
                    () = sender.closed() => break,
                    result = projected(&state, &current) => result,
                };
                sender.send_if_modified(|previous| {
                    if previous.as_ref() == Some(&result) {
                        return false;
                    }
                    *previous = Some(result);
                    true
                });
                tokio::select! {
                    () = state.stop.cancelled() => break,
                    () = sender.closed() => break,
                    changed = portal.changed() => if changed.is_err() { break; },
                }
            }
        });
        receiver
    }
}

async fn projected(state: &AppState, current: &super::PortalState) -> Projection {
    if let Some(error) = &current.error {
        return Err(error.clone());
    }
    let (mut inbox, hosts) = current
        .snapshot
        .as_ref()
        .and_then(|snapshot| Some((snapshot.inbox.clone()?, &snapshot.board.as_ref()?.hosts)))
        .ok_or_else(|| "inbox unavailable".to_owned())?;
    inbox.items.retain(|item| item.host == crate::hosts::LOCAL);
    let mut peers = tokio::task::JoinSet::new();
    for host in hosts
        .iter()
        .filter(|host| host.label != crate::hosts::LOCAL)
        .cloned()
    {
        if !host.reachable {
            inbox.gaps.push(portal_types::inbox::HostGap {
                host: host.label,
                error: host.reason.unwrap_or_else(|| "host unreachable".into()),
            });
            continue;
        }
        let state = state.clone();
        peers.spawn_blocking(move || {
            let result = super::agents::peer_call(
                &state,
                &host.label,
                "",
                &super::agents::PeerOperation::Inbox,
            )
            .and_then(|reply| {
                if reply.status != 200 {
                    return Err(crate::error::Error::Invalid("inbox unavailable".into()));
                }
                Ok(serde_json::from_value::<InboxView>(reply.body)?)
            });
            (host.label, result)
        });
    }
    while let Some(result) = peers.join_next().await {
        match result {
            Ok((host, Ok(mut peer))) => {
                peer.items.retain(|item| item.host == crate::hosts::LOCAL);
                for item in &mut peer.items {
                    item.host.clone_from(&host);
                }
                // Each peer contributes only its local items, never another peer's rows.
                inbox.items.extend(peer.items);
                inbox.gaps.extend(peer.gaps.into_iter().map(|mut gap| {
                    gap.host.clone_from(&host);
                    gap
                }));
            }
            Ok((host, Err(error))) => inbox.gaps.push(portal_types::inbox::HostGap {
                host,
                error: error.to_string(),
            }),
            Err(error) => return Err(error.to_string()),
        }
    }
    inbox.count = inbox.items.len();
    Ok(inbox)
}

/// Select the owning serve's publication, including an explicit unavailable result.
pub(super) async fn published(
    client: crate::observation::ipc::Client,
) -> crate::error::Result<InboxView> {
    let published = tokio::task::spawn_blocking(move || {
        client.current(&crate::observation::ipc::Select::views(["portal"]))
    })
    .await
    .map_err(|error| crate::error::Error::Invalid(error.to_string()))??;
    let snapshot: portal_types::PortalSnapshot =
        crate::observation::read::view(&published, "portal")?;
    snapshot
        .inbox
        .ok_or_else(|| crate::error::Error::Invalid("inbox observation unavailable".into()))
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
    let present = |pane: &str| {
        agents
            .iter()
            .any(|agent| agent.host == "local" && agent.pane == pane)
    };
    let mut items = Vec::new();
    for review in reviews
        .iter()
        .filter(|review| review.decision.is_none() && present(&review.pane))
    {
        let (agent_title, repository) = metadata(&review.pane);
        items.push(InboxItem {
            host: crate::hosts::LOCAL.into(),
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
    for question in questions.iter().filter(|question| present(&question.pane)) {
        let (agent_title, repository) = metadata(&question.pane);
        items.push(InboxItem {
            host: crate::hosts::LOCAL.into(),
            kind: InboxKind::Question,
            id: question.id.clone(),
            revision: 0,
            pane: question.pane.clone(),
            title: question_title(question),
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
            host: agent.host.clone(),
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
            host: row.host.clone(),
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
        gaps: Vec::new(),
    }
}

fn question_title(pending: &Pending) -> String {
    pending
        .questions
        .title
        .clone()
        .or_else(|| {
            pending
                .questions
                .questions
                .first()
                .map(|question| question.question.clone())
        })
        .unwrap_or_else(|| "Question".into())
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

    fn agent(host: &str, pane: &str, state: &str) -> AgentView {
        AgentView {
            host: host.into(),
            pane: pane.into(),
            model: None,
            name: Some("worker".into()),
            title: None,
            agent: None,
            state: state.into(),
            cwd: None,
            repository: None,
            blocked: None,
            reply: None,
            launch: None,
        }
    }

    fn question() -> Result<Pending> {
        Ok(serde_json::from_value(json!({
            "id": "question-one", "pane": "worker-pane", "session": "session",
            "cwd": "/repo", "created_ms": 5,
            "questions": {"questions": [{"id": "q", "type": "text", "question": "Which?"}]}
        }))?)
    }

    #[test]
    fn requests_require_a_present_local_owner_not_a_retained_row_or_peer() -> Result<()> {
        let mut board = board()?;
        let row = &mut board.repos[0].rows[0];
        row.state = "closed".into();
        row.unseen = false;
        for agents in [vec![], vec![agent("peer", "worker-pane", "idle")]] {
            let inbox = view(&board, &[review()], &[question()?], &agents, 20);
            assert_eq!(inbox.count, 0);
            assert_eq!(inbox.items, []);
        }
        // Leads have no retained worker row to remove.
        board.repos.clear();
        let mut review = review();
        review.pane = "lead-pane".into();
        let mut question = question()?;
        question.pane = "lead-pane".into();
        assert_eq!(view(&board, &[review], &[question], &[], 20).count, 0);
        Ok(())
    }

    #[test]
    fn present_owners_keep_requests_regardless_of_state() -> Result<()> {
        let mut board = board()?;
        board.repos.clear();
        for state in ["idle", "stopped", "blocked", "waiting", "asking", "done"] {
            let inbox = view(
                &board,
                &[review()],
                &[question()?],
                &[agent("local", "worker-pane", state)],
                20,
            );
            assert_eq!(inbox.count, 2, "{state}");
            assert_eq!(inbox.items[0].kind, InboxKind::Review);
            assert_eq!(inbox.items[1].kind, InboxKind::Question);
        }
        Ok(())
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
        let inbox = view(
            &board()?,
            &[review],
            &[question],
            &[agent("local", "worker-pane", "idle")],
            20,
        );
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
            let inbox = view(
                &board()?,
                &[review.clone()],
                &[],
                &[agent("local", "worker-pane", "idle")],
                20,
            );
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
    async fn live_socket_publishes_inbox_changes_and_stops_on_shutdown() -> Result<()> {
        use super::super::live::tests as live;
        let ledger = crate::ledger::Ledger::temporary();
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
        let server = live::Server::start(&state).await;
        let mut stream = server.connect().await;
        let initial = next_inbox(&mut stream).await;
        assert_eq!(initial.count, 0);
        let id = state.questions.create(serde_json::from_value(json!({
            "pane": "worker-pane", "session": "session", "cwd": "/repo",
            "questions": {"title": "Choose", "questions": [{"id": "q", "type": "text", "question": "Which?"}]}
        }))?).await?;
        sender.send_replace(super::super::test_support::publish(
            &mut publisher,
            &ledger,
            board.clone(),
            vec![agent("local", "worker-pane", "idle")],
        )?);
        let changed = next_inbox(&mut stream).await;
        assert_eq!(changed.count, 1);
        assert_eq!(changed.items[0].id, id);
        state.questions.answer(&id, vec![]).await?;
        sender.send_replace(super::super::test_support::publish(
            &mut publisher,
            &ledger,
            board,
            vec![agent("local", "worker-pane", "idle")],
        )?);
        let answered = next_inbox(&mut stream).await;
        assert_eq!(answered.count, 0);
        state.stop.cancel();
        tokio::time::timeout(crate::test_support::WAIT_DEADLINE, async {
            while live::read(&mut stream).await.is_some() {}
        })
        .await
        .expect("socket closes on shutdown");
        std::fs::remove_dir_all(ledger.dir)?;
        Ok(())
    }

    async fn next_inbox(socket: &mut super::super::live::tests::Socket) -> InboxView {
        super::super::live::tests::next(socket, |frame| match frame {
            portal_types::live::ServerFrame::Inbox { result } => {
                Some(result.expect("available inbox"))
            }
            _ => None,
        })
        .await
    }

    async fn next_snapshot(
        socket: &mut super::super::live::tests::Socket,
    ) -> portal_types::PortalSnapshot {
        super::super::live::tests::next(socket, |frame| match frame {
            portal_types::live::ServerFrame::Portal {
                event: portal_types::PortalEvent::Snapshot(snapshot),
            } => Some(*snapshot),
            _ => None,
        })
        .await
    }

    #[tokio::test]
    async fn closed_owner_publication_removes_requests_from_get_and_open_stream() -> Result<()> {
        let ledger = crate::ledger::Ledger::temporary();
        let review = review();
        crate::store::write(&ledger, |tx| {
            crate::web_store::reviews::put(
                tx,
                &super::super::reviews::Record {
                    review: review.clone(),
                    delivered: false,
                },
            )
        })?;
        let mut publisher = crate::observation::Publisher::new()?;
        let (sender, receiver) = tokio::sync::watch::channel(super::super::PortalState::default());
        let state = AppState::new(
            receiver,
            7465,
            None,
            super::super::guard::Token::new("token".into()),
            ledger.clone(),
        );
        let id = state.questions.create(serde_json::from_value(json!({
            "pane": "worker-pane", "session": "session", "cwd": "/repo",
            "questions": {"title": "Choose", "questions": [{"id": "q", "type": "text", "question": "Which?"}]}
        }))?).await?;
        let pending = state.questions.list().await?;
        let mut board = board()?;
        board.repos[0].rows[0].unseen = false;
        sender.send_replace(super::super::test_support::publish(
            &mut publisher,
            &ledger,
            board.clone(),
            vec![agent("local", "worker-pane", "stopped")],
        )?);
        let initial = served_inbox(&state).await?;
        assert_eq!(initial.count, 2);
        assert_eq!(initial.items[0].id, review.id);
        assert_eq!(initial.items[1].id, id);
        let live_server = super::super::live::tests::Server::start(&state).await;
        let mut stream = live_server.connect().await;
        let first = next_snapshot(&mut stream).await;
        assert_eq!(first.inbox, Some(initial));

        // Keep the historical board row and a same-pane peer, but remove the local owner.
        board.repos[0].rows[0].state = "closed".into();
        sender.send_replace(super::super::test_support::publish(
            &mut publisher,
            &ledger,
            board,
            vec![agent("peer", "worker-pane", "idle")],
        )?);
        let changed = next_snapshot(&mut stream).await;
        assert_eq!(changed.epoch, first.epoch);
        assert!(changed.revision > first.revision);
        let served = served_inbox(&state).await?;
        assert_eq!(served.count, 0);
        assert_eq!(served.items, []);
        assert_eq!(changed.inbox, Some(served));
        let row = &changed.board.expect("published board").repos[0].rows[0];
        assert_eq!(row.pane.as_deref(), Some("worker-pane"));
        assert_eq!(row.state, "closed");
        assert_eq!(state.questions.list().await?, pending);
        let retained =
            crate::store::read(&ledger, |tx| crate::web_store::reviews::get(tx, &review.id))?
                .expect("retained review");
        assert_eq!(retained.review, review);
        assert!(!retained.delivered);
        state.stop.cancel();
        drop(stream);
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
        let ledger = crate::ledger::Ledger::temporary();
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
        let blocked_agent = AgentView {
            host: "local".into(),
            pane: "worker-pane".into(),
            model: None,
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
            reply: None,
            launch: None,
        };
        let view = view(&board()?, &[review.clone()], &pending, &[blocked_agent], 20);
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
            vec![agent("local", "worker-pane", "idle")],
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
            vec![agent("local", "worker-pane", "idle")],
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
