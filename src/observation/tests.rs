mod actions;
mod context_fixture;
mod details;
mod local;
mod peer;
mod projection;
mod projection_import;
mod unreadable_hold;

mod publication {
    use super::super::*;
    use std::io::{BufRead as _, BufReader, Read as _, Write as _};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::time::Instant;

    use crate::test_support::WAIT_DEADLINE as WAIT;

    struct Fixture {
        publisher: Publisher,
        server: Option<ipc::Server>,
        client: ipc::Client,
        state: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let state = std::env::temp_dir().join(crate::ledger::new_id());
            let publisher = Publisher::new().unwrap();
            let mut server = ipc::Server::new(&state).unwrap();
            server.bind(publisher.subscribe()).unwrap();
            let client = ipc::Client::at(&state);
            Self {
                publisher,
                server: Some(server),
                client,
                state,
            }
        }

        fn publish(&mut self, value: u64) -> Arc<Published> {
            self.publisher.replace(input(value)).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.server.take();
            std::fs::remove_dir_all(&self.state).unwrap();
        }
    }

    fn input(value: u64) -> Publication {
        let payload = serde_json::json!({"value": value, "foreign_path": "/foreign/host/tree"});
        Publication {
            published_ms: value,
            sources: Sources {
                local: BTreeMap::from([("host".into(), value)]),
                peers: BTreeMap::new(),
            },
            local: BTreeMap::from([("host".into(), payload.clone())]),
            views: BTreeMap::from([("status".into(), payload)]),
            gaps: vec![Gap {
                domain: format!("detail-{value}"),
                availability: Availability::Unavailable,
                detail: "not collected".into(),
            }],
        }
    }

    fn consistent(value: &Published) {
        assert_eq!(value.version, VERSION);
        assert_eq!(value.local["host"], value.views["status"]);
        assert_eq!(value.local["host"]["value"], value.published_ms);
        assert_eq!(value.local["host"]["foreign_path"], "/foreign/host/tree");
        assert_eq!(value.sources.local["host"], value.published_ms);
        assert_eq!(
            value.gaps[0].domain,
            format!("detail-{}", value.published_ms)
        );
    }

    #[test]
    fn identical_replacement_retains_the_frame_without_notifying_subscribers() {
        let mut publisher = Publisher::new().unwrap();
        let first = publisher.replace(input(1)).unwrap();
        let receiver = publisher.subscribe();
        let frame = Arc::clone(receiver.borrow().as_ref().unwrap());
        let repeated = publisher.replace(input(1)).unwrap();
        assert!(Arc::ptr_eq(&first, &repeated));
        assert!(Arc::ptr_eq(&frame, receiver.borrow().as_ref().unwrap()));
        assert_eq!(
            frame.published.published_ms,
            receiver.borrow().as_ref().unwrap().published.published_ms
        );
        assert!(!receiver.has_changed().unwrap());
    }

    #[test]
    fn publication_time_alone_does_not_publish() {
        let mut publisher = Publisher::new().unwrap();
        let first = publisher.replace(input(1)).unwrap();
        let receiver = publisher.subscribe();
        let mut later = input(1);
        later.published_ms = 2;
        let repeated = publisher.replace(later).unwrap();
        assert!(Arc::ptr_eq(&first, &repeated));
        assert_eq!(repeated.published_ms, 1);
        assert!(!receiver.has_changed().unwrap());
    }

    #[test]
    fn portal_metadata_is_excluded_but_portal_content_and_wrapper_are_compared() {
        let mut publisher = Publisher::new().unwrap();
        let mut initial = input(1);
        initial.views.insert(
            "portal".into(),
            serde_json::json!({"data": {
                "version": portal_types::PORTAL_VERSION, "epoch": "", "revision": 0, "published_ms": 0,
                "board": null, "agents": null, "inbox": null, "repos": null, "gaps": []
            }, "gaps": []}),
        );
        let first = publisher.replace(initial.clone()).unwrap();
        let mut receiver = publisher.subscribe();
        let mut later = initial.clone();
        later.published_ms = 2;
        let portal = later.views.get_mut("portal").unwrap();
        portal["data"]["epoch"] = "ignored".into();
        portal["data"]["revision"] = 99.into();
        portal["data"]["published_ms"] = 99.into();
        assert!(Arc::ptr_eq(&first, &publisher.replace(later).unwrap()));
        assert!(!receiver.has_changed().unwrap());
        initial.views.get_mut("portal").unwrap()["data"]["agents"] = serde_json::json!([]);
        let changed = publisher.replace(initial.clone()).unwrap();
        assert_eq!(changed.revision, first.revision + 1);
        assert!(receiver.has_changed().unwrap());
        receiver.borrow_and_update();
        initial.views.get_mut("portal").unwrap()["gaps"] = serde_json::json!([{
            "domain": "agents", "availability": "failed", "detail": "failed capture"
        }]);
        assert_eq!(
            publisher.replace(initial).unwrap().revision,
            changed.revision + 1
        );
        assert!(receiver.has_changed().unwrap());
    }

    #[test]
    fn each_substantive_publication_change_advances_the_revision() {
        let initial = input(1);
        let mut cases = Vec::new();
        let mut measured = initial.clone();
        measured.sources.local.insert("host".into(), 2);
        cases.push(("measurement", measured, false));
        let mut gap = initial.clone();
        gap.gaps[0].detail = "capture failed".into();
        cases.push(("gap", gap, false));
        let mut view = initial.clone();
        view.views
            .insert("status".into(), serde_json::json!({"value": 2}));
        cases.push(("view", view, false));
        let mut local = initial.clone();
        local
            .local
            .insert("host".into(), serde_json::json!({"value": 2}));
        cases.push(("local", local, true));
        for (name, changed, local_changed) in cases {
            let mut publisher = Publisher::new().unwrap();
            let first = publisher.replace(initial.clone()).unwrap();
            let receiver = publisher.subscribe();
            let next = publisher.replace(changed).unwrap();
            assert_eq!(next.revision, first.revision + 1, "{name}");
            assert_eq!(
                next.local_revision,
                first.local_revision + u64::from(local_changed),
                "{name}"
            );
            assert!(receiver.has_changed().unwrap(), "{name}");
        }
    }

    #[test]
    fn every_peer_source_field_is_publication_content() {
        let mut initial = input(1);
        let peer = PeerSource {
            epoch: "peer-epoch".into(),
            local_revision: 1,
            received_ms: 1,
            retry_tokens: vec![],
            availability: Availability::Available,
        };
        initial.sources.peers.insert("peer".into(), peer.clone());
        let sources = [
            PeerSource {
                epoch: "restarted".into(),
                ..peer.clone()
            },
            PeerSource {
                local_revision: 2,
                ..peer.clone()
            },
            PeerSource {
                received_ms: 2,
                ..peer.clone()
            },
            PeerSource {
                retry_tokens: vec!["retry-ack".into()],
                ..peer.clone()
            },
            PeerSource {
                availability: Availability::Stale,
                ..peer
            },
        ];
        for peer in sources {
            let mut publisher = Publisher::new().unwrap();
            let first = publisher.replace(initial.clone()).unwrap();
            let receiver = publisher.subscribe();
            let mut changed = initial.clone();
            changed.sources.peers.insert("peer".into(), peer);
            let next = publisher.replace(changed).unwrap();
            assert_eq!(next.revision, first.revision + 1);
            assert_eq!(next.local_revision, first.local_revision);
            assert!(receiver.has_changed().unwrap());
        }
    }

    fn activity(received_at_ms: u64, valid_for_ms: u64) -> portal_types::agents::ActivityView {
        use portal_types::agents::{
            ActivityDetail, ActivityProducer, ActivityView, LivePhase, LiveState,
        };
        ActivityView {
            version: 1,
            pane: "w1:p1".into(),
            epoch: String::new(),
            revision: 0,
            session: Some("session".into()),
            coarse_state: Some("working".into()),
            producer: ActivityProducer {
                generation: 1,
                id: Some("producer".into()),
            },
            detail: ActivityDetail::Available {
                sequence: 1,
                received_at_ms,
                valid_for_ms,
                state: LiveState {
                    phase: LivePhase::Thinking,
                    active_tools: vec![],
                    reasoning: None,
                },
            },
        }
    }

    fn revision(frame: &live_activity::ActivityFrame) -> u64 {
        serde_json::from_slice::<portal_types::agents::ActivityView>(&frame.views["w1:p1"])
            .unwrap()
            .revision
    }

    #[test]
    fn unchanged_activity_and_a_falling_validity_budget_keep_the_existing_frame() {
        let mut publisher = live_activity::ActivityPublisher::new("serve".into());
        let first = publisher
            .replace(
                BTreeMap::from([("w1:p1".into(), activity(1000, 9000))]),
                Some(1000),
                std::collections::BTreeSet::new(),
            )
            .unwrap();
        let receiver = publisher.sender.subscribe();
        let frame = Arc::clone(&receiver.borrow());
        assert!(Arc::ptr_eq(
            &first,
            &publisher
                .replace(
                    BTreeMap::from([("w1:p1".into(), activity(1000, 9000))]),
                    Some(1000),
                    std::collections::BTreeSet::new()
                )
                .unwrap()
        ));
        let mut later = activity(1000, 8000);
        later.epoch = "ignored".into();
        later.revision = 99;
        let repeated = publisher
            .replace(
                BTreeMap::from([("w1:p1".into(), later)]),
                Some(1000),
                std::collections::BTreeSet::new(),
            )
            .unwrap();
        assert!(Arc::ptr_eq(&first, &repeated));
        assert!(Arc::ptr_eq(&frame, &receiver.borrow()));
        assert_eq!(frame.measured, receiver.borrow().measured);
        assert!(!receiver.has_changed().unwrap());
    }

    #[test]
    fn activity_renewal_publishes_even_when_the_remaining_budget_is_unchanged() {
        let mut main = Publisher::new().unwrap();
        let general = main.replace(input(1)).unwrap();
        let mut publisher = live_activity::ActivityPublisher::new(main.epoch().into());
        let first = publisher
            .replace(
                BTreeMap::from([("w1:p1".into(), activity(1000, 9000))]),
                Some(1000),
                std::collections::BTreeSet::new(),
            )
            .unwrap();
        let receiver = publisher.sender.subscribe();
        let renewed = publisher
            .replace(
                BTreeMap::from([("w1:p1".into(), activity(2000, 9000))]),
                Some(1000),
                std::collections::BTreeSet::new(),
            )
            .unwrap();
        assert_eq!(revision(&renewed), revision(&first) + 1);
        assert_eq!(
            main.current().unwrap().local_revision,
            general.local_revision
        );
        assert!(receiver.has_changed().unwrap());
    }

    #[test]
    fn activity_expiry_and_removal_are_content_changes() {
        let mut publisher = live_activity::ActivityPublisher::new("serve".into());
        let first = publisher
            .replace(
                BTreeMap::from([("w1:p1".into(), activity(1000, 9000))]),
                Some(1000),
                std::collections::BTreeSet::new(),
            )
            .unwrap();
        let mut receiver = publisher.sender.subscribe();
        let mut expired = activity(1000, 0);
        expired.detail = portal_types::agents::ActivityDetail::Unavailable {
            reason: portal_types::agents::UnavailableReason::Expired,
        };
        let expired = publisher
            .replace(
                BTreeMap::from([("w1:p1".into(), expired)]),
                Some(1000),
                std::collections::BTreeSet::new(),
            )
            .unwrap();
        assert_eq!(revision(&expired), revision(&first) + 1);
        assert!(receiver.has_changed().unwrap());
        receiver.borrow_and_update();
        let expired_revision = revision(&expired);
        let removed = publisher
            .replace(
                BTreeMap::new(),
                Some(1000),
                std::collections::BTreeSet::new(),
            )
            .unwrap();
        assert!(removed.views.is_empty());
        assert_eq!(publisher.revision, expired_revision + 1);
        assert!(receiver.has_changed().unwrap());
    }

    #[test]
    fn atomic_replacement_preserves_old_references_and_wire() {
        let mut fixture = Fixture::new();
        let old = fixture.publish(1);
        let receiver = fixture.publisher.subscribe();
        let old_frame = receiver.borrow().as_ref().unwrap().clone();
        let new = fixture.publish(2);
        assert_eq!(old.revision, 1);
        assert_eq!(old.local["host"]["value"], 1);
        assert_eq!(new.revision, 2);
        let lines = old_frame.reply(None).unwrap();
        assert_eq!(lines.len(), 1);
        let wire: Published = serde_json::from_slice(&lines[0]).unwrap();
        assert_eq!(wire, *old);
        assert_eq!(
            fixture
                .client
                .current(&crate::observation::ipc::Select::All)
                .unwrap(),
            *new
        );
    }

    #[test]
    fn current_and_subscription_never_mix_concurrent_revisions() {
        let mut fixture = Fixture::new();
        fixture.publish(1);
        let client = fixture.client.clone();
        std::thread::scope(|scope| {
            let reader = scope.spawn(move || {
                let mut subscription = client
                    .subscribe(&crate::observation::ipc::Select::All)
                    .unwrap();
                for _ in 0..64 {
                    consistent(
                        &client
                            .current(&crate::observation::ipc::Select::All)
                            .unwrap(),
                    );
                    consistent(&subscription.current().publication);
                }
                let end = Instant::now() + WAIT;
                while subscription.current().publication.revision < 128 {
                    assert!(
                        Instant::now() < end,
                        "subscriber did not reach final revision"
                    );
                    let connection = subscription.changed(WAIT);
                    assert_eq!(connection.status, ipc::Status::Connected);
                    consistent(&connection.publication);
                }
            });
            for value in 2..=128 {
                fixture.publish(value);
            }
            reader.join().unwrap();
        });
    }

    #[test]
    fn restart_uses_new_epoch_and_disconnects_retained_state() {
        let mut fixture = Fixture::new();
        let old = fixture.publish(1);
        let mut subscription = fixture
            .client
            .subscribe(&crate::observation::ipc::Select::All)
            .unwrap();
        fixture.server.take();
        let retained = subscription.changed(WAIT);
        assert!(matches!(retained.status, ipc::Status::Disconnected(_)));
        assert_eq!(*retained.publication, *old);
        assert!(
            fixture
                .client
                .current(&crate::observation::ipc::Select::All)
                .unwrap_err()
                .to_string()
                .contains("unavailable")
        );
        fixture.publisher = Publisher::new().unwrap();
        let mut server = ipc::Server::new(&fixture.state).unwrap();
        server.bind(fixture.publisher.subscribe()).unwrap();
        fixture.server = Some(server);
        let new = fixture.publish(2);
        assert_ne!(old.epoch, new.epoch);
        assert_eq!(new.revision, 1);
        assert_eq!(
            fixture
                .client
                .current(&crate::observation::ipc::Select::All)
                .unwrap(),
            *new
        );
        assert!(matches!(
            subscription.current().status,
            ipc::Status::Disconnected(_)
        ));
    }

    #[test]
    fn slow_subscriber_skips_to_newest_complete_publication() {
        let mut fixture = Fixture::new();
        fixture.publish(1);
        let mut subscription = fixture
            .client
            .subscribe(&crate::observation::ipc::Select::All)
            .unwrap();
        let mut receiver = fixture.publisher.subscribe();
        receiver.borrow_and_update();
        for value in 2..=512 {
            fixture.publish(value);
        }
        // The producer-side delivery slot is replaceable, not a revision queue.
        assert_eq!(
            receiver
                .borrow_and_update()
                .as_ref()
                .unwrap()
                .published
                .revision,
            512
        );
        let end = Instant::now() + WAIT;
        loop {
            let state = subscription.changed(WAIT);
            assert_eq!(state.status, ipc::Status::Connected);
            consistent(&state.publication);
            if state.publication.revision == 512 {
                break;
            }
            assert!(
                Instant::now() < end,
                "slow subscriber did not receive newest revision"
            );
        }
        assert_eq!(subscription.current().publication.revision, 512);
    }

    #[test]
    fn socket_backpressure_skips_pending_revisions_without_blocking_publisher() {
        let mut fixture = Fixture::new();
        let mut large = input(1);
        large
            .views
            .insert("large".into(), Value::String("x".repeat(4 * 1024 * 1024)));
        fixture.publisher.replace(large).unwrap();
        let mut socket = UnixStream::connect(fixture.state.join("observation.sock")).unwrap();
        socket.set_read_timeout(Some(WAIT)).unwrap();
        socket
            .write_all(b"{\"version\":2,\"operation\":\"subscribe\"}\n")
            .unwrap();
        // Reading one byte confirms revision 1 is in flight, then stops draining the socket.
        let mut first = [0];
        socket.read_exact(&mut first).unwrap();
        let started = Instant::now();
        for value in 2..=32 {
            fixture.publish(value);
        }
        assert!(
            // Behaviour budget: only publication runs after backpressure is established.
            started.elapsed() < std::time::Duration::from_secs(3),
            "backpressure blocked the publisher"
        );
        // Revision 1 has several frames. All of them arrive before the next revision starts.
        let mut reader = BufReader::new(std::io::Cursor::new(first).chain(socket));
        let mut next = || -> Published {
            serde_json::from_value(pages::read(&mut reader, ipc::MAX_FRAME_BYTES).unwrap()).unwrap()
        };
        let old = next();
        assert_eq!(old.revision, 1);
        assert_eq!(old.views["large"].as_str().unwrap().len(), 4 * 1024 * 1024);
        let newest = next();
        assert_eq!(newest.revision, 32);
        consistent(&newest);
    }

    #[test]
    fn unavailable_without_publisher_does_not_create_state() {
        let state = std::env::temp_dir().join(crate::ledger::new_id());
        let client = ipc::Client::at(&state);
        assert!(
            client
                .current(&crate::observation::ipc::Select::All)
                .unwrap_err()
                .to_string()
                .contains("observation unavailable")
        );
        assert!(
            client
                .subscribe(&crate::observation::ipc::Select::All)
                .unwrap_err()
                .to_string()
                .contains("observation unavailable")
        );
        assert!(!state.exists());
    }

    #[test]
    fn unavailable_before_first_publication_is_not_empty_success() {
        let fixture = Fixture::new();
        assert!(
            fixture
                .client
                .current(&crate::observation::ipc::Select::All)
                .unwrap_err()
                .to_string()
                .contains("observation unavailable")
        );
        assert!(
            fixture
                .client
                .subscribe(&crate::observation::ipc::Select::All)
                .unwrap_err()
                .to_string()
                .contains("observation unavailable")
        );
    }

    #[test]
    fn peer_only_changes_do_not_increment_local_revision() {
        let mut fixture = Fixture::new();
        fixture.publish(1);
        let mut peer_change = input(1);
        peer_change.sources.peers.insert(
            "peer".into(),
            PeerSource {
                epoch: "remote-epoch".into(),
                local_revision: 8,
                received_ms: 44,
                retry_tokens: Vec::new(),
                availability: Availability::Stale,
            },
        );
        peer_change
            .views
            .insert("peer".into(), serde_json::json!({"available": false}));
        let new = fixture.publisher.replace(peer_change).unwrap();
        assert_eq!(new.revision, 2);
        assert_eq!(new.local_revision, 1);
        assert_eq!(new.sources.local["host"], 1);
        assert_eq!(new.sources.peers["peer"].received_ms, 44);
        assert_eq!(fixture.publish(2).local_revision, 2);
    }

    #[test]
    fn rejects_unsupported_and_malformed_requests() {
        let mut fixture = Fixture::new();
        fixture.publish(1);
        for (request, code) in [
            (
                "{\"version\":1,\"operation\":\"current\"}\n".to_owned(),
                "unsupported",
            ),
            ("not json\n".to_owned(), "invalid"),
            (format!("{}\n", "x".repeat(300)), "invalid"),
        ] {
            let mut socket = UnixStream::connect(fixture.state.join("observation.sock")).unwrap();
            socket.set_read_timeout(Some(WAIT)).unwrap();
            socket.write_all(request.as_bytes()).unwrap();
            let mut line = String::new();
            BufReader::new(socket).read_line(&mut line).unwrap();
            let response: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(response["error"]["code"], code);
        }
        consistent(
            &fixture
                .client
                .current(&crate::observation::ipc::Select::All)
                .unwrap(),
        );
    }

    #[test]
    fn single_endpoint_owner_and_clean_shutdown() {
        let mut fixture = Fixture::new();
        fixture.publish(1);
        assert!(ipc::Server::new(&fixture.state).is_err());
        consistent(
            &fixture
                .client
                .current(&crate::observation::ipc::Select::All)
                .unwrap(),
        );
        let _silent = UnixStream::connect(fixture.state.join("observation.sock")).unwrap();
        let started = Instant::now();
        fixture.server.take();
        // Behaviour budget starts after the server and its silent client exist.
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
        assert!(!fixture.state.join("observation.sock").exists());
    }

    #[test]
    fn dropping_publisher_closes_subscription() {
        let mut fixture = Fixture::new();
        fixture.publish(1);
        let mut subscription = fixture
            .client
            .subscribe(&crate::observation::ipc::Select::All)
            .unwrap();
        fixture.publisher = Publisher::new().unwrap();
        let state = subscription.changed(WAIT);
        assert!(matches!(state.status, ipc::Status::Disconnected(_)));
        assert_eq!(state.publication.revision, 1);
        assert!(
            fixture
                .client
                .current(&crate::observation::ipc::Select::All)
                .unwrap_err()
                .to_string()
                .contains("unavailable")
        );
    }

    /// More than 32 MiB: 36,000 records of about 1 KiB, one record body of 3 MiB, and a
    /// 2 MiB host export.
    fn large(value: u64) -> Publication {
        let mut large = input(value);
        let records: Vec<Value> = (0..36_000)
            .map(|index| {
                serde_json::json!({
                    "id": format!("r-{value}-{index}"),
                    "request": {"task": "t".repeat(900), "value": value},
                })
            })
            .collect();
        large.views.insert(
            "queue".into(),
            serde_json::json!({"data": {"tickets": records, "body": "b".repeat(3 * 1024 * 1024)}}),
        );
        large
            .local
            .insert("todos".into(), Value::String("h".repeat(2 * 1024 * 1024)));
        large
    }

    /// The lines of one `current` reply, as the socket carries them.
    fn lines(fixture: &Fixture, request: &str) -> Vec<Vec<u8>> {
        let mut socket = UnixStream::connect(fixture.state.join("observation.sock")).unwrap();
        socket.set_read_timeout(Some(WAIT)).unwrap();
        socket.write_all(request.as_bytes()).unwrap();
        BufReader::new(socket)
            .split(b'\n')
            .map(std::result::Result::unwrap)
            .collect()
    }

    #[test]
    fn a_publication_larger_than_any_frame_is_published_and_read_exactly() {
        let mut fixture = Fixture::new();
        let published = fixture.publisher.replace(large(1)).unwrap();
        assert!(serde_json::to_vec(&*published).unwrap().len() > 32 * 1024 * 1024);

        // The whole publication: many frames, none over the limit, the exact value.
        let whole = lines(&fixture, "{\"version\":2,\"operation\":\"current\"}\n");
        assert!(whole.len() > 32, "{} frames", whole.len());
        assert!(whole.iter().all(|line| line.len() < ipc::MAX_FRAME_BYTES));
        assert_eq!(
            fixture.client.current(&ipc::Select::All).unwrap(),
            *published
        );

        // One small view: one frame, and nothing of the rest.
        let small = lines(
            &fixture,
            "{\"version\":2,\"operation\":\"current\",\"select\":{\"views\":[\"status\"]}}\n",
        );
        assert_eq!(small.len(), 1);
        assert!(small[0].len() < 1024, "{} bytes", small[0].len());
        let selected = fixture
            .client
            .current(&ipc::Select::views(["status"]))
            .unwrap();
        assert!(selected.local.is_empty());
        assert_eq!(selected.views.keys().collect::<Vec<_>>(), ["status"]);
        assert_eq!(selected.views["status"], published.views["status"]);
        assert_eq!(selected.revision, published.revision);
        assert_eq!(selected.sources, published.sources);
        assert_eq!(selected.gaps, published.gaps);

        // One large view, with a record body over one frame.
        let queue = fixture
            .client
            .current(&ipc::Select::views(["queue"]))
            .unwrap();
        assert_eq!(queue.views["queue"], published.views["queue"]);

        // The peer export has the same frames.
        let export = fixture.client.peer(peers::Request::default()).unwrap();
        assert_eq!(export.local["todos"], published.local["todos"]);
        assert_eq!(export.local_revision, published.local_revision);

        // A later publication is a normal revision.
        assert_eq!(fixture.publish(2).revision, 2);
    }

    #[test]
    fn a_reply_of_many_frames_holds_one_revision_while_newer_ones_are_published() {
        let mut fixture = Fixture::new();
        fixture.publisher.replace(large(1)).unwrap();
        let client = fixture.client.clone();
        let uniform = |published: &Published| {
            consistent(published);
            let value = published.published_ms;
            let tickets = published.views["queue"]["data"]["tickets"]
                .as_array()
                .unwrap();
            assert_eq!(tickets.len(), 36_000);
            assert!(
                tickets
                    .iter()
                    .all(|ticket| ticket["request"]["value"] == value)
            );
        };
        std::thread::scope(|scope| {
            let reader = scope.spawn(move || {
                let mut subscription = client.subscribe(&ipc::Select::All).unwrap();
                uniform(&subscription.current().publication);
                let end = Instant::now() + WAIT;
                while subscription.current().publication.revision < 6 {
                    assert!(Instant::now() < end, "subscriber did not reach revision 6");
                    let connection = subscription.changed(WAIT);
                    assert_eq!(connection.status, ipc::Status::Connected);
                    uniform(&connection.publication);
                    uniform(&client.current(&ipc::Select::All).unwrap());
                }
            });
            for value in 2..=6 {
                fixture.publisher.replace(large(value)).unwrap();
            }
            reader.join().unwrap();
        });
    }

    #[test]
    fn a_failed_pass_keeps_the_last_domains_and_publishes_the_failure() {
        let mut fixture = Fixture::new();
        let first = fixture.publisher.failed(5, "store is busy").unwrap();
        assert!(first.local.is_empty() && first.views.is_empty());
        let published = fixture.publish(1);
        let mut portal = input(1);
        portal.views.insert(
            "portal".into(),
            serde_json::json!({"data": {"version": 1, "epoch": "", "revision": 0, "published_ms": 1,
                "board": null, "agents": [], "inbox": null, "gaps": []}, "gaps": []}),
        );
        fixture.publisher.replace(portal).unwrap();
        for _ in 0..2 {
            let input = fixture.publisher.failed(9, "store is busy").unwrap();
            fixture.publisher.replace(input).unwrap();
        }
        let failed = fixture.client.current(&ipc::Select::All).unwrap();
        // The second identical failure is not a new revision.
        assert_eq!(failed.revision, published.revision + 2);
        assert_eq!(failed.local, published.local);
        assert_eq!(failed.views["status"], published.views["status"]);
        let gap = failed.gaps.last().unwrap();
        assert_eq!(
            (gap.domain.as_str(), gap.detail.as_str()),
            (FAILED_PASS, "store is busy")
        );
        assert_eq!(
            failed
                .gaps
                .iter()
                .filter(|gap| gap.domain == FAILED_PASS)
                .count(),
            1
        );
        let portal = &failed.views["portal"]["data"]["gaps"];
        assert_eq!(portal.as_array().unwrap().len(), 1);
        assert_eq!(portal[0]["detail"], "store is busy");
        // The next good pass clears it.
        assert!(
            fixture
                .publish(3)
                .gaps
                .iter()
                .all(|gap| gap.domain != FAILED_PASS)
        );
    }

    #[test]
    fn command_registration_uses_explicit_current_and_subscribe() {
        use clap::Parser as _;
        for operation in ["current", "subscribe"] {
            let cli =
                crate::cli::Cli::try_parse_from(["dogsled", "observation", operation]).unwrap();
            assert!(matches!(
                cli.command,
                crate::cli::Command::Observation { .. }
            ));
        }
    }
}
