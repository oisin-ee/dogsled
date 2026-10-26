//! Admission and reconciliation with injected timestamps and a scripted OS seam.
use crate::config::Policy;
use crate::hosts::{Fleet, Host};
use crate::items::{self, Item};
use crate::ledger::{Ledger, new_id};
use crate::node::Snapshot;
use crate::probe::{Agent, AgentCounts, Memory, Pane, Probe};
use crate::shell::Fake;
use crate::work::{self, Action, Request};

mod reconcile;

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
