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
    /// The custodian agent, when the test sets one.
    custodian: Mutex<Option<serde_json::Value>>,
    child: Mutex<Option<serde_json::Value>>,
    /// Capture the durable reservation when the inbox validates a prompt recipient.
    reservation: Mutex<Option<(std::path::PathBuf, Option<State>)>>,
}
impl Shell for Commands {
    fn run(&self, program: &str, args: &[String]) -> Result<Output> {
        let command = format!("{program} {}", args.join(" "));
        self.calls.lock().unwrap().push(command.clone());
        let stdout = if args == ["agent", "list"] {
            if let Some((dir, saved)) = self.reservation.lock().unwrap().as_mut() {
                let state: State = Ledger::new(dir.clone())
                    .read("watchdog.json")?
                    .unwrap_or_default();
                if saved.is_none()
                    && state
                        .stopped
                        .get("child")
                        .is_some_and(|episode| episode.attempts > 0 && !episode.delivered)
                {
                    *saved = Some(state);
                }
            }
            let mut agents =
                vec![json!({"pane_id":"owner","agent_status":*self.owner.lock().unwrap()})];
            if let Some(custodian) = self.custodian.lock().unwrap().clone() {
                let mut listed = custodian.clone();
                if let Some(session) = listed.get("session").cloned() {
                    listed["agent_session"] = json!({"kind":"id","value":session});
                    listed.as_object_mut().unwrap().remove("session");
                }
                agents.push(listed);
            }
            if let Some(child) = self.child.lock().unwrap().clone() {
                agents.push(child);
            }
            json!({"result":{"agents":agents}}).to_string()
        } else if program == "ssh" {
            serde_json::to_string("peer log line").unwrap()
        } else if args.get(1).is_some_and(|s| s == "read") {
            "line1\nline2\nline3\nline4\nline5\n".into()
        } else {
            "{}".into()
        };
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
    _service: std::fs::File,
}
impl Rig {
    fn new() -> Self {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        std::fs::create_dir_all(&ledger.dir).unwrap();
        let service = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(ledger.dir.join("service.lock"))
            .unwrap();
        service.try_lock().unwrap();
        let rig = Self {
            ledger,
            _service: service,
            shell: Commands::default(),
            policy: Policy::default(),
        };
        *rig.shell.owner.lock().unwrap() = "idle".into();
        rig.reader();
        rig
    }
    /// The owner reads its inbox.
    fn reader(&self) {
        crate::inbox::claim(
            &self.ledger,
            "term_owner",
            &crate::inbox::Reader {
                harness: "pi".into(),
                pane: "owner".into(),
                registered_ms: 0,
            },
            0,
        )
        .unwrap();
    }
    /// A custodian pane, idle and live, that runs `session` and reads its inbox.
    fn custodian(&self, session: Option<&str>) {
        let mut agent = json!({"pane_id":"custodian","agent":"pi","agent_status":"idle",
            "terminal_id":"term_custodian"});
        if let Some(session) = session {
            agent["session"] = json!(session);
        }
        *self.shell.custodian.lock().unwrap() = Some(agent);
        crate::inbox::claim(
            &self.ledger,
            "term_custodian",
            &crate::inbox::Reader {
                harness: "pi".into(),
                pane: "custodian".into(),
                registered_ms: 0,
            },
            0,
        )
        .unwrap();
    }
    /// The custody record of `item`, held by `pane`.
    fn custody(&self, item: &str, pane: &str, session: Option<&str>) {
        let mut record = json!({"item":item,"host":"local","owner_pane":"owner",
            "custodian_pane":pane,"revision":1,"claims":[]});
        if let Some(session) = session {
            record["custodian_session"] = json!(session);
        }
        self.ledger.write("custody.json", &json!([record])).unwrap();
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
                ..Item::default()
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
        let mut panes = vec![json!({"pane_id":"owner"}), json!({"pane_id":"child"})];
        let mut agents = vec![
            json!({"pane":"owner","agent":"pi","state":owner,"name":"parent","transcript":null,"transcript_modified":null,"terminal_id":"term_owner"}),
            json!({"pane":"child","agent":"pi","state":child,"name":"test-child","transcript":modified.map(|_|"/transcript"),"transcript_modified":modified}),
        ];
        if let Some(custodian) = self.shell.custodian.lock().unwrap().clone() {
            let mut entry = json!({"pane":custodian["pane_id"],"agent":"pi",
                "state":custodian["agent_status"],"name":"custodian","transcript":null,
                "transcript_modified":null,"terminal_id":custodian["terminal_id"]});
            if let Some(session) = custodian.get("session") {
                entry["agent_session"] = session.clone();
            }
            panes.push(json!({"pane_id":"custodian"}));
            agents.push(entry);
        }
        let mut snapshot: Snapshot = serde_json::from_value(json!({"sampled_ms":now,"probe":{
            "host":"local","label":"local","load1":0.0,"cores":1,"memory":{"used_bytes":0,"total_bytes":1},"disk_free_percent":99.0,"agents":{"total":2,"by_status":{}},"reservations":[],
            "panes":panes,"agent_panes":agents}})).unwrap();
        observe(&mut snapshot.probe, None, now);
        self.ledger.write("host.json", &snapshot).unwrap();
        self.fleet().status(now).unwrap()
    }
    /// The check-ins in the owner's inbox, oldest first.
    fn check_ins(&self) -> Vec<String> {
        let mut notes: Vec<_> = crate::inbox::received(&self.ledger, "term_owner")
            .unwrap()
            .into_iter()
            .filter(|r| r.kind == "check-in")
            .collect();
        notes.sort_by_key(|r| r.created_ms);
        notes.into_iter().map(|r| r.text).collect()
    }
    /// The check-ins in the custodian's inbox, oldest first.
    fn custodian_check_ins(&self) -> Vec<String> {
        let mut notes: Vec<_> = crate::inbox::received(&self.ledger, "term_custodian")
            .unwrap()
            .into_iter()
            .filter(|r| r.kind == "check-in")
            .collect();
        notes.sort_by_key(|r| r.created_ms);
        notes.into_iter().map(|r| r.text).collect()
    }
    fn prompts(&self) -> usize {
        // Nothing is ever typed into the owner's terminal.
        assert!(
            !self
                .shell
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|s| s.starts_with("herdr agent prompt"))
        );
        self.check_ins().len()
    }
}
impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.ledger.dir);
    }
}

fn stopped_reports(rig: &Rig, now: u64, raw: &str, seq: u64, since: u64) -> Vec<HostReport> {
    let mut reports = rig.reports(now, "working", raw, None);
    let agent = reports[0]
        .probe
        .as_mut()
        .unwrap()
        .agent_panes
        .iter_mut()
        .find(|agent| agent.pane == "child")
        .unwrap();
    agent.agent_session = Some("session-child".into());
    agent.terminal_id = Some("term_child".into());
    agent.state_change_seq = Some(seq);
    agent.state_since_ms = Some(since);
    *rig.shell.child.lock().unwrap() = Some(json!({"pane_id":"child","agent":"pi",
        "agent_status":raw,"terminal_id":"term_child","agent_session":{"kind":"id","value":"session-child"},
        "state_change_seq":seq}));
    rig.ledger
        .write(
            "host.json",
            &Snapshot {
                sampled_ms: now,
                probe: reports[0].probe.clone().unwrap(),
                jobs: vec![],
            },
        )
        .unwrap();
    reports
}

fn child_reader(rig: &Rig) {
    crate::inbox::claim(
        &rig.ledger,
        "term_child",
        &crate::inbox::Reader {
            harness: "pi".into(),
            pane: "child".into(),
            registered_ms: 0,
        },
        0,
    )
    .unwrap();
}

fn stopped_messages(rig: &Rig) -> Vec<crate::inbox::Record> {
    crate::inbox::received(&rig.ledger, "term_child")
        .unwrap()
        .into_iter()
        .filter(|record| record.text == STOPPED_PROMPT)
        .collect()
}

#[test]
fn stopped_child_gets_one_prompt_across_five_ticks_and_reload() {
    let rig = Rig::new();
    rig.item("agent");
    child_reader(&rig);
    for now in [1_000, 2_000, 3_000, 4_000, 5_000] {
        tick(&rig.fleet(), &stopped_reports(&rig, now, "idle", 1, 1), now).unwrap();
    }
    let messages = stopped_messages(&rig);
    assert_eq!(messages.len(), 1);
    let saved: State = rig.ledger.read("watchdog.json").unwrap().unwrap();
    let episode = &saved.stopped["child"];
    assert_eq!(episode.pane, "child");
    assert_eq!(episode.agent_session, "session-child");
    assert_eq!(episode.turn.state_change_seq, Some(1));
    assert_eq!(episode.message_id.as_deref(), Some(messages[0].id.as_str()));
    assert!(episode.delivered);
    let mut status = crate::commands::Status {
        hosts: stopped_reports(&rig, 5_000, "idle", 1, 1),
        items: crate::items::list(&rig.ledger).unwrap(),
        queue: vec![],
        orphans: vec![],
        sweep: None,
        stopped_activity: std::collections::BTreeMap::new(),
    };
    assert_eq!(
        stopped_activity(&rig.ledger, &status, 5_000).unwrap()["child"],
        "stopped; prompted 4s ago"
    );
    status.stopped_activity = stopped_activity(&rig.ledger, &status, 5_000).unwrap();
    let board = crate::board::build(&status, Some("owner"), 5_000);
    assert_eq!(board.rows[0].group, crate::board::Group::Attention);
    assert_eq!(board.rows[0].activity, "stopped; prompted 4s ago");
}

#[test]
fn stopped_failed_put_reuses_id_and_stops_after_three_ticks() {
    let rig = Rig::new();
    rig.item("agent");
    let mut id = None;
    for now in 1..=5 {
        tick(&rig.fleet(), &stopped_reports(&rig, now, "idle", 1, 1), now).unwrap();
        let saved: State = rig.ledger.read("watchdog.json").unwrap().unwrap();
        let episode = &saved.stopped["child"];
        assert_eq!(episode.attempts, now.min(3) as u8);
        if let Some(id) = &id {
            assert_eq!(&episode.message_id, id);
        } else {
            id = Some(episode.message_id.clone());
        }
        assert!(!episode.delivered);
    }
    child_reader(&rig);
    tick(&rig.fleet(), &stopped_reports(&rig, 6, "idle", 1, 1), 6).unwrap();
    assert_eq!(stopped_messages(&rig), []);
}

#[test]
fn stopped_retry_success_keeps_original_message_id() {
    let rig = Rig::new();
    rig.item("agent");
    tick(&rig.fleet(), &stopped_reports(&rig, 1, "idle", 1, 1), 1).unwrap();
    let saved: State = rig.ledger.read("watchdog.json").unwrap().unwrap();
    child_reader(&rig);
    tick(&rig.fleet(), &stopped_reports(&rig, 2, "idle", 1, 1), 2).unwrap();
    assert_eq!(
        stopped_messages(&rig)[0].id,
        saved.stopped["child"].message_id.clone().unwrap()
    );
}

#[test]
fn stopped_prompt_turn_does_not_loop_but_later_real_turn_rearms() {
    let rig = Rig::new();
    rig.item("agent");
    child_reader(&rig);
    for (now, raw, seq, since) in [
        (1_000, "idle", 1, 1),
        (2_000, "working", 2, 2_000),
        (3_000, "idle", 3, 3_000),
        (125_000, "idle", 3, 3_000),
    ] {
        tick(
            &rig.fleet(),
            &stopped_reports(&rig, now, raw, seq, since),
            now,
        )
        .unwrap();
    }
    assert_eq!(stopped_messages(&rig).len(), 1);
    tick(
        &rig.fleet(),
        &stopped_reports(&rig, 130_000, "working", 4, 130_000),
        130_000,
    )
    .unwrap();
    tick(
        &rig.fleet(),
        &stopped_reports(&rig, 140_000, "idle", 5, 140_000),
        140_000,
    )
    .unwrap();
    assert_eq!(stopped_messages(&rig).len(), 2);
}

#[test]
fn stopped_successful_put_crash_keeps_the_new_prompt_window_on_restart() {
    let rig = Rig::new();
    rig.item("agent");
    child_reader(&rig);
    for (now, raw, seq) in [(1_000, "idle", 1), (130_000, "working", 2)] {
        tick(
            &rig.fleet(),
            &stopped_reports(&rig, now, raw, seq, now),
            now,
        )
        .unwrap();
    }
    *rig.shell.reservation.lock().unwrap() = Some((rig.ledger.dir.clone(), None));
    tick(
        &rig.fleet(),
        &stopped_reports(&rig, 140_000, "idle", 3, 140_000),
        140_000,
    )
    .unwrap();
    let delivered = stopped_messages(&rig);
    assert_eq!(delivered.len(), 2);
    let (_, reserved) = rig.shell.reservation.lock().unwrap().take().unwrap();
    let reserved = reserved.unwrap();
    assert!(!reserved.stopped["child"].delivered);
    assert!(
        delivered
            .iter()
            .any(|record| { Some(&record.id) == reserved.stopped["child"].message_id.as_ref() })
    );
    // Keep the successful inbox put, but lose every write after its reservation.
    rig.ledger.write("watchdog.json", &reserved).unwrap();
    for (now, raw, seq) in [(141_000, "working", 4), (142_000, "idle", 5)] {
        tick(
            &rig.fleet(),
            &stopped_reports(&rig, now, raw, seq, now),
            now,
        )
        .unwrap();
    }
    assert_eq!(stopped_messages(&rig).len(), 2);
    assert_eq!(
        reserved.prompted_after[&session_key("local", "session-child")].prompted_after,
        140_000
    );
}

#[test]
fn stopped_consecutive_working_observations_track_the_latest_turn() {
    let rig = Rig::new();
    rig.item("agent");
    child_reader(&rig);
    for (now, raw, seq) in [
        (0, "idle", 1),
        (1_000, "working", 2),
        (130_000, "working", 4),
        (140_000, "idle", 5),
    ] {
        tick(
            &rig.fleet(),
            &stopped_reports(&rig, now, raw, seq, now),
            now,
        )
        .unwrap();
    }
    assert_eq!(stopped_messages(&rig).len(), 2);
}

#[test]
fn stopped_prompt_turn_started_in_window_stays_suppressed_after_long_activity() {
    let rig = Rig::new();
    rig.item("agent");
    child_reader(&rig);
    for (now, raw, seq, since) in [
        (1_000, "idle", 1, 1),
        (2_000, "working", 2, 2_000),
        (124_000, "working", 2, 124_000),
        (125_000, "idle", 3, 125_000),
    ] {
        tick(
            &rig.fleet(),
            &stopped_reports(&rig, now, raw, seq, since),
            now,
        )
        .unwrap();
    }
    assert_eq!(stopped_messages(&rig).len(), 1);
}

#[test]
fn non_stopped_children_never_get_a_corrective_prompt() {
    for case in [
        "working",
        "waiting",
        "asking",
        "blocked",
        "finishing",
        "unknown",
        "lost",
    ] {
        let rig = Rig::new();
        rig.item("agent");
        child_reader(&rig);
        let mut reports = stopped_reports(&rig, 1_000, "idle", 1, 1);
        let agent = &mut reports[0].probe.as_mut().unwrap().agent_panes[1];
        match case {
            "working" => agent.state = "working".into(),
            "waiting" => agent.owns_open = true,
            "asking" => agent.signal = Some("ask".into()),
            "blocked" => agent.signal = Some("blocked".into()),
            "finishing" => agent.finishing = true,
            "unknown" => agent.state = "unreadable".into(),
            "lost" => reports[0].service = "down".into(),
            _ => unreachable!(),
        }
        tick(&rig.fleet(), &reports, 1_000).unwrap();
        assert!(stopped_messages(&rig).is_empty(), "{case}");
    }
}

#[test]
fn lifecycle_changes_rearm_the_state_clock_but_herdr_done_does_not() {
    let rig = Rig::new();
    let mut probe = rig.reports(1, "idle", "idle", Some(1))[0]
        .probe
        .take()
        .unwrap();
    probe.agent_panes[1].state_since_ms = Some(1);
    let mut waiting = probe.clone();
    waiting.agent_panes[1].owns_open = true;
    observe(&mut waiting, Some(&probe), 10);
    assert_eq!(waiting.agent_panes[1].state_since_ms, Some(10));
    let mut unread = waiting.clone();
    unread.agent_panes[1].state = "done".into();
    observe(&mut unread, Some(&waiting), 20);
    assert_eq!(unread.agent_panes[1].state_since_ms, Some(10));
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
fn immediate_stall_waits_for_a_turn_that_ended_and_survives_restart_without_repeat() {
    let rig = Rig::new();
    rig.item("agent");
    // The check-in reaches an owner whose turn ended. A blocked owner is one: the
    // lifecycle owner derives the state, and a blocked agent waits for its owner.
    for (now, owner, count) in [
        (600_001, "working", 0),
        (660_001, "blocked", 1),
        (720_001, "idle", 1),
        (780_001, "done", 1),
    ] {
        let reports = rig.reports(now, owner, "working", Some(1));
        tick(&rig.fleet(), &reports, now).unwrap();
        assert_eq!(rig.prompts(), count);
    }
    let log = rig.check_ins().join("\n");
    assert!(log.contains("test-child on local: stalled; age 11m; transcript changed 11m ago"));
    assert!(log.contains("line5"));
    assert!(log.contains("dogsled send"));
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
            .any(|s| s.contains("--clear-token dogsled"))
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
fn an_owner_without_an_inbox_reader_gets_no_check_in_until_it_has_one() {
    let rig = Rig::new();
    std::fs::remove_file(rig.ledger.dir.join("inbox/term_owner.reader.json")).unwrap();
    rig.item("agent");
    let reports = rig.reports(600_001, "idle", "working", Some(1));
    tick(&rig.fleet(), &reports, 600_001).unwrap();
    assert_eq!(rig.prompts(), 0);
    // The stall stays due, so the first scan after a reader registers delivers it.
    rig.reader();
    let reports = rig.reports(660_001, "idle", "working", Some(1));
    tick(&rig.fleet(), &reports, 660_001).unwrap();
    assert_eq!(rig.prompts(), 1);
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
            ..Item::default()
        },
    )
    .unwrap();
    tick(&rig.fleet(), &reports, 600_001).unwrap();
    let log = rig.shell.calls.lock().unwrap().join("\n");
    assert!(rig.check_ins()[0].contains("remote-child on peer: stalled"));
    assert!(log.contains("host tail agent child"));
    assert!(rig.check_ins()[0].contains("peer log line"));
    assert!(!log.contains("herdr notification"));
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
            ..Item::default()
        },
    )
    .unwrap();
    let reports = rig.reports(720_001, "idle", "working", Some(60_001));
    tick(&rig.fleet(), &reports, 720_001).unwrap();
    assert_eq!(rig.prompts(), 1);
    let prompt = &rig.check_ins()[0];
    assert!(prompt.contains("new-child-name"));
    assert!(!prompt.contains("test-child on"));
}

/// The check-in reaches every owner whose turn ended: one that waits for input, and one
/// that stopped without a signal. The lifecycle owner derives both states.
#[test]
fn a_check_in_reaches_an_idle_owner_and_a_stopped_owner() {
    for (owner, owns_open) in [
        ("idle", true),
        ("idle", false),
        ("done", false),
        ("blocked", false),
    ] {
        let rig = Rig::new();
        rig.item("agent");
        // A working owner is in a turn: no check-in.
        let reports = rig.reports(1, "working", "working", Some(1));
        tick(&rig.fleet(), &reports, 1).unwrap();
        assert_eq!(rig.prompts(), 0, "{owner}");
        // The owner's turn ended: it hears about its open child.
        let mut reports = rig.reports(1_860_001, owner, "working", Some(1));
        let agent = &mut reports[0].probe.as_mut().unwrap().agent_panes[0];
        agent.owns_open = owns_open;
        assert_eq!(
            state_of(agent),
            if owns_open {
                crate::lifecycle::State::Waiting
            } else if owner == "blocked" {
                crate::lifecycle::State::Blocked
            } else {
                crate::lifecycle::State::Stopped
            }
        );
        tick(&rig.fleet(), &reports, 1_860_001).unwrap();
        assert_eq!(rig.prompts(), 1, "{owner}");
        assert!(rig.check_ins()[0].contains("test-child"), "{owner}");
    }
}

#[test]
fn a_finished_child_is_left_to_the_sweep_and_named_at_the_regular_check_in() {
    let rig = Rig::new();
    rig.item("agent");
    let reports = rig.reports(1, "working", "working", Some(1));
    tick(&rig.fleet(), &reports, 1).unwrap();
    // Idle without `done`: it waits for input, so it has not finished.
    let reports = rig.reports(30_001, "idle", "idle", Some(1));
    let item = &crate::items::list(&rig.ledger).unwrap()[0];
    assert!(!crate::items::finished(item, &reports));
    // The child called `done` and ended that turn, with work to land: the sweep sends its
    // reply, so no check-in now.
    let done = |mut reports: Vec<HostReport>| {
        for agent in &mut reports[0].probe.as_mut().unwrap().agent_panes {
            agent.done = agent.pane == "child";
        }
        reports
    };
    let reports = done(rig.reports(60_001, "idle", "done", Some(1)));
    crate::items::reconcile(&rig.ledger, &reports, 60_001).unwrap();
    let item = &crate::items::list(&rig.ledger).unwrap()[0];
    assert!(crate::items::finished(item, &reports));
    tick(&rig.fleet(), &reports, 60_001).unwrap();
    assert_eq!(rig.prompts(), 0);
    // The regular check-in names it while it stays open.
    let reports = done(rig.reports(1_860_001, "idle", "done", Some(1)));
    tick(&rig.fleet(), &reports, 1_860_001).unwrap();
    assert_eq!(rig.prompts(), 1);
    assert!(rig.check_ins()[0].contains("Still open with work to land: test-child."));
    // After it closes, the owner hears nothing more about it.
    crate::items::close(&rig.ledger, "local", "child", 1_900_000).unwrap();
    let reports = rig.reports(5_000_001, "idle", "done", Some(1));
    tick(&rig.fleet(), &reports, 5_000_001).unwrap();
    assert_eq!(rig.prompts(), 1);
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
fn a_claimed_child_is_checked_in_with_its_custodian_not_its_origin_owner() {
    let rig = Rig::new();
    rig.item("agent");
    rig.custodian(Some("claimant-session"));
    rig.custody("child", "custodian", Some("claimant-session"));
    let reports = rig.reports(1_800_001, "idle", "working", Some(1_800_001));
    tick(&rig.fleet(), &reports, 1_800_001).unwrap();
    // The check-in reaches the pane that holds the child now.
    assert_eq!(rig.custodian_check_ins().len(), 1);
    assert!(rig.custodian_check_ins()[0].contains("test-child on local"));
    // The origin owner holds no child now, so it hears nothing.
    assert_eq!(rig.prompts(), 0);
}

#[test]
fn a_child_without_a_custody_record_still_reaches_its_origin_owner() {
    let rig = Rig::new();
    rig.item("agent");
    let reports = rig.reports(1_800_001, "idle", "working", Some(1_800_001));
    tick(&rig.fleet(), &reports, 1_800_001).unwrap();
    assert_eq!(rig.prompts(), 1);
    assert!(rig.check_ins()[0].contains("test-child on local"));
}

#[test]
fn a_custodian_running_another_session_gets_no_check_in() {
    let rig = Rig::new();
    rig.item("agent");
    // The custodian pane is live, but it runs another session than the recorded one.
    rig.custodian(Some("other-session"));
    rig.custody("child", "custodian", Some("claimant-session"));
    let reports = rig.reports(1_800_001, "idle", "working", Some(1_800_001));
    tick(&rig.fleet(), &reports, 1_800_001).unwrap();
    // Nothing is delivered to a pane that does not hold the child.
    assert_eq!(rig.custodian_check_ins().len(), 0);
    assert_eq!(rig.prompts(), 0);
}

#[test]
fn a_claim_that_recorded_no_session_gets_no_check_in() {
    let rig = Rig::new();
    rig.item("agent");
    // The custodian pane is live and idle, but the claim recorded no session to match.
    rig.custodian(Some("live-session"));
    rig.custody("child", "custodian", None);
    let reports = rig.reports(1_800_001, "idle", "working", Some(1_800_001));
    tick(&rig.fleet(), &reports, 1_800_001).unwrap();
    // Missing evidence is never a match: the pane id alone does not hold the child.
    assert_eq!(rig.custodian_check_ins().len(), 0);
    assert_eq!(rig.prompts(), 0);
}

#[test]
fn check_ins_do_not_report_orphans_the_sweep_keeps() {
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
            ..Item::default()
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
    let reports = rig.reports(1_800_001, "idle", "done", Some(1));
    tick(&rig.fleet(), &reports, 1_800_001).unwrap();
    let prompt = &rig.check_ins()[0];
    // An orphan has no owner to hear about it, but `dogsled status` lists it. A check-in
    // must not repeat it on every turn.
    assert!(!prompt.contains("Orphaned children"), "{prompt}");
    assert!(!prompt.contains("lost-child"), "{prompt}");
}
