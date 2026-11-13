use super::*;

#[test]
fn foreign_agent_cannot_satisfy_local_detail_or_activity_selection() {
    let mut publication = super::super::Published {
        version: VERSION,
        epoch: "serve".into(),
        revision: 1,
        published_ms: 1,
        local_revision: 1,
        sources: super::super::Sources::default(),
        local: super::super::Domains::new(),
        views: super::super::Domains::from([(
            "portal".into(),
            serde_json::json!({"data":{"agents":[
                {"host":"netcup","pane":"w1:p1","state":"working"}
            ]}}),
        )]),
        gaps: vec![],
    };
    assert!(selected_agent(&publication, "w1:p1").unwrap().is_none());
    publication.views.get_mut("portal").unwrap()["data"]["agents"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"host":"local","pane":"w1:p1","state":"idle"}));
    let agent = selected_agent(&publication, "w1:p1").unwrap().unwrap();
    assert_eq!(agent.host, "local");
    assert_eq!(agent.state, "idle");
}

/// A state path too long for a socket address (any macOS `$TMPDIR` test directory) with
/// no state directory reads as "not running", like a missing socket on a short path.
#[test]
fn a_long_missing_state_directory_reads_as_not_running() {
    let state = std::env::temp_dir()
        .join("x".repeat(120))
        .join(crate::ledger::new_id());
    let error = Client::at(&state)
        .current(&crate::observation::ipc::Select::All)
        .unwrap_err()
        .to_string();
    assert!(error.contains("dogsled serve is not running"), "{error}");
}

#[test]
fn publication_shutdown_unlocks_even_with_an_inherited_descriptor() {
    let state = std::env::temp_dir().join(crate::ledger::new_id());
    let publisher = super::super::Publisher::new().unwrap();
    let mut server = Server::new(&state).unwrap();
    server.bind(publisher.subscribe()).unwrap();
    // A duplicated descriptor shares the open-file description, as an inherited fork
    // descriptor does before exec. Closing only the parent's file cannot release its lock.
    let inherited = server.lock.try_clone().unwrap();
    drop(server);
    let restarted = Server::new(&state);
    drop(inherited);
    let mut server = restarted.unwrap();
    server.bind(publisher.subscribe()).unwrap();
    drop(server);
    std::fs::remove_dir_all(state).unwrap();
}

/// A publisher may send one final publication and close before the subscriber clears its
/// read timeout. Darwin then refuses the socket option (EINVAL); the delivered publication
/// is still authoritative and the subscription reports a disconnect instead of failing.
#[test]
fn a_publication_followed_by_an_immediate_close_is_delivered() {
    let state = PathBuf::from("/tmp").join(crate::ledger::new_id());
    std::fs::create_dir_all(&state).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(state.join(SOCKET)).unwrap();
    let publisher = std::thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(socket.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        let detail = super::super::details::Detail::unavailable(None, "agent unavailable");
        let mut frame = serde_json::to_vec(&serde_json::json!({
            "version": VERSION, "epoch": "e", "revision": 1, "found": false,
            "agent": null, "detail": detail,
        }))
        .unwrap();
        frame.push(b'\n');
        (&socket).write_all(&frame).unwrap();
        socket.shutdown(std::net::Shutdown::Both).unwrap();
    });
    let subscription = Client::at(&state).detail("w9:p9").unwrap();
    publisher.join().unwrap();
    assert!(!subscription.current().publication.found);
    drop(subscription);
    std::fs::remove_dir_all(state).unwrap();
}

fn activity_input() -> super::super::Publication {
    super::super::Publication {
        published_ms: 1000,
        sources: super::super::Sources {
            local: std::collections::BTreeMap::from([(
                "terminals".into(),
                crate::ledger::now_ms(),
            )]),
            peers: std::collections::BTreeMap::new(),
        },
        local: super::super::Domains::new(),
        views: super::super::Domains::from([(
            "portal".into(),
            serde_json::json!({
                "data": {"version":1,"epoch":"","revision":0,"published_ms":1000,
                    "board":null,"agents":[],"inbox":null,"gaps":[]}, "gaps":[]
            }),
        )]),
        gaps: vec![],
    }
}

#[test]
fn activity_refusals_preserve_typed_codes_without_native_reads() {
    use portal_types::agents::ActivityCode;
    let state = std::env::temp_dir().join(crate::ledger::new_id());
    let mut publisher = super::super::Publisher::new().unwrap();
    let mut server = Server::new(&state).unwrap();
    let mut activity =
        super::super::live_activity::ActivityPublisher::new(publisher.epoch().into());
    let authority = super::super::live_activity::Authority::new(publisher.epoch().into());
    server.activity(authority.commands(), activity.sender.subscribe());
    server.bind(publisher.subscribe()).unwrap();
    let client = Client::at(&state);
    publisher.replace(activity_input()).unwrap();
    activity
        .replace(
            std::collections::BTreeMap::new(),
            Some(crate::ledger::now_ms()),
            std::collections::BTreeSet::new(),
        )
        .unwrap();
    assert_eq!(
        client.activity("absent").unwrap_err().code,
        ActivityCode::AgentMissing
    );
    assert_eq!(
        client.activity_subscribe("absent").unwrap_err().code,
        ActivityCode::AgentMissing
    );
    assert_eq!(
        client.activity("").unwrap_err().code,
        ActivityCode::InvalidRequest
    );
    activity
        .replace(
            std::collections::BTreeMap::new(),
            Some(1),
            std::collections::BTreeSet::new(),
        )
        .unwrap();
    assert_eq!(
        client.activity("absent").unwrap_err().code,
        ActivityCode::UpstreamUnavailable
    );
    drop(server);
    assert_eq!(
        client.activity("absent").unwrap_err().code,
        ActivityCode::UpstreamUnavailable
    );
    std::fs::remove_dir_all(state).unwrap();
}

#[test]
fn retained_activity_never_renews_its_relative_validity_or_leaks_into_general_frames() {
    use portal_types::agents::{
        ActivityDetail, ActivityProducer, ActivityView, LivePhase, LiveState,
    };
    let state = std::env::temp_dir().join(crate::ledger::new_id());
    let mut publisher = super::super::Publisher::new().unwrap();
    let mut server = Server::new(&state).unwrap();
    let mut activity =
        super::super::live_activity::ActivityPublisher::new(publisher.epoch().into());
    let authority = super::super::live_activity::Authority::new(publisher.epoch().into());
    server.activity(authority.commands(), activity.sender.subscribe());
    server.bind(publisher.subscribe()).unwrap();
    let client = Client::at(&state);
    activity
        .replace(
            std::collections::BTreeMap::from([(
                "pane".into(),
                ActivityView {
                    version: 1,
                    epoch: publisher.epoch().into(),
                    revision: 0,
                    pane: "pane".into(),
                    session: Some("native-key".into()),
                    coarse_state: Some("working".into()),
                    producer: ActivityProducer {
                        generation: 1,
                        id: Some("producer".into()),
                    },
                    detail: ActivityDetail::Available {
                        sequence: 1,
                        received_at_ms: 1000,
                        valid_for_ms: 10_000,
                        state: LiveState {
                            reasoning: None,
                            phase: LivePhase::Thinking,
                            active_tools: vec![],
                        },
                    },
                },
            )]),
            Some(crate::ledger::now_ms()),
            std::collections::BTreeSet::new(),
        )
        .unwrap();
    publisher.replace(activity_input()).unwrap();
    let frame = publisher.sender.borrow().clone().unwrap();
    let activity_frame = activity.sender.borrow().clone();
    activity
        .sender
        .send_replace(Arc::new(super::super::live_activity::ActivityFrame {
            views: activity_frame.views.clone(),
            bound: activity_frame.bound.clone(),
            bindings_measured_ms: activity_frame.bindings_measured_ms,
            measured: std::time::Instant::now()
                .checked_sub(Duration::from_secs(2))
                .unwrap(),
        }));
    let first = client.activity("pane").unwrap();
    assert!(matches!(
        first.detail,
        ActivityDetail::Available {
            valid_for_ms: 1..=8000,
            ..
        }
    ));
    assert!(
        !serde_json::to_string(
            &client
                .current(&crate::observation::ipc::Select::All)
                .unwrap()
        )
        .unwrap()
        .contains("active_tools")
    );
    assert!(
        !serde_json::to_string(&super::super::peers::Export::of(&frame.published))
            .unwrap()
            .contains("active_tools")
    );
    activity
        .sender
        .send_replace(Arc::new(super::super::live_activity::ActivityFrame {
            views: activity_frame.views.clone(),
            bound: activity_frame.bound.clone(),
            bindings_measured_ms: activity_frame.bindings_measured_ms,
            measured: std::time::Instant::now()
                .checked_sub(Duration::from_secs(10))
                .unwrap(),
        }));
    assert!(matches!(
        client.activity("pane").unwrap().detail,
        ActivityDetail::Unavailable { .. }
    ));
    drop(server);
    std::fs::remove_dir_all(state).unwrap();
}
