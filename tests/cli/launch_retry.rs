use super::*;

fn advance(rig: &Rig) -> Value {
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    let next = rig.read("state/queue.json")[0]["retry"]["next_ms"]
        .as_u64()
        .unwrap();
    fs::write(rig.dir.join("clock"), next.to_string()).unwrap();
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    rig.read("state/queue.json")[0].clone()
}

#[test]
fn branch_retry_keeps_preparation_host_and_terminal_across_service_restart() {
    let (rig, repo) = place::place_rig("branch-retry");
    fs::write(rig.dir.join("start-fails"), "").unwrap();
    let (code, queued) = rig.json(&[
        "agent",
        "--cwd",
        repo.to_str().unwrap(),
        "--branch",
        "retry-owned",
        "--base",
        "main",
        "--name",
        "worker",
        "--message",
        "go",
    ]);
    assert_eq!(code, 1, "{queued}");
    assert!(
        queued["retrying"]
            .as_str()
            .unwrap()
            .contains("original startup failed")
    );
    fs::remove_file(rig.dir.join("start-fails")).unwrap();
    rig.put("agents.json", &json!({"result":{"agents":[{"pane_id":"w8:p1","terminal_id":"term_tree","name":"worker","agent":"pi","agent_status":"working"}]}}));
    // A repeated preparation would now refuse an already-open workspace.
    fs::write(rig.dir.join("already-open"), "").unwrap();
    // A service restart during admission resumes only the saved launch.
    let mut queue = rig.read("state/queue.json");
    queue[0]["state"] = json!("admitting");
    rig.put("state/queue.json", &queue);
    let ticket = advance(&rig);
    assert_eq!(ticket["state"], "admitted", "{ticket}");
    let log = rig.log("herdr.log");
    assert_eq!(log.matches("worktree open").count(), 1, "{log}");
    assert_eq!(log.matches("agent start worker ").count(), 1, "{log}");
    assert!(!log.contains("pane close"));
    assert_eq!(rig.log("setup.log").lines().count(), 1);
    assert_eq!(
        rig.read("state/placements.json").as_array().unwrap().len(),
        1
    );
    assert_eq!(rig.read("state/items.json").as_array().unwrap().len(), 1);
}

fn host_request(rig: &Rig) -> Value {
    json!({"id":"recover-one","action":{"type":"agent","kind":"pi","name":"worker","message":"go"},"host":"local","cwd":rig.dir.join("home"),"pane":null,"workspace":"w1","workspace_name":"caller","owner":"parent"})
}

fn failed_host_start(rig: &Rig, request: &Value) {
    rig.script(
        "herdr",
        &fs::read_to_string(rig.dir.join("bin/herdr"))
            .unwrap()
            .replace(
                "  'agent start'*|",
                "  'agent start'*) echo 'startup response lost' >&2; exit 1 ;;\n  ",
            ),
    );
    let (code, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 2, "{error}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("startup response lost")
    );
}

#[test]
fn lost_start_response_reconciles_existing_agent_without_starting_twice() {
    let rig = Rig::new("started-response-lost");
    let request = host_request(&rig);
    failed_host_start(&rig, &request);
    rig.put("agents.json", &json!({"result":{"agents":[{"pane_id":"w2:p1","terminal_id":"term_child","name":"worker","agent":"pi","agent_status":"working"}]}}));
    let (code, result) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 0, "{result}");
    assert_eq!(result["pane"], "w2:p1");
    assert_eq!(
        rig.log("herdr.log").matches("agent start worker ").count(),
        1
    );
    assert!(!rig.log("herdr.log").contains("pane close"));
    // A receipt remains authoritative even if the agent subsequently exits.
    rig.put("agents.json", &json!({"result":{"agents":[]}}));
    assert_eq!(
        rig.json(&["host", "launch", "--request", &request.to_string()])
            .0,
        0
    );
    assert_eq!(
        rig.log("herdr.log").matches("agent start worker ").count(),
        1
    );
    let mut different = request;
    different["action"]["message"] = json!("another task");
    assert_eq!(
        rig.json(&["host", "launch", "--request", &different.to_string()])
            .0,
        2
    );
}

#[test]
fn retry_refuses_replaced_terminal_and_foreign_agent_and_ambiguous_exit() {
    let rig = Rig::new("launch-ownership");
    let request = host_request(&rig);
    failed_host_start(&rig, &request);
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[{"pane_id":"w2:p1","terminal_id":"replacement"}]}}),
    );
    let (_, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("terminal for w2:p1 changed")
    );
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[{"pane_id":"w2:p1","terminal_id":"term_child"}]}}),
    );
    rig.put("agents.json", &json!({"result":{"agents":[{"pane_id":"w2:p1","terminal_id":"term_child","name":"other","agent":"pi"}]}}));
    let (_, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert!(error["error"].as_str().unwrap().contains("another agent"));
    rig.put("agents.json", &json!({"result":{"agents":[]}}));
    rig.script(
        "herdr",
        &fs::read_to_string(rig.dir.join("bin/herdr"))
            .unwrap()
            .replace(
                "[{\"pid\":10,\"name\":\"zsh\"}]",
                "[{\"pid\":11,\"name\":\"pi\"}]",
            ),
    );
    let (_, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("startup outcome is unknown")
    );
    assert_eq!(
        rig.log("herdr.log").matches("agent start worker ").count(),
        1
    );
    assert_eq!(rig.log("herdr.log").matches("tab create").count(), 1);
    assert!(!rig.log("herdr.log").contains("pane close"));
}

#[test]
fn first_start_delegates_shell_readiness_to_herdr_without_a_racy_precheck() {
    let rig = Rig::new("native-shell-readiness");
    let request = host_request(&rig);
    rig.script(
        "herdr",
        &fs::read_to_string(rig.dir.join("bin/herdr"))
            .unwrap()
            .replace(
                "[{\"pid\":10,\"name\":\"zsh\"}]",
                "[{\"pid\":10,\"name\":\"zsh\"},{\"pid\":11,\"name\":\"mise\"}]",
            ),
    );
    let (code, result) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 0, "{result}");
    let log = rig.log("herdr.log");
    assert_eq!(log.matches("agent start worker ").count(), 1, "{log}");
    assert!(!log.contains("pane process-info"), "{log}");
    assert_eq!(log.matches("tab create").count(), 1, "{log}");
}

#[test]
fn native_busy_refusal_is_preserved_without_repeating_the_start() {
    let rig = Rig::new("native-busy-refusal");
    let request = host_request(&rig);
    rig.script(
        "herdr",
        &fs::read_to_string(rig.dir.join("bin/herdr"))
            .unwrap()
            .replace(
                "  'agent start'*|",
                "  'agent start'*) echo 'agent_pane_busy: not an available shell' >&2; exit 1 ;;\n  ",
            ),
    );
    let (code, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 2, "{error}");
    assert!(error["error"].as_str().unwrap().contains("agent_pane_busy"));
    let (code, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 2, "{error}");
    let error = error["error"].as_str().unwrap();
    assert!(error.contains("startup outcome is unknown"), "{error}");
    assert!(error.contains("agent_pane_busy"), "{error}");
    let log = rig.log("herdr.log");
    assert_eq!(log.matches("agent start worker ").count(), 1, "{log}");
    assert_eq!(log.matches("tab create").count(), 1, "{log}");
    assert!(!log.contains("pane close"), "{log}");
}

#[test]
fn remote_retry_keeps_selected_host_and_skips_checkout_and_prepare() {
    let rig = Rig::new("remote-retry-owned");
    let repo = rig.dir.join("home/repo");
    repository(&repo);
    rig.peer();
    rig.snapshot("state/host.json", now(), 0.0, 8);
    rig.script("ssh", &fs::read_to_string(rig.dir.join("bin/ssh")).unwrap().replace(
        "  *'host launch'*)", "  *'host launch'*) if ! test -f \"$RIG/response-lost\"; then touch \"$RIG/response-lost\"; echo 'connection lost after launch' >&2; exit 255; fi;",
    ));
    let (code, queued) = rig.json(&[
        "agent",
        "--host",
        "netcup",
        "--cwd",
        repo.to_str().unwrap(),
        "--branch",
        "remote-owned",
        "--base",
        "main",
        "--name",
        "worker",
        "--message",
        "go",
    ]);
    assert_eq!(code, 1, "{queued}");
    // Selection must not run again, even when the chosen host now refuses reservations.
    rig.put("reserve.json", &json!({"reserved":false,"headroom":null}));
    let ticket = advance(&rig);
    assert_eq!(ticket["state"], "admitted", "{ticket}");
    let log = rig.log("ssh.log");
    for command in ["host checkout", "host prepare", "host reserve"] {
        assert_eq!(log.matches(command).count(), 1, "{log}");
    }
    assert_eq!(log.matches("host launch").count(), 2);
    assert!(!rig.log("herdr.log").contains("agent start"));
    let request_id = ticket["request"]["id"].as_str().unwrap();
    let attempt = rig.read(&format!("state/admissions/{request_id}.json"));
    assert_eq!(attempt["launch"]["terminal"], "term_remote");
}

#[test]
fn interrupted_job_admission_preserves_running_record_without_relaunch() {
    let rig = Rig::new("interrupted-job-admission");
    let action = json!({"type":"job","cmd":["true"],"timeout_ms":null,"idle_timeout_ms":null});
    let request = json!({"id":"interrupted-job","action":action,"host":null,"branch":null,"base":null,
        "cwd":rig.dir.join("home"),"repository":null,"workspace":"w1","workspace_name":"caller","owner":"w1:p1"});
    let launch = json!({"id":"interrupted-job","action":action,"host":"local","cwd":rig.dir.join("home"),
        "pane":null,"workspace":"w1","workspace_name":"caller","owner":"w1:p1"});
    fs::create_dir_all(rig.dir.join("state/admissions")).unwrap();
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    rig.put("state/admissions/interrupted-job.json", &json!({"request":request,"host":{"id":"local","label":"local"},"launch":launch,"error":null}));
    let job = json!({"id":"interrupted-job","cmd":["true"],"cwd":rig.dir.join("home"),"host":"local","owner":"w1:p1",
        "pane":"w2:p1","state":"running","exit":null,"signal":null,"reason":null,"started_ms":123,"ended_ms":null,
        "timeout_ms":null,"idle_timeout_ms":null});
    rig.put("state/jobs/interrupted-job.json", &job);
    rig.put("state/queue.json", &json!([{"id":"interrupted-ticket","request":request,"created_ms":100,"state":"admitting",
        "waiting":false,"waiting_until_ms":null,"notified":false,"result":null,"error":null,"retry":{"attempts":0,"next_ms":100}}]));
    let ticket = advance(&rig);
    assert_eq!(ticket["state"], "failed", "{ticket}");
    assert_eq!(rig.read("state/jobs/interrupted-job.json"), job);
    assert!(!rig.dir.join("state/launches/interrupted-job.json").exists());
    let log = rig.log("herdr.log");
    assert!(!log.contains("tab create"), "{log}");
    assert!(!log.contains("pane run"), "{log}");
}

#[test]
fn first_host_launch_refuses_a_terminal_replaced_after_preparation() {
    let rig = Rig::new("replaced-before-first-launch");
    let mut request = host_request(&rig);
    request["pane"] = json!("w2:p1");
    request["terminal"] = json!("prepared_terminal");
    // No launch receipt: the first transport never reached the host. Its pane
    // now names a different terminal, even though the replacement shell is idle.
    let (code, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 2, "{error}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("terminal for w2:p1 changed")
    );
    assert!(!rig.log("herdr.log").contains("agent start"));
    assert!(!rig.log("herdr.log").contains("tab create"));
    request["id"] = json!("missing-terminal");
    request.as_object_mut().unwrap().remove("terminal");
    let (code, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 2, "{error}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("prepared pane has no terminal identity")
    );
    assert!(!rig.log("herdr.log").contains("agent start"));
}
