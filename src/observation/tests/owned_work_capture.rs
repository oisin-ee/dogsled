use super::super::{collect, owned_work};
use crate::items::{self, Item};
use crate::jobs::{self, Job, Startup};
use crate::ledger::Ledger;
use crate::node::{Snapshot, SnapshotReport};
use crate::probe::{Agent, Probe};
use crate::work::{Action, Ticket};
use serde_json::{Value, json};

fn report(ledger: &Ledger, agents: Vec<Agent>) -> SnapshotReport {
    SnapshotReport {
        service_lanes: Vec::new(),
        snapshot: Some(Snapshot {
            sampled_ms: 12,
            probe: Probe {
                host: "local".into(),
                label: "local".into(),
                load1: 0.0,
                cores: 1,
                cpu_ticks: None,
                busy: None,
                memory: crate::probe::Memory::default(),
                disk_free_percent: 50.0,
                agents: crate::probe::AgentCounts::from_agents(&agents),
                agent_panes: agents,
                reservations: Vec::new(),
                panes: Vec::new(),
            },
            jobs: jobs::list(ledger).unwrap(),
        }),
        running: Some(true),
        handoffs: Vec::new(),
        job_validity: None,
        repo_open: None,
        request_validity: None,
        placement_replay: None,
        transfer: None,
        holds: Vec::new(),
    }
}

fn pending(id: &str, action: &Action) -> Ticket {
    serde_json::from_value(json!({
        "id": format!("ticket-{id}"), "created_ms": 1, "state": "pending", "waiting": true,
        "notified": false,
        "request": {"id": id, "action": action, "owner": "owner", "cwd": "/checkout",
            "repository": {"path": "/checkout", "origin": "git@github.com:oisin-ee/dogsled.git",
                "relative": "dev/dogsled", "working_root": "/checkout"}},
    }))
    .unwrap()
}

fn agent_action() -> Action {
    Action::Agent {
        kind: "pi".into(),
        name: "worker".into(),
        message: "private task".into(),
        profile: None,
    }
}

fn launch(ledger: &Ledger, id: &str, terminal: &str) {
    crate::launch_store::put_progress_value(
        ledger,
        id,
        &json!({
            "request": {"id": id, "action": agent_action(), "host": "local", "cwd": "/checkout",
                "owner": "owner", "owner_host": "origin-host"},
            "pane": "executing-pane", "terminal": terminal, "attempted": true, "confirmed": true,
            "result": {"pane": "executing-pane"},
        }),
    )
    .unwrap();
}

fn capture(
    ledger: &Ledger,
    report: &SnapshotReport,
    todos: &[crate::board::TodoSections],
) -> Value {
    let status = collect::status(ledger, Vec::new(), 12).unwrap();
    owned_work::serialize_host(
        ledger,
        report,
        Some(&status),
        Some(todos),
        &mut owned_work::WorkerClocks::default(),
    )
    .unwrap()
}

#[test]
fn autonomy_host_extension_only_exports_switches_for_live_sessions() {
    let ledger = Ledger::temporary();
    for (pane, session) in [("lead", "live"), ("replaced", "old"), ("closed", "gone")] {
        crate::autonomy::set_agent(&ledger, pane, session, true, 1).unwrap();
    }
    let report = report(
        &ledger,
        [("lead", "live"), ("replaced", "new")]
            .into_iter()
            .map(|(pane, session)| Agent {
                pane: pane.into(),
                agent_session: Some(session.into()),
                ..Agent::default()
            })
            .collect(),
    );
    assert_eq!(
        capture(&ledger, &report, &[])["autonomy"],
        json!({
            "version":1,"fleet":false,"agents":["lead"],"decisions":[]
        })
    );
}

#[test]
fn owned_work_wire_capture_deduplicates_admission_and_filters_origin() {
    let ledger = Ledger::temporary();
    let mut tickets = [
        pending("b-pending", &agent_action()),
        pending("a-admitted", &agent_action()),
        pending("place", &Action::Place),
        pending("landed", &agent_action()),
    ];
    tickets[1].request.branch = Some("feat/calendar".into());
    crate::store::write(&ledger, |tx| crate::tables::sync_tickets(tx, &tickets)).unwrap();
    let admitted = Item {
        id: "a-admitted".into(),
        kind: "agent".into(),
        host: "local".into(),
        pane: "worker-pane".into(),
        owner_pane: "owner".into(),
        owner_session: Some("origin-session".into()),
        name: Some("admitted worker".into()),
        ..Item::default()
    };
    let retained = Item {
        id: "c-retained".into(),
        closed_ms: Some(5),
        branch: Some(crate::done::Branch {
            repo: "/checkout".into(),
            name: "topic".into(),
            ahead: 2,
            landed: false,
        }),
        ..admitted.clone()
    };
    let landed = Item {
        id: "landed".into(),
        pane: "closed-pane".into(),
        branch: retained.branch.clone().map(|mut branch| {
            branch.landed = true;
            branch
        }),
        ..retained.clone()
    };
    for item in [retained, landed, admitted] {
        items::record(&ledger, item).unwrap();
    }
    let report = report(&ledger, Vec::new());
    let host = capture(&ledger, &report, &[]);
    let wire = &host["owned_work"];
    assert_eq!(wire["version"], 1);
    assert_eq!(wire["status"], "ready");
    assert_eq!(wire["hostname"], crate::inbox::hostname());
    assert_eq!(
        wire["owned"],
        json!([
            {"request_id":"a-admitted", "kind":"agent", "owner_pane":"owner",
                "owner_session":"origin-session", "pane":"worker-pane", "phase":"admitted",
                "name":"admitted worker", "branch":"feat/calendar", "repository":"github.com/oisin-ee/dogsled"},
            {"request_id":"b-pending", "kind":"agent", "owner_pane":"owner", "owner_session":null,
                "pane":null, "phase":"pending", "name":"worker", "repository":"github.com/oisin-ee/dogsled"},
            {"request_id":"c-retained", "kind":"agent", "owner_pane":"owner", "owner_session":"origin-session",
                "pane":null, "phase":"retained", "name":"admitted worker", "branch":"topic", "repository":null},
        ])
    );
    assert_eq!(owned_work::decode(&host).unwrap().owned.len(), 3);
    let mut unknown = host;
    unknown["owned_work"]["version"] = json!(2);
    assert!(owned_work::decode(&unknown).is_none());
}

#[test]
fn owned_work_wire_capture_matches_current_terminal_and_owned_job() {
    let ledger = Ledger::temporary();
    let agent = Agent {
        pane: "executing-pane".into(),
        agent: Some("pi".into()),
        name: Some("worker".into()),
        terminal_id: Some("current-terminal".into()),
        ..Agent::default()
    };
    let todo = crate::board::TodoSections {
        session: "execution-session".into(),
        branch: None,
        revision: 1,
        current: "Fix admission".into(),
        pane: agent.pane.clone(),
        sections: Vec::new(),
    };
    let mut job: Job = serde_json::from_value(json!({
        "id": "job-1", "cmd":["/bin/sh", "-c", "private script"], "cwd":"/checkout",
        "host":"local", "owner":"owner", "pane":"job-pane", "state":"running",
    }))
    .unwrap();
    job.startup = Some(Startup::captured(
        &job.cwd,
        Some(&crate::cleanup::Tree {
            repo: "/checkout".into(),
            path: "/checkout".into(),
            branch: "topic".into(),
            head: String::new(),
            identity: None,
        }),
    ));
    for (id, owner) in [("job-1", "owner"), ("job-unmatched", "other-pane")] {
        job.id = id.into();
        jobs::save(&ledger, &job).unwrap();
        crate::store::write(&ledger, |tx| {
            crate::job_store::put_validity(tx, id, jobs::Validity::new(1))?;
            crate::job_store::remember_owner_in(tx, id, owner, Some("origin-host"))
        })
        .unwrap();
    }
    let report = report(&ledger, vec![agent]);

    launch(&ledger, "child-1", "old-terminal");
    let host = capture(&ledger, &report, &[]);
    assert_eq!(owned_work::decode(&host).unwrap().executions.len(), 1);

    // The worker can be observed on the execution host before an origin item exists.
    launch(&ledger, "child-1", "current-terminal");
    let host = capture(&ledger, &report, std::slice::from_ref(&todo));
    let wire = &host["owned_work"];
    assert_eq!(wire["owned"], json!([]));
    assert_eq!(
        wire["executions"],
        json!([
            {"request_id":"child-1", "kind":"agent", "owner_hostname":"origin-host",
                "owner_pane":"owner", "pane":"executing-pane", "title":"Fix admission", "job_label":null},
            {"request_id":"job-1", "kind":"job", "owner_hostname":"origin-host",
                "owner_pane":"owner", "pane":"job-pane", "title":"sh", "job_label":"sh · topic"},
        ])
    );
    assert_eq!(
        capture(&ledger, &report, &[])["owned_work"]["executions"][0]["title"],
        "worker"
    );
    assert!(!wire.to_string().contains("private"));

    launch(&ledger, "competing-child", "other-terminal");
    let host = capture(&ledger, &report, &[todo]);
    let evidence = owned_work::decode(&host).unwrap();
    assert_eq!(evidence.executions.len(), 1);
    assert_eq!(evidence.executions[0].request_id, "job-1");
}

#[test]
fn owned_worker_clock_uses_the_same_turn_as_conversation_detail() {
    use super::super::{details::Session, live_activity::ActivityPublisher};
    use portal_types::agents::{ActivityView, LivePhase, TurnClock};
    use std::io::Write;
    let ledger = Ledger::temporary();
    launch(&ledger, "child", "current-terminal");
    let path = ledger.dir.join("session.jsonl");
    let mut file = std::fs::File::create(&path).unwrap();
    let mut record = |role, ms| {
        writeln!(file, "{}", json!({"type":"message", "message":{"role":role, "content":"a message", "timestamp":ms}})).unwrap();
    };
    record("user", 1_000);
    record("assistant", 9_000);
    record("user", 100_000);
    record("assistant", 142_500);
    let agent = Agent {
        pane: "executing-pane".into(),
        state: "working".into(),
        agent: Some("pi".into()),
        name: Some("worker".into()),
        agent_session: Some("worker-session".into()),
        terminal_id: Some("current-terminal".into()),
        transcript: Some(path.to_str().unwrap().into()),
        ..Default::default()
    };
    let session = serde_json::to_string(&Session::of(&agent).unwrap()).unwrap();
    let report = report(&ledger, vec![agent]);
    let status = collect::status(&ledger, Vec::new(), 12).unwrap();
    let mut publisher = ActivityPublisher::new("serve".into());
    let mut clocks = owned_work::WorkerClocks::with_feed(publisher.sender.subscribe());
    let mut capture = |session: &str, phase| {
        let view: ActivityView = serde_json::from_value(json!({"version":1, "pane":"executing-pane", "epoch":"serve", "revision":1,
            "session":session, "producer":{"generation":1,"id":"producer"}, "detail":{"availability":"available", "sequence":1,
            "received_at_ms":1, "valid_for_ms":10_000, "state":{"phase":phase,"active_tools":[]}}})).unwrap();
        publisher
            .replace(
                [(view.pane.clone(), view)].into(),
                Some(1),
                std::collections::BTreeSet::new(),
            )
            .unwrap();
        let host =
            owned_work::serialize_host(&ledger, &report, Some(&status), Some(&[]), &mut clocks)
                .unwrap();
        owned_work::decode(&host).unwrap().executions[0].turn_clock
    };
    let running = capture(&session, LivePhase::Thinking).unwrap();
    assert_eq!(running.seconds(170_500), Some(70));
    assert_eq!(running.seconds(171_500), Some(71));
    assert_eq!(running.seconds(running.valid_until_ms), None);
    let idle = capture(&session, LivePhase::Idle).unwrap();
    let detail = crate::activity::try_conversation("pi", path.to_str().unwrap()).unwrap();
    assert_eq!(
        Some(idle.clock),
        TurnClock::from_history(&detail.turns, LivePhase::Idle)
    );
    assert_eq!(idle.seconds(200_000), Some(42));
    record("user", 201_000);
    assert_eq!(
        capture(&session, LivePhase::Working)
            .unwrap()
            .seconds(202_000),
        Some(1)
    );
    assert_eq!(capture("other session", LivePhase::Working), None);
}

#[test]
fn owned_work_wire_capture_failure_preserves_base_report() {
    let ledger = Ledger::temporary();
    let report = report(&ledger, Vec::new());
    let mut host = owned_work::serialize_host(
        &ledger,
        &report,
        None,
        Some(&[]),
        &mut owned_work::WorkerClocks::default(),
    )
    .unwrap();
    assert!(owned_work::decode(&host).is_none());
    let extension = host.as_object_mut().unwrap().remove("owned_work").unwrap();
    assert_eq!(
        extension,
        json!({"version": 1, "status": "unavailable", "reason": "status unavailable"})
    );
    assert_eq!(
        host.as_object_mut().unwrap().remove("autonomy").unwrap(),
        json!({"version": 1, "fleet": false, "agents": [], "decisions": []})
    );
    assert_eq!(host, serde_json::to_value(&report).unwrap());
}
