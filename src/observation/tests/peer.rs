use super::super::{Availability, Domains, Publication, Publisher, Sources, ipc, peers};
use crate::config::Policy;
use crate::drop::Checked;
use crate::hosts::{Fleet, Host};
use crate::ledger::Ledger;
use crate::node::Node;
use crate::shell::{Fake, Output};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

struct Fixture {
    ledger: Ledger,
    shell: Fake,
    policy: Policy,
    host: Host,
}

impl Fixture {
    fn new() -> Self {
        Self {
            ledger: Ledger::new(std::env::temp_dir().join(crate::ledger::new_id())),
            shell: Fake::new(),
            policy: Policy::default(),
            host: Host {
                id: "peer-id".into(),
                label: "peer-label".into(),
                target: Some("peer-ssh".into()),
            },
        }
    }

    fn fleet(&self) -> Fleet<'_> {
        Fleet {
            shell: &self.shell,
            ledger: &self.ledger,
            policy: &self.policy,
            home: "/home".into(),
            hosts: vec![self.host.clone()],
        }
    }

    fn request(&self, request: &peers::Request) -> String {
        let args = [
            "observation".into(),
            "--request".into(),
            serde_json::to_string(request).unwrap(),
        ];
        format!(
            "ssh {}",
            self.fleet().remote_argv(&self.host, &args).join(" ")
        )
    }

    fn answer(&self, export: &peers::Export) {
        self.shell.on(
            &self.request(&peers::Request::default()),
            &serde_json::to_string(export).unwrap(),
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.ledger.dir);
    }
}

fn input(value: u64) -> Publication {
    let host = json!({"sampled_ms":value,"running":true,"probe":{
        "host":"local", "label":"local", "load1":0.0, "cores":8,
        "cpu_ticks":null, "busy":0.0, "memory":{"used_bytes":0,"total_bytes":100},
        "disk_free_percent":50.0, "agents":{"total":0,"by_status":{}},
        "agent_panes":[],"reservations":[],"panes":[]
    }, "handoffs":[], "holds":[], "jobs":[]});
    Publication {
        published_ms: value,
        sources: Sources {
            local: BTreeMap::from([("host".into(), value)]),
            peers: BTreeMap::new(),
        },
        local: BTreeMap::from([("host".into(), host)]),
        views: Domains::new(),
        gaps: Vec::new(),
    }
}

fn check() -> Checked {
    Checked {
        item: "child".into(),
        branch: crate::done::Branch {
            repo: "/execution-only/repo".into(),
            name: "retained".into(),
            ahead: 3,
            landed: false,
        },
    }
}

#[test]
fn export_has_no_fleet_projection_and_foreign_paths_survive() {
    let mut publisher = Publisher::new().unwrap();
    let mut original = input(100);
    original.local.insert(
        "branches".into(),
        serde_json::to_value([peers::BranchObservation {
            checked: check(),
            measured_ms: 100,
        }])
        .unwrap(),
    );
    original
        .local
        .insert("status".into(), json!({"foreign_items":["from-peer"]}));
    original
        .views
        .insert("fleet".into(), json!({"peer":"aggregated"}));
    let first = publisher.replace(original.clone()).unwrap();
    let export = peers::Export::of(&first);
    assert!(!export.local.contains_key("status"));
    assert!(!export.local.contains_key("fleet"));
    assert_eq!(
        export.local["branches"][0]["checked"]["branch"]["repo"],
        "/execution-only/repo"
    );
    original
        .local
        .insert("status".into(), json!({"foreign_items":["new-peer-value"]}));
    original
        .views
        .insert("fleet".into(), json!({"peer":"new-view"}));
    let second = publisher.replace(original).unwrap();
    assert!(second.revision > first.revision);
    assert_eq!(
        second.local_revision, first.local_revision,
        "peer-derived inventory must not feed back into local revisions"
    );
}

#[test]
fn bounded_peer_long_poll_ignores_fleet_only_publications() {
    let fixture = Fixture::new();
    let mut publisher = Publisher::new().unwrap();
    let mut server = ipc::Server::new(&fixture.ledger.dir).unwrap();
    server.bind(publisher.subscribe()).unwrap();
    let first = publisher.replace(input(100)).unwrap();
    let client = ipc::Client::at(&fixture.ledger.dir);
    let request = peers::Request {
        epoch: Some(first.epoch.clone()),
        local_revision: Some(first.local_revision),
        wait: true,
        ..peers::Request::default()
    };
    let (send, recv) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || send.send(client.peer(request)).unwrap());
    let mut peer_only = input(100);
    peer_only.views.insert("peer".into(), json!(200));
    publisher.replace(peer_only).unwrap();
    assert!(recv.recv_timeout(Duration::from_millis(100)).is_err());
    let next = publisher.replace(input(200)).unwrap();
    let export = recv.recv_timeout(Duration::from_secs(3)).unwrap().unwrap();
    assert_eq!(export.local_revision, next.local_revision);
    assert_eq!(export.measured["host"], 200);
    worker.join().unwrap();
}

#[test]
fn missing_mark_branch_observation_is_host_owned_and_guarded_on_import() {
    let fixture = Fixture::new();
    let checked = check();
    crate::items::record(
        &fixture.ledger,
        crate::items::Item {
            id: checked.item.clone(),
            kind: "agent".into(),
            host: "local".into(),
            pane: "gone".into(),
            owner_pane: "owner".into(),
            closed_ms: Some(1),
            branch: Some(checked.branch.clone()),
            ..crate::items::Item::default()
        },
    )
    .unwrap();
    for reference in ["refs/heads/retained", "refs/remotes/origin/retained"] {
        fixture.shell.on_output(
            &format!("git -C /execution-only/repo show-ref --verify --quiet {reference}"),
            Output {
                status: 1,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
    }
    let node = Node::new(&fixture.shell, &fixture.ledger, &fixture.policy, "/home");
    let mut branches = peers::Branches::default();
    assert_eq!(branches.observe(&node, &[], 100).unwrap(), []);
    let item = crate::items::list(&fixture.ledger).unwrap().remove(0);
    assert!(item.branch.as_ref().unwrap().landed);
    assert_eq!(item.branch_checked_ms, Some(100));
    assert!(
        crate::done::list(&fixture.ledger).unwrap().is_empty(),
        "no mark was invented"
    );
    assert_eq!(branches.results()[0].measured_ms, 100);
    let calls = fixture.shell.calls.lock().unwrap().len();
    branches
        .observe(&node, std::slice::from_ref(&checked), 200)
        .unwrap();
    assert_eq!(
        fixture.shell.calls.lock().unwrap().len(),
        calls,
        "interests share the hourly bound"
    );
    let mut replacement = item;
    replacement.branch = Some(crate::done::Branch {
        name: "replacement".into(),
        ..checked.branch
    });
    replacement.branch_checked_ms = None;
    crate::store::write(&fixture.ledger, |tx| {
        crate::tables::sync_items(tx, &[replacement])
    })
    .unwrap();
    peers::import(&fixture.ledger, "local", &branches.results()).unwrap();
    let item = crate::items::list(&fixture.ledger).unwrap().remove(0);
    assert_eq!(item.branch.unwrap().name, "replacement");
    assert_eq!(item.branch_checked_ms, None);
}

#[test]
fn failed_branch_observation_retains_success_and_measurement_time() {
    let fixture = Fixture::new();
    let checked = check();
    let missing = "git -C /execution-only/repo show-ref --verify --quiet refs/heads/retained";
    fixture.shell.on_output(
        missing,
        Output {
            status: 1,
            stdout: String::new(),
            stderr: String::new(),
        },
    );
    fixture.shell.on_output(
        "git -C /execution-only/repo show-ref --verify --quiet refs/remotes/origin/retained",
        Output {
            status: 1,
            stdout: String::new(),
            stderr: String::new(),
        },
    );
    let node = Node::new(&fixture.shell, &fixture.ledger, &fixture.policy, "/home");
    let mut branches = peers::Branches::default();
    branches
        .observe(&node, std::slice::from_ref(&checked), 100)
        .unwrap();
    let successful = branches.results();
    fixture.shell.on_output(
        missing,
        Output {
            status: 2,
            stdout: String::new(),
            stderr: "cannot observe branch".into(),
        },
    );
    let gaps = branches.observe(&node, &[checked], 3_600_100).unwrap();
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].availability, Availability::Failed);
    assert_eq!(branches.results(), successful);
    assert_eq!(branches.results()[0].measured_ms, 100);
    assert_eq!(
        branches.gaps(),
        gaps,
        "peer-only composition retains branch failures"
    );
    let mut cold = input(100);
    cold.gaps = gaps;
    let mut publisher = Publisher::new().unwrap();
    let export = peers::Export::of(&publisher.replace(cold).unwrap());
    assert_eq!(
        export.gaps.len(),
        1,
        "cold branch failures must be exported without a successful branches domain"
    );
}

#[test]
fn stale_peers_keep_original_measurement_and_cannot_become_eligible() {
    let fixture = Fixture::new();
    let mut publisher = Publisher::new().unwrap();
    let export = peers::Export::of(&publisher.replace(input(100)).unwrap());
    fixture.answer(&export);
    let fleet = fixture.fleet();
    let mut ingestion = peers::Ingestion::default();
    ingestion
        .refresh(&fleet, &fixture.host, 31_001, Vec::new(), false)
        .unwrap();
    assert_eq!(ingestion.source.unwrap().availability, Availability::Stale);
    let calls = fixture.shell.calls.lock().unwrap().len();
    let report = fleet.status(31_002).unwrap().remove(0);
    assert_eq!(report.sampled_ms, Some(100));
    assert!(!report.fresh(31_002));
    assert!(!report.headroom.unwrap().eligible);
    assert_eq!(
        fixture.shell.calls.lock().unwrap().len(),
        calls,
        "status must not refresh the peer"
    );
}

#[test]
fn unsupported_peer_export_does_not_downgrade_or_break_status() {
    let fixture = Fixture::new();
    let mut publisher = Publisher::new().unwrap();
    let mut export = peers::Export::of(&publisher.replace(input(100)).unwrap());
    export.version += 1;
    fixture.answer(&export);
    let fleet = fixture.fleet();
    let mut ingestion = peers::Ingestion::default();
    ingestion
        .refresh(&fleet, &fixture.host, 100, Vec::new(), false)
        .unwrap();
    assert_eq!(
        ingestion.source.unwrap().availability,
        Availability::Unsupported
    );
    let report = fleet.status(101).unwrap().remove(0);
    assert!(!report.reachable);
    assert_eq!(
        fixture.shell.calls.lock().unwrap().len(),
        1,
        "unsupported versions cannot fall back to an older protocol"
    );
}

#[test]
fn native_export_recovers_after_a_temporary_legacy_fallback() {
    let fixture = Fixture::new();
    let mut publisher = Publisher::new().unwrap();
    let original = peers::Export::of(&publisher.replace(input(100)).unwrap());
    fixture.answer(&original);
    let fleet = fixture.fleet();
    let mut ingestion = peers::Ingestion::default();
    ingestion
        .refresh(&fleet, &fixture.host, 100, Vec::new(), false)
        .unwrap();
    assert_eq!(ingestion.source.as_ref().unwrap().epoch, original.epoch);
    let native = fixture.request(&peers::Request {
        epoch: Some(original.epoch.clone()),
        local_revision: Some(original.local_revision),
        ..peers::Request::default()
    });
    fixture.shell.on_output(
        &native,
        Output {
            status: 2,
            stdout: json!({"error":"observation unavailable"}).to_string(),
            stderr: String::new(),
        },
    );
    let legacy = format!(
        "ssh {}",
        fleet
            .remote_argv(&fixture.host, &["snapshot".into()])
            .join(" ")
    );
    fixture
        .shell
        .on(&legacy, &input(10_100).local["host"].to_string());
    ingestion
        .refresh(&fleet, &fixture.host, 10_100, Vec::new(), false)
        .unwrap();
    assert_eq!(ingestion.source.as_ref().unwrap().epoch, "legacy");
    assert_eq!(fleet.status(10_100).unwrap()[0].sampled_ms, Some(10_100));
    let recovered = peers::Export::of(&publisher.replace(input(20_100)).unwrap());
    fixture
        .shell
        .on(&native, &serde_json::to_string(&recovered).unwrap());
    ingestion
        .refresh(&fleet, &fixture.host, 20_100, Vec::new(), false)
        .unwrap();
    assert_eq!(ingestion.source.as_ref().unwrap().epoch, recovered.epoch);
    assert_eq!(fleet.status(20_100).unwrap()[0].sampled_ms, Some(20_100));
    assert_eq!(
        fixture.shell.calls.lock().unwrap().as_slice(),
        &[
            fixture.request(&peers::Request::default()),
            native.clone(),
            legacy,
            native
        ]
    );
}

#[test]
fn failed_peer_refresh_never_doubles_ssh_and_keeps_original_failure() {
    for (status, stdout, stderr, failure) in [
        (
            255,
            "",
            "Connection refused",
            crate::hosts::Failure::ConnectionRefused,
        ),
        (
            2,
            r#"{"deadline":true,"error":"PRIVATE_REMOTE_DEADLINE"}"#,
            "",
            crate::hosts::Failure::Deadline,
        ),
    ] {
        let fixture = Fixture::new();
        let mut publisher = Publisher::new().unwrap();
        let original = peers::Export::of(&publisher.replace(input(100)).unwrap());
        fixture.answer(&original);
        let fleet = fixture.fleet();
        let mut ingestion = peers::Ingestion::default();
        ingestion
            .refresh(&fleet, &fixture.host, 100, Vec::new(), false)
            .unwrap();
        let native = fixture.request(&peers::Request {
            epoch: Some(original.epoch),
            local_revision: Some(original.local_revision),
            ..peers::Request::default()
        });
        fixture.shell.on_output(
            &native,
            Output {
                status,
                stdout: stdout.into(),
                stderr: stderr.into(),
            },
        );
        for (attempt, now) in [(1, 10_100), (2, 20_100)] {
            let before = fixture.shell.calls.lock().unwrap().len();
            ingestion
                .refresh(&fleet, &fixture.host, now, Vec::new(), false)
                .unwrap();
            let report = fleet.status(now).unwrap().remove(0);
            assert!(!report.reachable);
            assert_eq!(report.failure, Some(failure));
            assert_eq!(report.error, Some(failure.to_string()));
            assert_eq!(report.sampled_ms, Some(100));
            assert_eq!(fixture.shell.calls.lock().unwrap().len(), before + 1);
            let cached = crate::host_store::peer(&fixture.ledger, &fixture.host.id)
                .unwrap()
                .unwrap();
            assert_eq!(cached.retry.attempts, attempt);
            assert!(cached.retry.next_ms > now);
            assert_eq!(
                ingestion.source.as_ref().unwrap().availability,
                Availability::Unavailable
            );
        }
    }
}

#[test]
fn branch_interest_registration_does_not_run_git_and_is_idempotent() {
    let fixture = Fixture::new();
    let publisher = Publisher::new().unwrap();
    let mut server = ipc::Server::new(&fixture.ledger.dir).unwrap();
    server.bind(publisher.subscribe()).unwrap();
    let interests = peers::Interests::default();
    let checked = check();
    assert!(interests.register(std::slice::from_ref(&checked)).unwrap());
    assert!(!interests.register(std::slice::from_ref(&checked)).unwrap());
    assert_eq!(interests.current().unwrap(), vec![checked]);
    assert!(fixture.shell.calls.lock().unwrap().is_empty());
}
