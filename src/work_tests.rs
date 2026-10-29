//! Admission and reconciliation with injected timestamps and a scripted OS seam.
use crate::config::{Policy, Thresholds};
use crate::hosts::{Fleet, Host};
use crate::items::{self, Item};
use crate::ledger::{Ledger, new_id};
use crate::node::Snapshot;
use crate::probe::{Agent, AgentCounts, Memory, Pane, Probe};
use crate::shell::Fake;
use crate::work::{self, Action, Request};
use serde_json::json;

mod reconcile;

#[test]
fn enqueue_writes_an_array_readable_by_older_servers() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let id = work::enqueue(&ledger, place_request("array-queue"), false, 100).unwrap();
    let bytes = std::fs::read(ledger.dir.join("queue.json")).unwrap();
    let tickets: Vec<work::Ticket> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(tickets.len(), 1);
    assert_eq!(tickets[0].id, id);
    assert_eq!(
        ledger
            .read::<serde_json::Value>("queue-format.json")
            .unwrap(),
        Some(json!({"version": 1}))
    );
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn an_envelope_queue_is_read_and_rewritten_as_an_array() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let id = work::enqueue(&ledger, place_request("envelope-queue"), false, 100).unwrap();
    let tickets = work::queue(&ledger).unwrap();
    ledger
        .write("queue.json", &json!({"version": 1, "tickets": tickets}))
        .unwrap();
    std::fs::remove_file(ledger.dir.join("queue-format.json")).unwrap();
    assert_eq!(work::queue(&ledger).unwrap()[0].id, id);
    work::cancel(&ledger, &id).unwrap();
    let bytes = std::fs::read(ledger.dir.join("queue.json")).unwrap();
    let tickets: Vec<work::Ticket> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(tickets.len(), 1);
    assert_eq!(tickets[0].id, id);
    assert_eq!(tickets[0].state, "cancelled");
    assert_eq!(
        ledger
            .read::<serde_json::Value>("queue-format.json")
            .unwrap(),
        Some(json!({"version": 1}))
    );
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn a_newer_queue_sidecar_refuses_writes_without_changing_the_files() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let id = work::enqueue(&ledger, place_request("future-queue"), false, 100).unwrap();
    let future = json!({"version": 2});
    ledger.write("queue-format.json", &future).unwrap();
    let before = std::fs::read(ledger.dir.join("queue.json")).unwrap();
    for error in [
        work::enqueue(&ledger, place_request("new-request"), false, 100).unwrap_err(),
        work::cancel(&ledger, &id).unwrap_err(),
    ] {
        assert!(
            matches!(
                error,
                crate::error::Error::Refused {
                    code: crate::error::Refusal::QueueVersionUnsupported,
                    ..
                }
            ),
            "{error}"
        );
    }
    assert_eq!(
        std::fs::read(ledger.dir.join("queue.json")).unwrap(),
        before
    );
    assert_eq!(
        ledger
            .read::<serde_json::Value>("queue-format.json")
            .unwrap(),
        Some(future)
    );
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn a_newer_queue_version_refuses_writes_without_changing_the_file() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let future = json!({"version": u32::MAX, "tickets": []});
    ledger.write("queue.json", &future).unwrap();
    let error = work::enqueue(
        &ledger,
        Request {
            id: "future-queue".into(),
            action: Action::Place,
            host: None,
            branch: None,
            base: None,
            cwd: "/".into(),
            repository: None,
            workspace: None,
            workspace_name: None,
            owner: "owner".into(),
            owner_host: None,
        },
        false,
        100,
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            crate::error::Error::Refused {
                code: crate::error::Refusal::QueueVersionUnsupported,
                ..
            }
        ),
        "{error}"
    );
    assert_eq!(
        ledger.read::<serde_json::Value>("queue.json").unwrap(),
        Some(future)
    );
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn request_sidecars_expire_with_terminal_tickets_but_not_pending_work() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let created = crate::ledger::now_ms();
    for id in ["terminal", "pending"] {
        let mut request = place_request(id);
        request.action = Action::Job {
            cmd: vec!["true".into()],
            timeout_ms: None,
            idle_timeout_ms: None,
            validity: Some(crate::jobs::Validity::new(created)),
        };
        let ticket = work::enqueue(&ledger, request, false, created).unwrap();
        if id == "terminal" {
            work::cancel(&ledger, &ticket).unwrap();
        }
    }
    // Cancellation alone proves no terminal job outcome. This local record does.
    ledger
        .child("jobs")
        .write(
            "terminal.json",
            &json!({
                "id":"terminal", "cmd":["true"], "cwd":"/", "host":"local",
                "owner":"owner", "pane":"pane", "state":"idle", "exit":0,
                "signal":null, "reason":null, "started_ms":created, "ended_ms":created,
                "timeout_ms":null, "idle_timeout_ms":null,
                "validity":{"version":1,"created_ms":created}, "output_expired":false
            }),
        )
        .unwrap();
    let orphan = crate::jobs::Validity::new(created);
    crate::jobs::record_request_validity(&ledger, "orphan", Some(orphan)).unwrap();
    work::expire_requests(&ledger, created + crate::jobs::RECORD_KEEP_MS).unwrap();
    assert_eq!(work::queue(&ledger).unwrap().len(), 2);
    assert!(
        crate::jobs::request_validity(&ledger, "terminal")
            .unwrap()
            .is_some()
    );
    work::expire_requests(&ledger, created + crate::jobs::RECORD_KEEP_MS + 1).unwrap();
    assert_eq!(work::queue(&ledger).unwrap().len(), 1);
    assert!(
        crate::jobs::request_validity(&ledger, "terminal")
            .unwrap()
            .is_none()
    );
    assert!(
        crate::jobs::request_validity(&ledger, "pending")
            .unwrap()
            .is_some()
    );
    assert!(
        crate::jobs::request_validity(&ledger, "orphan")
            .unwrap()
            .is_none()
    );
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn request_expiry_keeps_failed_lost_replies_and_non_job_tickets() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let created = crate::ledger::now_ms();
    for id in ["lost-reply", "cancelled", "placed", "agent"] {
        let mut request = place_request(id);
        if matches!(id, "lost-reply" | "cancelled") {
            request.action = Action::Job {
                cmd: vec!["true".into()],
                timeout_ms: None,
                idle_timeout_ms: None,
                validity: Some(crate::jobs::Validity::new(created)),
            };
        } else if id == "agent" {
            request.action = Action::Agent {
                kind: "pi".into(),
                name: "agent".into(),
                message: "task".into(),
                profile: None,
            };
        }
        work::enqueue(&ledger, request, false, created).unwrap();
    }
    let mut tickets = work::queue(&ledger).unwrap();
    tickets[0].state = "failed".into();
    tickets[0].retry.attempts = 3;
    tickets[0].error = Some("remote launch reply lost".into());
    tickets[1].state = "cancelled".into();
    tickets[2].state = "admitted".into();
    tickets[3].state = "admitted".into();
    ledger.write("queue.json", &tickets).unwrap();
    work::expire_requests(&ledger, created + crate::jobs::RECORD_KEEP_MS + 1).unwrap();
    assert_eq!(work::queue(&ledger).unwrap().len(), 4);
    for id in ["lost-reply", "cancelled"] {
        assert!(crate::jobs::record(&ledger, id).unwrap().is_none());
        assert!(
            crate::jobs::request_validity(&ledger, id)
                .unwrap()
                .is_some()
        );
    }
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn orphan_request_sidecars_expire_only_without_launch_receipts() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let created = crate::ledger::now_ms();
    for id in ["orphan", "receipt"] {
        crate::jobs::record_request_validity(
            &ledger,
            id,
            Some(crate::jobs::Validity::new(created)),
        )
        .unwrap();
    }
    ledger
        .child("launches")
        .write("receipt.json", &json!({"id":"receipt"}))
        .unwrap();
    work::expire_requests(&ledger, created + crate::jobs::RECORD_KEEP_MS + 1).unwrap();
    assert!(
        crate::jobs::request_validity(&ledger, "orphan")
            .unwrap()
            .is_none()
    );
    assert!(
        crate::jobs::request_validity(&ledger, "receipt")
            .unwrap()
            .is_some()
    );
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn launch_ingress_never_restores_a_receipted_requests_missing_sidecar() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let now = crate::ledger::now_ms();
    let shell = Fake::new();
    let launch = work::Launch {
        preparation_protocol: None,
        preparation: None,
        id: "receipted".into(),
        unit: None,
        action: Action::Job {
            cmd: vec!["true".into()],
            timeout_ms: None,
            idle_timeout_ms: None,
            validity: Some(crate::jobs::Validity::new(now)),
        },
        host: "local".into(),
        cwd: "/work".into(),
        pane: None,
        terminal: None,
        workspace: None,
        workspace_name: None,
        owner: "owner".into(),
        owner_host: None,
    };
    let saved = json!({
        "id":"receipted", "host":"local", "pane":"pane",
        "request":{"cmd":["true"], "cwd":"/work", "owner":"owner",
            "timeout_ms":null, "idle_timeout_ms":null,
            "validity":{"version":1,"created_ms":now}}
    });
    ledger
        .child("launches")
        .write("receipted.json", &saved)
        .unwrap();
    let result = work::receive_launch(&ledger, &shell, &launch, now, Some(2));
    assert!(
        crate::jobs::request_validity(&ledger, "receipted")
            .unwrap()
            .is_none()
    );
    assert!(result.is_err(), "missing authority refuses: {result:?}");
    assert!(crate::jobs::record(&ledger, "receipted").unwrap().is_none());
    assert!(shell.calls.lock().unwrap().is_empty());
    assert_eq!(
        ledger
            .child("launches")
            .read::<serde_json::Value>("receipted.json")
            .unwrap(),
        Some(saved)
    );
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

fn service(ledger: &Ledger) -> std::fs::File {
    std::fs::create_dir_all(&ledger.dir).unwrap();
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(ledger.dir.join("service.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    lock
}

fn snapshot(now: u64) -> Snapshot {
    Snapshot {
        sampled_ms: now,
        jobs: Vec::new(),
        probe: Probe {
            host: "local".into(),
            label: "local".into(),
            load1: 0.0,
            cores: 1,
            cpu_ticks: None,
            busy: Some(0.0),
            memory: Memory::default(),
            disk_free_percent: 99.0,
            agents: AgentCounts::default(),
            agent_panes: vec![Agent {
                pane: "owner".into(),
                agent: None,
                state: "working".into(),
                terminal_id: Some("term_owner".into()),
                name: None,
                transcript: None,
                transcript_modified: None,
                stalled_since_ms: None,
                state_since_ms: None,
                ..Agent::default()
            }],
            reservations: Vec::new(),
            panes: vec![Pane {
                pane_id: "child".into(),
                ..Pane::default()
            }],
        },
    }
}

#[test]
fn fifo_admission_rechecks_reservations_and_notifies_once_the_owner_reads_its_inbox() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let _service = service(&ledger);
    let shell = Fake::new();
    let policy = Policy {
        memory_reserve_bytes: 0,
        ..Policy::default()
    };
    let fleet = Fleet {
        ledger: &ledger,
        shell: &shell,
        policy: &policy,
        home: "/".into(),
        hosts: vec![Host::local()],
    };
    ledger.write("host.json", &snapshot(100)).unwrap();
    for id in ["first", "second"] {
        work::enqueue(
            &ledger,
            Request {
                id: id.into(),
                action: Action::Place,
                host: None,
                branch: None,
                base: None,
                cwd: "/".into(),
                repository: None,
                workspace: None,
                workspace_name: None,
                owner: "owner".into(),
                owner_host: None,
            },
            false,
            100,
        )
        .unwrap();
    }
    work::tick(&fleet, 100).unwrap();
    let tickets = work::queue(&ledger).unwrap();
    assert_eq!(tickets[0].state, "admitted");
    assert_eq!(tickets[1].state, "pending");
    // Without an inbox reader the owner is not told yet, and nothing is typed into it.
    assert!(!tickets[0].notified);
    crate::inbox::claim(
        &ledger,
        "term_owner",
        &crate::inbox::Reader {
            harness: "claude".into(),
            pane: "owner".into(),
            registered_ms: 100,
        },
        100,
    )
    .unwrap();
    // A working owner hears too: its reader delivers at the next step.
    shell.on("herdr agent list", r#"{"result":{"agents":[{"pane_id":"owner","agent":"claude","agent_status":"working","terminal_id":"term_owner"}]}}"#);
    work::tick(&fleet, 101).unwrap();
    assert!(work::queue(&ledger).unwrap()[0].notified);
    let told = crate::inbox::received(&ledger, "term_owner").unwrap();
    assert_eq!(told.len(), 1);
    let result = tickets[0].result.as_ref().unwrap();
    assert_eq!(
        told[0].text,
        format!(
            "dogsled: ticket {} admitted on local: {result}",
            tickets[0].id
        )
    );
    work::tick(&fleet, 102).unwrap();
    assert_eq!(
        crate::inbox::received(&ledger, "term_owner").unwrap().len(),
        1
    );
    assert_eq!(*shell.calls.lock().unwrap(), ["herdr agent list"]);
    // Exactly at TTL, a fresh sample admits the second ticket without a sleep.
    let mut refreshed = snapshot(90_100);
    refreshed.probe.agent_panes[0].state = "working".into();
    ledger.write("host.json", &refreshed).unwrap();
    work::tick(&fleet, 90_100).unwrap();
    assert_eq!(work::queue(&ledger).unwrap()[1].state, "admitted");
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn reconciliation_does_not_close_from_stale_or_pre_creation_snapshots() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let _service = service(&ledger);
    let shell = Fake::new();
    let policy = Policy::default();
    let fleet = Fleet {
        ledger: &ledger,
        shell: &shell,
        policy: &policy,
        home: "/".into(),
        hosts: vec![Host::local()],
    };
    items::record(
        &ledger,
        Item {
            id: "child".into(),
            kind: "agent".into(),
            host: "local".into(),
            pane: "child".into(),
            owner_pane: "owner".into(),
            name: None,
            created_ms: 200,
            closed_ms: None,
            ..Item::default()
        },
    )
    .unwrap();
    let mut current = snapshot(100);
    current.probe.panes.clear();
    ledger.write("host.json", &current).unwrap();
    items::reconcile(&ledger, &fleet.status(200).unwrap(), 200).unwrap();
    assert!(items::list(&ledger).unwrap()[0].closed_ms.is_none());
    current.sampled_ms = 200;
    ledger.write("host.json", &current).unwrap();
    items::reconcile(&ledger, &fleet.status(30_201).unwrap(), 30_201).unwrap();
    assert!(items::list(&ledger).unwrap()[0].closed_ms.is_none());
    items::reconcile(&ledger, &fleet.status(201).unwrap(), 201).unwrap();
    assert_eq!(items::list(&ledger).unwrap()[0].closed_ms, Some(201));
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

fn place_request(id: &str) -> Request {
    Request {
        id: id.into(),
        action: Action::Place,
        host: None,
        branch: None,
        base: None,
        cwd: "/".into(),
        repository: None,
        workspace: None,
        workspace_name: None,
        owner: "owner".into(),
        owner_host: None,
    }
}

#[test]
fn placement_replay_preserves_the_complete_origin_expectation() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let _service = service(&ledger);
    let shell = Fake::new();
    let policy = Policy {
        memory_reserve_bytes: 0,
        ..Policy::default()
    };
    let fleet = Fleet {
        ledger: &ledger,
        shell: &shell,
        policy: &policy,
        home: "/".into(),
        hosts: vec![Host::local()],
    };
    ledger.write("host.json", &snapshot(100)).unwrap();
    let request = place_request("captured-placement");
    assert!(work::perform(&fleet, &request, 100).unwrap().is_some());
    let admission = ledger.child("admissions");
    let before = std::fs::read(admission.dir.join("captured-placement.json")).unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(saved["request"], serde_json::to_value(&request).unwrap());
    assert_eq!(saved["placement"]["phase"], "completed");
    assert_eq!(
        saved["placement"]["saved"]["prepared"]["grant"]["envelope"]["request"],
        saved["request"]
    );
    let hold = std::fs::read(ledger.dir.join("placements.json")).unwrap();
    for field in ["cwd", "owner", "branch", "host"] {
        let mut changed = serde_json::to_value(&request).unwrap();
        changed[field] = json!("changed");
        let changed: Request = serde_json::from_value(changed).unwrap();
        assert!(work::perform(&fleet, &changed, 101).is_err(), "{field}");
        assert_eq!(
            std::fs::read(admission.dir.join("captured-placement.json")).unwrap(),
            before
        );
        assert_eq!(
            std::fs::read(ledger.dir.join("placements.json")).unwrap(),
            hold
        );
    }
    let redirected = Fleet {
        hosts: vec![Host {
            target: Some("different-target".into()),
            ..Host::local()
        }],
        ..fleet
    };
    assert!(work::perform(&redirected, &request, 101).is_err());
    assert_eq!(
        std::fs::read(admission.dir.join("captured-placement.json")).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read(ledger.dir.join("placements.json")).unwrap(),
        hold
    );
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn placement_admission_never_downgrades_missing_or_newer_progress() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let shell = Fake::new();
    let policy = Policy::default();
    let fleet = Fleet {
        ledger: &ledger,
        shell: &shell,
        policy: &policy,
        home: "/".into(),
        hosts: vec![Host::local()],
    };
    let request = place_request("no-downgrade");
    let admissions = ledger.child("admissions");
    for progress in [
        json!(null),
        json!({"version":3,"phase":"recorded","declined":[]}),
    ] {
        let record =
            json!({"request":request,"host":null,"launch":null,"error":null,"placement":progress});
        admissions.write("no-downgrade.json", &record).unwrap();
        assert!(work::perform(&fleet, &request, 100).is_err());
        assert_eq!(
            admissions
                .read::<serde_json::Value>("no-downgrade.json")
                .unwrap(),
            Some(record)
        );
        assert!(!ledger.dir.join("placements.json").exists());
        assert!(
            !ledger
                .dir
                .join("placement-grants/no-downgrade.json")
                .exists()
        );
    }
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

fn pinned(id: &str, host: &str) -> Request {
    Request {
        host: Some(host.into()),
        ..place_request(id)
    }
}

#[test]
fn a_full_host_holds_back_only_the_tickets_that_need_it() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let _service = service(&ledger);
    let shell = Fake::new();
    let policy = Policy {
        memory_reserve_bytes: 0,
        ..Policy::default()
    };
    let peer = Host {
        id: "peer".into(),
        label: "peer".into(),
        target: Some("peer-ssh".into()),
    };
    let fleet = Fleet {
        ledger: &ledger,
        shell: &shell,
        policy: &policy,
        home: "/".into(),
        hosts: vec![Host::local(), peer.clone()],
    };
    // Local has room for every local ticket in this test.
    let mut roomy = snapshot(100);
    roomy.probe.cores = 8;
    ledger.write("host.json", &roomy).unwrap();
    // The peer has no disk to spare, so nothing fits there.
    let mut full = snapshot(100);
    full.probe.host = "peer".into();
    full.probe.label = "peer".into();
    full.probe.disk_free_percent = 1.0;
    shell.on(
        &format!(
            "ssh {}",
            fleet.remote_argv(&peer, &["snapshot".into()]).join(" ")
        ),
        &serde_json::to_string(&full).unwrap(),
    );
    for request in [
        pinned("peer-first", "peer"),
        pinned("local-after", "local"),
        pinned("peer-after", "peer"),
    ] {
        work::enqueue(&ledger, request, false, 100).unwrap();
    }
    // A new request for local still waits behind the pending local ticket.
    assert!(
        work::submit(&fleet, &pinned("local-new", "local"), 100)
            .unwrap()
            .is_none(),
        "a pending ticket for the same host is still ahead of it"
    );
    work::tick(&fleet, 100).unwrap();
    let states: Vec<_> = work::queue(&ledger)
        .unwrap()
        .into_iter()
        .map(|t| (t.request.id, t.state))
        .collect();
    assert_eq!(
        states,
        [
            ("peer-first".to_owned(), "pending".to_owned()),
            ("local-after".to_owned(), "admitted".to_owned()),
            ("peer-after".to_owned(), "pending".to_owned()),
        ]
    );
    // With nothing pending for local, a local request runs at once while the peer's queue waits.
    let placed = work::submit(&fleet, &pinned("local-now", "local"), 100)
        .unwrap()
        .unwrap();
    assert_eq!(placed["host"]["id"], "local");
    // A request that may use any host still waits behind the peer's tickets.
    assert!(
        work::submit(&fleet, &place_request("anywhere"), 100)
            .unwrap()
            .is_none()
    );
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn submit_waits_behind_pending_tickets_even_with_room() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let _service = service(&ledger);
    let shell = Fake::new();
    let policy = Policy {
        memory_reserve_bytes: 0,
        ..Policy::default()
    };
    let fleet = Fleet {
        ledger: &ledger,
        shell: &shell,
        policy: &policy,
        home: "/".into(),
        hosts: vec![Host::local()],
    };
    ledger.write("host.json", &snapshot(100)).unwrap();
    let ahead = work::enqueue(&ledger, place_request("ahead"), false, 100).unwrap();
    // The host has room, but a ticket waits ahead: the new request must queue.
    assert!(
        work::submit(&fleet, &place_request("late"), 100)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        ledger
            .reservations(100, policy.reservation_ttl_ms)
            .unwrap()
            .len(),
        0
    );
    work::cancel(&ledger, &ahead).unwrap();
    // With nothing ahead it is placed at once.
    let placed = work::submit(&fleet, &place_request("next"), 100)
        .unwrap()
        .unwrap();
    assert_eq!(placed["host"]["id"], "local");
    assert_eq!(
        ledger
            .reservations(100, policy.reservation_ttl_ms)
            .unwrap()
            .len(),
        1
    );
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

/// A job's durable record says who may run it. A missing record is nobody's, a pending record
/// is the wrapper's to run, and a record that started, ended, or stopped uncertainly is nobody's
/// to take over.
#[test]
fn a_job_record_says_who_may_run_the_job() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let job = crate::jobs::Job {
        id: "claimed".into(),
        cmd: vec!["true".into()],
        cwd: "/".into(),
        host: "local".into(),
        owner: "owner".into(),
        pane: "pane".into(),
        state: crate::jobs::PENDING.into(),
        exit: None,
        signal: None,
        reason: None,
        started_ms: None,
        ended_ms: None,
        timeout_ms: None,
        idle_timeout_ms: None,
        validity: None,
        startup: None,
        output_expired: false,
    };
    assert!(matches!(
        crate::jobs::ownership(&ledger, "claimed").unwrap(),
        crate::jobs::Ownership::Unclaimed
    ));
    // The intent is published before the pane side effect, and it is not executable.
    crate::jobs::publish_intent(&ledger, &job).unwrap();
    let held = crate::jobs::ownership(&ledger, "claimed").unwrap();
    assert!(matches!(
        held,
        crate::jobs::Ownership::Uncertain(ref held) if held.state == crate::jobs::CLAIMING
    ));
    // A second intent never takes the job over.
    assert!(crate::jobs::publish_intent(&ledger, &job).is_err());
    // The claim becomes executable once its pane is known.
    crate::jobs::publish_claim(&ledger, &job).unwrap();
    assert!(matches!(
        crate::jobs::ownership(&ledger, "claimed").unwrap(),
        crate::jobs::Ownership::Pending(_)
    ));
    // An uncertain claim authorizes nothing, and a claim is never overwritten.
    crate::jobs::uncertain(&ledger, &job, "dispatch outcome unknown".into()).unwrap();
    assert!(matches!(
        crate::jobs::ownership(&ledger, "claimed").unwrap(),
        crate::jobs::Ownership::Uncertain(ref held) if held.state == crate::jobs::UNCERTAIN
    ));
    assert!(crate::jobs::publish_claim(&ledger, &job).is_err());
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

/// A record stored under one name and holding another job's identity proves nothing. The
/// lookup refuses it, and the intent-to-claim publication refuses a changed request.
#[test]
fn a_record_that_embeds_another_job_is_never_its_own_claim() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let job = crate::jobs::Job {
        id: "claimed".into(),
        cmd: vec!["true".into()],
        cwd: "/".into(),
        host: "local".into(),
        owner: "owner".into(),
        pane: "pane".into(),
        state: crate::jobs::PENDING.into(),
        exit: None,
        signal: None,
        reason: None,
        started_ms: None,
        ended_ms: None,
        timeout_ms: None,
        idle_timeout_ms: None,
        validity: None,
        startup: None,
        output_expired: false,
    };
    // A.json holds B's identity.
    let mut other = job.clone();
    other.id = "other".into();
    crate::jobs::save(&ledger, &other).unwrap();
    std::fs::rename(
        crate::jobs::store(&ledger).dir.join("other.json"),
        crate::jobs::store(&ledger).dir.join("claimed.json"),
    )
    .unwrap();
    // The lookup refuses the contradictory identity.
    let error = crate::jobs::ownership(&ledger, "claimed").unwrap_err();
    assert!(
        error.to_string().contains("belongs to job other"),
        "{error}"
    );
    // No publication takes that record over.
    let error = crate::jobs::publish_intent(&ledger, &job).unwrap_err();
    assert!(
        error.to_string().contains("belongs to job other"),
        "{error}"
    );
    // A real job's intent keeps its own request: only the pane may change.
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    crate::jobs::publish_intent(&ledger, &job).unwrap();
    let mut changed = job.clone();
    changed.cwd = "/elsewhere".into();
    let error = crate::jobs::publish_claim(&ledger, &changed).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("changed after its intent was published"),
        "{error}"
    );
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

/// A job launch receipt keeps the request it dispatched. The receipt alone carries a replay,
/// so the prune of the job record does not break it, and a receipt for another command or a
/// receipt without the evidence fails closed.
#[test]
fn a_job_launch_receipt_carries_its_own_request_evidence() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    std::fs::create_dir_all(&ledger.dir).unwrap();
    let launch = crate::work::Launch {
        preparation_protocol: None,
        preparation: None,
        id: "replayed".into(),
        unit: None,
        action: Action::Job {
            cmd: vec!["true".into()],
            timeout_ms: None,
            idle_timeout_ms: None,
            validity: None,
        },
        host: "local".into(),
        cwd: "/work".into(),
        pane: None,
        terminal: None,
        workspace: None,
        workspace_name: None,
        owner: "owner".into(),
        owner_host: None,
    };
    let cmd = vec!["true".to_owned()];
    let saved = json!({
        "id":"replayed","host":"local","pane":"w2:p1",
        "request":{"cmd":["true"],"cwd":"/work","owner":"owner",
            "timeout_ms":null,"idle_timeout_ms":null}
    });
    // The receipt correlates to the request it made.
    assert!(work::correlate(&ledger, &launch, &cmd, None, None, &saved).is_ok());
    // The prune removed the job record. The receipt still carries the replay: the record is
    // a second check, not a prerequisite.
    assert!(crate::jobs::record(&ledger, "replayed").unwrap().is_none());
    assert!(work::correlate(&ledger, &launch, &cmd, None, None, &saved).is_ok());
    // A receipt that names another command is refused.
    let mut other = saved.clone();
    other["request"]["cmd"] = json!(["false"]);
    let error = work::correlate(&ledger, &launch, &cmd, None, None, &other).unwrap_err();
    assert!(
        error.to_string().contains("another command or request"),
        "{error}"
    );
    // A recordless receipt without a complete pane is refused.
    for pane in [json!(null), json!(""), json!(7)] {
        let mut incomplete = saved.clone();
        incomplete["pane"] = pane;
        let error = work::correlate(&ledger, &launch, &cmd, None, None, &incomplete).unwrap_err();
        assert!(error.to_string().contains("without a pane"), "{error}");
    }
    let mut missing = saved.clone();
    missing.as_object_mut().unwrap().remove("pane");
    let error = work::correlate(&ledger, &launch, &cmd, None, None, &missing).unwrap_err();
    assert!(error.to_string().contains("without a pane"), "{error}");
    // A receipt that predates the evidence is refused.
    let legacy = json!({"id":"replayed","host":"local","pane":"w2:p1"});
    let error = work::correlate(&ledger, &launch, &cmd, None, None, &legacy).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("legacy launch receipt without request evidence"),
        "{error}"
    );
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

/// A reservation for an expired request changes nothing. The capacity record is
/// byte-identical, with or without an existing hold, and the request is never reserved.
#[test]
fn a_reservation_for_an_expired_request_changes_nothing() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    std::fs::create_dir_all(&ledger.dir).unwrap();
    let shell = crate::shell::Fake::new();
    let policy = Policy {
        memory_reserve_bytes: 0,
        ..Policy::default()
    };
    let node = crate::node::Node {
        shell: &shell,
        ledger: &ledger,
        thresholds: Thresholds::from(&policy),
        home: "/",
        cleanup: &policy.cleanup,
    };
    let now = crate::jobs::VALIDITY_WINDOW_MS + 1_000_000_u64;
    ledger
        .write(
            "host.json",
            &crate::node::Snapshot {
                sampled_ms: now,
                jobs: Vec::new(),
                probe: Probe {
                    host: "local".into(),
                    label: "local".into(),
                    load1: 0.0,
                    cores: 8,
                    cpu_ticks: None,
                    busy: Some(0.0),
                    memory: Memory::default(),
                    disk_free_percent: 50.0,
                    agents: AgentCounts::default(),
                    agent_panes: Vec::new(),
                    reservations: Vec::new(),
                    panes: Vec::new(),
                },
            },
        )
        .unwrap();
    // The request is older than the validity window.
    let expired = crate::jobs::Validity::new(now - crate::jobs::VALIDITY_WINDOW_MS - 1);
    // The clock file is written directly, so the test controls the host time.
    std::fs::write(
        ledger.dir.join("job-validity-clock.json"),
        serde_json::to_vec(&crate::jobs::Clock {
            version: crate::jobs::Clock::VERSION,
            high_water_ms: now,
        })
        .unwrap(),
    )
    .unwrap();
    // Without an existing hold, the refusal changes no capacity record.
    let error = node
        .reserve("expired", now, Some(expired))
        .expect_err("an expired request is refused");
    assert!(error.to_string().contains("older than 30 days"), "{error}");
    assert!(
        !ledger.dir.join("placements.json").exists(),
        "the refused reservation wrote no capacity record"
    );
    // With an existing hold, the refusal leaves the hold untouched.
    let holds = vec![crate::ledger::Reservation {
        id: "expired".into(),
        created_ms: now,
    }];
    ledger.write("placements.json", &holds).unwrap();
    let before = std::fs::read(ledger.dir.join("placements.json")).unwrap();
    let error = node
        .reserve("expired", now, Some(expired))
        .expect_err("an expired request is refused");
    assert!(error.to_string().contains("older than 30 days"), "{error}");
    let after = std::fs::read(ledger.dir.join("placements.json")).unwrap();
    assert_eq!(after, before, "the refused reservation changed the hold");
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

/// A missing clock with a local record that cannot be read refuses. The bootstrap cannot
/// account for a record it cannot read, so it never starts the clock behind it.
#[test]
fn a_missing_clock_with_an_unreadable_record_refuses() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    std::fs::create_dir_all(&ledger.dir).unwrap();
    // A record that cannot be parsed.
    let store = crate::jobs::store(&ledger);
    std::fs::create_dir_all(&store.dir).unwrap();
    std::fs::write(store.dir.join("broken.json"), "not json").unwrap();
    let error = crate::jobs::clock(&ledger).expect_err("the bootstrap refuses");
    assert!(
        error.to_string().contains("clock is unavailable"),
        "{error}"
    );
    // No clock was written.
    assert!(!ledger.dir.join("job-validity-clock.json").exists());
    // A missing record directory is empty, and the clock starts at the wall clock.
    let empty = Ledger::new(std::env::temp_dir().join(new_id()));
    std::fs::create_dir_all(&empty.dir).unwrap();
    assert!(crate::jobs::clock(&empty).is_ok());
    std::fs::remove_dir_all(&ledger.dir).unwrap();
    std::fs::remove_dir_all(&empty.dir).unwrap();
}

/// The host's validity clock never moves back. A host clock that moves back cannot reopen
/// an expired request, and the refusal is recorded at the time it was refused.
#[test]
fn the_validity_clock_never_moves_back() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    std::fs::create_dir_all(&ledger.dir).unwrap();
    let created = 1_000_000_u64;
    let validity = crate::jobs::Validity::new(created);
    // The clock starts at the wall clock, which the test cannot move. The clock file is
    // read and written directly here, so the test controls the host time.
    let clock_file = ledger.dir.join("job-validity-clock.json");
    let write = |high_water_ms: u64| {
        std::fs::write(
            &clock_file,
            serde_json::to_vec(&crate::jobs::Clock {
                version: crate::jobs::Clock::VERSION,
                high_water_ms,
            })
            .unwrap(),
        )
        .unwrap();
    };
    // A request 30 days old is still valid, and one millisecond more is not.
    let at = created + crate::jobs::VALIDITY_WINDOW_MS;
    write(at);
    assert!(crate::jobs::check_validity_at(&ledger, Some(&validity), at).is_ok());
    write(at + 1);
    let error = crate::jobs::check_validity_at(&ledger, Some(&validity), at + 1).unwrap_err();
    assert!(error.to_string().contains("older than 30 days"), "{error}");
    // The refusal advanced the clock, and the clock file holds the later time.
    let saved: crate::jobs::Clock =
        serde_json::from_slice(&std::fs::read(&clock_file).unwrap()).unwrap();
    assert!(saved.high_water_ms > created + crate::jobs::VALIDITY_WINDOW_MS);
    // A request without evidence never starts new execution.
    let error = crate::jobs::check_validity_at(&ledger, None, at + 1).unwrap_err();
    assert!(
        error.to_string().contains("validity is unprovable"),
        "{error}"
    );
    // A request from the future is refused.
    let ahead = crate::jobs::Validity::new(u64::MAX / 2);
    let error = crate::jobs::check_validity_at(&ledger, Some(&ahead), at + 1).unwrap_err();
    assert!(
        error.to_string().contains("ahead of this host clock"),
        "{error}"
    );
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

/// A custody record that cannot be written after the child started is a diagnostic only.
/// The launch result is returned, and the child stays recorded.
#[test]
fn a_custody_failure_after_the_launch_keeps_the_launch_result() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
    let item = Item {
        id: "child".into(),
        kind: "agent".into(),
        host: "local".into(),
        pane: "child".into(),
        owner_pane: "owner".into(),
        owner_session: Some("owner-session".into()),
        created_ms: 100,
        ..Item::default()
    };
    // The custody file holds something that is not a custody record, so the record cannot
    // be read and cannot be written back.
    std::fs::create_dir_all(&ledger.dir).unwrap();
    std::fs::write(ledger.dir.join("custody.json"), b"not a custody record").unwrap();
    // The custody record cannot be read or written here.
    assert!(
        crate::custody::ensure(&ledger, &item).is_err(),
        "the custody store must fail here"
    );
    let result = work::record_child(&ledger, &item, json!({"pane":"child"}));
    let result = result.expect("a custody failure must not fail the launch");
    assert_eq!(result["pane"], json!("child"));
    // The child is recorded, so the launch is not lost.
    let recorded = items::list(&ledger).unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].id, "child");
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

/// A launch retires its own pane only when the terminal identity is proven: the launch
/// recorded one, the pane is listed, and the listed terminal equals it. Missing,
/// unlisted, or different identity is never proof, so the pane stays open.
#[test]
fn retire_pane_closes_only_on_proven_terminal_identity() {
    let script = |listed: &str| {
        let shell = Fake::new();
        shell.on("herdr pane list", listed);
        shell.on("herdr agent list", r#"{"result":{"agents":[]}}"#);
        shell.on(
            "herdr pane process-info --pane pane_new",
            r#"{"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":10,"name":"zsh","argv":["zsh"]}]}}}"#,
        );
        shell.on("herdr pane close pane_new", "{}");
        shell
    };
    let closed = |shell: &Fake| {
        shell
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call == "herdr pane close pane_new")
    };
    let listed = r#"{"result":{"panes":[{"pane_id":"pane_new","terminal_id":"term_a"}]}}"#;
    let unlisted = r#"{"result":{"panes":[{"pane_id":"other","terminal_id":"term_a"}]}}"#;

    // Positive control: recorded and live identity are equal.
    let shell = script(listed);
    work::retire_pane(&shell, "pane_new", Some("term_a")).expect("proven identity retires");
    assert!(closed(&shell), "a proven identity closes the pane");

    for (what, listed, terminal) in [
        ("no recorded identity", listed, None),
        ("an empty recorded identity", listed, Some("")),
        ("a different live identity", listed, Some("term_b")),
        ("a pane that is not listed", unlisted, Some("term_a")),
    ] {
        let shell = script(listed);
        assert!(
            work::retire_pane(&shell, "pane_new", terminal).is_err(),
            "{what} must refuse"
        );
        assert!(!closed(&shell), "{what} must not close the pane");
    }
}
