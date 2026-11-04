use super::super::{Publisher, collect::Collector, ipc};
use crate::config::Policy;
use crate::ledger::Ledger;
use crate::node::Node;
use crate::shell::{Fake, Output};
use serde_json::json;
use std::collections::BTreeMap;

struct Fixture {
    ledger: Ledger,
    shell: Fake,
    policy: Policy,
}

impl Fixture {
    fn new() -> Self {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        std::fs::create_dir_all(&ledger.dir).unwrap();
        let transcript = ledger.dir.join("session.jsonl");
        std::fs::write(
            &transcript,
            "{\"type\":\"session\",\"id\":\"local-session\"}\n{\"type\":\"model_change\",\"modelId\":\"local-model\"}\n",
        )
        .unwrap();
        let shell = Fake::new();
        shell
            .on("herdr pane list", r#"{"result":{"panes":[]}}"#)
            .on("read /proc/stat", "cpu 100 20 30 400 50 6 7 8 9 10")
            .on("read /proc/loadavg", "0.5 0.2 0.1 1/10 42")
            .on(
                "read /proc/meminfo",
                "MemTotal: 100 kB\nMemAvailable: 75 kB",
            )
            .on("df -Pk /home", "FS 1000 500 400 60% /");
        shell.on(
            "herdr agent list",
            &json!({"result":{"agents":[{
                "pane_id":"worker", "agent":"pi", "agent_status":"working",
                "terminal_id":"terminal", "name":"Local worker",
                "agent_session":{"kind":"path", "value":transcript}
            }]}})
            .to_string(),
        );
        Self {
            ledger,
            shell,
            policy: Policy {
                memory_reserve_bytes: 0,
                ..Policy::default()
            },
        }
    }

    fn node(&self) -> Node<'_> {
        Node::new(&self.shell, &self.ledger, &self.policy, "/home")
    }

    fn fail_probe(&self) {
        self.shell.on_output(
            "herdr agent list",
            Output {
                status: 1,
                stdout: String::new(),
                stderr: "agent source failed".into(),
            },
        );
    }

    fn seed_todo(&self) {
        let todo = crate::board::TodoSections {
            session: "local-session".into(),
            branch: Some("local-branch".into()),
            revision: 7,
            current: "Collect facts".into(),
            pane: "worker".into(),
            sections: ["in_progress", "pending", "completed", "abandoned"]
                .into_iter()
                .map(|status| {
                    serde_json::from_value(json!({"status":status,"count":0,"items":[]})).unwrap()
                })
                .collect(),
        };
        crate::store::write(&self.ledger, |tx| {
            crate::web_store::board::put_todo(tx, "published", &todo)
        })
        .unwrap();
    }

    fn seed_branch(&self) {
        let branch = crate::done::Branch {
            repo: "/execution-host/repository".into(),
            name: "retained-branch".into(),
            ahead: 3,
            landed: false,
        };
        let mark: crate::done::Mark = serde_json::from_value(json!({
            "pane":"closed-worker", "item":"child", "session":"closed-session",
            "marked_ms":10, "closed_ms":20, "branch":branch
        }))
        .unwrap();
        crate::host_store::put_marks(
            &self.ledger,
            &BTreeMap::from([(crate::done::key(&mark), mark)]),
        )
        .unwrap();
        crate::items::record(
            &self.ledger,
            crate::items::Item {
                id: "child".into(),
                kind: "agent".into(),
                host: "local".into(),
                pane: "closed-worker".into(),
                owner_pane: "owner".into(),
                closed_ms: Some(20),
                branch: Some(branch),
                ..crate::items::Item::default()
            },
        )
        .unwrap();
    }

    fn seed_job(&self, id: &str) {
        let job = serde_json::from_value(json!({
            "id":id,"cmd":["true"],"cwd":"/execution-host/job","host":"local",
            "owner":"owner","pane":"job-pane","state":"exited","exit":0,
            "started_ms":10,"ended_ms":20
        }))
        .unwrap();
        crate::jobs::save(&self.ledger, &job).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.ledger.dir).unwrap();
    }
}

#[test]
fn complete_local_facts_include_ownership_todos_capacity_and_diagnostics() {
    let fixture = Fixture::new();
    fixture.seed_todo();
    fixture.seed_branch();
    fixture.seed_job("job");
    crate::store::write(&fixture.ledger, |tx| {
        crate::tables::record_lane(tx, "handoff", Some("handoff unavailable"), 50)
    })
    .unwrap();
    let mut collector = Collector::default();
    let collection = collector.collect(&fixture.node(), 100);
    collection.sample.unwrap();
    let input = collection.input;
    assert_eq!(input.gaps, []);
    assert!(input.views.is_empty(), "fleet projections belong to O6");
    assert_eq!(input.local.len(), 4);
    assert!(input.sources.local.values().all(|time| *time == 100));
    let host = &input.local["host"];
    assert_eq!(host["sampled_ms"], 100);
    assert_eq!(host["running"], true);
    assert_eq!(host["jobs"][0]["id"], "job");
    assert_eq!(host["service_lanes"][0]["error"], "handoff unavailable");
    assert_eq!(host["job_validity"], crate::jobs::VALIDITY_PROTOCOL_VERSION);
    assert_eq!(
        host["request_validity"],
        crate::jobs::REQUEST_VALIDITY_PROTOCOL_VERSION
    );
    assert_eq!(host["placement_replay"], crate::node::PLACEMENT_VERSION);
    let agent = &host["probe"]["agent_panes"][0];
    assert_eq!(agent["pane"], "worker");
    assert_eq!(agent["session"]["model"], "local-model");
    assert_eq!(
        host["handoffs"][0]["branch"]["repo"],
        "/execution-host/repository"
    );
    assert_eq!(input.local["headroom"]["reason"], "no CPU sample yet");
    assert_eq!(input.local["headroom"]["eligible"], false);
    assert_eq!(input.local["todos"][0]["revision"], 7);
    let status = &input.local["status"];
    assert_eq!(status["items"][0]["id"], "child");
    let holds = status["holds"].as_array().unwrap();
    assert_eq!(
        holds
            .iter()
            .filter(|hold| hold["code"] == "retained_branch")
            .count(),
        1
    );
    assert_eq!(holds[0]["owner_pane"], "owner");
    assert_eq!(crate::node::samples(&fixture.ledger).unwrap().len(), 1);
    assert_eq!(
        fixture
            .shell
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| *call == "herdr agent list")
            .count(),
        1
    );
    fixture
        .shell
        .on("read /proc/stat", "cpu 110 20 30 500 50 6 7 8 9 10");
    let next = collector.collect(&fixture.node(), 200).input;
    assert_eq!(next.gaps, []);
    assert!(next.local["headroom"]["headroom"].is_number());
    assert!(next.local["host"]["probe"]["busy"].is_number());
}

#[test]
fn failed_collection_retains_values_and_times_then_recovers() {
    let fixture = Fixture::new();
    let mut collector = Collector::default();
    let mut publisher = Publisher::new().unwrap();
    let old = publisher
        .replace(collector.collect(&fixture.node(), 100).input)
        .unwrap();
    fixture.fail_probe();
    let collection = collector.collect(&fixture.node(), 200);
    assert!(collection.sample.is_err());
    let failed = publisher.replace(collection.input).unwrap();
    assert_eq!(failed.published_ms, 200);
    assert_eq!(failed.local, old.local);
    assert_eq!(failed.sources.local, old.sources.local);
    assert_eq!(failed.local_revision, old.local_revision);
    assert_eq!(failed.gaps.len(), 4);
    assert!(
        failed
            .gaps
            .iter()
            .all(|gap| gap.availability == super::super::Availability::Failed)
    );
    assert!(
        failed
            .gaps
            .iter()
            .all(|gap| gap.detail.contains("agent source failed"))
    );
    assert!(
        old.gaps.is_empty(),
        "failure cannot modify an older publication"
    );
    fixture
        .shell
        .on("herdr agent list", r#"{"result":{"agents":[]}}"#);
    let recovered = collector.collect(&fixture.node(), 300).input;
    assert_eq!(recovered.gaps, []);
    assert!(recovered.sources.local.values().all(|time| *time == 300));
    assert_eq!(recovered.local["host"]["sampled_ms"], 300);
}

#[test]
fn cold_collection_failure_is_not_empty_success() {
    let fixture = Fixture::new();
    fixture.fail_probe();
    let collection = Collector::default().collect(&fixture.node(), 100);
    assert!(collection.sample.is_err());
    let input = collection.input;
    assert!(input.local.is_empty());
    assert!(input.sources.local.is_empty());
    assert_eq!(input.gaps.len(), 4);
    for domain in ["host", "headroom", "status", "todos"] {
        assert!(input.gaps.iter().any(|gap| gap.domain == domain));
    }
}

#[test]
fn publication_freezes_inputs_and_readers_do_not_collect() {
    let fixture = Fixture::new();
    fixture.seed_job("first");
    let mut collector = Collector::default();
    let input = collector.collect(&fixture.node(), 100).input;
    fixture.seed_job("second");
    let mut publisher = Publisher::new().unwrap();
    let mut server = ipc::Server::new(&fixture.ledger.dir).unwrap();
    server.bind(publisher.subscribe()).unwrap();
    let frame = publisher.replace(input).unwrap();
    let calls = fixture.shell.calls.lock().unwrap().len();
    let client = ipc::Client::at(&fixture.ledger.dir);
    fixture
        .ledger
        .with_lock("state-db", || {
            for _ in 0..8 {
                let read = client.current()?;
                assert_eq!(read, *frame);
                assert_eq!(read.local["host"]["jobs"].as_array().unwrap().len(), 1);
                let subscription = client.subscribe()?;
                assert_eq!(*subscription.current().publication, *frame);
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(fixture.shell.calls.lock().unwrap().len(), calls);
    assert_eq!(crate::node::samples(&fixture.ledger).unwrap().len(), 1);
    let next = publisher
        .replace(collector.collect(&fixture.node(), 200).input)
        .unwrap();
    assert_eq!(next.local["host"]["jobs"].as_array().unwrap().len(), 2);
    assert_eq!(frame.local["host"]["jobs"].as_array().unwrap().len(), 1);
    drop(server);
}

#[test]
fn serve_refuses_an_existing_publisher_before_collecting() {
    let fixture = Fixture::new();
    let publisher = Publisher::new().unwrap();
    let mut server = ipc::Server::new(&fixture.ledger.dir).unwrap();
    server.bind(publisher.subscribe()).unwrap();
    let error = crate::service::serve(
        &fixture.node(),
        &fixture.policy,
        std::time::Duration::from_secs(1),
        true,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("observation publisher already owned")
    );
    assert!(fixture.shell.calls.lock().unwrap().is_empty());
    assert_eq!(crate::node::samples(&fixture.ledger).unwrap(), []);
    drop(server);
}

#[test]
fn failed_endpoint_bind_retains_exclusive_publication_ownership() {
    let fixture = Fixture::new();
    let state = fixture.ledger.dir.join("long-state-directory-".repeat(8));
    let publisher = Publisher::new().unwrap();
    let mut server = ipc::Server::new(&state).unwrap();
    assert!(server.bind(publisher.subscribe()).is_err());
    assert!(ipc::Server::new(&state).is_err());
    assert!(
        ipc::Client::at(&state)
            .current()
            .unwrap_err()
            .to_string()
            .contains("unavailable")
    );
    drop(server);
    let owner = ipc::Server::new(&state).unwrap();
    drop(owner);
}

#[test]
fn observation_without_serve_never_falls_back_to_persisted_facts() {
    let fixture = Fixture::new();
    Collector::default().collect(&fixture.node(), 100);
    let calls = fixture.shell.calls.lock().unwrap().len();
    let client = ipc::Client::at(&fixture.ledger.dir);
    assert!(
        client
            .current()
            .unwrap_err()
            .to_string()
            .contains("unavailable")
    );
    assert!(
        client
            .subscribe()
            .unwrap_err()
            .to_string()
            .contains("unavailable")
    );
    assert_eq!(fixture.shell.calls.lock().unwrap().len(), calls);
    assert_eq!(crate::node::samples(&fixture.ledger).unwrap().len(), 1);
    assert!(!fixture.ledger.dir.join("observation.sock").exists());
}
