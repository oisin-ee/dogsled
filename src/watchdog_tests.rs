//! Durable watchdog behavior with injected timestamps and a recorded OS seam.
use super::*;
use crate::config::Policy;
use crate::hosts::Host;
use crate::node::Snapshot;
use crate::shell::{Output, Shell};
use serde_json::json;
use std::sync::Mutex;

#[derive(Debug, Default)]
struct Commands {
    calls: Mutex<Vec<String>>,
    owner: Mutex<String>,
    fail_prompt: Mutex<bool>,
}
impl Shell for Commands {
    fn run(&self, program: &str, args: &[String]) -> Result<Output> {
        let command = format!("{program} {}", args.join(" "));
        self.calls.lock().unwrap().push(command.clone());
        let stdout = if args == ["agent", "list"] {
            json!({"result":{"agents":[{"pane_id":"owner","agent_status":*self.owner.lock().unwrap()}]}}).to_string()
        } else if program == "ssh" {
            serde_json::to_string("peer log line").unwrap()
        } else if args.get(1).is_some_and(|s| s == "read") {
            "line1\nline2\nline3\nline4\nline5\n".into()
        } else {
            "{}".into()
        };
        if command.starts_with("herdr agent prompt") && *self.fail_prompt.lock().unwrap() {
            return Err(crate::error::Error::Deadline(command));
        }
        Ok(Output {
            status: 0,
            stdout,
            stderr: String::new(),
        })
    }
}
struct Rig {
    ledger: Ledger,
    shell: Commands,
    policy: Policy,
}
impl Rig {
    fn new() -> Self {
        let rig = Self {
            ledger: Ledger::new(std::env::temp_dir().join(crate::ledger::new_id())),
            shell: Commands::default(),
            policy: Policy::default(),
        };
        *rig.shell.owner.lock().unwrap() = "idle".into();
        rig
    }
    fn fleet(&self) -> Fleet<'_> {
        Fleet {
            shell: &self.shell,
            ledger: &self.ledger,
            policy: &self.policy,
            home: "/".into(),
            hosts: vec![Host::local()],
        }
    }
    fn item(&self, kind: &str) {
        crate::items::record(
            &self.ledger,
            Item {
                id: "child".into(),
                kind: kind.into(),
                host: "local".into(),
                pane: "child".into(),
                owner_pane: "owner".into(),
                name: Some("test-child".into()),
                created_ms: 1,
                closed_ms: None,
            },
        )
        .unwrap();
    }
    fn reports(
        &self,
        now: u64,
        owner: &str,
        child: &str,
        modified: Option<u64>,
    ) -> Vec<HostReport> {
        *self.shell.owner.lock().unwrap() = owner.into();
        let mut snapshot: Snapshot = serde_json::from_value(json!({"sampled_ms":now,"probe":{
            "host":"local","label":"local","load1":0.0,"cores":1,"memory":{"used_bytes":0,"total_bytes":1},"disk_free_percent":99.0,"agents":{"total":2,"by_status":{}},"reservations":[],
            "panes":[{"pane_id":"owner"},{"pane_id":"child"}],"agent_panes":[
                {"pane":"owner","agent":"pi","state":owner,"name":"parent","transcript":null,"transcript_modified":null},
                {"pane":"child","agent":"pi","state":child,"name":"test-child","transcript":modified.map(|_|"/transcript"),"transcript_modified":modified}
            ]}})).unwrap();
        observe(&mut snapshot.probe, None, now);
        self.ledger.write("host.json", &snapshot).unwrap();
        self.fleet().status(now).unwrap()
    }
    fn prompts(&self) -> usize {
        self.shell
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.starts_with("herdr agent prompt"))
            .count()
    }
}
impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.ledger.dir);
    }
}

#[test]
fn stall_requires_old_transcript_and_working_or_unknown() {
    let rig = Rig::new();
    for (state, modified, stalled) in [
        ("working", Some(1), true),
        ("unknown", Some(1), true),
        ("working", Some(599_999), false),
        ("working", None, false),
        ("idle", Some(1), false),
        ("blocked", Some(1), false),
    ] {
        let reports = rig.reports(600_001, "idle", state, modified);
        assert_eq!(
            reports[0].probe.as_ref().unwrap().agent_panes[1]
                .stalled_since_ms
                .is_some(),
            stalled
        );
    }
}

#[test]
fn immediate_stall_waits_for_idle_and_survives_restart_without_repeat() {
    let rig = Rig::new();
    rig.item("agent");
    for (now, owner, count) in [
        (600_001, "working", 0),
        (660_001, "blocked", 0),
        (720_001, "idle", 1),
        (780_001, "done", 1),
    ] {
        let reports = rig.reports(now, owner, "working", Some(1));
        tick(&rig.fleet(), &reports, now).unwrap();
        assert_eq!(rig.prompts(), count);
    }
    let log = rig.shell.calls.lock().unwrap().join("\n");
    assert!(log.contains("test-child on local: stalled; age 12m; transcript changed 12m ago"));
    assert!(log.contains("line5"));
    assert!(log.contains("agents send"));
    // Unknown/working changes within the same stall do not rearm its event.
    let reports = rig.reports(840_001, "idle", "unknown", Some(1));
    tick(&rig.fleet(), &reports, 840_001).unwrap();
    assert_eq!(rig.prompts(), 1);
    let reports = rig.reports(900_001, "idle", "working", Some(900_001));
    tick(&rig.fleet(), &reports, 900_001).unwrap();
    assert!(
        rig.shell
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.contains("--clear-token wrangle"))
    );
    let reports = rig.reports(1_500_001, "idle", "working", Some(900_001));
    tick(&rig.fleet(), &reports, 1_500_001).unwrap();
    assert_eq!(rig.prompts(), 2);
}

#[test]
fn regular_checkin_uses_events_not_every_transcript_write() {
    let rig = Rig::new();
    rig.item("agent");
    for now in [1, 60_001, 1_740_001] {
        let reports = rig.reports(now, "idle", "working", Some(now));
        tick(&rig.fleet(), &reports, now).unwrap();
        assert_eq!(rig.prompts(), 0);
    }
    let reports = rig.reports(1_800_001, "idle", "working", Some(1_800_001));
    tick(&rig.fleet(), &reports, 1_800_001).unwrap();
    assert_eq!(rig.prompts(), 1);
    let reports = rig.reports(1_860_001, "idle", "working", Some(1_860_001));
    tick(&rig.fleet(), &reports, 1_860_001).unwrap();
    assert_eq!(rig.prompts(), 1);
}

#[test]
fn ended_job_event_is_captured_between_scans_before_reconciliation() {
    let rig = Rig::new();
    rig.item("job");
    let reports = rig.reports(1, "working", "working", None);
    tick(&rig.fleet(), &reports, 1).unwrap();
    let mut reports = rig.reports(2, "working", "working", None);
    reports[0].jobs.push(serde_json::from_value(json!({"id":"child","cmd":["test"],"cwd":"/","host":"local","owner":"owner","pane":"child","state":"idle","exit":0,"signal":null,"reason":null,"started_ms":1,"ended_ms":2,"timeout_ms":null,"idle_timeout_ms":null})).unwrap());
    tick(&rig.fleet(), &reports, 2).unwrap();
    crate::items::reconcile(&rig.ledger, &reports, 2).unwrap();
    assert_eq!(
        crate::items::list(&rig.ledger).unwrap()[0].closed_ms,
        Some(2)
    );
    reports[0].sampled_ms = Some(60_001);
    reports[0].probe.as_mut().unwrap().agent_panes[0].state = "idle".into();
    *rig.shell.owner.lock().unwrap() = "idle".into();
    tick(&rig.fleet(), &reports, 60_001).unwrap();
    assert_eq!(rig.prompts(), 1);
    tick(&rig.fleet(), &reports, 120_001).unwrap();
    assert_eq!(rig.prompts(), 1);
}

#[test]
fn top_level_notification_once_per_stall_and_recovery() {
    let rig = Rig::new();
    for now in [600_001, 660_001, 720_001] {
        let reports = rig.reports(now, "idle", "working", Some(1));
        tick(&rig.fleet(), &reports, now).unwrap();
    }
    assert_eq!(
        rig.shell
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.starts_with("herdr notification show"))
            .count(),
        1
    );
    let reports = rig.reports(780_001, "idle", "working", Some(780_001));
    tick(&rig.fleet(), &reports, 780_001).unwrap();
    let reports = rig.reports(1_380_001, "idle", "working", Some(780_001));
    tick(&rig.fleet(), &reports, 1_380_001).unwrap();
    assert_eq!(
        rig.shell
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.starts_with("herdr notification show"))
            .count(),
        2
    );
}

#[test]
fn uncertain_delivery_is_durable_and_retried_after_thirty_minutes() {
    let rig = Rig::new();
    rig.item("agent");
    *rig.shell.fail_prompt.lock().unwrap() = true;
    let reports = rig.reports(600_001, "idle", "working", Some(1));
    assert!(tick(&rig.fleet(), &reports, 600_001).is_err());
    *rig.shell.fail_prompt.lock().unwrap() = false;
    let reports = rig.reports(660_001, "idle", "working", Some(1));
    tick(&rig.fleet(), &reports, 660_001).unwrap();
    assert_eq!(rig.prompts(), 1);
    let reports = rig.reports(2_400_001, "idle", "working", Some(1));
    tick(&rig.fleet(), &reports, 2_400_001).unwrap();
    assert_eq!(rig.prompts(), 2);
}

#[test]
fn remote_stall_uses_report_state_and_bounded_remote_tail() {
    let rig = Rig::new();
    let mut reports = rig.reports(600_001, "idle", "working", Some(1));
    let mut peer = reports[0].clone();
    peer.host = Host {
        id: "peer".into(),
        label: "peer".into(),
        target: Some("peer-ssh".into()),
    };
    reports[0]
        .probe
        .as_mut()
        .unwrap()
        .agent_panes
        .retain(|a| a.pane == "owner");
    reports.push(peer);
    crate::items::record(
        &rig.ledger,
        Item {
            id: "remote".into(),
            kind: "agent".into(),
            host: "peer".into(),
            pane: "child".into(),
            owner_pane: "owner".into(),
            name: Some("remote-child".into()),
            created_ms: 1,
            closed_ms: None,
        },
    )
    .unwrap();
    tick(&rig.fleet(), &reports, 600_001).unwrap();
    let log = rig.shell.calls.lock().unwrap().join("\n");
    assert!(log.contains("remote-child on peer: stalled"));
    assert!(log.contains("host tail agent child"));
    assert!(log.contains("peer log line"));
    assert!(!log.contains("herdr notification"));
}

#[test]
fn owner_must_still_be_idle_after_log_reads() {
    let rig = Rig::new();
    rig.item("agent");
    let reports = rig.reports(600_001, "idle", "working", Some(1));
    *rig.shell.owner.lock().unwrap() = "working".into();
    tick(&rig.fleet(), &reports, 600_001).unwrap();
    assert_eq!(rig.prompts(), 0);
}

#[test]
fn closed_item_does_not_observe_reused_or_retracked_pane() {
    let rig = Rig::new();
    rig.item("agent");
    let reports = rig.reports(1, "working", "working", Some(1));
    tick(&rig.fleet(), &reports, 1).unwrap();
    let reports = rig.reports(60_001, "working", "idle", Some(1));
    tick(&rig.fleet(), &reports, 60_001).unwrap();
    crate::items::reconcile(&rig.ledger, &reports, 60_001).unwrap();
    assert_eq!(crate::items::list(&rig.ledger).unwrap()[0].closed_ms, None);
    assert_eq!(
        crate::items::close(&rig.ledger, "local", "child", 60_001).unwrap(),
        1
    );
    let reports = rig.reports(660_001, "idle", "working", Some(60_001));
    tick(&rig.fleet(), &reports, 660_001).unwrap();
    assert_eq!(rig.prompts(), 0);
    crate::items::record(
        &rig.ledger,
        Item {
            id: "new-child".into(),
            kind: "agent".into(),
            host: "local".into(),
            pane: "child".into(),
            owner_pane: "owner".into(),
            name: Some("new-child-name".into()),
            created_ms: 660_001,
            closed_ms: None,
        },
    )
    .unwrap();
    let reports = rig.reports(720_001, "idle", "working", Some(60_001));
    tick(&rig.fleet(), &reports, 720_001).unwrap();
    assert_eq!(rig.prompts(), 1);
    let calls = rig.shell.calls.lock().unwrap();
    let prompt = calls
        .iter()
        .find(|s| s.starts_with("herdr agent prompt"))
        .unwrap();
    assert!(prompt.contains("new-child-name"));
    assert!(!prompt.contains("test-child on"));
}

#[test]
fn finished_child_prompts_its_owner_at_once_and_until_it_is_closed() {
    let rig = Rig::new();
    rig.item("agent");
    let reports = rig.reports(1, "working", "working", Some(1));
    tick(&rig.fleet(), &reports, 1).unwrap();
    assert_eq!(rig.prompts(), 0);
    // The child finishes while its owner is idle: the owner hears at once, with the close command.
    let reports = rig.reports(60_001, "idle", "done", Some(1));
    crate::items::reconcile(&rig.ledger, &reports, 60_001).unwrap();
    let item = &crate::items::list(&rig.ledger).unwrap()[0];
    assert_eq!(item.closed_ms, None);
    assert!(crate::items::finished(item, &reports));
    tick(&rig.fleet(), &reports, 60_001).unwrap();
    assert_eq!(rig.prompts(), 1);
    assert!(
        rig.shell.calls.lock().unwrap().iter().any(
            |s| s.contains("finished") && s.contains("run: `wrangle close child --host local`")
        )
    );
    // Done and idle are one event: a flip does not prompt again before the check-in.
    let reports = rig.reports(120_001, "idle", "idle", Some(1));
    tick(&rig.fleet(), &reports, 120_001).unwrap();
    assert_eq!(rig.prompts(), 1);
    // The regular check-in repeats while the child stays open.
    let reports = rig.reports(1_860_001, "idle", "done", Some(1));
    tick(&rig.fleet(), &reports, 1_860_001).unwrap();
    assert_eq!(rig.prompts(), 2);
    // After `wrangle close`, the owner hears nothing more about it.
    crate::items::close(&rig.ledger, "local", "child", 1_900_000).unwrap();
    let reports = rig.reports(5_000_001, "idle", "done", Some(1));
    tick(&rig.fleet(), &reports, 5_000_001).unwrap();
    assert_eq!(rig.prompts(), 2);
}

#[test]
fn finished_child_ends_its_item_when_its_pane_is_gone() {
    let rig = Rig::new();
    rig.item("agent");
    let mut reports = rig.reports(60_001, "idle", "done", Some(1));
    reports[0]
        .probe
        .as_mut()
        .unwrap()
        .panes
        .retain(|p| p.pane_id != "child");
    crate::items::reconcile(&rig.ledger, &reports, 60_001).unwrap();
    assert_eq!(
        crate::items::list(&rig.ledger).unwrap()[0].closed_ms,
        Some(60_001)
    );
}

#[test]
fn check_ins_report_orphans_the_sweep_keeps() {
    let rig = Rig::new();
    rig.item("agent");
    crate::items::record(
        &rig.ledger,
        Item {
            id: "orphan".into(),
            kind: "agent".into(),
            host: "local".into(),
            pane: "lost".into(),
            owner_pane: "gone".into(),
            name: Some("lost-child".into()),
            created_ms: 1,
            closed_ms: None,
        },
    )
    .unwrap();
    rig.ledger
        .write(
            "orphans.json",
            &json!([{"item":"orphan","host":"local","pane":"lost","owner_pane":"gone",
                "name":"lost-child","reason":"owner gone is gone; the child is working","since_ms":1}]),
        )
        .unwrap();
    let reports = rig.reports(60_001, "idle", "done", Some(1));
    tick(&rig.fleet(), &reports, 60_001).unwrap();
    let calls = rig.shell.calls.lock().unwrap();
    let prompt = calls
        .iter()
        .find(|s| s.starts_with("herdr agent prompt owner"))
        .unwrap();
    assert!(
        prompt.contains(
            "Orphaned children the sweep keeps: lost-child on local (owner gone is gone; the child is working)."
        ),
        "{prompt}"
    );
}
