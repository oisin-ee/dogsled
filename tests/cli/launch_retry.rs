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
}
