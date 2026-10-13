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
