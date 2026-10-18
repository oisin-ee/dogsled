//! Admission and reconciliation with injected timestamps and a scripted OS seam.
use crate::config::Policy;
use crate::hosts::{Fleet, Host};
use crate::items::{self, Item};
use crate::ledger::{Ledger, new_id};
use crate::node::Snapshot;
use crate::probe::{Agent, AgentCounts, Memory, Pane, Probe};
use crate::shell::Fake;
use crate::work::{self, Action, Request};

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
    work::tick(&fleet, 101).unwrap();
    assert!(work::queue(&ledger).unwrap()[0].notified);
    let told = crate::inbox::received(&ledger, "term_owner").unwrap();
    assert_eq!(told.len(), 1);
    let result = tickets[0].result.as_ref().unwrap();
    assert_eq!(
        told[0].text,
        format!(
            "wrangle: ticket {} admitted on local: {result}",
            tickets[0].id
        )
    );
    work::tick(&fleet, 102).unwrap();
    assert_eq!(
        crate::inbox::received(&ledger, "term_owner").unwrap().len(),
        1
    );
    assert_eq!(shell.calls.lock().unwrap().len(), 0);
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
    }
}

#[test]
fn submit_waits_behind_pending_tickets_even_with_room() {
    let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
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
