mod local;

mod publication {
    use super::super::*;
    use std::io::{BufRead as _, BufReader, Read as _, Write as _};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    const WAIT: Duration = Duration::from_secs(3);

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
    fn atomic_replacement_preserves_old_references_and_wire() {
        let mut fixture = Fixture::new();
        let old = fixture.publish(1);
        let receiver = fixture.publisher.subscribe();
        let old_frame = receiver.borrow().as_ref().unwrap().clone();
        let new = fixture.publish(2);
        assert_eq!(old.revision, 1);
        assert_eq!(old.local["host"]["value"], 1);
        assert_eq!(new.revision, 2);
        let wire: Published = serde_json::from_slice(&old_frame.bytes).unwrap();
        assert_eq!(wire, *old);
        assert_eq!(fixture.client.current().unwrap(), *new);
    }

    #[test]
    fn current_and_subscription_never_mix_concurrent_revisions() {
        let mut fixture = Fixture::new();
        fixture.publish(1);
        let client = fixture.client.clone();
        std::thread::scope(|scope| {
            let reader = scope.spawn(move || {
                let mut subscription = client.subscribe().unwrap();
                for _ in 0..64 {
                    consistent(&client.current().unwrap());
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
        let mut subscription = fixture.client.subscribe().unwrap();
        fixture.server.take();
        let retained = subscription.changed(WAIT);
        assert!(matches!(retained.status, ipc::Status::Disconnected(_)));
        assert_eq!(*retained.publication, *old);
        assert!(
            fixture
                .client
                .current()
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
        assert_eq!(fixture.client.current().unwrap(), *new);
        assert!(matches!(
            subscription.current().status,
            ipc::Status::Disconnected(_)
        ));
    }

    #[test]
    fn slow_subscriber_skips_to_newest_complete_publication() {
        let mut fixture = Fixture::new();
        fixture.publish(1);
        let mut subscription = fixture.client.subscribe().unwrap();
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
            .write_all(b"{\"version\":1,\"operation\":\"subscribe\"}\n")
            .unwrap();
        // Reading one byte confirms revision 1 is in flight, then stops draining the socket.
        let mut first = [0];
        socket.read_exact(&mut first).unwrap();
        let started = Instant::now();
        for value in 2..=32 {
            fixture.publish(value);
        }
        assert!(
            started.elapsed() < WAIT,
            "backpressure blocked the publisher"
        );
        let mut reader = BufReader::new(socket);
        let mut bytes = vec![first[0]];
        reader.read_until(b'\n', &mut bytes).unwrap();
        let old: Published = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(old.revision, 1);
        bytes.clear();
        reader.read_until(b'\n', &mut bytes).unwrap();
        let newest: Published = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(newest.revision, 32);
        consistent(&newest);
    }

    #[test]
    fn unavailable_without_publisher_does_not_create_state() {
        let state = std::env::temp_dir().join(crate::ledger::new_id());
        let client = ipc::Client::at(&state);
        assert!(
            client
                .current()
                .unwrap_err()
                .to_string()
                .contains("observation unavailable")
        );
        assert!(
            client
                .subscribe()
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
                .current()
                .unwrap_err()
                .to_string()
                .contains("observation unavailable")
        );
        assert!(
            fixture
                .client
                .subscribe()
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
                "{\"version\":2,\"operation\":\"current\"}\n".to_owned(),
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
        consistent(&fixture.client.current().unwrap());
    }

    #[test]
    fn single_endpoint_owner_and_clean_shutdown() {
        let mut fixture = Fixture::new();
        fixture.publish(1);
        assert!(ipc::Server::new(&fixture.state).is_err());
        consistent(&fixture.client.current().unwrap());
        let _silent = UnixStream::connect(fixture.state.join("observation.sock")).unwrap();
        let started = Instant::now();
        fixture.server.take();
        assert!(started.elapsed() < WAIT);
        assert!(!fixture.state.join("observation.sock").exists());
    }

    #[test]
    fn dropping_publisher_closes_subscription() {
        let mut fixture = Fixture::new();
        fixture.publish(1);
        let mut subscription = fixture.client.subscribe().unwrap();
        fixture.publisher = Publisher::new().unwrap();
        let state = subscription.changed(WAIT);
        assert!(matches!(state.status, ipc::Status::Disconnected(_)));
        assert_eq!(state.publication.revision, 1);
        assert!(
            fixture
                .client
                .current()
                .unwrap_err()
                .to_string()
                .contains("unavailable")
        );
    }

    #[test]
    fn oversized_replacement_keeps_previous_revision() {
        let mut fixture = Fixture::new();
        let old = fixture.publish(1);
        let mut large = input(2);
        large.views.insert(
            "large".into(),
            Value::String("x".repeat(ipc::MAX_FRAME_BYTES)),
        );
        assert!(fixture.publisher.replace(large).is_err());
        assert_eq!(fixture.client.current().unwrap(), *old);
        assert_eq!(fixture.publish(3).revision, 2);
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
