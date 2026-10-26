use super::*;

fn complete(id: &str) -> Value {
    json!({"id":id,"cmd":["echo","hi"],"cwd":"/tmp","host":"local","owner":"w1:p1","pane":"w2:p1",
        "state":"idle","exit":0,"signal":null,"reason":null,"started_ms":100,"ended_ms":101,"timeout_ms":null,"idle_timeout_ms":null})
}

/// A wrapper runs nothing when its claim is missing, corrupt, for another command, already
/// started, ended, or uncertain. No process, no log, and no pane exist for such a job.
#[test]
fn exec_without_an_executable_claim_never_runs_the_command() {
    let rig = Rig::new("exec-no-claim");
    let marker = rig.dir.join("marker");
    let command = format!("touch {}", marker.display());

    // No record at all: the old behavior ran this.
    let output = rig.run(&["exec", "missing", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no claim to run"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!marker.exists());

    // A corrupt record proves nothing, and never runs.
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    fs::write(rig.dir.join("state/jobs/corrupt.json"), "not json").unwrap();
    let output = rig.run(&["exec", "corrupt", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!marker.exists());

    // A claim for another command does not cover this one.
    rig.claim("other-command", &["sh", "-c", "true"]);
    let output = rig.run(&["exec", "other-command", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!marker.exists());

    // A claim that already started never runs a second time.
    rig.claim("already-started", &["sh", "-c", &command]);
    let mut started = rig.read("state/jobs/already-started.json");
    started["started_ms"] = json!(100);
    started["state"] = json!("working");
    rig.put("state/jobs/already-started.json", &started);
    let output = rig.run(&["exec", "already-started", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!marker.exists());

    // A claim that ended never runs again.
    rig.claim("already-ended", &["sh", "-c", &command]);
    let mut ended = rig.read("state/jobs/already-ended.json");
    ended["ended_ms"] = json!(101);
    ended["state"] = json!("idle");
    rig.put("state/jobs/already-ended.json", &ended);
    let output = rig.run(&["exec", "already-ended", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!marker.exists());

    // An uncertain claim authorizes nothing.
    rig.claim("uncertain-claim", &["sh", "-c", &command]);
    let mut uncertain = rig.read("state/jobs/uncertain-claim.json");
    uncertain["state"] = json!("uncertain");
    rig.put("state/jobs/uncertain-claim.json", &uncertain);
    let output = rig.run(&["exec", "uncertain-claim", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!marker.exists());

    // A pre-allocation intent is not executable either.
    rig.claim("intent-only", &["sh", "-c", &command]);
    let mut intent = rig.read("state/jobs/intent-only.json");
    intent["state"] = json!("claiming");
    rig.put("state/jobs/intent-only.json", &intent);
    let output = rig.run(&["exec", "intent-only", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!marker.exists());

    // Not one of them left a log, a pane, or a running record behind.
    let jobs = fs::read_dir(rig.dir.join("state/jobs")).unwrap();
    for entry in jobs {
        let entry = entry.unwrap();
        assert!(
            entry.path().extension().is_none_or(|e| e != "log"),
            "{entry:?}"
        );
    }
    assert!(!rig.log("herdr.log").contains("pane close"));
}

/// A record that carries another job's identity never runs. The requested id, the record's
/// own id, and the record the wrapper reads must all name the same job. A record that
/// embeds another id is refused before any lock, process, log, or pane.
#[test]
fn exec_refuses_a_record_that_embeds_another_job() {
    let rig = Rig::new("exec-embedded-id");
    let marker = rig.dir.join("marker");
    let command = format!("touch {}", marker.display());
    let home = rig.dir.join("home").display().to_string();
    // B's own directory exists, so B is a valid, executable claim of its own.
    fs::create_dir_all(rig.dir.join("home/other")).unwrap();
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    // A.json holds B's identity. Its command matches the request, so only the identity
    // can refuse it.
    let a = json!({"id":"B","cmd":["sh","-c",&command],"cwd":home,"host":"local","owner":"w1:p1",
        "pane":"w2:p1","state":"pending","exit":Value::Null,"signal":Value::Null,"reason":Value::Null,
        "started_ms":Value::Null,"ended_ms":Value::Null,"timeout_ms":Value::Null,"idle_timeout_ms":Value::Null});
    rig.put("state/jobs/A.json", &a);
    // B.json is a valid, executable claim of its own, with its own cwd, options, and pane.
    let b = json!({"id":"B","cmd":["sh","-c",&command],"cwd":format!("{home}/other"),"host":"local",
        "owner":"w1:p1","pane":"w3:p1","state":"pending","exit":Value::Null,"signal":Value::Null,
        "reason":Value::Null,"started_ms":Value::Null,"ended_ms":Value::Null,"timeout_ms":json!(1000),
        "idle_timeout_ms":Value::Null});
    rig.put("state/jobs/B.json", &b);
    let output = rig.run(&["exec", "A", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    // Both records are unchanged: A is not B, and B was never started.
    assert_eq!(rig.read("state/jobs/A.json"), a);
    assert_eq!(rig.read("state/jobs/B.json"), b);
    // No process, log, or pane ran for either job.
    assert!(!marker.exists());
    assert!(!rig.dir.join("state/jobs/A.log").exists());
    assert!(!rig.dir.join("state/jobs/B.log").exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("belongs to job B"), "{stderr}");
    let herdr = rig.log("herdr.log");
    assert!(!herdr.contains("pane run"), "{herdr}");
    assert!(!herdr.contains("pane close"), "{herdr}");
}

/// A pending claim is executable only when it is complete. An empty pane, a result it
/// already carries, a missing field, or a record that changed since it was read is refused
/// without mutation.
#[test]
fn exec_refuses_an_incomplete_or_conflicting_pending_claim() {
    let rig = Rig::new("exec-incomplete-claim");
    let marker = rig.dir.join("marker");
    let command = format!("touch {}", marker.display());

    // A pending claim with no allocated pane proves nothing.
    rig.claim_with(
        "empty-pane",
        &["sh", "-c", &command],
        &json!(null),
        &json!(null),
        "",
    );
    // A pending claim that already carries a terminal result is not executable.
    rig.claim("carried-exit", &["sh", "-c", &command]);
    let mut carried = rig.read("state/jobs/carried-exit.json");
    carried["exit"] = json!(0);
    rig.put("state/jobs/carried-exit.json", &carried);
    rig.claim("carried-signal", &["sh", "-c", &command]);
    let mut carried = rig.read("state/jobs/carried-signal.json");
    carried["signal"] = json!(15);
    rig.put("state/jobs/carried-signal.json", &carried);
    rig.claim("carried-reason", &["sh", "-c", &command]);
    let mut carried = rig.read("state/jobs/carried-reason.json");
    carried["reason"] = json!("timeout");
    rig.put("state/jobs/carried-reason.json", &carried);
    // A pending claim without a directory, a host, or an owner is incomplete.
    for (id, field) in [
        ("no-cwd", "cwd"),
        ("no-host", "host"),
        ("no-owner", "owner"),
    ] {
        rig.claim(id, &["sh", "-c", &command]);
        let mut claim = rig.read(&format!("state/jobs/{id}.json"));
        claim[field] = json!("");
        rig.put(&format!("state/jobs/{id}.json"), &claim);
    }

    for id in [
        "empty-pane",
        "carried-exit",
        "carried-signal",
        "carried-reason",
        "no-cwd",
        "no-host",
        "no-owner",
    ] {
        let before = rig.read(&format!("state/jobs/{id}.json"));
        let output = rig.run(&["exec", id, "--", "sh", "-c", &command]);
        assert_eq!(output.status.code(), Some(2), "{id}");
        // The record is unchanged: nothing started, nothing ended, nothing ran.
        assert_eq!(rig.read(&format!("state/jobs/{id}.json")), before, "{id}");
        assert!(
            !rig.dir.join(format!("state/jobs/{id}.log")).exists(),
            "{id}"
        );
    }
    assert!(!marker.exists());
    let herdr = rig.log("herdr.log");
    assert!(!herdr.contains("pane run"), "{herdr}");
    assert!(!herdr.contains("pane close"), "{herdr}");
}

/// A request that names different options than its claim asks for a different job.
#[test]
fn exec_refuses_options_that_differ_from_the_claim() {
    let rig = Rig::new("exec-options");
    rig.claim_with("claimed", &["true"], &json!(null), &json!(null), "w1:p1");
    let output = rig.run(&["exec", "claimed", "--timeout", "40ms", "--", "true"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("options differ from the launch record"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let job = rig.read("state/jobs/claimed.json");
    assert!(job["started_ms"].is_null());
    assert!(job["ended_ms"].is_null());
}

#[test]
fn run_wait_captures_exit_output_metadata_and_owner() {
    let rig = Rig::new("run-wait");
    rig.snapshot("state/host.json", now(), 0.0, 8);
    let output = rig.run(&[
        "run",
        "--wait",
        "--",
        "sh",
        "-c",
        "echo hi; echo err >&2; exit 3",
    ]);
    assert_eq!(
        output.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    let id = result["id"].as_str().unwrap();
    let job = rig.read(&format!("state/jobs/{id}.json"));
    assert_eq!(job["exit"], 3);
    assert_eq!(job["state"], "blocked");
    let log = rig.log(&format!("state/jobs/{id}.log"));
    assert!(log.contains("hi\n") && log.contains("err\n"));
    assert_eq!(rig.log("job-output"), log);
    assert_eq!(rig.read("state/items.json")[0]["owner_pane"], "w1:p1");
    let herdr = rig.log("herdr.log");
    assert!(herdr.contains("tab create --workspace w1"));
    assert!(herdr.contains("--source custom:dogsled --agent dogsled-job --state working"));
    assert!(herdr.contains("--token dogsled=exit 3"));
    assert_eq!(rig.read(&format!("at-close/{id}.json")), job);
    assert!(job["ended_ms"].is_number());
    assert!(herdr.ends_with("pane close w2:p1\n"));
    assert_eq!(
        rig.run(&["wait", id, "--max", "30s"]).status.code(),
        Some(3)
    );
    assert_eq!(rig.run(&["logs", id, "--follow"]).stdout, log.as_bytes());
    assert_eq!(rig.json(&["ps", "--all"]).1[0]["exit"], 3);
    assert_eq!(rig.json(&["ps"]).1, json!([]));
    assert_eq!(
        rig.json(&["items", "--owner", "other"]).1["items"],
        json!([])
    );
    assert_eq!(
        rig.json(&["items", "--owner", "w1:p1"]).1["items"][0]["id"],
        id
    );
    assert!(rig.run(&["serve", "--once"]).status.success());
    assert_eq!(rig.read("state/host.json")["jobs"][0]["exit"], 3);
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_number());
}

#[test]
fn exec_signal_and_timeouts_persist_terminal_result_and_kill_descendants() {
    let rig = Rig::new("exec");
    rig.claim("signal", &["sh", "-c", "kill -TERM $$"]);
    let out = rig.run(&["exec", "signal", "--", "sh", "-c", "kill -TERM $$"]);
    assert_eq!(out.status.code(), Some(143));
    assert_eq!(rig.read("state/jobs/signal.json")["signal"], 15);
    for (id, flag, reason) in [
        ("timed", "--timeout", "timeout"),
        ("silent", "--idle-timeout", "idle-timeout"),
    ] {
        // Both parent and descendant block on child-driven timers. No test sleeps.
        let (timeout_ms, idle_timeout_ms) = if flag == "--timeout" {
            (json!(40), json!(null))
        } else {
            (json!(null), json!(40))
        };
        rig.claim_with(
            id,
            &["sh", "-c", "sleep 30 & echo $!; wait"],
            &timeout_ms,
            &idle_timeout_ms,
            "w1:p1",
        );
        let out = rig.run(&[
            "exec",
            id,
            flag,
            "40ms",
            "--",
            "sh",
            "-c",
            "sleep 30 & echo $!; wait",
        ]);
        assert_eq!(
            out.status.code(),
            Some(124),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let result = rig.read(&format!("state/jobs/{id}.json"));
        assert_eq!(result["reason"], reason);
        assert_eq!(result["signal"], 9);
        let pid = String::from_utf8(out.stdout).unwrap().trim().to_owned();
        let ps = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid])
            .output()
            .unwrap();
        // A killed orphan can remain a zombie until the host's init reaps it.
        assert!(
            !ps.status.success() || String::from_utf8_lossy(&ps.stdout).trim().starts_with('Z')
        );
        assert!(rig.log("herdr.log").contains(&format!("dogsled={reason}")));
    }
    rig.claim("ok", &["true"]);
    assert!(rig.run(&["exec", "ok", "--", "true"]).status.success());
    assert_eq!(rig.read("state/jobs/ok.json")["exit"], 0);
    // A claim that already started never runs a second time.
    assert_eq!(
        rig.run(&["exec", "ok", "--", "true"]).status.code(),
        Some(2)
    );
    rig.claim("bad", &["missing-command-xyz"]);
    assert_eq!(
        rig.run(&["exec", "bad", "--", "missing-command-xyz"])
            .status
            .code(),
        Some(2)
    );
    assert!(rig.read("state/jobs/bad.json")["ended_ms"].is_number());
}

#[test]
fn exec_close_failure_preserves_final_record_exit_and_logs() {
    let rig = Rig::new("exec-close-failure");
    fs::write(rig.dir.join("fail-close"), "").unwrap();
    for (id, command, code) in [
        ("success", "echo kept", 0),
        ("failure", "echo kept; exit 7", 7),
    ] {
        rig.claim(id, &["sh", "-c", command]);
        let output = rig.run(&["exec", id, "--", "sh", "-c", command]);
        assert_eq!(output.status.code(), Some(code));
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("dogsled pane close:"), "{stderr}");
        assert!(stderr.contains("close-refused"), "{stderr}");
        let job = rig.read(&format!("state/jobs/{id}.json"));
        assert_eq!(job["exit"], code);
        assert_eq!(job["reason"], Value::Null);
        assert!(job["ended_ms"].is_number());
        assert_eq!(rig.read(&format!("at-close/{id}.json")), job);
        assert_eq!(rig.run(&["wait", id]).status.code(), Some(code));
        assert_eq!(rig.run(&["logs", id]).stdout, b"kept\n");
    }
}

#[test]
fn exec_closes_after_a_spawn_error_and_skips_close_without_a_pane() {
    let rig = Rig::new("exec-close-error");
    rig.claim_with(
        "spawn-error",
        &["missing-command-xyz"],
        &json!(null),
        &json!(null),
        "w1:p1",
    );
    let output = rig.run(&["exec", "spawn-error", "--", "missing-command-xyz"]);
    assert_eq!(output.status.code(), Some(2));
    let job = rig.read("state/jobs/spawn-error.json");
    assert_eq!(rig.read("at-close/spawn-error.json"), job);
    assert!(job["ended_ms"].is_number());
    assert!(job["reason"].is_string());
    assert!(rig.log("herdr.log").ends_with("pane close w1:p1\n"));
    fs::remove_file(rig.dir.join("herdr.log")).unwrap();
    // A claim without an allocated pane is not executable. It closes no pane, and it leaves
    // its own record untouched.
    rig.claim_with("no-pane", &["true"], &json!(null), &json!(null), "");
    let output = rig
        .cmd(&["exec", "no-pane", "--", "true"])
        .env_remove("HERDR_PANE_ID")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(!rig.log("herdr.log").contains("pane close"));
    let job = rig.read("state/jobs/no-pane.json");
    assert!(job["started_ms"].is_null(), "{job}");
    assert!(job["ended_ms"].is_null(), "{job}");
    assert!(!rig.dir.join("state/jobs/no-pane.log").exists());
    // A valid claim runs without a current HERDR pane in its own environment. It closes
    // the pane its claim names, and nothing else.
    rig.claim_with(
        "no-current-pane",
        &["true"],
        &json!(null),
        &json!(null),
        "w2:p1",
    );
    let output = rig
        .cmd(&["exec", "no-current-pane", "--", "true"])
        .env_remove("HERDR_PANE_ID")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(rig.read("state/jobs/no-current-pane.json")["ended_ms"].is_number());
    assert!(rig.log("herdr.log").contains("pane close w2:p1"));
}

#[test]
fn wait_max_and_stop_use_durable_local_job_state() {
    let rig = Rig::new("stop-wait");
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    let mut job = complete("running");
    job["ended_ms"] = Value::Null;
    job["exit"] = Value::Null;
    rig.put("state/jobs/running.json", &job);
    let out = rig.run(&["wait", "running", "--max", "0ms"]);
    assert_eq!(out.status.code(), Some(124));
    assert_eq!(out.stdout, b"still running\n");
    assert!(rig.run(&["stop", "running"]).status.success());
    assert_eq!(rig.read("state/jobs/running.stop"), true);
    assert_eq!(rig.run(&["stop", "../escape"]).status.code(), Some(2));
}

#[test]
fn agent_places_starts_with_its_task_and_tracks_any_kind() {
    let rig = Rig::new("agent");
    let confirmed = now();
    fs::write(rig.dir.join("clock"), confirmed.to_string()).unwrap();
    rig.snapshot("state/host.json", confirmed, 0.0, 8);
    let (code, result) = rig.json(&[
        "agent",
        "--kind",
        "codex",
        "--name",
        "worker",
        "--message",
        "literal ' message",
    ]);
    assert_eq!(code, 0, "{result}");
    assert_eq!(
        result,
        json!({"host":"local","pane":"w2:p1","name":"worker","launch_confirmed_ms":confirmed})
    );
    let log = rig.log("herdr.log");
    // The task is read from its file, named on a line that also holds the child contract as
    // Codex developer instructions; nothing is typed into the pane.
    let task = log
        .lines()
        .find(|l| l.starts_with("agent start worker "))
        .and_then(|l| l.split("Your task is in ").nth(1))
        .and_then(|l| l.split(". Read that file").next())
        .unwrap()
        .to_owned();
    assert_eq!(fs::read_to_string(&task).unwrap(), "literal ' message");
    assert!(
        log.contains(&format!(
            "agent start worker --kind codex --pane w2:p1 -- -c developer_instructions=\"{CONTRACT}\" Your task is in {task}. Read that file and do the task.\n"
        )),
        "{log}"
    );
    assert!(!log.contains("agent prompt"), "{log}");
    assert_eq!(rig.read("state/items.json")[0]["kind"], "agent");
    assert_eq!(
        rig.read("state/items.json")[0]["launch_confirmed_ms"],
        confirmed
    );
    assert_eq!(
        rig.json(&["track", "local", "w3:p1", "--name", "other"]).0,
        0
    );
    assert_eq!(rig.read("state/items.json").as_array().unwrap().len(), 2);
    assert!(
        String::from_utf8(rig.run(&["status"]).stdout)
            .unwrap()
            .contains("owner w1:p1")
    );
}

#[test]
fn concurrent_item_and_queue_writers_keep_every_record() {
    let rig = Rig::new("concurrent-items-queue");
    let barrier = std::sync::Barrier::new(8);
    rig.snapshot("state/host.json", now(), 99.0, 2);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let rig = &rig;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    assert_eq!(rig.json(&["track", "local", &format!("w3:p{i}")]).0, 0);
                    barrier.wait();
                    assert_eq!(rig.json(&["place"]).0, 1);
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
    });
    assert_eq!(rig.read("state/items.json").as_array().unwrap().len(), 8);
    assert_eq!(rig.read("state/queue.json").as_array().unwrap().len(), 8);
}

#[test]
fn queue_admission_is_fifo_and_the_owner_hears_through_its_inbox() {
    let rig = Rig::new("queue-admit");
    rig.config("free_core_fraction_min = 2\n");
    rig.snapshot("state/host.json", now(), 0.0, 8);
    let (_, first) = rig.json(&["place"]);
    let (_, second) = rig.json(&[
        "agent",
        "--kind",
        "pi",
        "--name",
        "queued",
        "--message",
        "hello",
    ]);
    assert_eq!(
        rig.run(&["run", "--wait", "--max", "200ms", "--", "true"])
            .status
            .code(),
        Some(124)
    );
    let tickets = rig.read("state/queue.json");
    assert_eq!(tickets.as_array().unwrap().len(), 3);
    assert_eq!(rig.json(&["items"]).1["queue"][2]["waiting"], false);
    assert_eq!(
        rig.run(&["cancel", second["ticket"].as_str().unwrap()])
            .status
            .code(),
        Some(0)
    );
    assert!(
        String::from_utf8(rig.run(&["status", "--brief"]).stdout)
            .unwrap()
            .contains("2 queued")
    );
    // Admission behavior is independent of the CI runner's real probe load/disk.
    // Exact capacity thresholds and FIFO blocking use the injected-time unit fixture.
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    let owner = json!({"result":{"agents":[{"pane_id":"w1:p1","agent":"pi",
        "agent_status":"working","terminal_id":"term_owner"}]}});
    rig.put("agents.json", &owner);
    assert!(rig.run(&["serve", "--once"]).status.success());
    let tickets = rig.read("state/queue.json");
    assert_eq!(tickets[0]["id"], first["ticket"]);
    assert_eq!(tickets[0]["state"], "admitted");
    assert_eq!(tickets[1]["state"], "cancelled");
    assert_eq!(tickets[2]["state"], "admitted");
    // The owner has no inbox reader yet, so nothing tells it, and nothing types into it.
    assert_eq!(tickets[0]["notified"], false);
    // Once it reads its inbox, the notice arrives there, while the owner still works.
    assert_eq!(rig.json(&["inbox", "claim", "--harness", "pi"]).0, 0);
    assert!(rig.run(&["serve", "--once"]).status.success());
    assert_eq!(rig.read("state/queue.json")[0]["notified"], true);
    let (_, claimed) = rig.json(&["inbox", "claim", "--harness", "pi"]);
    assert!(
        claimed[0]["text"].as_str().unwrap().starts_with(&format!(
            "dogsled: ticket {} admitted on local",
            first["ticket"].as_str().unwrap()
        )),
        "{claimed}"
    );
    assert!(!rig.log("herdr.log").contains("agent prompt"));
    assert_eq!(
        rig.run(&["cancel", first["ticket"].as_str().unwrap()])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn remote_job_controls_use_configured_transport_and_owner_records() {
    let rig = Rig::new("remote-controls");
    rig.peer();
    rig.config("remote_command = ['custom-dogsled']\n");
    let mut job = complete("remote-job");
    job["host"] = json!("8103");
    job["exit"] = json!(3);
    rig.put("job.json", &job);
    rig.put("jobs.json", &json!([job.clone()]));
    let mut snapshot = rig.read("remote.json");
    snapshot["jobs"] = json!([job]);
    rig.put("remote.json", &snapshot);
    rig.put("logs.json", &json!({"bytes":[],"next":0,"done":true}));
    fs::write(rig.dir.join("state/items.json"), serde_json::to_vec(&json!([{
        "id":"remote-job","kind":"job","host":"8103","pane":"w9:p1","owner_pane":"w1:p1","created_ms":0,"closed_ms":null,"name":null
    }])).unwrap()).unwrap();
    assert_eq!(
        rig.run(&["wait", "remote-job", "--max", "30s"])
            .status
            .code(),
        Some(3)
    );
    assert!(rig.run(&["stop", "remote-job"]).status.success());
    assert!(
        rig.run(&["logs", "remote-job", "--follow"])
            .status
            .success()
    );
    assert_eq!(rig.json(&["ps", "--all"]).1[0]["host"], "8103");
    assert!(
        rig.log("ssh.log")
            .contains("custom-dogsled host job remote-job --budget-ms")
    );
}

#[test]
fn checkout_clones_missing_path_accepts_ssh_origin_rejects_mismatch_and_reports_clone_failure() {
    let rig = Rig::new("checkout");
    rig.script("git", r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/git.log"
case "$*" in
  'clone '*) if test -f "$RIG/fail-clone"; then echo credentials >&2; exit 1; fi; mkdir -p "$4"; printf '[tools]\n' > "$4/mise.toml" ;;
  *'remote get-url origin') if test -f "$RIG/origin"; then cat "$RIG/origin"; else printf '%s\n' 'https://other.invalid/repo.git'; fi ;;
  *'rev-parse --show-toplevel') printf '%s\n' "$2" ;;
  *) exit 2 ;;
esac
"#);
    rig.script(
        "mise",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/mise.log"
case "$1" in
 trust) touch "$RIG/trusted" ;;
 *) if ! test -f "$RIG/trusted"; then echo 'config is not trusted' >&2; exit 1; fi; printf '{}\n' ;;
esac
"#,
    );
    let args = [
        "host",
        "checkout",
        "--path",
        "repo",
        "--origin",
        "https://expected.invalid/repo.git",
        "--host-label",
        "peer",
    ];
    assert_eq!(rig.json(&args).0, 0);
    assert!(
        rig.log("git.log")
            .contains("clone -- https://expected.invalid/repo.git")
    );
    assert!(
        rig.log("mise.log")
            .contains(&format!("trust {}/home/repo/mise.toml", rig.dir.display()))
    );
    let (code, error) = rig.json(&args);
    assert_eq!(code, 2);
    let text = error["error"].as_str().unwrap();
    assert_eq!(error["diagnostic"]["cause"], "checkout_identity_mismatch");
    assert_eq!(
        error["diagnostic"]["context"]["workspace"],
        json!(rig.dir.join("home/repo"))
    );
    assert!(text.contains("checkout identity mismatch"));
    assert!(!text.contains("https://expected.invalid/repo.git"));
    assert!(!text.contains("https://other.invalid/repo.git"));
    assert_eq!(rig.log("git.log").matches("clone --").count(), 1);
    fs::write(rig.dir.join("origin"), "git@expected.invalid:repo.git\n").unwrap();
    assert_eq!(rig.json(&args).0, 0);
    fs::remove_file(rig.dir.join("origin")).unwrap();
    fs::write(rig.dir.join("fail-clone"), "").unwrap();
    let (code, error) = rig.json(&[
        "host",
        "checkout",
        "--path",
        "missing",
        "--origin",
        "https://expected.invalid/repo.git",
        "--host-label",
        "peer",
    ]);
    assert_eq!(code, 2);
    assert_eq!(error["diagnostic"]["cause"], "clone_failed");
    assert_eq!(error["code"], "CLONE_FAILED");
    assert!(error["error"].as_str().unwrap().contains("could not clone"));
    assert!(!error.to_string().contains("credentials"));
    assert!(
        !error
            .to_string()
            .contains("https://expected.invalid/repo.git")
    );
}
