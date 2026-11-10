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
fn quota_cli_only_requests_and_serve_consumes_the_request() {
    let rig = Rig::new("quota-cli-request");
    let (code, plain) = rig.json(&["quota"]);
    assert_eq!(code, 0, "{plain}");
    assert!(rig.state(&["quota-request"]).is_null());
    let (code, refreshed) = rig.json(&["quota", "--refresh"]);
    assert_eq!(code, 0, "{refreshed}");
    assert_eq!(refreshed, plain);
    assert!(!rig.state(&["quota-request"]).is_null());
    // The CLI reads storage only: no fleet discovery, Herdr, SSH or Keychain.
    assert_eq!(rig.log("herdr.log"), "");
    assert_eq!(rig.log("ssh.log"), "");
    serve(&rig);
    assert!(rig.state(&["quota-request"]).is_null());
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
    for pass in 0..2 {
        fs::write(rig.dir.join("clock"), (first_ms + pass * 1_000).to_string()).unwrap();
        journal.push_str(&serve(&rig));
    }
    fs::write(rig.dir.join("clock"), (first_ms + 2_000).to_string()).unwrap();
    let mut observer = rig.serve_at(&rig.dir.join("state"), "3600");
    super::wait_publication(&rig, |frame| {
        frame["views"]["status"]["data"]["hosts"][0]["service_lanes"]
            .as_array()
            .is_some_and(|lanes| {
                lanes
                    .iter()
                    .any(|lane| lane["lane"] == "quota" && lane["repeats"] == 3)
            })
    });
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

    assert_eq!(lanes(&rig)[0]["repeats"], 3);
    observer.owned.terminate();
    journal.push_str(&observer.output().1);
    drop(observer);
    assert_eq!(
        journal.matches("dogsled service lane quota:").count(),
        1,
        "{journal}"
    );
    rig.fault_host_store("clear-quota", false);
    let mut observer = rig.serve_at(&rig.dir.join("state"), "3600");
    super::wait_publication(&rig, |frame| {
        frame["local"]["host"]["service_lanes"] == json!([])
    });
    observer.owned.terminate();
    let recovered = observer.output().1;
    assert_eq!(
        recovered
            .matches("dogsled service lane quota: recovered")
            .count(),
        1,
        "{recovered}"
    );
    drop(observer);
    assert!(!serve(&rig).contains("dogsled service lane quota:"));
    let _observer = rig.serve_at(&rig.dir.join("state"), "3600");
    super::wait_publication(&rig, |frame| {
        frame["local"]["host"]["service_lanes"] == json!([])
    });
    assert_eq!(lanes(&rig), json!([]));
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
    let _observer = rig.serve();
    super::wait_publication(&rig, |frame| {
        frame["views"]["status"]["data"]["hosts"][1]["service_lanes"] == remote["service_lanes"]
    });
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
    let _observer = rig.serve();
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

/// A real serve whose lanes and baseline sample have finished, with Herdr events held by the test.
pub(super) struct Observer {
    pub(super) rig: Rig,
    child: Owned,
    events: std::os::unix::net::UnixStream,
    subscriptions: Vec<Value>,
}

impl Observer {
    pub(super) fn start(name: &str, interval: &str) -> Self {
        use std::io::{BufRead as _, BufReader, Write as _};
        let rig = Rig::short(name);
        rig.script("df", "#!/bin/sh\nprintf 'Filesystem 1024-blocks Used Available Capacity Mounted on\nfixture 1000 500 500 50%% /\n'\n");
        let path = rig.dir.join("herdr.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut cmd = rig.cmd(&["serve", "--interval", interval]);
        cmd.env("HERDR_SOCKET_PATH", &path)
            .stdout(Stdio::null())
            .stderr(fs::File::create(rig.dir.join("journal")).unwrap());
        let child = Owned::spawn(&mut cmd);
        let deadline = Instant::now() + PROCESS_DEADLINE;
        let (events, subscriptions) = loop {
            assert!(Instant::now() < deadline, "Herdr subscription missing");
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::yield_now();
                continue;
            };
            // Darwin streams inherit the listener's O_NONBLOCK; Linux streams do not.
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(PROCESS_DEADLINE)).unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], "events.subscribe");
            writeln!(
                stream,
                "{}",
                json!({"id":request["id"],"result":{"type":"subscription_started"}})
            )
            .unwrap();
            let filters = request["params"]["subscriptions"].as_array().unwrap();
            if filters.iter().any(|filter| filter["pane_id"] == "w1:p1") {
                break (stream, filters.clone());
            }
        };
        let observer = Self {
            rig,
            child,
            events,
            subscriptions,
        };
        observer.wait(|_| {
            let db = observer.rig.database();
            db.query_row::<i64, _, _>("SELECT count(*) FROM service_lanes", [], |row| row.get(0))
                .unwrap()
                == 7
        });
        observer.wait(|frame| {
            let samples: i64 = observer
                .rig
                .database()
                .query_row("SELECT COUNT(*) FROM samples", [], |row| row.get(0))
                .unwrap();
            samples >= 2
                && frame["local"]["host"]["sampled_ms"]
                    .as_u64()
                    .is_some_and(|at| at >= observer.rig.origin_ms + 1_000)
                && frame["local"]["host"]["probe"]["busy"].is_number()
        });
        observer.quiet();
        observer
    }

    pub(super) fn current(&self) -> Value {
        use std::io::{BufRead as _, BufReader, Write as _};
        let mut stream =
            std::os::unix::net::UnixStream::connect(self.rig.dir.join("state/observation.sock"))
                .unwrap();
        stream.set_read_timeout(Some(PROCESS_DEADLINE)).unwrap();
        writeln!(stream, "{{\"version\":1,\"operation\":\"current\"}}").unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    pub(super) fn wait(&self, condition: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + PROCESS_DEADLINE;
        loop {
            let frame = self.current();
            if condition(&frame) {
                return frame;
            }
            assert!(
                Instant::now() < deadline,
                "observation did not change: {frame}"
            );
            std::thread::yield_now();
        }
    }

    pub(super) fn quiet(&self) -> Value {
        let deadline = Instant::now() + PROCESS_DEADLINE;
        let mut frame = self.current();
        let mut stable = Instant::now();
        loop {
            assert!(Instant::now() < deadline, "observation never became quiet");
            let next = self.current();
            if next["revision"] != frame["revision"] {
                stable = Instant::now();
                frame = next;
            }
            if stable.elapsed() >= Duration::from_millis(200) {
                return frame;
            }
            std::thread::yield_now();
        }
    }

    pub(super) fn event(&mut self, name: &str) {
        use std::io::Write as _;
        writeln!(
            self.events,
            "{}",
            json!({"event":name,"data":{"pane_id":"w1:p1"}})
        )
        .unwrap();
    }

    pub(super) fn samples(&self) -> usize {
        self.rig.state(&["samples"]).as_array().unwrap().len()
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        self.child.terminate();
    }
}

#[test]
fn cold_serve_samples_cpu_before_long_interval_without_local_events() {
    let rig = Rig::short("observe-baseline");
    rig.script("df", "#!/bin/sh\nprintf 'Filesystem 1024-blocks Used Available Capacity Mounted on\nfixture 1000 500 500 50%% /\n'\n");
    let mut command = rig.cmd(&["serve", "--interval", "3600"]);
    command.env("HERDR_SOCKET_PATH", rig.dir.join("no-herdr-events.sock"));
    let _observer = Rig::background(command);
    let frame = wait_publication(&rig, |frame| {
        frame["local"]["host"]["probe"]["busy"].is_number()
            && frame["local"]["host"]["sampled_ms"]
                .as_u64()
                .is_some_and(|at| at >= rig.origin_ms + 1_000)
    });
    let probe = &frame["local"]["host"]["probe"];
    let cores = probe["cores"].as_f64().unwrap();
    let busy = probe["busy"].as_f64().unwrap();
    let memory = &probe["memory"];
    let free_memory =
        memory["total_bytes"].as_u64().unwrap() - memory["used_bytes"].as_u64().unwrap();
    // The untouched default policy requires 25% free cores, 3 GiB free RAM and 15% disk.
    let expected = cores * (1.0 - busy) >= cores * 0.25
        && free_memory >= 3 * 1024 * 1024 * 1024
        && probe["disk_free_percent"].as_f64().unwrap() >= 15.0
        && memory["pressure_level"]
            .as_u64()
            .is_none_or(|level| level < 4);
    assert_eq!(frame["local"]["headroom"]["eligible"], expected, "{frame}");
    assert_ne!(frame["local"]["headroom"]["reason"], "no CPU sample yet");
    assert!(frame["local"]["host"]["probe"]["busy"].is_number());
    let samples = rig.state(&["samples"]);
    let samples = samples.as_array().unwrap();
    assert_eq!(samples.len(), 2);
    let first = samples[0]["ts_ms"].as_u64().unwrap();
    let second = samples[1]["ts_ms"].as_u64().unwrap();
    // service::BASELINE_SAMPLE_DELAY is 1 s. Keep the 5 s behaviour bound,
    // including collection headroom, but exclude startup before the first sample.
    assert!(second - first < 5_000, "{samples:?}");
}

#[test]
fn observation_commit_wakes_inventory_without_terminal_resampling() {
    let observer = Observer::start("observe-commit", "3600");
    let before = observer.samples();
    let item = json!({"id":"new","kind":"agent","host":"local","pane":"gone:p1","owner_pane":"w1:p1","created_ms":1,"closed_ms":2});
    observer.rig.put_items(&json!([item]));
    let frame = observer.wait(|frame| frame["local"]["status"]["items"][0]["id"] == "new");
    assert_eq!(frame["local"]["status"]["items"][0]["closed_ms"], 2);
    assert_eq!(
        observer.samples(),
        before,
        "ledger input resampled terminals"
    );
}

#[test]
fn observation_herdr_status_event_wakes_local_collection() {
    let mut observer = Observer::start("observe-herdr", "3600");
    for event in [
        "pane.created",
        "pane.closed",
        "pane.updated",
        "pane.exited",
        "pane.agent_detected",
        "worktree.created",
        "worktree.opened",
        "worktree.removed",
    ] {
        assert!(
            observer
                .subscriptions
                .iter()
                .any(|filter| filter["type"] == event),
            "missing {event}"
        );
    }
    assert!(observer.subscriptions.iter().any(|filter| filter["type"]
        == "pane.agent_status_changed"
        && filter["pane_id"] == "w1:p1"));
    let before = observer.samples();
    let mut panes = observer.rig.read("panes.json");
    panes["result"]["panes"][0]["name"] = json!("changed");
    observer.rig.put("panes.json", &panes);
    observer.event("pane.agent_status_changed");
    observer.wait(|frame| frame["local"]["host"]["probe"]["panes"][0]["name"] == "changed");
    assert_eq!(observer.samples(), before + 1);
}

#[test]
fn observation_commit_burst_coalesces_after_inflight_collection() {
    let mut observer = Observer::start("observe-coalesce", "3600");
    let before = observer.quiet();
    let samples = observer.samples();
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(observer.rig.dir.join("state/host.lock"))
        .unwrap();
    lock.lock().unwrap();
    let calls = fs::read_to_string(observer.rig.dir.join("herdr.log"))
        .unwrap()
        .matches("agent list")
        .count();
    observer.event("pane.updated");
    let deadline = Instant::now() + PROCESS_DEADLINE;
    while fs::read_to_string(observer.rig.dir.join("herdr.log"))
        .unwrap()
        .matches("agent list")
        .count()
        == calls
    {
        assert!(Instant::now() < deadline, "collection did not start");
        std::thread::yield_now();
    }
    for id in 0..12 {
        observer.rig.put_state(
            &["state", "ingress", &format!("burst-{id}"), "--put"],
            &json!({"version":1}),
        );
    }
    lock.unlock().unwrap();
    observer.wait(|frame| {
        frame["revision"].as_u64().unwrap() >= before["revision"].as_u64().unwrap() + 2
    });
    let after = observer.quiet();
    assert_eq!(
        after["revision"].as_u64().unwrap(),
        before["revision"].as_u64().unwrap() + 2
    );
    assert_eq!(
        observer.samples(),
        samples + 1,
        "commit follow-up must reuse the terminal sample"
    );
    let db = observer.rig.database();
    assert_eq!(
        db.query_row::<i64, _, _>("SELECT count(*) FROM request_ingress", [], |row| row.get(0))
            .unwrap(),
        12
    );
}

#[test]
fn observation_fallback_collects_without_supported_event_or_commit() {
    let observer = Observer::start("observe-fallback", "1");
    let before = observer.samples();
    let mut panes = observer.rig.read("panes.json");
    panes["result"]["panes"][0]["foreground_cwd"] = json!("/changed-without-event");
    observer.rig.put("panes.json", &panes);
    observer.wait(|frame| {
        frame["local"]["host"]["probe"]["panes"][0]["foreground_cwd"] == "/changed-without-event"
    });
    assert!(observer.samples() > before);
}

#[test]
fn observation_producer_writes_do_not_create_feedback_or_reschedule_lanes() {
    let observer = Observer::start("observe-no-feedback", "3600");
    let before = observer.quiet();
    let samples = observer.samples();
    let lanes: Vec<(String, i64)> = observer
        .rig
        .database()
        .prepare("SELECT lane, last_ms FROM service_lanes ORDER BY lane")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    observer.rig.put_state(
        &["state", "ingress", "input", "--put"],
        &json!({"version":1}),
    );
    observer
        .wait(|frame| frame["revision"].as_u64().unwrap() > before["revision"].as_u64().unwrap());
    let after = observer.quiet();
    assert_eq!(
        after["revision"].as_u64().unwrap(),
        before["revision"].as_u64().unwrap() + 1
    );
    assert_eq!(observer.samples(), samples);
    let after_lanes: Vec<(String, i64)> = observer
        .rig
        .database()
        .prepare("SELECT lane, last_ms FROM service_lanes ORDER BY lane")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(after_lanes, lanes, "input wake reran action lanes");
}
