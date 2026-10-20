use super::*;
use std::process::Stdio;
use std::time::{Duration, Instant};

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
    rig.snapshot("state/host.json", now(), 1.0, 8);
    let mut host = rig.read("state/host.json");
    host["probe"]["agent_panes"][0]["worktree"] = json!({
        "path":"/t","branch":"feat/x","ahead":0,"dirty":true,"landed":true,"head":"h","base":"b"
    });
    host["probe"]["agent_panes"][0]["done"] = json!(true);
    rig.put("state/host.json", &host);
    let mut items = json!([
        {"id":"mine","kind":"agent","host":"local","pane":"w1:p1","owner_pane":"w1:p1",
         "name":"worker","created_ms":1,"closed_ms":null},
        {"id":"theirs","kind":"agent","host":"local","pane":"w1:p1","owner_pane":"w9:p9",
         "name":"other","created_ms":1,"closed_ms":null}
    ]);
    rig.put("state/items.json", &items);
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
    assert!(
        String::from_utf8(bounded(cmd).stdout)
            .unwrap()
            .starts_with("Your children and jobs: 1 to land\nTo land: worker (uncommitted)")
    );
    // Committing the work changes the signature, so the owner gets an integration reminder.
    host["probe"]["agent_panes"][0]["worktree"]["dirty"] = json!(false);
    host["probe"]["agent_panes"][0]["worktree"]["ahead"] = json!(1);
    host["probe"]["agent_panes"][0]["worktree"]["landed"] = json!(false);
    host["probe"]["agent_panes"][0]["state"] = json!("done");
    rig.put("state/host.json", &host);
    assert!(gate().contains("worker (+1)"));
    assert_eq!(gate(), "");
    // Closed children, working writers and clean landed writers do not block.
    items[0]["closed_ms"] = json!(2);
    rig.put("state/items.json", &items);
    assert_eq!(gate(), "");
    items[0]["closed_ms"] = Value::Null;
    rig.put("state/items.json", &items);
    for state in ["working", "blocked", "unknown"] {
        host["probe"]["agent_panes"][0]["state"] = json!(state);
        rig.put("state/host.json", &host);
        assert_eq!(gate(), "");
    }
    host["probe"]["agent_panes"][0]["state"] = json!("idle");
    host["probe"]["agent_panes"][0]["worktree"]["landed"] = json!(true);
    rig.put("state/host.json", &host);
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
    rig.snapshot("state/host.json", now(), 1.0, 8);
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
    rig.snapshot("state/host.json", now(), 1.0, 8);
    let mut cmd = rig.cmd(&["hook", "context", "--harness", "claude"]);
    cmd.env("HERDR_ENV", "1");
    let out = bounded(cmd);
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        value["hookSpecificOutput"]["hookEventName"],
        "UserPromptSubmit"
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
    rig.snapshot("state/host.json", now(), 1.0, 8);
    // The snapshot's idle agent is in pane w1:p1. Both items name it; only one is the caller's.
    let mut host = rig.read("state/host.json");
    host["probe"]["agent_panes"][0]["worktree"] = json!({
        "path":"/t","branch":"feat/x","ahead":2,"dirty":false,"landed":false,"head":"h","base":"b"
    });
    host["probe"]["agent_panes"][0]["done"] = json!(true);
    rig.put("state/host.json", &host);
    rig.put(
        "state/items.json",
        &json!([
            {"id":"mine","kind":"agent","host":"local","pane":"w1:p1","owner_pane":"w1:p1",
             "name":"worker","created_ms":1,"closed_ms":null},
            {"id":"theirs","kind":"agent","host":"local","pane":"w1:p1","owner_pane":"w9:p9",
             "name":"other","created_ms":1,"closed_ms":null}
        ]),
    );
    let status = String::from_utf8(rig.run(&["status", "--brief"]).stdout).unwrap();
    assert!(status.contains("(2 finished)"), "{status}");
    assert!(!status.contains("dogsled close"), "{status}");
    let mut cmd = rig.cmd(&["hook", "context", "--harness", "pi"]);
    cmd.env("HERDR_ENV", "1");
    let out = String::from_utf8(bounded(cmd).stdout).unwrap();
    assert!(
        out.starts_with("Your children and jobs: 1 to land\nTo land: worker (+2)."),
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
    rig.put("state/host.json", &host);
    let mut cmd = rig.cmd(&["hook", "context", "--harness", "pi"]);
    cmd.env("HERDR_ENV", "1");
    let out = String::from_utf8(bounded(cmd).stdout).unwrap();
    assert!(!out.contains("To land"), "{out}");
}
