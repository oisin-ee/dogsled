use super::*;
use std::process::Stdio;
use std::time::{Duration, Instant};

fn replace(rig: &Rig, publisher: &super::observation::Supplied, publication: &mut Value) {
    let revision = publication["revision"].as_u64().unwrap() + 1;
    publication["revision"] = json!(revision);
    publisher.replace(publication.clone());
    wait_publication(rig, |frame| frame["revision"] == revision);
}

// Keep stdin open deliberately: lifecycle hooks must never wait for its EOF.
fn bounded(mut command: Command) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("context hook exceeded its deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

#[test]
fn stop_gate_is_owner_scoped_durable_and_tracks_changed_work() {
    let rig = Rig::new("stop-gate");
    rig.snapshot("state", now(), 1.0, 8);
    let mut host = rig.host();
    host["probe"]["agent_panes"][0]["worktree"] = json!({
        "path":"/t","branch":"feat/x","ahead":0,"dirty":true,"landed":true,"head":"h","base":"b"
    });
    host["probe"]["agent_panes"][0]["done"] = json!(true);
    rig.put_host(&host);
    let mut items = json!([
        {"id":"mine","kind":"agent","host":"local","pane":"w1:p1","owner_pane":"w1:p1",
         "name":"worker","created_ms":1,"closed_ms":null},
        {"id":"theirs","kind":"agent","host":"local","pane":"w1:p1","owner_pane":"w9:p9",
         "name":"other","created_ms":1,"closed_ms":null}
    ]);
    rig.put_items(&items);
    let mut publication = super::observation::fixtures::context_pre_handoff();
    publication["views"]["boards"]["data"]["w1:p1"]["rows"][0]["dirty"] = json!(true);
    publication["views"]["boards"]["data"]["w1:p1"]["rows"][0]["ahead"] = Value::Null;
    let publisher = super::observation::Supplied::start(&rig, publication.clone());
    let gate = || {
        let mut cmd = rig.cmd(&["hook", "gate"]);
        cmd.env("HERDR_ENV", "1");
        let out = bounded(cmd);
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    };
    let first = gate();
    assert!(first.contains("worker (uncommitted)"), "{first}");
    assert!(!first.contains("other"), "{first}");
    // A fresh process must not repeat an unchanged reminder.
    assert_eq!(gate(), "");
    let mut cmd = rig.cmd(&["hook", "context", "--harness", "pi"]);
    cmd.env("HERDR_ENV", "1");
    assert!(String::from_utf8(bounded(cmd).stdout).unwrap().starts_with(
        "Your children and jobs: 1 ready to merge\nReady to merge: worker (uncommitted)"
    ));
    // Committing the work changes the signature, so the owner gets an integration reminder.
    host["probe"]["agent_panes"][0]["worktree"]["dirty"] = json!(false);
    host["probe"]["agent_panes"][0]["worktree"]["ahead"] = json!(1);
    host["probe"]["agent_panes"][0]["worktree"]["landed"] = json!(false);
    host["probe"]["agent_panes"][0]["state"] = json!("done");
    rig.put_host(&host);
    publication["views"]["boards"]["data"]["w1:p1"]["rows"][0]["dirty"] = json!(false);
    publication["views"]["boards"]["data"]["w1:p1"]["rows"][0]["ahead"] = json!(1);
    replace(&rig, &publisher, &mut publication);
    assert!(gate().contains("worker (+1)"));
    assert_eq!(gate(), "");
    // Closed children, working writers and clean landed writers do not block.
    items[0]["closed_ms"] = json!(2);
    rig.put_items(&items);
    let row = publication["views"]["boards"]["data"]["w1:p1"]["rows"][0].clone();
    publication["views"]["boards"]["data"]["w1:p1"]["rows"] = json!([]);
    replace(&rig, &publisher, &mut publication);
    assert_eq!(gate(), "");
    items[0]["closed_ms"] = Value::Null;
    rig.put_items(&items);
    for state in ["working", "blocked", "unknown"] {
        host["probe"]["agent_panes"][0]["state"] = json!(state);
        rig.put_host(&host);
        let mut active = row.clone();
        active["group"] = json!("running");
        active["state"] = json!(state);
        publication["views"]["boards"]["data"]["w1:p1"]["rows"] = json!([active]);
        replace(&rig, &publisher, &mut publication);
        assert_eq!(gate(), "");
    }
    host["probe"]["agent_panes"][0]["state"] = json!("idle");
    host["probe"]["agent_panes"][0]["worktree"]["landed"] = json!(true);
    rig.put_host(&host);
    publication["views"]["boards"]["data"]["w1:p1"]["rows"] = json!([]);
    replace(&rig, &publisher, &mut publication);
    assert_eq!(gate(), "");
    // A missing owner never falls back to the fleet's children.
    let out = rig
        .cmd(&["hook", "gate"])
        .env("HERDR_ENV", "1")
        .env_remove("HERDR_PANE_ID")
        .output()
        .unwrap();
    assert!(out.status.success() && out.stdout.is_empty());
    let out = rig
        .cmd(&["hook", "gate"])
        .env_remove("HERDR_ENV")
        .output()
        .unwrap();
    assert!(out.status.success() && out.stdout.is_empty());
}

#[test]
fn context_selects_the_supplied_revision_with_store_and_observers_unavailable() {
    let rig = Rig::short("context-isolated");
    let publication = super::observation::fixtures::context_pre_handoff();
    let _publisher = super::observation::Supplied::start(&rig, publication);
    for program in ["git", "herdr", "ssh"] {
        rig.script(
            program,
            "#!/bin/sh\nprintf '%s\n' \"$*\" >> \"$RIG/reader-access.log\"\nexit 97\n",
        );
    }
    fs::create_dir(rig.dir.join("state/state.db")).unwrap();
    let _locks: Vec<_> = [
        "service",
        "state-db",
        "host",
        "branch-checks",
        "observation",
    ]
    .into_iter()
    .map(|name| {
        let file = fs::File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(rig.dir.join(format!("state/{name}.lock")))
            .unwrap();
        file.try_lock().unwrap();
        file
    })
    .collect();
    let mut cmd = rig.cmd(&["hook", "context", "--harness", "pi"]);
    cmd.env("HERDR_ENV", "1");
    let out = bounded(cmd);
    assert!(out.status.success());
    assert!(out.stderr.is_empty(), "{out:?}");
    let context = String::from_utf8(out.stdout).unwrap();
    assert!(
        context
            .starts_with("Your children and jobs: 1 ready to merge\nReady to merge: worker (+2)."),
        "{context}"
    );
    assert_eq!(rig.log("reader-access.log"), "");
}

#[test]
fn context_without_herdr_exits_silently_before_config_or_discovery() {
    let rig = Rig::new("context-no-herdr");
    rig.config("invalid = [");
    for harness in ["claude", "codex", "pi"] {
        let mut cmd = rig.cmd(&["hook", "context", "--harness", harness]);
        cmd.env_remove("HERDR_ENV");
        let out = bounded(cmd);
        assert!(out.status.success());
        assert_eq!(out.stdout, [] as [u8; 0]);
        assert_eq!(out.stderr, [] as [u8; 0]);
    }
    assert_eq!(rig.log("herdr.log"), "");
}

#[test]
fn context_without_snapshot_exits_silently() {
    let rig = Rig::new("context-no-snapshot");
    for harness in ["claude", "codex", "pi"] {
        let mut cmd = rig.cmd(&["hook", "context", "--harness", harness]);
        cmd.env("HERDR_ENV", "1");
        let out = bounded(cmd);
        assert!(out.status.success());
        assert_eq!(out.stdout, [] as [u8; 0]);
        assert_eq!(out.stderr, [] as [u8; 0]);
    }
}

#[test]
fn context_uses_status_brief_for_every_harness_and_lifecycle_event() {
    let rig = Rig::new("context-output");
    // Both paths share a real observation and a fixed clock, so formatting is stable.
    rig.snapshot("state", now() - 60_000, 1.0, 8);
    fs::write(rig.dir.join("clock"), now().to_string()).unwrap();
    let _observer = rig.serve_at(&rig.dir.join("state"), "3600");
    wait_publication(&rig, |frame| {
        let samples: i64 = rig
            .database()
            .query_row("SELECT COUNT(*) FROM samples", [], |row| row.get(0))
            .unwrap();
        samples >= 2 && frame["local"]["host"]["probe"] == rig.host()["probe"]
    });
    let brief = String::from_utf8(rig.run(&["status", "--brief"]).stdout).unwrap();
    for harness in ["claude", "codex", "pi"] {
        for event in ["SessionStart", "UserPromptSubmit"] {
            let mut cmd = rig.cmd(&["hook", "context", "--harness", harness, "--event", event]);
            cmd.env("HERDR_ENV", "1");
            let out = bounded(cmd);
            assert!(out.status.success());
            assert_eq!(out.stderr, [] as [u8; 0]);
            if harness == "pi" {
                assert_eq!(String::from_utf8(out.stdout).unwrap(), brief);
            } else {
                assert_eq!(
                    serde_json::from_slice::<Value>(&out.stdout).unwrap(),
                    json!({
                        "hookSpecificOutput": {"hookEventName": event, "additionalContext": brief}
                    })
                );
            }
        }
    }
}

#[test]
fn context_default_event_is_prompt_submit_and_failures_do_not_block() {
    let rig = Rig::new("context-failure");
    rig.snapshot("state", now(), 1.0, 8);
    let observer = rig.serve();
    let mut cmd = rig.cmd(&["hook", "context", "--harness", "claude"]);
    cmd.env("HERDR_ENV", "1");
    let out = bounded(cmd);
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        value["hookSpecificOutput"]["hookEventName"],
        "UserPromptSubmit"
    );
    drop(observer);
    let (code, unavailable) = rig.json(&["observation", "current"]);
    assert_eq!(code, 2, "{unavailable}");
    assert!(
        unavailable["error"]
            .as_str()
            .unwrap()
            .contains("unavailable")
    );
    rig.script("herdr", "#!/bin/sh\nexec sleep 60\n");
    let mut cmd = rig.cmd(&[
        "hook",
        "context",
        "--harness",
        "codex",
        "--budget-ms",
        "100",
    ]);
    cmd.env("HERDR_ENV", "1");
    let out = bounded(cmd);
    assert!(out.status.success());
    assert_eq!(out.stdout, [] as [u8; 0]);
    assert_eq!(out.stderr, [] as [u8; 0]);
    // Without an explicit budget, the prompt still has its own five-second cap.
    let mut cmd = rig.cmd(&["hook", "context", "--harness", "pi"]);
    cmd.env("HERDR_ENV", "1");
    let out = bounded(cmd);
    assert!(out.status.success());
    assert_eq!(out.stdout, [] as [u8; 0]);
    assert_eq!(out.stderr, [] as [u8; 0]);
    rig.config("invalid = [");
    let mut cmd = rig.cmd(&["hook", "context", "--harness", "pi"]);
    cmd.env("HERDR_ENV", "1");
    assert!(bounded(cmd).status.success());
}

#[test]
fn context_names_only_the_callers_children_with_work_to_land() {
    let rig = Rig::new("context-close");
    rig.snapshot("state", now(), 1.0, 8);
    // The real producer fixture captures the open-finished window before handoff.
    let publication = super::observation::fixtures::context_pre_handoff();
    fs::write(
        rig.dir.join("clock"),
        publication["published_ms"].to_string(),
    )
    .unwrap();
    let mut host = publication["local"]["host"].clone();
    rig.put_host(&host);
    rig.put_items(&publication["views"]["status"]["data"]["items"]);
    let mut publication = publication;
    let publisher = super::observation::Supplied::start(&rig, publication.clone());
    let status = String::from_utf8(rig.run(&["status", "--brief"]).stdout).unwrap();
    assert!(status.contains("(2 finished)"), "{status}");
    assert!(!status.contains("dogsled close"), "{status}");
    let mut cmd = rig.cmd(&["hook", "context", "--harness", "pi"]);
    cmd.env("HERDR_ENV", "1");
    let out = String::from_utf8(bounded(cmd).stdout).unwrap();
    assert!(
        out.starts_with("Your children and jobs: 1 ready to merge\nReady to merge: worker (+2)."),
        "{out}"
    );
    assert!(!out.contains("other"), "{out}");
    assert!(out.ends_with(&status), "{out}");
    let (code, board) = rig.json(&["board"]);
    assert_eq!(code, 0);
    assert_eq!(board["rows"].as_array().unwrap().len(), 1);
    assert_eq!(board["rows"][0]["group"], "to_land");
    assert_eq!(board["rows"][0]["what"], "pi");
    assert_eq!(board["rows"][0]["ahead"], 2);
    assert_eq!(
        rig.json(&["board", "--all"]).1["rows"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    // A finished reader with nothing to land is not named.
    host["probe"]["agent_panes"][0]["worktree"]["ahead"] = json!(0);
    rig.put_host(&host);
    publication["views"]["boards"]["data"]["w1:p1"]["rows"] = json!([]);
    replace(&rig, &publisher, &mut publication);
    let mut cmd = rig.cmd(&["hook", "context", "--harness", "pi"]);
    cmd.env("HERDR_ENV", "1");
    let out = String::from_utf8(bounded(cmd).stdout).unwrap();
    assert!(!out.contains("Ready to merge"), "{out}");
}

#[test]
fn status_rows_derive_lifecycle_even_without_a_published_label() {
    let rig = Rig::new("context-status-lifecycle");
    for (terminal, expected) in [
        ("idle", "stopped"),
        ("done", "stopped"),
        ("blocked", "blocked"),
    ] {
        rig.snapshot("state", now(), 1.0, 8);
        let mut host = rig.host();
        host["probe"]["agent_panes"][0]["state"] = json!(terminal);
        host["probe"]["agent_panes"][0]["lifecycle"] = json!("done");
        rig.put_host(&host);
        rig.put("agents.json", &json!({"result":{"agents":[{"pane_id":"w1:p1","agent":"pi","agent_status":terminal}]}}));
        let _observer = rig.serve();
        super::wait_publication(&rig, |frame| {
            frame["views"]["status"]["data"]["hosts"][0]["probe"]["agent_panes"][0]["state"]
                == terminal
        });
        let output = rig.run(&["status"]);
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains(&format!("w1:p1 {expected} ")), "{text}");
        assert!(text.contains(&format!("({expected} 1)")), "{text}");
    }
}

/// The board's own word stays the same, and the lifecycle state travels beside it. An
/// unreadable terminal state is `unknown`: it never reads as `stopped` or `done`.
#[test]
fn the_board_publishes_the_lifecycle_state_beside_its_own_word() {
    let rig = Rig::new("context-lifecycle");
    rig.snapshot("state", now(), 1.0, 8);
    let mut host = rig.host();
    host["probe"]["agent_panes"] = json!([{
        "pane":"w1:p1","agent":"pi","state":"unknown","name":null
    }]);
    rig.put_host(&host);
    rig.put_items(
        &json!([{"id":"mine","kind":"agent","host":"local","pane":"w1:p1","owner_pane":"w1:p1",
             "name":"worker","created_ms":1,"closed_ms":null}]),
    );
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{"pane_id":"w1:p1","agent":"pi","agent_status":"unknown"}]}}),
    );
    let _observer = rig.serve();
    super::wait_publication(&rig, |frame| {
        frame["views"]["board"]["data"]["rows"][0]["id"] == "mine"
    });
    let (code, board) = rig.json(&["board"]);
    assert_eq!(code, 0);
    let row = &board["rows"][0];
    // The board's word for an unreadable terminal state stays `unknown`.
    assert_eq!(row["state"], "unknown", "{board}");
    // The row stays under ATTENTION: an unreadable agent needs the owner.
    assert_eq!(row["group"], "attention", "{board}");
}

/// Missing observations without launch progress leave the lifecycle unknown.
/// The board must not claim startup or a landed result.
#[test]
fn an_unobserved_child_is_unknown_and_never_claims_a_landed_result() {
    let rig = Rig::new("context-starting");
    rig.snapshot("state", now(), 1.0, 8);
    rig.put_items(
        &json!([{"id":"mine","kind":"agent","host":"local","pane":"w1:p2","owner_pane":"w1:p1",
             "name":"worker","created_ms":1,"closed_ms":null}]),
    );
    let _observer = rig.serve();
    super::wait_publication(&rig, |frame| {
        frame["views"]["board"]["data"]["rows"][0]["id"] == "mine"
    });
    let (code, board) = rig.json(&["board"]);
    assert_eq!(code, 0);
    let row = &board["rows"][0];
    assert_eq!(row["state"], "unknown", "{board}");
    assert_eq!(row["group"], "attention", "{board}");
    assert!(row["ahead"].is_null(), "{board}");
    assert!(!row["dirty"].as_bool().unwrap_or(false), "{board}");
}
