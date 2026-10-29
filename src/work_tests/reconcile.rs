use super::*;
use serde_json::json;

fn capable_snapshot(current: &Snapshot) -> String {
    let mut value = serde_json::to_value(current).unwrap();
    value["request_validity"] = json!(1);
    value.to_string()
}

fn missing_pane_snapshot(peer: &Host) -> Snapshot {
    let mut current = snapshot(100);
    current.probe.host = peer.id.clone();
    current.probe.panes.clear();
    current.probe.disk_free_percent = 1.0;
    current
}

#[test]
fn late_remote_admission_uses_launch_host_clock_before_closing_a_missing_pane() {
    // With skew, the owner's admission time is later than even the disappearance sample.
    for owner_skew in [0, 20_000] {
        let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
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
            hosts: vec![peer.clone()],
        };
        let remote = |args: &[String]| format!("ssh {}", fleet.remote_argv(&peer, args).join(" "));
        let mut current = missing_pane_snapshot(&peer);
        shell.on(&remote(&["snapshot".into()]), &capable_snapshot(&current));
        let request = Request {
            action: Action::Agent {
                kind: "pi".into(),
                name: "worker".into(),
                message: "go".into(),
                profile: None,
            },
            ..pinned(&ledger, "late-child", "peer")
        };
        let queued_at = 100 + owner_skew;
        work::enqueue(&ledger, request.clone(), false, queued_at).unwrap();
        work::tick(&fleet, queued_at).unwrap();
        assert_eq!(work::queue(&ledger).unwrap()[0].state, "pending");
        items::reconcile(&ledger, &fleet.status(queued_at).unwrap(), queued_at).unwrap();
        assert!(items::list(&ledger).unwrap().is_empty());

        // This snapshot permits admission but still predates the pane's creation.
        current.sampled_ms = 11_000;
        current.probe.disk_free_percent = 99.0;
        shell.on(&remote(&["snapshot".into()]), &capable_snapshot(&current));
        let launch: work::Launch = serde_json::from_value(json!({
            "id":request.id,"validity":request.validity,"action":request.action,"host":"peer","cwd":"/",
            "owner":"owner","pane":null,"workspace":null,"workspace_name":null
        }))
        .unwrap();
        let receipt = json!({"host":"peer","pane":"child","launch_confirmed_ms":22_000});
        shell.on(
            &remote(&[
                "launch".into(),
                "--preparation-protocol".into(),
                crate::prepare::disposition::VERSION.to_string(),
                "--request".into(),
                serde_json::to_string(&launch).unwrap(),
                "--request-validity".into(),
                "1".into(),
            ]),
            &receipt.to_string(),
        );
        // Reservation IDs are generated at runtime; script just that transport operation.
        let reserving = ReservingShell(&shell);
        let fleet = Fleet {
            shell: &reserving,
            ..fleet
        };
        let admitted_at = 11_000 + owner_skew;
        work::tick(&fleet, admitted_at).unwrap();
        assert_eq!(work::queue(&ledger).unwrap()[0].state, "admitted");
        let item = items::list(&ledger).unwrap().remove(0);
        assert_eq!(item.created_ms, admitted_at);
        assert_eq!(item.launch_confirmed_ms, Some(22_000));
        let mut reports = fleet.status(admitted_at).unwrap();
        for sampled in [11_000, 22_000] {
            reports[0].sampled_ms = Some(sampled);
            items::reconcile(&ledger, &reports, admitted_at).unwrap();
            assert!(items::list(&ledger).unwrap()[0].closed_ms.is_none());
        }
        reports[0].sampled_ms = Some(22_001);
        reports[0].probe.as_mut().unwrap().panes.push(Pane {
            pane_id: "child".into(),
            ..Pane::default()
        });
        items::reconcile(&ledger, &reports, admitted_at).unwrap();
        assert!(items::list(&ledger).unwrap()[0].closed_ms.is_none());
        reports[0].probe.as_mut().unwrap().panes.clear();
        // Neither another host nor a stale report proves this child's disappearance.
        reports[0].host.id = "other".into();
        items::reconcile(&ledger, &reports, admitted_at).unwrap();
        reports[0].host.id = "peer".into();
        items::reconcile(&ledger, &reports, 60_000).unwrap();
        assert!(items::list(&ledger).unwrap()[0].closed_ms.is_none());
        items::reconcile(&ledger, &reports, admitted_at).unwrap();
        assert_eq!(
            items::list(&ledger).unwrap()[0].closed_ms,
            Some(admitted_at)
        );
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
}

#[derive(Debug)]
struct ReservingShell<'a>(&'a Fake);

impl crate::shell::Shell for ReservingShell<'_> {
    fn run(&self, program: &str, args: &[String]) -> crate::error::Result<crate::shell::Output> {
        if program == "ssh" && args.last().is_some_and(|arg| arg.contains("host reserve ")) {
            return Ok(crate::shell::Output {
                status: 0,
                stdout: json!({"reserved":true,"headroom":null}).to_string(),
                stderr: String::new(),
            });
        }
        self.0.run(program, args)
    }
}

#[test]
fn job_end_rule_does_not_use_agent_launch_evidence() {
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
    let mut current = snapshot(200);
    current.jobs.push(
        serde_json::from_value(json!({
            "id":"job","cmd":["true"],"cwd":"/","host":"local","owner":"owner",
            "pane":"child","state":"exited","exit":0,"signal":null,"reason":null,
            "started_ms":100,"ended_ms":200,"timeout_ms":null,"idle_timeout_ms":null
        }))
        .unwrap(),
    );
    ledger.write("host.json", &current).unwrap();
    items::record(
        &ledger,
        Item {
            id: "job".into(),
            kind: "job".into(),
            host: "local".into(),
            pane: "child".into(),
            created_ms: 100,
            launch_confirmed_ms: Some(300),
            ..Item::default()
        },
    )
    .unwrap();
    items::reconcile(&ledger, &fleet.status(200).unwrap(), 200).unwrap();
    assert_eq!(items::list(&ledger).unwrap()[0].closed_ms, Some(200));
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn a_running_job_stays_open_until_a_sample_after_its_launch_lacks_its_pane() {
    // Admission at 100, pane and dispatch confirmed at 300. A sample from 200 predates the
    // pane, so its missing pane is no evidence; that rule once closed live jobs at start.
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
    let mut current = snapshot(200);
    current.probe.panes.clear();
    current.jobs.push(
        serde_json::from_value(json!({
            "id":"job","cmd":["true"],"cwd":"/","host":"local","owner":"owner",
            "pane":"child","state":"working","exit":null,"signal":null,"reason":null,
            "started_ms":250,"ended_ms":null,"timeout_ms":null,"idle_timeout_ms":null
        }))
        .unwrap(),
    );
    ledger.write("host.json", &current).unwrap();
    items::record(
        &ledger,
        Item {
            id: "job".into(),
            kind: "job".into(),
            host: "local".into(),
            pane: "child".into(),
            created_ms: 100,
            launch_confirmed_ms: Some(300),
            ..Item::default()
        },
    )
    .unwrap();
    for sampled in [200, 300] {
        let mut reports = fleet.status(sampled).unwrap();
        reports[0].sampled_ms = Some(sampled);
        items::reconcile(&ledger, &reports, sampled).unwrap();
        assert_eq!(
            items::list(&ledger).unwrap()[0].closed_ms,
            None,
            "sample {sampled}"
        );
    }
    let mut reports = fleet.status(301).unwrap();
    reports[0].sampled_ms = Some(301);
    items::reconcile(&ledger, &reports, 301).unwrap();
    assert_eq!(items::list(&ledger).unwrap()[0].closed_ms, Some(301));
    std::fs::remove_dir_all(ledger.dir).unwrap();
}
