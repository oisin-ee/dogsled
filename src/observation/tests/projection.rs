use super::super::{Availability, Domains, Gap, Publication, Publisher, Sources, project};
use crate::commands::Status;
use crate::config::Policy;
use crate::hosts::{Fleet, Host, HostReport};
use crate::items::Item;
use crate::ledger::Ledger;
use crate::probe::Agent;
use crate::shell::Fake;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

pub(super) const NOW: u64 = 10_000;

pub(super) fn host(id: &str, state: &str) -> HostReport {
    let local = id == "local";
    let mut report: HostReport = serde_json::from_value(json!({
        "id":id,"label":id,"target":if local {None} else {Some("peer-ssh")},
        "service":"up","reachable":true,"sampled_ms":NOW,
        "probe":{
            "host":id,"label":id,"load1":0.0,"cores":8,"busy":0.2,
            "memory":{"total_bytes":32_000_000_000u64,"used_bytes":8_000_000_000u64},
            "disk_free_percent":50.0,"agents":{"total":1,"by_status":{state:1}},
            "agent_panes":[],"reservations":[],"panes":[]
        },
        "headroom":null,"error":null,"jobs":[]
    }))
    .unwrap();
    report.probe.as_mut().unwrap().agent_panes.push(Agent {
        pane: format!("{id}-worker"),
        agent: Some("pi".into()),
        state: state.into(),
        agent_session: Some(format!("{id}-session")),
        handoff: Some("none".into()),
        name: Some(format!("{id} worker")),
        repository: Some("github.com/owner/repository".into()),
        ..Agent::default()
    });
    report.headroom = report
        .probe
        .as_ref()
        .map(|probe| crate::plan::evaluate(probe, &(&Policy::default()).into()));
    report
}

fn item(id: &str, host: &str, kind: &str) -> Item {
    Item {
        id: id.into(),
        kind: kind.into(),
        host: host.into(),
        pane: if kind == "job" {
            format!("{host}-job")
        } else {
            format!("{host}-worker")
        },
        owner_pane: "owner".into(),
        created_ms: 1_000,
        ..Item::default()
    }
}

pub(super) struct Fixture {
    pub(super) ledger: Ledger,
    pub(super) shell: Fake,
    pub(super) policy: Policy,
    pub(super) local: HostReport,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let local = host("local", "idle");
        crate::items::record(&ledger, item("local-child", "local", "agent")).unwrap();
        crate::items::record(&ledger, item("peer-child", "peer", "agent")).unwrap();
        crate::items::record(&ledger, item("job", "local", "job")).unwrap();
        let episode = crate::watchdog::StoppedEpisode {
            pane: "peer-worker".into(),
            agent_session: "peer-session".into(),
            turn: crate::done::Turn {
                state_change_seq: Some(1),
                state_since_ms: Some(1),
            },
            message_id: Some("prompt".into()),
            attempts: 1,
            delivered: false,
            prompted_after: None,
        };
        let state = crate::watchdog::State {
            stopped: BTreeMap::from([("peer-child".into(), episode)]),
            ..crate::watchdog::State::default()
        };
        crate::host_store::put_watchdog(&ledger, &state).unwrap();
        Self {
            ledger,
            shell: Fake::new(),
            policy: Policy::default(),
            local,
        }
    }

    pub(super) fn fleet(&self) -> Fleet<'_> {
        Fleet::cold(
            &self.shell,
            &self.ledger,
            &self.policy,
            "/home".into(),
            vec![Host::local()],
        )
    }

    pub(super) fn input(&self) -> Publication {
        let probe = self.local.probe.as_ref().unwrap();
        let agents: Vec<_> = probe.agent_panes.iter().map(|agent| json!({
            "pane_id":agent.pane,"agent":agent.agent,"agent_status":agent.state,"name":agent.name,
            "agent_session":agent.agent_session.as_ref().map(|session| json!({"kind":"path","value":session})),
            "terminal_id":agent.terminal_id,"cwd":agent.cwd,
            "completion_seq":agent.completion_seq,"state_change_seq":agent.state_change_seq,"state_since_ms":agent.state_since_ms
        })).collect();
        self.shell.on(
            "herdr agent list",
            &json!({"result":{"agents":agents}}).to_string(),
        );
        self.shell.on(
            "herdr pane list",
            &json!({"result":{"panes":probe.panes}}).to_string(),
        );
        let terminals = crate::probe::terminals(&self.shell).unwrap();
        let status =
            crate::observation::collect::status(&self.ledger, vec![self.local.clone()], NOW)
                .unwrap();
        let todo = json!({"session":"local-session","branch":null,"revision":1,
            "current":"Captured title","pane":"local-worker","sections":[]});
        Publication {
            published_ms: NOW,
            sources: Sources {
                local: BTreeMap::from([
                    ("status".into(), NOW),
                    ("todos".into(), NOW),
                    ("terminals".into(), NOW),
                ]),
                ..Sources::default()
            },
            local: Domains::from([
                ("terminals".into(), serde_json::to_value(terminals).unwrap()),
                ("status".into(), serde_json::to_value(status).unwrap()),
                ("todos".into(), json!([todo])),
                (
                    "sidebar_preferences".into(),
                    serde_json::to_value(super::super::sidebar::preferences(&self.ledger).unwrap())
                        .unwrap(),
                ),
            ]),
            views: Domains::new(),
            gaps: vec![],
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.ledger.dir).unwrap();
    }
}

#[test]
fn status_display_derives_lifecycle_without_trusting_a_published_label() {
    let fixture = Fixture::new();
    for (terminal, expected) in [
        ("idle", "stopped"),
        ("done", "stopped"),
        ("blocked", "blocked"),
    ] {
        let mut report = host("local", terminal);
        report.probe.as_mut().unwrap().agent_panes[0].lifecycle = Some("done".into());
        let status = super::super::collect::status(&fixture.ledger, vec![report], NOW).unwrap();
        let display = super::super::display::Display::capture(&status);
        let text = display.render(NOW);
        assert!(
            text.contains(&format!("local-worker {expected} ")),
            "{text}"
        );
        assert!(text.contains(&format!("({expected} 1)")), "{text}");
    }
}

#[test]
fn closed_branch_has_no_cli_pane_but_keeps_portal_identity() {
    let mut fixture = Fixture::new();
    fixture.local.probe.as_mut().unwrap().agent_panes.clear();
    let mut closed = item("local-child", "local", "agent");
    closed.closed_ms = Some(NOW - 1);
    closed.branch = Some(crate::done::Branch {
        repo: "/execution-only/repo".into(),
        name: "feat/retained".into(),
        ahead: 2,
        landed: false,
    });
    let mut items = crate::items::list(&fixture.ledger).unwrap();
    let previous = items.iter_mut().find(|item| item.id == closed.id).unwrap();
    *previous = closed;
    crate::store::put_items(&fixture.ledger, &items).unwrap();
    let mut input = fixture.input();
    let captured = project::Projector::default().inputs(&fixture.fleet(), &input);
    project::derive(
        &captured,
        vec![fixture.local.clone()],
        &(&fixture.policy).into(),
        &mut input,
    )
    .unwrap();
    let rows = input.views["board"]["data"]["rows"].as_array().unwrap();
    let row = rows.iter().find(|row| row["id"] == "local-child").unwrap();
    assert!(row["pane"].is_null());
    assert_eq!(row["title"], "Captured title");
    let portal = &input.views["portal_board"]["data"];
    let row = portal["repos"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|repo| repo["rows"].as_array().unwrap())
        .find(|row| row["id"] == "local-child")
        .unwrap();
    assert_eq!(row["pane"], "local-worker");
    assert_eq!(row["title"], "Captured title");
    let mut previous: crate::board::Board =
        serde_json::from_value(input.views["board"]["data"].clone()).unwrap();
    previous
        .rows
        .iter_mut()
        .find(|row| row.id == "local-child")
        .unwrap()
        .pane = Some("local-worker".into());
    previous
        .rows
        .iter_mut()
        .find(|row| row.id == "local-child")
        .unwrap()
        .title = Some("Captured title".into());
    let todos =
        serde_json::from_value::<Vec<crate::board::TodoSections>>(input.local["todos"].clone())
            .unwrap();
    let expected = crate::web::board::view(&previous, &[], &todos, NOW);
    assert_eq!(*portal, serde_json::to_value(expected).unwrap());
}

/// Job history is keyed by owner and host-qualified job identity, never by host or id alone.
fn assert_owner_scoped_job_history(views: &crate::observation::Domains) {
    let history = &views["job_history"]["data"]["owner"];
    assert_eq!(
        history["jobs"].as_array().unwrap().len(),
        1,
        "host-qualified job identity"
    );
    assert_eq!(history["jobs"][0]["host"], "local");
    let other = &views["job_history"]["data"]["other"]["jobs"];
    assert_eq!(other.as_array().unwrap().len(), 1, "{other}");
    assert_eq!(other[0]["id"], "foreign");
    assert_eq!(history["hosts"].as_array().unwrap().len(), 2);
    let empty = &views["empty_job_history"]["data"];
    assert_eq!(empty["jobs"], json!([]));
    assert_eq!(empty["hosts"], history["hosts"]);
}

#[test]
fn all_views_share_one_capture_and_published_revision() {
    let fixture = Fixture::new();
    // Another owner's job: history is scoped by owner, not by host or id alone.
    let mut foreign = item("foreign", "peer", "job");
    foreign.owner_pane = "other".into();
    crate::items::record(&fixture.ledger, foreign).unwrap();
    let mut input = fixture.input();
    let mut projector = project::Projector::default();
    let local = projector.inputs(&fixture.fleet(), &input);
    // Later ledger writes cannot leak into any projection of this capture.
    let mut late = item("late-child", "local", "agent");
    late.pane = "late-pane".into();
    crate::items::record(&fixture.ledger, late).unwrap();
    assert!(
        crate::items::list(&fixture.ledger)
            .unwrap()
            .iter()
            .any(|item| item.id == "late-child")
    );
    let mut reports = vec![fixture.local.clone(), host("peer", "working")];
    for report in &mut reports {
        report.jobs.push(
            serde_json::from_value(json!({
                "id":"job","cmd":["true"],"cwd":"/host-owned/job","host":report.host.id,
                "owner":"owner","pane":"job-pane","state":"exited","exit":0,
                "started_ms":1,"ended_ms":2
            }))
            .unwrap(),
        );
    }
    reports[1].jobs.push(
        serde_json::from_value(json!({
            "id":"foreign","cmd":["true"],"cwd":"/host-owned/foreign","host":"peer",
            "owner":"other","pane":"peer-job","state":"exited","exit":0,
            "started_ms":1,"ended_ms":2
        }))
        .unwrap(),
    );
    project::derive(&local, reports, &(&fixture.policy).into(), &mut input).unwrap();
    assert!(input.gaps.is_empty(), "{:?}", input.gaps);
    for key in [
        "hosts",
        "status",
        "todos",
        "board",
        "sidebar_hosts",
        "jobs",
        "boards",
        "findings",
        "job_history",
        "empty_job_history",
        "agents",
        "portal_board",
        "inbox",
        "portal",
    ] {
        assert!(
            input.views[key]["data"].is_object() || input.views[key]["data"].is_array(),
            "{key}"
        );
        assert_eq!(input.views[key]["gaps"], json!([]));
        assert!(
            !input.views[key].to_string().contains("late-child"),
            "{key}"
        );
    }
    let owner = &input.views["boards"]["data"]["owner"];
    assert!(
        owner["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["title"] == "Captured title")
    );
    assert_owner_scoped_job_history(&input.views);
    let mut producer = Publisher::new().unwrap();
    let published = producer.replace(input).unwrap();
    let portal: portal_types::PortalSnapshot =
        serde_json::from_value(published.views["portal"]["data"].clone()).unwrap();
    assert_eq!(portal.epoch, published.epoch);
    assert_eq!(portal.revision, published.revision);
    assert_eq!(portal.published_ms, published.published_ms);
    assert_eq!(
        serde_json::to_value(portal.board.unwrap()).unwrap(),
        published.views["portal_board"]["data"]
    );
    assert_eq!(
        serde_json::to_value(portal.agents.unwrap()).unwrap(),
        published.views["agents"]["data"]
    );
    assert_eq!(
        serde_json::to_value(portal.inbox.unwrap()).unwrap(),
        published.views["inbox"]["data"]
    );
}

#[test]
fn published_owner_board_preserves_captured_enrichment() {
    let mut fixture = Fixture::new();
    let agent = &mut fixture.local.probe.as_mut().unwrap().agent_panes[0];
    agent.done = true;
    agent.handoff = Some("recorded".into());
    agent.worktree = Some(crate::facts::Worktree {
        path: "/host-owned/tree".into(),
        branch: "work".into(),
        ahead: 2,
        dirty: true,
        ..crate::facts::Worktree::default()
    });
    let message = serde_json::from_value(json!({
        "id":"unread","terminal":"owner-terminal","pane":"owner",
        "from":{"host":"local","pane":"local-worker"},"kind":"message","text":"result",
        "created_ms":2,"state":"pending","attempts":0,"next_ms":0
    }))
    .unwrap();
    crate::store::write(&fixture.ledger, |tx| {
        crate::web_store::inbox::put_records(tx, "owner-terminal", &[message])
    })
    .unwrap();
    let mut input = fixture.input();
    let status: Status = serde_json::from_value(input.local["status"].clone()).unwrap();
    let todos: Vec<crate::board::TodoSections> =
        serde_json::from_value(input.local["todos"].clone()).unwrap();
    let mut expected = crate::board::build(&status, Some("owner"), NOW);
    let messages =
        crate::store::read(&fixture.ledger, crate::web_store::inbox::all_records).unwrap();
    let captured = crate::board::capture_enrichment(&expected, &status, messages, &fixture.shell);
    crate::board::enrich_captured(&mut expected, &status, &todos, &captured);
    let local = project::Projector::default().inputs(&fixture.fleet(), &input);
    project::derive(
        &local,
        vec![fixture.local.clone()],
        &(&fixture.policy).into(),
        &mut input,
    )
    .unwrap();
    assert_eq!(
        input.views["boards"]["data"]["owner"],
        serde_json::to_value(expected).unwrap()
    );
    let row = input.views["boards"]["data"]["owner"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "local-child")
        .unwrap();
    assert_eq!(row["unseen"], true);
    assert_eq!(row["cleanup"]["action"], "commit");
    assert_eq!(row["title"], "Captured title");
}

fn local_parts(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(
            values
                .iter()
                .filter(|value| {
                    !["host", "id", "label"]
                        .iter()
                        .any(|key| value.get(key).and_then(Value::as_str) == Some("peer"))
                        && value.get("id").and_then(Value::as_str) != Some("peer-child")
                })
                .map(local_parts)
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .filter(|(key, _)| !matches!(key.as_str(), "peer-child" | "published_ms"))
                .map(|(key, value)| (key.clone(), local_parts(value)))
                .collect(),
        ),
        value => value.clone(),
    }
}

#[test]
fn peer_only_compose_reuses_local_inputs_without_ledger_or_shell_reads() {
    let fixture = Fixture::new();
    let input = fixture.input();
    let mut projector = project::Projector::default();
    let local = projector.inputs(&fixture.fleet(), &input);
    let mut observers = super::super::peers_runtime::Observers::default();
    let mut before = input.clone();
    observers
        .compose(
            &fixture.fleet(),
            &mut before,
            NOW,
            vec![fixture.local.clone(), host("peer", "working")],
        )
        .unwrap();
    let calls = fixture.shell.calls.lock().unwrap().len();
    // A directory blocker makes any attempted ledger connection fail, instead of relying
    // on an in-memory counter that could miss an alternative database access path.
    let saved = fixture.ledger.dir.with_extension("saved");
    std::fs::rename(&fixture.ledger.dir, &saved).unwrap();
    std::fs::write(&fixture.ledger.dir, b"ledger access forbidden").unwrap();
    let reused = projector.inputs(&fixture.fleet(), &input);
    assert!(Arc::ptr_eq(&local, &reused));
    let mut after = input;
    after.published_ms += 100;
    observers
        .compose(
            &fixture.fleet(),
            &mut after,
            NOW + 100,
            vec![fixture.local.clone(), host("peer", "idle")],
        )
        .unwrap();
    assert!(
        before.views["status"]["data"]["stopped_activity"]
            .get("peer-child")
            .is_none()
    );
    assert_eq!(
        after.views["status"]["data"]["stopped_activity"]["peer-child"],
        "stopped; prompt undelivered"
    );
    for key in ["hosts", "sidebar_hosts"] {
        assert_eq!(
            before.views[key]["data"][0], after.views[key]["data"][0],
            "{key} local bytes"
        );
    }
    for key in ["board", "boards"] {
        let a = if key == "board" {
            &before.views[key]["data"]
        } else {
            &before.views[key]["data"]["owner"]
        };
        let b = if key == "board" {
            &after.views[key]["data"]
        } else {
            &after.views[key]["data"]["owner"]
        };
        let local_rows = |board: &Value| {
            board["rows"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| row["host"] == "local")
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(
            serde_json::to_vec(&local_rows(a)).unwrap(),
            serde_json::to_vec(&local_rows(b)).unwrap()
        );
    }
    for key in [
        "todos",
        "agents",
        "jobs",
        "findings",
        "job_history",
        "inbox",
    ] {
        assert_eq!(
            serde_json::to_vec(&before.views[key]).unwrap(),
            serde_json::to_vec(&after.views[key]).unwrap(),
            "{key}"
        );
    }
    for (key, view) in &before.views {
        assert_eq!(
            serde_json::to_vec(&local_parts(view)).unwrap(),
            serde_json::to_vec(&local_parts(&after.views[key])).unwrap(),
            "{key} local-derived bytes"
        );
    }
    assert_ne!(before.views["status"], after.views["status"]);
    assert_eq!(calls, fixture.shell.calls.lock().unwrap().len());
    assert_eq!(before.gaps, []);
    assert_eq!(after.gaps, []);
    std::fs::remove_file(&fixture.ledger.dir).unwrap();
    std::fs::rename(saved, &fixture.ledger.dir).unwrap();
}

#[test]
fn collection_gaps_reach_every_view_and_missing_inputs_are_not_empty_success() {
    let fixture = Fixture::new();
    let mut input = fixture.input();
    let mut projector = project::Projector::default();
    let previous = projector.inputs(&fixture.fleet(), &input);
    input.local.remove("status");
    input.gaps.push(Gap {
        domain: "status".into(),
        availability: Availability::Failed,
        detail: "inventory capture failed".into(),
    });
    let local = projector.inputs(&fixture.fleet(), &input);
    assert!(
        !Arc::ptr_eq(&previous, &local),
        "failed captures cannot reuse stale success"
    );
    project::derive(
        &local,
        vec![fixture.local.clone()],
        &(&fixture.policy).into(),
        &mut input,
    )
    .unwrap();
    for (key, view) in &input.views {
        // The agent list and the GitHub catalogue do not derive from the failed capture;
        // the catalogue reports its own read failure in `data.error`.
        if key == "agents" || key == "repos" {
            assert_eq!(view["gaps"], json!([]));
            continue;
        }
        assert!(
            view["gaps"]
                .as_array()
                .unwrap()
                .iter()
                .any(|gap| gap["detail"] == "inventory capture failed"),
            "{key}"
        );
    }
    for key in [
        "status",
        "board",
        "boards",
        "job_history",
        "portal_board",
        "inbox",
    ] {
        assert!(input.views[key]["data"].is_null(), "{key}");
    }
    assert!(input.views["agents"]["data"].is_array());
    assert_ne!(
        input.views["portal"]["data"]["gaps"]
            .as_array()
            .unwrap()
            .as_slice(),
        [] as [Value; 0]
    );
}

#[test]
fn failed_terminal_capture_gaps_agents_and_owner_findings() {
    let fixture = Fixture::new();
    let _ = fixture.input();
    fixture
        .shell
        .on("herdr agent list", "unavailable terminal source");
    let mut collector = super::super::collect::Collector::default();
    let collected = collector.collect(&fixture.fleet().node(), NOW);
    assert!(collected.sample.is_err());
    let mut input = collected.input;
    let captured = project::Projector::default().inputs(&fixture.fleet(), &input);
    project::derive(
        &captured,
        collector.reports(),
        &(&fixture.policy).into(),
        &mut input,
    )
    .unwrap();
    for key in ["agents", "findings"] {
        assert!(input.views[key]["data"].is_null(), "{key}");
        assert!(
            input.views[key]["gaps"]
                .as_array()
                .unwrap()
                .iter()
                .any(|gap| gap["domain"] == "terminals" && gap["availability"] == "failed"),
            "{key}"
        );
    }
}

#[test]
fn headroom_wire_round_trip_preserves_no_sample_unbounded_and_ordering() {
    let values: Vec<_> = [f64::NEG_INFINITY, -2.0, 0.0, 8.5, f64::INFINITY]
        .into_iter()
        .map(|headroom| crate::plan::Headroom {
            host: format!("host-{headroom}"),
            headroom,
            eligible: headroom != f64::NEG_INFINITY,
            reason: None,
            live_agents: 0,
        })
        .collect();
    let wire = serde_json::to_value(&values).unwrap();
    assert_eq!(wire[0]["headroom"], "no_sample");
    assert_eq!(wire[4]["headroom"], "unbounded");
    let old = r#"{"host":"old","headroom":null,"eligible":false,"live_agents":0}"#;
    let old: crate::plan::Headroom = serde_json::from_str(old).unwrap();
    assert_eq!(old.headroom, f64::NEG_INFINITY);
    let finite = r#"{"host":"finite","headroom":8.5,"eligible":true,"live_agents":2}"#;
    let finite_value: crate::plan::Headroom = serde_json::from_str(finite).unwrap();
    assert_eq!(serde_json::to_string(&finite_value).unwrap(), finite);
    let decoded: Vec<crate::plan::Headroom> = serde_json::from_value(wire).unwrap();
    assert_eq!(decoded, values);
    assert_eq!(crate::plan::choose(&decoded), crate::plan::choose(&values));
    let mut no_sample = host("local", "idle");
    no_sample.probe.as_mut().unwrap().busy = None;
    no_sample.headroom = Some(crate::plan::evaluate(
        no_sample.probe.as_ref().unwrap(),
        &(&Policy::default()).into(),
    ));
    let decoded: HostReport =
        serde_json::from_value(serde_json::to_value(&no_sample).unwrap()).unwrap();
    assert_eq!(decoded.headroom.unwrap().headroom, f64::NEG_INFINITY);
}
