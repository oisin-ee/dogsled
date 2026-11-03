use super::*;

fn serve(rig: &Rig) -> String {
    let mut child = rig.start(&["serve", "--once", "--interval", "1"]);
    let status = child.wait();
    let (_, stderr) = child.output();
    assert!(status.success(), "{stderr}");
    stderr
}

fn context(rig: &Rig) -> String {
    use std::io::Read as _;
    let mut cmd = rig.cmd(&["hook", "context", "--harness", "pi"]);
    cmd.env("HERDR_ENV", "1").stdout(Stdio::piped());
    let mut child = Owned::spawn(&mut cmd);
    assert!(child.wait().success());
    let mut text = String::new();
    child
        .child_mut()
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    text
}

fn lanes(rig: &Rig) -> Value {
    let (code, status) = rig.json(&["status"]);
    assert_eq!(code, 0, "{status}");
    status["hosts"][0]["service_lanes"].clone()
}

#[test]
fn lane_status_write_failure_does_not_stop_service() {
    let rig = Rig::new("service-lane-write-refused");
    serve(&rig);
    let db = rig.database();
    db.execute_batch(
        "CREATE TRIGGER fail_lanes_insert BEFORE INSERT ON service_lanes
         BEGIN SELECT RAISE(ABORT, 'lane write refused'); END;
         CREATE TRIGGER fail_lanes_update BEFORE UPDATE ON service_lanes
         BEGIN SELECT RAISE(ABORT, 'lane write refused'); END;",
    )
    .unwrap();
    drop(db);
    rig.fault_host_store("quota", false);
    let lane_error = "dogsled service lane quota: invalid quota snapshot";
    let recording_error =
        "dogsled service lane quota: cannot record lane status: SQLite: lane write refused";
    let journal = serve(&rig);
    assert!(journal.contains(lane_error), "{journal}");
    assert!(journal.contains(recording_error), "{journal}");

    let before = rig.state(&["samples"]).as_array().unwrap().len();
    let mut child = rig.start(&["serve", "--interval", "1"]);
    let deadline = Instant::now() + PROCESS_DEADLINE;
    // Observe publication passes, rather than assuming how long each pass takes.
    while rig.state(&["samples"]).as_array().unwrap().len() < before + 4 {
        assert!(Instant::now() < deadline, "service did not keep publishing");
        std::thread::yield_now();
    }
    child.owned.terminate();
    let (_, journal) = child.output();
    assert_eq!(journal.matches(lane_error).count(), 1, "{journal}");
    assert_eq!(journal.matches(recording_error).count(), 1, "{journal}");
}

#[test]
fn lane_errors_persist_across_service_runs_and_recover_once() {
    let rig = Rig::new("service-lane-recovery");
    let first_ms = now();
    fs::write(rig.dir.join("clock"), first_ms.to_string()).unwrap();
    // Only the quota lane reads this cache. Publication and status remain available.
    rig.fault_host_store("quota", false);
    let mut journal = String::new();
    for pass in 0..3 {
        fs::write(rig.dir.join("clock"), (first_ms + pass * 1_000).to_string()).unwrap();
        journal.push_str(&serve(&rig));
    }
    assert_eq!(
        journal.matches("dogsled service lane quota:").count(),
        1,
        "{journal}"
    );
    let failing = lanes(&rig);
    assert_eq!(failing.as_array().unwrap().len(), 1, "{failing}");
    let lane = &failing[0];
    assert_eq!(lane["lane"], "quota");
    assert_eq!(lane["since_ms"], first_ms);
    assert_eq!(lane["last_ms"], first_ms + 2_000);
    assert_eq!(lane["repeats"], 3);
    let error = lane["error"].as_str().unwrap();
    assert!(error.contains("invalid quota snapshot"), "{error}");
    let time = chrono::DateTime::from_timestamp_millis(i64::try_from(first_ms).unwrap())
        .unwrap()
        .format("%H:%M UTC")
        .to_string();
    let expected = format!("local service: quota lane failing since {time} (x3): {error}");
    for args in [vec!["status"], vec!["status", "--brief"]] {
        let out = rig.start(&args).output().0;
        assert!(out.contains(&expected), "{out}");
    }
    let context = context(&rig);
    assert!(context.contains(&expected), "{context}");
    let (_, snapshot) = rig.json(&["host", "snapshot"]);
    assert_eq!(snapshot["service_lanes"], failing);

    rig.fault_host_store("clear-quota", false);
    let recovered = serve(&rig);
    assert_eq!(
        recovered
            .matches("dogsled service lane quota: recovered")
            .count(),
        1,
        "{recovered}"
    );
    assert_eq!(lanes(&rig), json!([]));
    assert!(!serve(&rig).contains("dogsled service lane quota:"));
    for args in [vec!["status"], vec!["status", "--brief"]] {
        let out = rig.start(&args).output().0;
        assert!(!out.contains("lane failing"), "{out}");
    }
    let db = rig.database();
    let (error, since, repeats): (Option<String>, Option<i64>, i64) = db
        .query_row(
            "SELECT error, since_ms, repeats FROM service_lanes WHERE lane = 'quota'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!((error, since, repeats), (None, None, 0));
}

#[test]
fn remote_lane_failures_reach_full_brief_and_context_status() {
    let rig = Rig::new("service-lane-remote");
    rig.peer();
    let mut remote = rig.read("remote.json");
    remote["service_lanes"] = json!([{
        "lane":"admission", "error":"remote admission cannot read state",
        "since_ms":43_260_000, "last_ms":43_600_000, "repeats":340
    }]);
    rig.put("remote.json", &remote);
    let expected = "netcup service: admission lane failing since 12:01 UTC (x340): remote admission cannot read state";
    for args in [vec!["status"], vec!["status", "--brief"]] {
        let out = rig.start(&args).output().0;
        assert!(out.contains(expected), "{out}");
        assert!(!out.contains("local service:"), "{out}");
    }
    let context = context(&rig);
    assert!(context.contains(expected), "{context}");
    let (_, status) = rig.json(&["status"]);
    assert_eq!(status["hosts"][1]["service_lanes"], remote["service_lanes"]);
}

#[test]
fn handoff_passes_take_no_handoff_file_lock() {
    let rig = Rig::new("service-handoff-no-file-lock");
    rig.done("missing-terminal", "gone:p1", 10);
    // Both the handoff lane and the children lane run a handoff pass here.
    serve(&rig);
    serve(&rig);
    assert!(!rig.dir.join("state/handoff.lock").exists());
    let failing = lanes(&rig);
    assert!(
        failing
            .as_array()
            .unwrap()
            .iter()
            .all(|lane| lane["lane"] != "children" && lane["lane"] != "handoff"),
        "{failing}"
    );
}

#[test]
fn repeated_handoff_warning_prints_once_across_lane_threads() {
    let rig = Rig::new("service-handoff-warning");
    rig.done("missing-terminal", "gone:p1", 10);
    let original = rig.state(&["done"]);
    // Each lane pass has a new worker thread, but the diagnostic source is the same.
    let mut child = rig.start(&["serve", "--interval", "1"]);
    let deadline = Instant::now() + PROCESS_DEADLINE;
    let mut last_ms = 0;
    let mut passes = 0;
    while passes < 3 {
        assert!(
            Instant::now() < deadline,
            "handoff lane did not complete three passes"
        );
        if rig.dir.join("state/state.db").exists() {
            let db = rusqlite::Connection::open(rig.dir.join("state/state.db")).unwrap();
            db.busy_timeout(Duration::from_secs(5)).unwrap();
            if let Ok(ms) = db.query_row::<i64, _, _>(
                "SELECT last_ms FROM service_lanes WHERE lane = 'handoff'",
                [],
                |row| row.get(0),
            ) && ms > last_ms
            {
                last_ms = ms;
                passes += 1;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.owned.terminate();
    let (_, journal) = child.output();
    assert_eq!(
        journal
            .matches("dogsled handoff: gone:p1: removed checkout handoff is unproven")
            .count(),
        1,
        "{journal}"
    );
    assert_eq!(rig.state(&["done"]), original);
}
