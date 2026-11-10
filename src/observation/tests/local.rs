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
fn catalogue_failed_refresh_publishes_previous_repos_and_recovers_in_portal() {
    use super::super::{project, read};
    use crate::hosts::{Fleet, Host};
    let fixture = Fixture::new();
    let command = "gh api user/repos?per_page=100&affiliation=owner,collaborator,organization_member --paginate";
    fixture.shell.on(command, r#"[{"full_name":"owner/repo","description":"Kept","pushed_at":"2026-11-01T00:00:00Z","private":false,"archived":false}]"#);
    let fleet = Fleet::producer(
        &fixture.shell,
        &fixture.ledger,
        &fixture.policy,
        "/home".into(),
        vec![Host::local()],
    );
    let mut collector = Collector::default();
    let mut projector = project::Projector::default();
    let mut publisher = Publisher::new().unwrap();
    let mut publish = |collector: &mut Collector, now| {
        let mut input = collector.refresh(&fixture.node(), now, false).input;
        let captured = projector.inputs(&fleet, &input);
        project::derive(
            &captured,
            collector.reports(),
            &fixture.node().thresholds,
            &mut input,
        )
        .unwrap();
        let frame = publisher.replace(input).unwrap();
        let list = read::view::<portal_types::agents::RepoList>(&frame, "repos").unwrap();
        assert_eq!(
            frame.views["portal"]["data"]["repos"],
            serde_json::to_value(&list).unwrap(),
            "the same publication must deliver the catalogue to the mounted portal"
        );
        list
    };
    let first = publish(&mut collector, 100);
    assert_eq!(first.repos[0].slug, "owner/repo");
    fixture.shell.on_output(
        command,
        Output {
            status: 1,
            stdout: String::new(),
            stderr: "GitHub authentication failed".into(),
        },
    );
    assert_eq!(publish(&mut collector, 3_600_099), first);
    let failed = publish(&mut collector, 3_600_100);
    assert_eq!(failed.repos, first.repos);
    assert!(
        failed
            .error
            .unwrap()
            .contains("GitHub authentication failed")
    );
    fixture.shell.on(command, r#"[{"full_name":"owner/recovered","description":null,"pushed_at":null,"private":false,"archived":false}]"#);
    let retained = publish(&mut collector, 7_200_099);
    assert_eq!(retained.repos, first.repos);
    assert!(retained.error.is_some());
    let recovered = publish(&mut collector, 7_200_100);
    assert_eq!(recovered.repos[0].slug, "owner/recovered");
    assert_eq!(recovered.error, None);
    let calls = fixture.shell.calls.lock().unwrap().len();
    let mut peer_only = collector.publication(10_800_101);
    let local = projector.inputs(&fleet, &peer_only);
    project::derive(
        &local,
        collector.reports(),
        &fixture.node().thresholds,
        &mut peer_only,
    )
    .unwrap();
    assert_eq!(
        peer_only.views["repos"]["data"],
        serde_json::to_value(&recovered).unwrap()
    );
    assert_eq!(
        fixture.shell.calls.lock().unwrap().len(),
        calls,
        "peer wake must not fetch after the hourly boundary"
    );
    assert_eq!(
        fixture
            .shell
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.as_str() == command)
            .count(),
        3
    );
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
    assert_eq!(input.local.len(), 7);
    assert_eq!(
        input.local["terminals"]["agents"],
        input.local["host"]["probe"]["agent_panes"]
    );
    assert_eq!(
        input.local["terminals"]["panes"],
        input.local["host"]["probe"]["panes"]
    );
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
fn collection_prunes_expired_reservations_without_rewriting_live_rows() {
    let fixture = Fixture::new();
    let reservations = [
        crate::ledger::Reservation {
            id: "expired".into(),
            created_ms: 0,
        },
        crate::ledger::Reservation {
            id: "live".into(),
            created_ms: 100_000,
        },
    ];
    fixture.seed_job("live");
    crate::job_store::put_reservations(&fixture.ledger, &reservations).unwrap();
    // Audit real table deletes: rewriting a live row can resurrect a release committed
    // between the collector's read and write, even though its final value is identical.
    crate::store::write(&fixture.ledger, |tx| {
        tx.execute_batch(
            "CREATE TABLE reservation_deletes (id TEXT NOT NULL);
             CREATE TRIGGER audit_reservation_delete AFTER DELETE ON placements
             BEGIN INSERT INTO reservation_deletes VALUES (OLD.id); END;",
        )?;
        Ok(())
    })
    .unwrap();
    let mut collector = Collector::default();
    let collection = collector.collect(&fixture.node(), 100_100);
    collection.sample.unwrap();
    assert_eq!(collection.input.gaps, []);
    assert_eq!(
        collection.input.local["host"]["probe"]["reservations"],
        json!([reservations[1]])
    );
    assert_eq!(
        crate::job_store::reservations(&fixture.ledger).unwrap(),
        reservations[1..]
    );
    let deleted = crate::store::read(&fixture.ledger, |tx| {
        let mut statement = tx.prepare("SELECT id FROM reservation_deletes ORDER BY id")?;
        Ok(statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    })
    .unwrap();
    assert_eq!(
        deleted,
        ["expired"],
        "collection must not rewrite live reservations"
    );
}

#[test]
fn reservation_pruning_preserves_committed_clears_releases_and_creations() {
    for clear in [false, true] {
        let fixture = Fixture::new();
        let now = 100_100;
        let old = [
            crate::ledger::Reservation {
                id: "released".into(),
                created_ms: 100_000,
            },
            crate::ledger::Reservation {
                id: "renewed".into(),
                created_ms: 0,
            },
        ];
        crate::job_store::put_reservations(&fixture.ledger, &old).unwrap();
        let read = fixture
            .ledger
            .reservations(now, fixture.policy.reservation_ttl_ms)
            .unwrap();
        assert_eq!(read, old[..1]);
        let committed = [
            crate::ledger::Reservation {
                id: "renewed".into(),
                created_ms: now,
            },
            crate::ledger::Reservation {
                id: "new".into(),
                created_ms: now,
            },
            crate::ledger::Reservation {
                id: "expired".into(),
                created_ms: 0,
            },
        ];
        // Finish the normal mutation path on another thread after the collector's read
        // and before its prune. No sleeps, timing assumptions, or production test hooks.
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    if clear {
                        crate::job_store::put_reservations(&fixture.ledger, &[]).unwrap();
                    } else {
                        crate::job_store::release_reservation(&fixture.ledger, "released").unwrap();
                    }
                    crate::job_store::put_reservations(&fixture.ledger, &committed).unwrap();
                })
                .join()
                .unwrap();
        });
        fixture.node().prune_reservations(now).unwrap();
        assert_eq!(
            crate::job_store::reservations(&fixture.ledger).unwrap(),
            committed[..2]
        );
        fixture.seed_job("renewed");
        fixture.seed_job("new");
        let collection = Collector::default().collect(&fixture.node(), now);
        collection.sample.unwrap();
        assert_eq!(collection.input.gaps, []);
        let frame = Publisher::new().unwrap().replace(collection.input).unwrap();
        assert_eq!(
            frame.local["host"]["probe"]["reservations"],
            json!(committed[..2])
        );
        assert!(
            !frame.local["host"]["holds"]
                .as_array()
                .unwrap()
                .iter()
                .any(|hold| hold["request_id"] == "released")
        );
    }
}

#[test]
fn reservation_commits_refresh_probe_holds_and_headroom_without_resampling() {
    let fixture = Fixture::new();
    let mut collector = Collector::default();
    let mut publisher = Publisher::new().unwrap();
    collector.collect(&fixture.node(), 100).sample.unwrap();
    fixture
        .shell
        .on("read /proc/stat", "cpu 110 20 30 500 50 6 7 8 9 10");
    let original = publisher
        .replace(collector.collect(&fixture.node(), 120).input)
        .unwrap();
    let calls = fixture.shell.calls.lock().unwrap().len();
    let reservations = [crate::ledger::Reservation {
        id: "committed".into(),
        created_ms: 150,
    }];
    fixture.seed_job("committed");
    crate::job_store::put_reservations(&fixture.ledger, &reservations).unwrap();
    let created = publisher
        .replace(collector.refresh(&fixture.node(), 200, false).input)
        .unwrap();
    assert_eq!(
        created.local["host"]["probe"]["reservations"],
        json!(reservations)
    );
    assert!(
        created.local["headroom"]["headroom"].as_f64().unwrap()
            < original.local["headroom"]["headroom"].as_f64().unwrap()
    );
    assert!(
        created.local["host"]["holds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|hold| hold["code"] == "capacity")
    );
    crate::job_store::put_reservations(&fixture.ledger, &[]).unwrap();
    let released = publisher
        .replace(collector.refresh(&fixture.node(), 300, false).input)
        .unwrap();
    assert_eq!(released.local["host"]["probe"]["reservations"], json!([]));
    assert_eq!(released.local["headroom"], original.local["headroom"]);
    assert!(
        !released.local["host"]["holds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|hold| hold["code"] == "capacity")
    );
    assert_eq!(crate::job_store::reservations(&fixture.ledger).unwrap(), []);
    assert_eq!(released.local["host"]["sampled_ms"], 120);
    assert_eq!(fixture.shell.calls.lock().unwrap().len(), calls);
    assert_eq!(crate::node::samples(&fixture.ledger).unwrap().len(), 2);
    assert_eq!(original.local["host"]["probe"]["reservations"], json!([]));
    assert_eq!(
        created.local["host"]["probe"]["reservations"],
        json!(reservations)
    );
}

#[test]
fn ledger_refresh_reuses_terminal_sample_and_preserves_failed_sample_gaps() {
    let fixture = Fixture::new();
    let mut collector = Collector::default();
    let original = collector.collect(&fixture.node(), 100).input;
    let calls = fixture.shell.calls.lock().unwrap().len();
    fixture.seed_job("new-job");
    let refreshed = collector.refresh(&fixture.node(), 200, false).input;
    assert_eq!(refreshed.local["host"]["sampled_ms"], 100);
    assert_eq!(refreshed.local["host"]["jobs"][0]["id"], "new-job");
    assert_eq!(
        refreshed.local["host"]["probe"],
        original.local["host"]["probe"]
    );
    assert_eq!(fixture.shell.calls.lock().unwrap().len(), calls);
    assert_eq!(crate::node::samples(&fixture.ledger).unwrap().len(), 1);
    fixture.fail_probe();
    let failed = collector.collect(&fixture.node(), 300).input;
    let calls = fixture.shell.calls.lock().unwrap().len();
    assert_eq!(
        failed.sources.local["terminals"],
        original.sources.local["terminals"]
    );
    let terminal_failure = failed
        .gaps
        .iter()
        .find(|gap| gap.domain == "terminals")
        .unwrap();
    let ledger_only = collector.refresh(&fixture.node(), 400, false).input;
    assert_eq!(
        ledger_only
            .gaps
            .iter()
            .find(|gap| gap.domain == "terminals")
            .unwrap(),
        terminal_failure
    );
    assert_eq!(ledger_only.local, failed.local);
    for domain in ["host", "status", "headroom", "terminals"] {
        assert_eq!(
            ledger_only.sources.local[domain],
            failed.sources.local[domain]
        );
        assert!(ledger_only.gaps.iter().any(|gap| gap.domain == domain));
    }
    assert_eq!(fixture.shell.calls.lock().unwrap().len(), calls);
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
    for domain in ["host", "headroom", "status", "terminals"] {
        assert_eq!(failed.sources.local[domain], old.sources.local[domain]);
    }
    assert_eq!(failed.sources.local["todos"], 200);
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
    assert_eq!(recovered.sources.local["repos"], 100);
    assert!(
        recovered
            .sources
            .local
            .iter()
            .filter(|(domain, _)| domain.as_str() != "repos")
            .all(|(_, time)| *time == 300)
    );
    assert_eq!(recovered.local["host"]["sampled_ms"], 300);
}

#[test]
fn cold_collection_failure_is_not_empty_success() {
    let fixture = Fixture::new();
    fixture.fail_probe();
    let collection = Collector::default().collect(&fixture.node(), 100);
    assert!(collection.sample.is_err());
    let input = collection.input;
    assert_eq!(input.local.len(), 3);
    assert_eq!(input.local["todos"], json!([]));
    assert_eq!(input.local["sidebar_preferences"], json!({}));
    assert_eq!(
        input.sources.local,
        BTreeMap::from([
            ("todos".into(), 100),
            ("sidebar_preferences".into(), 100),
            ("repos".into(), 100)
        ])
    );
    assert_eq!(input.gaps.len(), 4);
    for domain in ["host", "headroom", "status", "terminals"] {
        assert!(input.gaps.iter().any(|gap| gap.domain == domain));
    }
}

#[test]
fn preferences_stay_fresh_when_host_capture_fails() {
    let fixture = Fixture::new();
    let mut collector = Collector::default();
    collector.collect(&fixture.node(), 100).sample.unwrap();
    let saved = crate::sidebar::model::Saved {
        focus: crate::sidebar::model::Panel::Todos,
        collapsed: vec![crate::sidebar::model::Panel::Agents],
        fold_open: true,
        selected_card: Some("child".into()),
    };
    crate::sidebar::model::save(&fixture.ledger, "owner", &saved).unwrap();
    fixture.fail_probe();
    let collection = collector.collect(&fixture.node(), 200);
    assert!(collection.sample.is_err());
    assert_eq!(collection.input.sources.local["host"], 100);
    assert_eq!(collection.input.sources.local["sidebar_preferences"], 200);
    assert_eq!(
        collection.input.local["sidebar_preferences"]["owner"],
        serde_json::to_value(saved).unwrap()
    );
    assert!(
        !collection
            .input
            .gaps
            .iter()
            .any(|gap| gap.domain == "sidebar_preferences")
    );
}

#[test]
fn failed_preferences_capture_retains_last_value_without_failing_other_domains() {
    let fixture = Fixture::new();
    let saved = crate::sidebar::model::Model::default().saved();
    crate::sidebar::model::save(&fixture.ledger, "owner", &saved).unwrap();
    let mut collector = Collector::default();
    let original = collector.collect(&fixture.node(), 100);
    original.sample.unwrap();
    crate::store::write(&fixture.ledger, |tx| {
        tx.execute(
            "ALTER TABLE sidebar_state RENAME TO unavailable_sidebar_state",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let failed = collector.collect(&fixture.node(), 200);
    failed.sample.unwrap();
    assert_eq!(
        failed.input.local["sidebar_preferences"],
        original.input.local["sidebar_preferences"]
    );
    assert_eq!(failed.input.sources.local["sidebar_preferences"], 100);
    assert_eq!(failed.input.sources.local["host"], 200);
    assert_eq!(failed.input.sources.local["todos"], 200);
    assert_eq!(failed.input.gaps.len(), 1);
    assert_eq!(failed.input.gaps[0].domain, "sidebar_preferences");
    assert_eq!(
        failed.input.gaps[0].availability,
        super::super::Availability::Failed
    );
}

#[test]
fn failed_host_capture_still_imports_and_publishes_todos() {
    let fixture = Fixture::new();
    fixture.seed_todo();
    crate::store::write(&fixture.ledger, |tx| {
        tx.execute(
            "ALTER TABLE service_lanes RENAME TO unavailable_service_lanes",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(crate::board::cached_todos(&fixture.ledger).unwrap(), []);
    let collection = Collector::default().collect(&fixture.node(), 100);
    collection.sample.unwrap();
    let mut publisher = Publisher::new().unwrap();
    let frame = publisher.replace(collection.input).unwrap();
    assert_eq!(frame.local.len(), 4);
    assert_eq!(frame.local["terminals"]["agents"][0]["pane"], "worker");
    assert_eq!(frame.local["todos"][0]["revision"], 7);
    assert_eq!(frame.local["todos"][0]["pane"], "worker");
    assert_eq!(
        frame.sources.local,
        BTreeMap::from([
            ("todos".into(), 100),
            ("terminals".into(), 100),
            ("sidebar_preferences".into(), 100),
            ("repos".into(), 100)
        ])
    );
    assert_eq!(frame.gaps.len(), 3);
    for domain in ["host", "headroom", "status"] {
        let gap = frame.gaps.iter().find(|gap| gap.domain == domain).unwrap();
        assert_eq!(gap.availability, super::super::Availability::Failed);
        assert!(gap.detail.contains("no such table: service_lanes"));
    }
    assert_eq!(
        crate::board::cached_todos(&fixture.ledger).unwrap().len(),
        1
    );
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
    let endpoint = super::super::socket_path(&state, "observation.sock").unwrap();
    std::fs::write(&endpoint, b"not a socket").unwrap();
    assert!(server.bind(publisher.subscribe()).is_err());
    assert_eq!(std::fs::read(&endpoint).unwrap(), b"not a socket");
    assert!(ipc::Server::new(&state).is_err());
    assert!(
        ipc::Client::at(&state)
            .current()
            .unwrap_err()
            .to_string()
            .contains("unavailable")
    );
    drop(server);
    assert_eq!(std::fs::read(&endpoint).unwrap(), b"not a socket");
    let owner = ipc::Server::new(&state).unwrap();
    drop(owner);
    assert_eq!(std::fs::read(&endpoint).unwrap(), b"not a socket");
    std::fs::remove_file(endpoint).unwrap();
}

#[test]
fn failed_local_capture_keeps_inventory_and_peer_only_composition_immutable() {
    use super::super::{Availability, project};
    use crate::hosts::{Fleet, Host};
    use std::sync::Arc;
    for warm in [false, true] {
        let fixture = Fixture::new();
        fixture.seed_job("first");
        let mut collector = Collector::default();
        if warm {
            collector.collect(&fixture.node(), 100);
        }
        fixture.fail_probe();
        let mut input = collector.collect(&fixture.node(), 30_101).input;
        let fleet = Fleet::cold(
            &fixture.shell,
            &fixture.ledger,
            &fixture.policy,
            "/home".into(),
            vec![Host::local()],
        );
        let mut projector = project::Projector::default();
        let captured = projector.inputs(&fleet, &input);
        project::derive(
            &captured,
            collector.reports(),
            &fixture.node().thresholds,
            &mut input,
        )
        .unwrap();
        let report = &input.views["status"]["data"]["hosts"][0];
        assert_eq!(report["service"], "unavailable");
        assert!(
            input
                .gaps
                .iter()
                .any(|gap| gap.domain == "host" && gap.availability == Availability::Failed)
        );
        assert!(!collector.reports()[0].fresh(30_101));
        if warm {
            assert_eq!(report["sampled_ms"], 100);
            assert_eq!(report["headroom"]["eligible"], false);
            assert!(!report["probe"].is_null());
        } else {
            assert!(report["sampled_ms"].is_null());
            assert!(report["probe"].is_null());
            assert!(report["headroom"].is_null());
        }
        assert!(
            crate::plan::choose(
                &collector
                    .reports()
                    .iter()
                    .filter_map(|report| report.headroom.clone())
                    .collect::<Vec<_>>()
            )
            .is_none()
        );
        let original_inventory = input.views["inventory"].clone();
        // A newer ledger row must not leak into a peer-only composition.
        crate::items::record(
            &fixture.ledger,
            crate::items::Item {
                id: "later".into(),
                kind: "agent".into(),
                host: "local".into(),
                pane: "later".into(),
                owner_pane: "owner".into(),
                ..crate::items::Item::default()
            },
        )
        .unwrap();
        let calls = fixture.shell.calls.lock().unwrap().len();
        fixture
            .ledger
            .with_lock("state-db", || {
                let mut peer_only = collector.publication(40_000);
                let reused = projector.inputs(&fleet, &peer_only);
                assert!(Arc::ptr_eq(&captured, &reused));
                project::derive(
                    &reused,
                    collector.reports(),
                    &fixture.node().thresholds,
                    &mut peer_only,
                )?;
                assert_eq!(peer_only.views["inventory"], original_inventory);
                Ok(())
            })
            .unwrap();
        assert_eq!(fixture.shell.calls.lock().unwrap().len(), calls);
    }
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
