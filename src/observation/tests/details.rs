use super::super::details::{Capture, Codec, Content, ReadState, Reference, fixture::Fixture};
use super::super::{Published, ipc, peers, read};
use crate::herdr_socket::fake;
use crate::probe::Agent;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

struct Native {
    server: fake::Server,
    raw: Arc<Mutex<Value>>,
    screen: Arc<Mutex<Value>>,
    directory: std::path::PathBuf,
}

impl Native {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(crate::ledger::new_id());
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("session.jsonl");
        std::fs::write(&path, "{\"type\":\"session\",\"id\":\"first\"}\n{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"original task\"}]}}\n").unwrap();
        let raw = Arc::new(Mutex::new(
            json!({"pane_id":"w1:p1","agent":"pi","agent_status":"working","terminal_id":"terminal-1","agent_session":{"kind":"path","value":path}}),
        ));
        let screen = Arc::new(Mutex::new(json!({"text":"original screen", "revision":7})));
        let source = Arc::clone(&raw);
        let output = Arc::clone(&screen);
        let server = fake::Server::start(move |method, _| match method {
            "agent.list" => json!({"result":{"agents":[source.lock().unwrap().clone()]}}),
            "pane.read" => json!({"result":{"read":output.lock().unwrap().clone()}}),
            other => panic!("unexpected observation method {other}"),
        });
        Self {
            server,
            raw,
            screen,
            directory,
        }
    }

    fn agent(&self) -> Agent {
        let shell = crate::shell::Fake::new();
        shell.on(
            "herdr agent list",
            &json!({"result":{"agents":[self.raw.lock().unwrap().clone()]}}).to_string(),
        );
        crate::probe::agent_list(&shell).unwrap().remove(0)
    }

    fn reads(&self) -> usize {
        self.server
            .methods()
            .iter()
            .filter(|method| *method == "pane.read")
            .count()
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.directory).unwrap();
    }
}

fn detail(frame: &Published) -> Option<Reference> {
    read::view::<BTreeMap<String, Reference>>(frame, "agent_details")
        .unwrap()
        .remove("w1:p1")
}

fn available(frame: &Published) -> bool {
    detail(frame)
        .is_some_and(|detail| matches!(detail.screen, ReadState::Available | ReadState::Empty))
}

#[test]
fn interests_scope_ticks_and_viewers_share_one_capture() {
    let native = Native::new();
    let fixture = Fixture::new(native.server.client());
    assert_eq!(native.reads(), 0);
    assert!(!fixture.tick("w1:p1"));
    let viewer = fixture.client.detail("w1:p1").unwrap();
    let first = fixture.wait(available);
    assert_eq!(native.reads(), 1);
    assert_eq!(fixture.collector.lock().unwrap().transcript_reads(), 1);
    let viewers: Vec<_> = (0..8)
        .map(|_| fixture.client.detail("w1:p1").unwrap())
        .collect();
    for count in 2..=5 {
        assert!(fixture.tick("w1:p1"));
        assert_eq!(native.reads(), count);
        assert_eq!(fixture.collector.lock().unwrap().transcript_reads(), 1);
        assert_eq!(fixture.client.current().unwrap().revision, first.revision);
    }
    assert_eq!(
        native
            .server
            .methods()
            .iter()
            .filter(|method| *method == "events.subscribe")
            .count(),
        0
    );
    drop(viewers);
    drop(viewer);
    fixture.wait(|frame| detail(frame).is_none());
    assert!(fixture.no_viewers());
    assert!(!fixture.tick("w1:p1"));
    assert_eq!(native.reads(), 5);
}

#[test]
fn native_revision_change_publishes_once_and_peer_export_excludes_content() {
    let native = Native::new();
    let fixture = Fixture::new(native.server.client());
    let _viewer = fixture.client.detail("w1:p1").unwrap();
    let first = fixture.wait(available);
    *native.screen.lock().unwrap() = json!({"text":"changed screen", "revision":8});
    fixture.tick("w1:p1");
    let changed =
        fixture.wait(|frame| detail(frame).is_some_and(|detail| detail.screen_revision == Some(8)));
    assert_eq!(changed.revision, first.revision + 1);
    assert_eq!(native.reads(), 2);
    fixture.tick("w1:p1");
    assert_eq!(fixture.client.current().unwrap().revision, changed.revision);
    let export = serde_json::to_string(&peers::Export::of(&changed)).unwrap();
    assert!(!export.contains("changed screen"));
    assert!(!export.contains("original task"));
    assert!(!export.contains("agent_details"));
}

#[test]
fn session_replacement_never_reuses_old_conversation_or_screen() {
    let native = Native::new();
    let fixture = Fixture::new(native.server.client());
    let _viewer = fixture.client.detail("w1:p1").unwrap();
    let first = fixture.wait(available);
    let old = detail(&first).unwrap();
    let new_path = native.directory.join("second.jsonl");
    std::fs::write(&new_path, "{\"type\":\"session\",\"id\":\"second\"}\n").unwrap();
    {
        let mut raw = native.raw.lock().unwrap();
        raw["terminal_id"] = json!("terminal-2");
        raw["agent_session"]["value"] = json!(new_path);
    }
    *native.screen.lock().unwrap() = json!({"text":"", "revision":7});
    fixture.wake();
    let new = fixture.wait(|frame| {
        detail(frame).is_some_and(|detail| {
            detail.session != old.session
                && matches!(detail.screen, ReadState::Available | ReadState::Empty)
        })
    });
    let detail = detail(&new).unwrap();
    assert!(matches!(detail.conversation, ReadState::Empty));
    assert!(matches!(detail.screen, ReadState::Empty));
}

#[test]
fn empty_unavailable_and_failed_are_distinct_and_do_not_poison_portal() {
    let native = Native::new();
    let mut capture = Capture::default();
    let mut agent = native.agent();
    let empty = native.directory.join("empty.jsonl");
    std::fs::write(&empty, "").unwrap();
    agent.transcript = Some(empty.display().to_string());
    let empty = capture.tick(&agent, Ok(&native.server.client()));
    assert!(
        matches!(empty.conversation, Content::Available(conversation) if conversation.turns.is_empty())
    );
    agent.transcript = None;
    let unavailable = capture.tick(&agent, Err("socket unavailable".into()));
    assert!(matches!(unavailable.conversation, Content::Unavailable(_)));
    assert!(matches!(unavailable.screen, Content::Unavailable(_)));
    agent.transcript = Some(native.directory.display().to_string());
    let failed = capture.tick(&agent, Ok(&native.server.client()));
    assert!(matches!(failed.conversation, Content::Failed(_)));
    native.raw.lock().unwrap()["agent_session"]["value"] = json!(native.directory);
    let fixture = Fixture::new(native.server.client());
    let _viewer = fixture.client.detail("w1:p1").unwrap();
    let frame = fixture.wait(|frame| {
        detail(frame).is_some_and(|detail| matches!(detail.conversation, ReadState::Failed(_)))
    });
    let portal: portal_types::PortalSnapshot = read::view(&frame, "portal").unwrap();
    assert!(portal.board.is_some());
    assert_eq!(portal.agents.unwrap().len(), 1);
    assert!(portal.inbox.is_some());
}

#[test]
fn transcript_identity_length_and_mtime_skip_unchanged_reads() {
    let native = Native::new();
    let mut capture = Capture::default();
    let agent = native.agent();
    let first = capture.tick(&agent, Ok(&native.server.client()));
    for _ in 0..3 {
        assert_eq!(capture.tick(&agent, Ok(&native.server.client())), first);
    }
    assert_eq!(capture.transcript_reads, 1);
    let path = agent.transcript.as_ref().unwrap();
    std::fs::write(path, "").unwrap();
    capture.tick(&agent, Ok(&native.server.client()));
    assert_eq!(capture.transcript_reads, 2);
    std::fs::remove_file(path).unwrap();
    std::fs::write(path, "{\"type\":\"session\",\"id\":\"replacement\"}\n").unwrap();
    capture.tick(&agent, Ok(&native.server.client()));
    assert_eq!(capture.transcript_reads, 3);
}

#[tokio::test]
async fn conversation_over_main_frame_limit_loads_only_through_detail_route() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    let native = Native::new();
    let fixture = Fixture::new(native.server.client());
    let _viewer = fixture.client.detail("w1:p1").unwrap();
    let small = fixture.wait(available);
    let previous = detail(&small).unwrap().revision;
    let text = "x".repeat(17 * 1024 * 1024);
    let path = native.agent().transcript.unwrap();
    let message =
        json!({"type":"message","message":{"role":"user","content":[{"type":"text","text":text}]}});
    std::fs::write(path, message.to_string() + "\n").unwrap();
    fixture.tick("w1:p1");
    let large = fixture.wait(|frame| {
        detail(frame).is_some_and(|detail| {
            detail.revision > previous && matches!(detail.conversation, ReadState::Available)
        })
    });
    let small_bytes = serde_json::to_vec(&*small).unwrap();
    let large_bytes = serde_json::to_vec(&*large).unwrap();
    assert!(large_bytes.len().abs_diff(small_bytes.len()) < 256);
    assert!(
        !String::from_utf8(large_bytes)
            .unwrap()
            .contains(&"x".repeat(1024))
    );
    let (_sender, portal) = tokio::sync::watch::channel(crate::web::PortalState::default());
    let state = crate::web::AppState::new(
        portal,
        7465,
        None,
        crate::web::guard::Token::new(String::new()),
        fixture.ledger.clone(),
    );
    let response = crate::web::agents::test_request(
        state,
        Request::builder()
            .uri("/api/agents/w1:p1/conversation")
            .header("host", "localhost:7465")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(
        response.into_body(),
        crate::activity::CONVERSATION_BYTES as usize,
    )
    .await
    .unwrap();
    let conversation: portal_types::agents::Conversation = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(conversation.turns.len(), 1);
    assert_eq!(conversation.turns[0].text.len(), text.len());
    assert_eq!(conversation.turns[0].text, text);
}

#[test]
fn a_slow_detail_capture_does_not_delay_portal_publication() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;
    let native = Native::new();
    let source = Arc::clone(&native.raw);
    let (entered, started) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let first = AtomicBool::new(true);
    let socket = fake::Server::start(move |method, _| match method {
        "agent.list" => json!({"result":{"agents":[source.lock().unwrap().clone()]}}),
        "pane.read" => {
            if first.swap(false, Ordering::AcqRel) {
                entered.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(3)).unwrap();
            }
            json!({"result":{"read":{"text":"ready","revision":1}}})
        }
        other => panic!("unexpected method {other}"),
    });
    let fixture = Fixture::new(socket.client());
    let _viewer = fixture.client.detail("w1:p1").unwrap();
    started.recv_timeout(Duration::from_secs(3)).unwrap();
    let previous = fixture.client.current().unwrap().revision;
    fixture.republish();
    let frame = fixture.wait(|frame| frame.revision > previous);
    let portal: portal_types::PortalSnapshot = read::view(&frame, "portal").unwrap();
    assert!(portal.board.is_some());
    assert!(portal.agents.is_some());
    assert!(portal.inbox.is_some());
    release.send(()).unwrap();
    fixture.wait(available);
}

#[test]
fn json_escaping_uses_the_derived_wire_ceiling_and_read_limit_is_unchanged() {
    let codec = Codec::new(4096);
    let content = "\u{0}".repeat(4096);
    let encoded = codec.encode(&content).unwrap();
    assert!(encoded.len() > 4096 * 6);
    assert!(encoded.len() <= codec.wire_limit());
    let decoded: String = Codec::decode(&encoded, codec.wire_limit()).unwrap();
    assert_eq!(decoded, content);
    let native = Native::new();
    let path = native.directory.join("limit.jsonl");
    let line = json!({"type":"message","message":{"role":"user","content":[{"type":"text","text":"bounded turn"}]}}).to_string() + "\n";
    std::fs::write(&path, &line).unwrap();
    let path = path.to_str().unwrap();
    assert_eq!(
        crate::activity::conversation_with_limit("pi", path, line.len() as u64)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        crate::activity::conversation_with_limit("pi", path, line.len() as u64 - 1).unwrap(),
        Vec::<portal_types::agents::Turn>::new()
    );
}

#[test]
fn disconnected_interest_releases_without_a_lease_timeout() {
    let native = Native::new();
    let fixture = Fixture::new(native.server.client());
    let viewer = fixture.client.detail("w1:p1").unwrap();
    fixture.wait(available);
    drop(viewer);
    fixture.wait(|frame| detail(frame).is_none());
    assert!(fixture.no_viewers());
    assert!(!fixture.tick("w1:p1"));
    assert_eq!(native.reads(), 1);
    assert!(
        ipc::Client::at(&native.directory.join("not-serving"))
            .detail("w1:p1")
            .is_err()
    );
}
