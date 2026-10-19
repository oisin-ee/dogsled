use super::*;

fn complete(id: &str) -> Value {
    json!({"id":id,"cmd":["echo","hi"],"cwd":"/tmp","host":"local","owner":"w1:p1","pane":"w2:p1",
        "state":"idle","exit":0,"signal":null,"reason":null,"started_ms":100,"ended_ms":101,"timeout_ms":null,"idle_timeout_ms":null})
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
    let out = rig.run(&["exec", "signal", "--", "sh", "-c", "kill -TERM $$"]);
    assert_eq!(out.status.code(), Some(143));
    assert_eq!(rig.read("state/jobs/signal.json")["signal"], 15);
    for (id, flag, reason) in [
        ("timed", "--timeout", "timeout"),
        ("silent", "--idle-timeout", "idle-timeout"),
    ] {
        // Both parent and descendant block on child-driven timers. No test sleeps.
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
    assert!(rig.run(&["exec", "ok", "--", "true"]).status.success());
    assert_eq!(rig.read("state/jobs/ok.json")["exit"], 0);
    assert_eq!(
        rig.run(&["exec", "ok", "--", "true"]).status.code(),
        Some(2)
    );
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
    let output = rig.run(&["exec", "spawn-error", "--", "missing-command-xyz"]);
    assert_eq!(output.status.code(), Some(2));
    let job = rig.read("state/jobs/spawn-error.json");
    assert_eq!(rig.read("at-close/spawn-error.json"), job);
    assert!(job["ended_ms"].is_number());
    assert!(job["reason"].is_string());
    assert!(rig.log("herdr.log").ends_with("pane close w1:p1\n"));
    fs::remove_file(rig.dir.join("herdr.log")).unwrap();
    let output = rig
        .cmd(&["exec", "no-pane", "--", "true"])
        .env_remove("HERDR_PANE_ID")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(!rig.log("herdr.log").contains("pane close"));
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
    rig.snapshot("state/host.json", now(), 0.0, 8);
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
        json!({"host":"local","pane":"w2:p1","name":"worker"})
    );
    let log = rig.log("herdr.log");
    // The task is the harness's launch argument; nothing is typed into the pane.
    assert!(
        log.contains("agent start worker --kind codex --pane w2:p1 -- literal ' message\n"),
        "{log}"
    );
    assert!(!log.contains("agent prompt"), "{log}");
    assert_eq!(rig.read("state/items.json")[0]["kind"], "agent");
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
    assert!(
        text.contains("peer")
            && text.contains("https://expected.invalid/repo.git")
            && text.contains("https://other.invalid/repo.git")
    );
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
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("peer: clone https://expected.invalid/repo.git")
    );
}
