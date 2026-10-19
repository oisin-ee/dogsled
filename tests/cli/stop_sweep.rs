use super::close::{close_rig, git};
use super::*;
use std::process::Stdio;
use std::time::{Duration, Instant};

fn gate(rig: &Rig, pane: &str) -> String {
    let out = rig
        .cmd(&["hook", "gate"])
        .env("HERDR_ENV", "1")
        .env("HERDR_PANE_ID", pane)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn serve(rig: &Rig) -> String {
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stderr).unwrap()
}

#[test]
fn stop_requests_a_pass_and_the_interval_never_runs_doctor() {
    let (rig, tree) = close_rig("stop-request");
    rig.put(
        "state/repositories.json",
        &json!([rig.dir.join("home/repo")]),
    );
    serve(&rig);
    serve(&rig);
    assert!(tree.exists());
    assert!(!rig.dir.join("state/sweep.json").exists());
    assert!(!rig.log("herdr.log").contains("workspace list"));
    assert_eq!(gate(&rig, "w1:p1"), "");
    assert!(rig.dir.join("state/sweep-request.json").exists());
    assert!(!rig.dir.join("state/sweep.json").exists());
    assert!(tree.exists(), "the hook must not remove anything");
    serve(&rig);
    assert!(!tree.exists());
    let report = rig.read("state/sweep.json");
    assert!(
        report["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r.as_str().unwrap().contains(tree.to_str().unwrap()))
    );
    assert!(report["finished_ms"].as_u64().unwrap() >= report["started_ms"].as_u64().unwrap());
    assert!(!rig.dir.join("state/sweep-request.json").exists());
    serve(&rig);
    assert_eq!(rig.read("state/sweep.json"), report);
    let history = fs::read_to_string(rig.dir.join("state/sweeps.jsonl")).unwrap();
    assert_eq!(history.lines().count(), 1);
}

#[test]
fn failed_survey_is_recorded_in_status_context_and_gate() {
    let rig = Rig::new("stop-failed-survey");
    // Publication does not list workspaces. Only the requested doctor survey fails here.
    rig.script("herdr", r#"#!/bin/sh
case "$*" in
 'machine list --json') printf '[]\n' ;;
 'agent list') printf '{"result":{"agents":[]}}\n' ;;
 'pane list') cat "$RIG/panes.json" ;;
 'pane process-info'*) printf '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[]}}}\n' ;;
 'workspace list') echo survey-unavailable >&2; exit 1 ;;
 *) exit 2 ;;
esac
"#);
    gate(&rig, "w1:p1");
    assert!(serve(&rig).contains("dogsled sweep: failed:"));
    let (code, status) = rig.json(&["status"]);
    assert_eq!(code, 0);
    let failure = status["sweep"]["failures"][0].as_str().unwrap();
    assert!(failure.contains("survey-unavailable"), "{status}");
    assert!(status["sweep"]["finished_ms"].is_u64());
    let human = String::from_utf8(rig.run(&["status"]).stdout).unwrap();
    assert!(human.contains("Last cleanup:"));
    assert!(human.contains(failure));
    let context = rig
        .cmd(&["hook", "context", "--harness", "pi"])
        .env("HERDR_ENV", "1")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&context.stdout).contains("1 failures, 0 repository warnings"));
    let message = gate(&rig, "w1:p1");
    assert!(message.contains("survey-unavailable"), "{message}");
    assert!(message.contains("Do not bypass cleanup safeguards"));
    assert_eq!(gate(&rig, "w1:p1"), "");
}

#[test]
fn missing_repository_warns_once_per_set_and_does_not_stop_other_cleanup() {
    let (rig, tree) = close_rig("stop-missing-repo");
    let missing = rig.dir.join("gone-repo");
    let repo = rig.dir.join("home/repo");
    rig.put("state/repositories.json", &json!([missing, repo]));
    gate(&rig, "w1:p1");
    serve(&rig);
    assert!(
        !tree.exists(),
        "one missing repository must not stop cleanup"
    );
    let message = gate(&rig, "w1:p1");
    for expected in [
        missing.to_str().unwrap(),
        "repositories.json",
        "repository entry",
        "dogsled agent --profile general --name <name> --message-file <brief>",
        "Do not edit the state by hand",
    ] {
        assert!(message.contains(expected), "{expected}: {message}");
    }
    assert_eq!(gate(&rig, "w1:p1"), "");
    // A new set speaks once; returning to the first set never loops the Stop hook.
    let second = rig.dir.join("another-gone-repo");
    rig.put("state/repositories.json", &json!([missing, second, repo]));
    serve(&rig);
    assert!(gate(&rig, "w1:p1").contains(second.to_str().unwrap()));
    assert_eq!(gate(&rig, "w1:p1"), "");
    rig.put("state/repositories.json", &json!([missing, repo]));
    serve(&rig);
    assert_eq!(gate(&rig, "w1:p1"), "");
    let (_, status) = rig.json(&["status"]);
    assert_eq!(status["sweep"]["repositories"][0]["id"], json!(missing));
    let human = String::from_utf8(rig.run(&["status"]).stdout).unwrap();
    assert!(human.contains(missing.to_str().unwrap()));
}

#[test]
fn landed_dirty_reaches_only_owners_in_the_same_main_repository() {
    let (rig, tree) = close_rig("stop-landed-dirty");
    let repo = rig.dir.join("home/repo");
    let other = rig.dir.join("home/other");
    fs::create_dir_all(&other).unwrap();
    git(&other, &["init", "-q", "-b", "main"]);
    let alias = rig.dir.join("repo-link");
    std::os::unix::fs::symlink(&repo, &alias).unwrap();
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[
            {"pane_id":"w1:p1","terminal_id":"same","cwd":alias},
            {"pane_id":"w3:p1","terminal_id":"other","cwd":other}
        ]}}),
    );
    rig.put("state/repositories.json", &json!([repo]));
    fs::write(tree.join("notes.txt"), "keep this work").unwrap();
    gate(&rig, "w1:p1");
    serve(&rig);
    assert!(tree.exists());
    let message = gate(&rig, "w1:p1");
    for expected in [
        tree.to_str().unwrap(),
        "feat/x",
        "commits are already in the default branch",
        "notes.txt",
        "Read the dirty files",
        "commit them on a branch and land them",
        "ask the user before you discard",
        "dogsled close <pane>",
        "dogsled prune worktrees",
        "Never use `git worktree remove --force`, `git clean`, or `rm -rf`",
    ] {
        assert!(message.contains(expected), "{expected}: {message}");
    }
    assert_eq!(gate(&rig, "w1:p1"), "");
    assert_eq!(gate(&rig, "w3:p1"), "");
    let mut panes = rig.read("panes.json");
    panes["result"]["panes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"pane_id":"w4:p1","terminal_id":"linked","cwd":tree}));
    rig.put("panes.json", &panes);
    assert!(gate(&rig, "w4:p1").contains("notes.txt"));
    assert_eq!(gate(&rig, "w4:p1"), "");
    let report = rig.read("state/sweep.json");
    assert_eq!(report["landed_dirty"][0]["kind"], "landed_dirty");
    assert_eq!(report["landed_dirty"][0]["removable"], false);
}

#[test]
fn claude_and_codex_stop_deliver_cleanup_and_request_the_next_pass() {
    let rig = Rig::new("stop-inbox-cleanup");
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[
            {"pane_id":"w1:p1","agent":"claude","agent_status":"idle","terminal_id":"term_owner"},
            {"pane_id":"w2:p1","agent":"codex","agent_status":"idle","terminal_id":"term_child"}
        ]}}),
    );
    rig.put("state/sweep.json", &json!({"started_ms":1,"finished_ms":2,"results":[],"failures":["survey unavailable"],"repositories":[],"landed_dirty":[]}));
    for (harness, pane) in [("claude", "w1:p1"), ("codex", "w2:p1")] {
        let run = || {
            rig.cmd(&["hook", "inbox", "--harness", harness, "--event", "Stop"])
                .env("HERDR_ENV", "1")
                .env("HERDR_PANE_ID", pane)
                .output()
                .unwrap()
        };
        let out = run();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(value["decision"], "block");
        assert!(
            value["reason"]
                .as_str()
                .unwrap()
                .contains("survey unavailable")
        );
        assert!(rig.dir.join("state/sweep-request.json").exists());
        assert_eq!(run().stdout, [] as [u8; 0]);
    }
}

struct Service(std::process::Child);
impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn until(check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(6);
    while !check() {
        assert!(
            Instant::now() < deadline,
            "service did not respond before the next interval"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn stop_wakes_the_service_without_waiting_for_the_publication_interval() {
    let rig = Rig::new("stop-wakeup");
    let _service = Service(
        rig.cmd(&["serve", "--interval", "60"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    until(|| rig.dir.join("state/host.json").exists());
    assert!(!rig.dir.join("state/sweep.json").exists());
    gate(&rig, "w1:p1");
    until(|| rig.dir.join("state/sweep.json").exists());
    until(|| !rig.dir.join("state/sweep-request.json").exists());
    let first = rig.read("state/sweep.json");
    gate(&rig, "w1:p1");
    until(|| rig.read("state/sweep.json") != first);
}
