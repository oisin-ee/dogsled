use super::super::*;
use crate::hosts::{ActionFleet, Fleet, Host};
use crate::ledger::{Ledger, new_id};
use crate::shell::Fake;
use serde_json::json;

fn snapshot() -> crate::node::Snapshot {
    serde_json::from_value(json!({"sampled_ms":10_000,"probe":{
        "host":"local","label":"local","load1":0.0,"cores":8,"busy":0.1,
        "memory":{"used_bytes":1,"total_bytes":32_000_000_000u64},"disk_free_percent":99.0,
        "agents":{"total":1,"by_status":{"idle":1}},"reservations":[],"panes":[],
        "agent_panes":[{"pane":"owner","agent":"pi","state":"idle","terminal_id":"term_owner"}]
    }}))
    .unwrap()
}

fn lock(ledger: &Ledger, name: &str) -> std::fs::File {
    let file = std::fs::File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(ledger.dir.join(format!("{name}.lock")))
        .unwrap();
    file.try_lock().unwrap();
    file
}

#[test]
fn recorded_local_pane_send_revalidates_locally_without_any_ipc_attempt() {
    use std::os::unix::net::UnixListener;
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let shell = Fake::new();
    let policy = crate::config::Policy::default();
    let mut sampled = snapshot();
    sampled.probe.agent_panes[0].pane = "recorded:p1".into();
    sampled.probe.agent_panes[0].terminal_id = Some("term_worker".into());
    crate::host_store::put_snapshot(&ledger, &sampled).unwrap();
    shell.on(
        "herdr agent list",
        &json!({"result":{"agents":[{
            "pane_id":"recorded:p1","terminal_id":"term_worker","agent":"pi","agent_status":"idle"
        }]}})
        .to_string(),
    );
    crate::items::record(
        &ledger,
        crate::items::Item {
            id: "child".into(),
            kind: "agent".into(),
            host: "local".into(),
            pane: "recorded:p1".into(),
            owner_pane: "owner".into(),
            ..Default::default()
        },
    )
    .unwrap();
    crate::inbox::claim(
        &ledger,
        "term_worker",
        &crate::inbox::Reader {
            pane: "recorded:p1".into(),
            harness: "pi".into(),
            registered_ms: 1,
        },
        10_000,
    )
    .unwrap();
    let listener = UnixListener::bind(ledger.dir.join("observation.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let sent = crate::inbox::send_to(
        &Fleet::cold(
            &shell,
            &ledger,
            &policy,
            "/".into(),
            vec![crate::hosts::Host::local()],
        ),
        "recorded:p1",
        None,
        "message",
        "continue".into(),
        10_000,
    )
    .unwrap();
    assert_eq!(sent.pane, "recorded:p1");
    assert_eq!(sent.terminal, "term_worker");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert!(
        shell
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| call == "herdr agent list")
    );
    drop(listener);
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn cold_fleet_resolves_remote_close_host_from_the_supplied_publication() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let shell = Fake::new();
    let policy = crate::config::Policy::default();
    crate::host_store::put_snapshot(&ledger, &snapshot()).unwrap();
    let peer = Host {
        id: "peer".into(),
        label: "remote-label".into(),
        target: Some("peer-ssh".into()),
    };
    let producer = Fleet::producer(
        &shell,
        &ledger,
        &policy,
        "/".into(),
        vec![Host::local(), peer.clone()],
    );
    let captured = captured_actions(&producer, 10_000).unwrap().published;
    let mut publisher = Publisher::new().unwrap();
    publisher
        .replace(Publication {
            published_ms: captured.published_ms,
            sources: captured.sources.clone(),
            local: captured.local.clone(),
            views: captured.views.clone(),
            gaps: captured.gaps.clone(),
        })
        .unwrap();
    let mut server = ipc::Server::new(&ledger.dir).unwrap();
    server.bind(publisher.subscribe()).unwrap();
    let cold = Fleet::cold(&shell, &ledger, &policy, "/".into(), vec![Host::local()]);
    assert!(
        cold.find("peer").is_err(),
        "the command has no preloaded peer mapping"
    );
    assert_eq!(
        crate::hosts::command_host(&cold, "remote-label").unwrap(),
        peer
    );
    assert!(
        shell.calls.lock().unwrap().is_empty(),
        "resolution must not discover hosts"
    );
    drop(server);
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn producer_fleet_refuses_command_resolution_without_any_ipc_attempt() {
    use std::os::unix::net::UnixListener;
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    std::fs::create_dir_all(&ledger.dir).unwrap();
    let shell = Fake::new();
    let policy = crate::config::Policy::default();
    let producer = Fleet::producer(&shell, &ledger, &policy, "/".into(), vec![Host::local()]);
    let listener = UnixListener::bind(ledger.dir.join("observation.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    for target in ["local", "peer", "missing"] {
        assert_eq!(
            crate::hosts::command_host(&producer, target)
                .unwrap_err()
                .to_string(),
            "producer fleet cannot resolve command hosts"
        );
    }
    assert_eq!(
        ActionFleet::from_fleet(&producer).unwrap_err().to_string(),
        "producer fleet cannot resolve command hosts"
    );
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert!(shell.calls.lock().unwrap().is_empty());
    drop(listener);
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn remote_release_lane_and_removed_host_never_connect_to_their_own_publisher() {
    use std::os::unix::net::UnixListener;
    for removed in [false, true] {
        let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
        let shell = Fake::new();
        let policy = crate::config::Policy::default();
        crate::host_store::put_snapshot(&ledger, &snapshot()).unwrap();
        let peer = Host {
            id: "peer".into(),
            label: "peer".into(),
            target: Some("peer-ssh".into()),
        };
        let mut hosts = vec![Host::local()];
        if !removed {
            hosts.push(peer.clone());
        }
        // This is producer construction, not the lane's effect context.
        let producer = Fleet::cold(&shell, &ledger, &policy, "/".into(), hosts);
        let lane = captured_actions(&producer, 10_000).unwrap();
        crate::items::record(
            &ledger,
            crate::items::Item {
                id: "remote-job".into(),
                kind: "job".into(),
                host: peer.id.clone(),
                pane: "remote:p1".into(),
                owner_pane: "owner".into(),
                created_ms: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let argv = lane.remote_argv(&peer, &["job".into(), "remote-job".into()]);
        shell.on(
            &format!("ssh {}", argv.join(" ")),
            &json!({
                "id":"remote-job","cmd":["true"],"cwd":"/","host":"peer","owner":"owner",
                "pane":"remote:p1","state":"idle","exit":0,"started_ms":1,"ended_ms":5
            })
            .to_string(),
        );
        let listener = UnixListener::bind(ledger.dir.join("observation.sock")).unwrap();
        listener.set_nonblocking(true).unwrap();
        let result = crate::work::tick(&lane, &crate::launch::ReleaseRetries::default(), 10_000);
        if removed {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("unknown host peer")
            );
            assert!(crate::items::list(&ledger).unwrap()[0].closed_ms.is_none());
            assert!(shell.calls.lock().unwrap().is_empty());
        } else {
            result.unwrap();
            assert_eq!(crate::items::list(&ledger).unwrap()[0].closed_ms, Some(5));
            assert_eq!(shell.calls.lock().unwrap().len(), 1);
        }
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        drop(listener);
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
}

#[test]
fn origin_validity_commit_wakes_an_observer_with_owned_holds_before_admission() {
    use std::os::unix::net::UnixDatagram;
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let shell = Fake::new();
    let policy = crate::config::Policy::default();
    crate::host_store::put_snapshot(&ledger, &snapshot()).unwrap();
    crate::jobs::clock(&ledger).unwrap();
    let notifications = UnixDatagram::bind(ledger.dir.join("observation-wake.sock")).unwrap();
    notifications
        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .unwrap();
    let fleet = Fleet::cold(
        &shell,
        &ledger,
        &policy,
        "/".into(),
        vec![crate::hosts::Host::local()],
    );
    let node = fleet.node();
    for index in 0..8 {
        let id = format!("issuance-{index}");
        crate::jobs::issue_request_validity(&ledger, &id, "owner", "origin").unwrap();
        let mut bytes = [0; 16];
        let received = notifications.recv(&mut bytes).unwrap();
        assert_eq!(&bytes[..received], b"ledger");
        // Observe at the real post-commit wake, before any admission or ticket exists.
        let report =
            crate::store::observing(|| collect::host(&node, Some(snapshot()), true, 10_000))
                .unwrap();
        let hold = report
            .holds
            .iter()
            .find(|hold| hold.resource == format!("job-validity/{id}"))
            .unwrap();
        assert_eq!(hold.owner_pane.as_deref(), Some("owner"));
        assert_eq!(hold.owner, "owner");
        assert!(!crate::admissions::exists(&ledger, &id).unwrap());
        assert_eq!(crate::work::queue(&ledger).unwrap().len(), 0);
        // A second transaction for ownership would emit another wake: issuance is one commit.
        notifications.set_nonblocking(true).unwrap();
        assert_eq!(
            notifications.recv(&mut bytes).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        notifications.set_nonblocking(false).unwrap();
    }
    drop(notifications);
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn lanes_use_the_in_process_publication_without_snapshot_reads_or_ipc() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let shell = Fake::new();
    let policy = crate::config::Policy::default();
    crate::host_store::put_snapshot(&ledger, &snapshot()).unwrap();
    let fleet = Fleet::cold(&shell, &ledger, &policy, "/".into(), vec![Host::local()]);
    let actions = captured_actions(&fleet, 10_000).unwrap();
    assert!(shell.calls.lock().unwrap().is_empty());
    let locks: Vec<_> = ["observation", "host", "branch-checks"]
        .into_iter()
        .map(|name| lock(&ledger, name))
        .collect();
    std::fs::create_dir(ledger.dir.join("observation.sock")).unwrap();
    crate::store::write(&ledger, |tx| {
        tx.execute(
            "ALTER TABLE host_snapshot RENAME TO captured_host_snapshot",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(crate::host_store::snapshot(&ledger).is_err());
    crate::work::tick(&actions, &crate::launch::ReleaseRetries::default(), 10_000).unwrap();
    crate::inbox::tick(&actions, 10_000).unwrap();
    crate::sweep::children(&actions, 10_000).unwrap();
    crate::watchdog::tick(&actions, &actions.reports().unwrap(), 10_000).unwrap();
    assert!(shell.calls.lock().unwrap().is_empty());
    drop(locks);
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn placement_selects_published_capabilities_and_revalidates_capacity_at_reserve() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let shell = Fake::new();
    let policy = crate::config::Policy::default();
    crate::host_store::put_snapshot(&ledger, &snapshot()).unwrap();
    let service = lock(&ledger, "service");
    let fleet = Fleet::cold(&shell, &ledger, &policy, "/".into(), vec![Host::local()]);
    let actions = captured_actions(&fleet, 10_000).unwrap();
    assert!(
        actions.reports().unwrap()[0]
            .headroom
            .as_ref()
            .unwrap()
            .eligible
    );
    assert!(shell.calls.lock().unwrap().is_empty());
    // Reserve must acquire the host lock for its current-time capacity check. Only
    // observation locks are held across this explicit mutation.
    let locks: Vec<_> = ["observation", "branch-checks"]
        .into_iter()
        .map(|name| lock(&ledger, name))
        .collect();
    let mut full = snapshot();
    full.probe.disk_free_percent = 0.0;
    crate::host_store::put_snapshot(&ledger, &full).unwrap();
    let validity = crate::jobs::origin_validity(&ledger).unwrap();
    let request = serde_json::from_value(json!({
        "id":"stale-capacity", "validity":validity,
        "action":{"type":"job", "cmd":["true"], "timeout_ms":null, "idle_timeout_ms":null, "validity":validity},
        "host":null, "branch":null, "base":null, "cwd":"/", "repository":null,
        "workspace":null, "workspace_name":null, "owner":"owner", "owner_host":"origin"
    })).unwrap();
    assert!(
        crate::work::submit(&actions, &request, 10_000)
            .unwrap()
            .is_none()
    );
    assert_eq!(crate::job_store::reservations(&ledger).unwrap(), []);
    assert!(
        actions.reports().unwrap()[0]
            .headroom
            .as_ref()
            .unwrap()
            .eligible
    );
    assert!(shell.calls.lock().unwrap().is_empty());
    drop((service, locks));
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn publication_selection_needs_no_store_shell_or_observation_locks() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let shell = Fake::new();
    let policy = crate::config::Policy::default();
    crate::host_store::put_snapshot(&ledger, &snapshot()).unwrap();
    let fleet = Fleet::cold(&shell, &ledger, &policy, "/".into(), vec![Host::local()]);
    let sampled = snapshot();
    let agent = &sampled.probe.agent_panes[0];
    shell.on("herdr agent list", &json!({"result":{"agents":[{"pane_id":agent.pane,"agent":agent.agent,"agent_status":agent.state,"terminal_id":agent.terminal_id}]}}).to_string());
    shell.on(
        "herdr pane list",
        &json!({"result":{"panes":sampled.probe.panes}}).to_string(),
    );
    let terminals = crate::probe::terminals(&shell).unwrap();
    let published = captured_actions_with_terminals(&fleet, 10_000, Some(terminals))
        .unwrap()
        .published;
    // End the producer phase; every call below belongs to the isolated actor.
    shell.calls.lock().unwrap().clear();
    assert!(shell.calls.lock().unwrap().is_empty());
    // A store connection cannot open this directory as SQLite. Every observation lock is
    // already held; selection must only decode the producer's frozen projections.
    std::fs::rename(ledger.dir.join("state.db"), ledger.dir.join("saved.db")).unwrap();
    std::fs::create_dir(ledger.dir.join("state.db")).unwrap();
    let locks: Vec<_> = [
        "service",
        "state-db",
        "host",
        "branch-checks",
        "observation",
    ]
    .into_iter()
    .map(|name| {
        let file = std::fs::File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(ledger.dir.join(format!("{name}.lock")))
            .unwrap();
        file.try_lock().unwrap();
        file
    })
    .collect();
    let actions =
        ActionFleet::new(&shell, &ledger, &policy, "/".into(), Arc::clone(&published)).unwrap();
    assert_eq!(actions.reports().unwrap()[0].sampled_ms, Some(10_000));
    let target = crate::inbox::resolve(&actions, "owner", None).unwrap();
    assert_eq!(target.agent.terminal_id.as_deref(), Some("term_owner"));
    assert!(
        crate::gate::notices(&ledger, &published, "owner")
            .unwrap()
            .is_empty()
    );
    assert!(
        crate::context::format(
            crate::context::Harness::Pi,
            crate::context::Event::SessionStart,
            &published,
            Some("owner")
        )
        .unwrap()
        .contains("local")
    );
    assert!(shell.calls.lock().unwrap().is_empty());
    drop(locks);
    std::fs::remove_dir_all(ledger.dir).unwrap();
}
