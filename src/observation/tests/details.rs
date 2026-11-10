use super::super::details::{
    Capture, Codec, Content, ReadState, Reference,
    fixture::{Fixture, GitShell, Repository},
};
use super::super::{Published, ipc, peers, read};
use crate::herdr_socket::fake;
use crate::probe::Agent;
use portal_types::files::FilesState;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    detail(frame).is_some_and(|detail| {
        matches!(detail.screen, ReadState::Available | ReadState::Empty)
            && !matches!(detail.files, ReadState::Pending)
    })
}

#[test]
fn detail_subscribers_receive_worker_captures_without_another_publication() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    let native = Native::new();
    let source = Arc::clone(&native.raw);
    let screen = Arc::clone(&native.screen);
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
            json!({"result":{"read":screen.lock().unwrap().clone()}})
        }
        other => panic!("unexpected method {other}"),
    });
    let fixture = Fixture::paused(socket.client());
    let publication = fixture.client.current().unwrap();
    let mut viewer = fixture.client.detail("w1:p1").unwrap();
    started.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(matches!(
        viewer.current().publication.detail.conversation,
        Content::Pending
    ));
    let mut second = fixture.client.detail("w1:p1").unwrap();
    release.send(()).unwrap();
    for viewer in [&mut viewer, &mut second] {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let connection = viewer.current();
            assert_eq!(connection.status, ipc::Status::Connected);
            assert_eq!(connection.publication.revision, publication.revision);
            if let Content::Available(conversation) = &connection.publication.detail.conversation {
                assert_eq!(conversation.turns[0].text, "original task");
                break;
            }
            assert!(Instant::now() < deadline, "worker detail was not delivered");
            viewer.changed(Duration::from_millis(100));
        }
    }
    assert_eq!(fixture.collector.lock().unwrap().transcript_reads(), 1);
    *native.screen.lock().unwrap() = json!({"text":"direct update", "revision":8});
    assert!(fixture.tick("w1:p1"));
    let deadline = Instant::now() + Duration::from_secs(3);
    while !matches!(&viewer.current().publication.detail.screen, Content::Available(screen) if screen.revision == 8)
    {
        assert!(Instant::now() < deadline, "next capture was not delivered");
        viewer.changed(Duration::from_millis(100));
    }
    assert_eq!(fixture.client.current().unwrap(), publication);
    drop(second);
    drop(viewer);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !fixture.no_viewers() {
        assert!(Instant::now() < deadline, "detail lease was not released");
        std::thread::yield_now();
    }
    assert!(!fixture.tick("w1:p1"));
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
    let original = detail(&first).unwrap();
    let updated = detail(&changed).unwrap();
    assert_eq!(updated.parts.conversation, original.parts.conversation);
    assert!(updated.parts.screen > original.parts.screen);
    assert_eq!(native.reads(), 2);
    fixture.tick("w1:p1");
    assert_eq!(fixture.client.current().unwrap().revision, changed.revision);
    let export = serde_json::to_string(&peers::Export::of(&changed)).unwrap();
    assert!(!export.contains("changed screen"));
    assert!(!export.contains("original task"));
    assert!(!export.contains("agent_details"));
}

#[test]
fn transcript_change_advances_only_the_conversation_revision() {
    let native = Native::new();
    let fixture = Fixture::new(native.server.client());
    let _viewer = fixture.client.detail("w1:p1").unwrap();
    let first = detail(&fixture.wait(available)).unwrap();
    std::fs::write(
        native.agent().transcript.unwrap(),
        "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"changed task\"}}\n",
    )
    .unwrap();
    assert!(fixture.tick("w1:p1"));
    let changed = fixture.wait(|frame| {
        detail(frame).is_some_and(|detail| detail.parts.conversation > first.parts.conversation)
    });
    let updated = detail(&changed).unwrap();
    assert_eq!(updated.parts.screen, first.parts.screen);
    assert_eq!(updated.parts.conversation, updated.revision);
    assert_eq!(fixture.collector.lock().unwrap().transcript_reads(), 2);
}

#[test]
fn file_edit_changes_only_files_and_unchanged_tree_does_not_publish() {
    let native = Native::new();
    let repo = Repository::new();
    native.raw.lock().unwrap()["cwd"] = json!(repo.0);
    let fixture = Fixture::new(native.server.client());
    let mut viewer = fixture.client.detail("w1:p1").unwrap();
    let first = fixture.wait(|frame| {
        available(frame) && detail(frame).is_some_and(|detail| detail.files == ReadState::Empty)
    });
    let original = detail(&first).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !matches!(
        viewer.current().publication.detail.files,
        FilesState::Current(_)
    ) {
        assert!(Instant::now() < deadline);
        viewer.changed(Duration::from_millis(10));
    }
    std::fs::write(repo.0.join("file"), "edited\n").unwrap();
    assert!(fixture.files_tick("w1:p1"));
    let changed = fixture.wait(|frame| {
        detail(frame).is_some_and(|detail| detail.parts.files > original.parts.files)
    });
    let updated = detail(&changed).unwrap();
    assert_eq!(updated.parts.conversation, original.parts.conversation);
    assert_eq!(updated.parts.screen, original.parts.screen);
    assert_eq!(updated.files, ReadState::Available);
    while viewer.current().publication.parts.as_ref() != Some(&updated.parts) {
        assert!(Instant::now() < deadline);
        viewer.changed(Duration::from_millis(10));
    }
    let snapshot = viewer.current().publication.detail.files.clone();
    assert!(fixture.files_tick("w1:p1"));
    fixture.wake();
    assert_eq!(fixture.client.current().unwrap().revision, changed.revision);
    assert_eq!(viewer.current().publication.detail.files, snapshot);
}

#[test]
fn slow_git_does_not_delay_transcript_or_screen_publication() {
    let native = Native::new();
    let repo = Repository::new();
    native.raw.lock().unwrap()["cwd"] = json!(repo.0);
    let shell = Arc::new(GitShell::default());
    let (entered, release) = shell.hold();
    let fixture = Fixture::with_files_shell(native.server.client(), shell);
    let mut viewer = fixture.client.detail("w1:p1").unwrap();
    entered.recv_timeout(Duration::from_secs(3)).unwrap();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        release.send(()).unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(1);
    while matches!(
        viewer.current().publication.detail.conversation,
        Content::Pending
    ) {
        assert!(Instant::now() < deadline);
        viewer.changed(Duration::from_millis(10));
    }
    assert_eq!(
        viewer.current().publication.detail.files,
        FilesState::Pending
    );
    std::fs::write(native.agent().transcript.unwrap(),
        "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"Git is still running\"}}\n").unwrap();
    *native.screen.lock().unwrap() = json!({"text":"updated while Git waits", "revision":8});
    let start = Instant::now();
    fixture.tick("w1:p1");
    loop {
        let frame = viewer.current().publication;
        if matches!(&frame.detail.conversation, Content::Available(value) if value.turns[0].text == "Git is still running")
        {
            assert!(
                matches!(&frame.detail.screen, Content::Available(value) if value.revision == 8)
            );
            assert_eq!(frame.detail.files, FilesState::Pending);
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "Git blocked the transcript"
        );
        viewer.changed(Duration::from_millis(10));
    }
    assert!(start.elapsed() < Duration::from_secs(1));
    releaser.join().unwrap();
}

#[test]
fn replaced_session_starts_pending_and_discards_the_old_git_result() {
    let native = Native::new();
    let repo = Repository::new();
    native.raw.lock().unwrap()["cwd"] = json!(repo.0);
    let shell = Arc::new(GitShell::default());
    let (old_entered, old_release) = shell.hold();
    let fixture = Fixture::with_files_shell(native.server.client(), shell.clone());
    let mut viewer = fixture.client.detail("w1:p1").unwrap();
    old_entered.recv_timeout(Duration::from_secs(3)).unwrap();
    let old_session = viewer.current().publication.detail.session.clone();
    let (new_entered, new_release) = shell.hold();
    std::fs::write(repo.0.join("new-session-only"), "new").unwrap();
    native.raw.lock().unwrap()["terminal_id"] = json!("replacement-terminal");
    let start = Instant::now();
    fixture.wake();
    new_entered.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "retirement waited for old Git"
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while viewer.current().publication.detail.session == old_session {
        assert!(Instant::now() < deadline);
        viewer.changed(Duration::from_millis(10));
    }
    assert_eq!(
        viewer.current().publication.detail.files,
        FilesState::Pending
    );
    old_release.send(()).unwrap();
    fixture.tick("w1:p1");
    assert_eq!(
        viewer.current().publication.detail.files,
        FilesState::Pending
    );
    new_release.send(()).unwrap();
    loop {
        let frame = viewer.current().publication;
        if let FilesState::Current(files) = &frame.detail.files {
            assert_eq!(files.entries.len(), 1);
            assert_eq!(files.entries[0].path.text, "new-session-only");
            break;
        }
        assert!(Instant::now() < deadline);
        viewer.changed(Duration::from_millis(10));
    }
}

#[test]
fn failed_git_keeps_only_a_snapshot_from_the_same_checkout() {
    let native = Native::new();
    let repo = Repository::new();
    let other = Repository::new();
    native.raw.lock().unwrap()["cwd"] = json!(repo.0);
    let shell = Arc::new(GitShell::default());
    let fixture = Fixture::with_files_shell(native.server.client(), shell.clone());
    let mut viewer = fixture.client.detail("w1:p1").unwrap();
    fixture.wait(available);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !matches!(
        viewer.current().publication.detail.files,
        FilesState::Current(_)
    ) {
        assert!(Instant::now() < deadline);
        viewer.changed(Duration::from_millis(10));
    }
    let FilesState::Current(original) = viewer.current().publication.detail.files.clone() else {
        unreachable!()
    };
    shell.fail.store(true, Ordering::Release);
    fixture.files_tick("w1:p1");
    while !matches!(
        viewer.current().publication.detail.files,
        FilesState::Stale { .. }
    ) {
        assert!(Instant::now() < deadline);
        viewer.changed(Duration::from_millis(10));
    }
    assert_eq!(
        viewer.current().publication.detail.files,
        FilesState::Stale {
            files: original,
            error: "fixture Git failure".into()
        }
    );
    let subdir = repo.0.join("subdirectory");
    std::fs::create_dir(&subdir).unwrap();
    native.raw.lock().unwrap()["cwd"] = json!(subdir);
    fixture.wake();
    fixture.wait(|frame| frame.local["terminals"]["agents"][0]["cwd"] == json!(subdir));
    fixture.tick("w1:p1");
    fixture.wait(|frame| detail(frame).is_some_and(|detail| detail.files == ReadState::Pending));
    fixture.files_tick("w1:p1");
    while !matches!(&viewer.current().publication.detail.files,
        FilesState::Stale { files, .. } if files.root == repo.0.to_str().unwrap())
    {
        assert!(Instant::now() < deadline);
        viewer.changed(Duration::from_millis(10));
    }
    native.raw.lock().unwrap()["cwd"] = json!(other.0);
    fixture.wake();
    fixture.wait(|frame| frame.local["terminals"]["agents"][0]["cwd"] == json!(other.0));
    fixture.files_tick("w1:p1");
    while !matches!(
        viewer.current().publication.detail.files,
        FilesState::Unavailable(_)
    ) {
        assert!(Instant::now() < deadline);
        viewer.changed(Duration::from_millis(10));
    }
    assert_eq!(
        viewer.current().publication.detail.files,
        FilesState::Unavailable("fixture Git failure".into())
    );
}

#[test]
fn too_many_replaces_the_previous_list_instead_of_retaining_stale_files() {
    let native = Native::new();
    let repo = Repository::new();
    native.raw.lock().unwrap()["cwd"] = json!(repo.0);
    let shell = Arc::new(GitShell::default());
    let fixture = Fixture::with_files_shell(native.server.client(), shell.clone());
    let mut viewer = fixture.client.detail("w1:p1").unwrap();
    fixture.wait(|frame| detail(frame).is_some_and(|detail| detail.files == ReadState::Empty));
    shell.too_many.store(true, Ordering::Release);
    fixture.files_tick("w1:p1");
    let deadline = Instant::now() + Duration::from_secs(3);
    while viewer.current().publication.detail.files != FilesState::TooMany {
        assert!(
            Instant::now() < deadline,
            "TooMany must replace the previous snapshot"
        );
        viewer.changed(Duration::from_millis(10));
    }
}

#[test]
fn symlink_retarget_does_not_inherit_another_roots_stale_files() {
    use std::os::unix::fs::symlink;
    let native = Native::new();
    let repo = Repository::new();
    let other = Repository::new();
    let link = native.directory.join("checkout");
    symlink(&repo.0, &link).unwrap();
    native.raw.lock().unwrap()["cwd"] = json!(link);
    let shell = Arc::new(GitShell::default());
    let fixture = Fixture::with_files_shell(native.server.client(), shell.clone());
    let mut viewer = fixture.client.detail("w1:p1").unwrap();
    fixture.wait(|frame| detail(frame).is_some_and(|detail| detail.files == ReadState::Empty));
    shell.fail.store(true, Ordering::Release);
    std::fs::remove_file(&link).unwrap();
    symlink(&other.0, &link).unwrap();
    fixture.files_tick("w1:p1");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !matches!(
        viewer.current().publication.detail.files,
        FilesState::Unavailable(_)
    ) {
        assert!(Instant::now() < deadline);
        viewer.changed(Duration::from_millis(10));
    }
    shell.fail.store(false, Ordering::Release);
    fixture.files_tick("w1:p1");
    loop {
        if let FilesState::Current(files) = &viewer.current().publication.detail.files {
            assert_eq!(files.root, other.0.to_str().unwrap());
            break;
        }
        assert!(Instant::now() < deadline);
        viewer.changed(Duration::from_millis(10));
    }
}

#[test]
fn older_part_revisions_default_files_to_zero() {
    let revisions: portal_types::agents::DetailRevisions =
        serde_json::from_value(json!({"conversation":4,"screen":7})).unwrap();
    assert_eq!(revisions.files, 0);
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
    assert!(detail.parts.conversation > old.parts.conversation);
    assert!(detail.parts.screen > old.parts.screen);
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
async fn conversation_content_loads_only_through_detail_route() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    let native = Native::new();
    let fixture = Fixture::new(native.server.client());
    let _viewer = fixture.client.detail("w1:p1").unwrap();
    let small = fixture.wait(available);
    let previous = detail(&small).unwrap().revision;
    let text = "x".repeat(4 * 1024);
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
            .uri("/api/agents/local/w1:p1/conversation")
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

#[tokio::test]
async fn detail_codec_and_route_body_limit_accept_content_over_main_frame_limit() {
    let codec = super::super::details::CODEC;
    assert!(codec.wire_limit() > ipc::MAX_FRAME_BYTES);
    let text = "x".repeat(ipc::MAX_FRAME_BYTES + 1024);
    let encoded = codec.encode(&text).unwrap();
    assert!(encoded.len() > ipc::MAX_FRAME_BYTES);
    // Match the response body limit in web::agents::peer_read.
    let bytes = axum::body::to_bytes(axum::body::Body::from(encoded), codec.wire_limit())
        .await
        .unwrap();
    let decoded: String = Codec::decode(&bytes, codec.wire_limit()).unwrap();
    assert_eq!(decoded.len(), text.len());
    assert_eq!(decoded, text);
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
    fixture.rename_agent("renamed while capture is blocked");
    let frame = fixture.wait(|frame| frame.revision > previous);
    let portal: portal_types::PortalSnapshot = read::view(&frame, "portal").unwrap();
    assert!(portal.board.is_some());
    assert_eq!(
        portal.agents.unwrap()[0].name.as_deref(),
        Some("renamed while capture is blocked")
    );
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
    let complete = crate::activity::conversation_with_limit("pi", path, line.len() as u64).unwrap();
    assert_eq!(complete.turns.len(), 1);
    assert!(!complete.truncated);
    let cut = crate::activity::conversation_with_limit("pi", path, line.len() as u64 - 1).unwrap();
    assert_eq!(cut.turns, Vec::<portal_types::agents::Turn>::new());
    assert!(cut.truncated);
}

#[test]
fn conversation_capture_preserves_session_metadata_and_truncation() {
    let native = Native::new();
    let agent = native.agent();
    let mut transcript = String::new();
    for index in 0..201 {
        transcript.push_str(&json!({"type":"message","id":format!("message-{index}"),"message":{"role":"assistant","timestamp":42,"model":"recorded-model","provider":"recorded-provider","content":"reply"}}).to_string());
        transcript.push('\n');
    }
    std::fs::write(agent.transcript.as_ref().unwrap(), transcript).unwrap();
    let mut capture = Capture::default();
    let detail = capture.tick(&agent, Ok(&native.server.client()));
    let Content::Available(conversation) = &detail.conversation else {
        panic!("expected conversation, got {:?}", detail.conversation);
    };
    assert_eq!(conversation.session, detail.key().unwrap());
    assert_eq!(conversation.pane, agent.pane);
    assert!(conversation.truncated);
    assert_eq!(conversation.turns.len(), 200);
    assert_eq!(conversation.turns[0].id, "message-1");
    let encoded = super::super::details::CODEC.encode(&detail).unwrap();
    let decoded: super::super::details::Detail =
        Codec::decode(&encoded, super::super::details::CODEC.wire_limit()).unwrap();
    assert_eq!(decoded, detail);
    let wire: Value = serde_json::from_slice(&encoded).unwrap();
    let turn = &wire["conversation"]["data"]["turns"][0];
    assert_eq!(turn["timestamp"], json!({"ms":42,"meaning":"message"}));
    assert_eq!(
        turn["source"],
        json!({"kind":"harness","harness":"pi","model":"recorded-model","provider":"recorded-provider"})
    );
    assert_eq!(capture.tick(&agent, Ok(&native.server.client())), detail);
    assert_eq!(capture.transcript_reads, 1);
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
