use super::close::{close_rig, git};
use super::*;
use std::os::unix::fs::FileTypeExt as _;
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

/// Record that `owner` launched a child into `tree`: the ledger's proof that it created the work.
/// The child's pane is gone, so the record holds nothing.
fn launched_into(rig: &Rig, id: &str, owner: &str, tree: &Path) {
    fs::create_dir_all(tree).unwrap();
    fs::create_dir_all(rig.dir.join("state/launches")).unwrap();
    rig.put(
        &format!("state/launches/{id}.progress.json"),
        &json!({
            "request":{"id":id,"action":{"type":"agent","kind":"pi","name":id,"message":"go"},
                "host":"local","cwd":tree,"owner":owner},
            "pane":format!("gone:{id}"),"terminal":id,"attempted":true,"confirmed":true,
                "result":{},"error":null
        }),
    );
}

/// The latest pass failed on each worktree with its detail.
fn failed_pass(rig: &Rig, failed: &[(&Path, &str)]) {
    let entries: Vec<Value> = failed
        .iter()
        .map(|(tree, detail)| {
            json!({"entry":{"class":"worktrees","id":tree,"detail":detail,"removable":true,
                "tree":{"repo":rig.dir.join("home/repo"),"path":tree,"branch":"feat/x"}},
                "detail":detail})
        })
        .collect();
    let details: Vec<&str> = failed.iter().map(|(_, detail)| *detail).collect();
    rig.put(
        "state/sweep.json",
        &json!({"started_ms":1,"finished_ms":2,"results":[],"failures":details,
            "failure_entries":entries,"repositories":[],"landed_dirty":[]}),
    );
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
    // Successful removals remain visible in status, but never continue an agent turn.
    assert_eq!(gate(&rig, "w1:p1"), "");
    let (_, status) = rig.json(&["status"]);
    assert_eq!(status["sweep"]["results"], report["results"]);
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
 'pane process-info'*) printf '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":10,"name":"zsh","argv":["zsh"]}]}}}\n' ;;
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
    let brief = String::from_utf8(rig.run(&["status", "--brief"]).stdout).unwrap();
    assert!(
        brief.contains("1 failures, 0 repository warnings"),
        "{brief}"
    );
    // No agent created the host, so a host-level failure stays in status and reaches no agent.
    let context = rig
        .cmd(&["hook", "context", "--harness", "pi"])
        .env("HERDR_ENV", "1")
        .env("HERDR_PANE_ID", "w1:p1")
        .output()
        .unwrap();
    let context = String::from_utf8_lossy(&context.stdout);
    assert!(!context.contains("Cleanup at"), "{context}");
    assert_eq!(gate(&rig, "w1:p1"), "");
}

#[test]
fn git_pruned_worktree_whose_directory_remains_is_held_once_and_never_removed() {
    let (rig, tree) = close_rig("stop-pruned-worktree");
    let repo = rig.dir.join("home/repo").canonicalize().unwrap();
    let head = git(&tree, &["rev-parse", "HEAD"]).trim().to_owned();
    rig.put("state/repositories.json", &json!([repo]));
    let registration = json!([{"repo":repo,"path":tree,"branch":"feat/x","head":head}]);
    rig.put("state/worktrees.json", &registration);
    // The tova case: the `.git` file went missing, so Git marked the worktree prunable, and
    // `git worktree prune` dropped it. The directory and its generated output remain.
    fs::create_dir_all(tree.join("build")).unwrap();
    fs::write(tree.join("build/generated"), "output\n").unwrap();
    fs::remove_file(tree.join(".git")).unwrap();
    git(&repo, &["worktree", "unlock", tree.to_str().unwrap()]);
    git(&repo, &["worktree", "prune"]);
    assert!(!git(&repo, &["worktree", "list"]).contains(".worktrees/x"));

    gate(&rig, "w1:p1");
    let log = serve(&rig);
    let reason = format!(
        "worktree unregistered by git; directory remains: {}",
        tree.display()
    );
    assert!(log.contains("dogsled sweep: hold:"), "{log}");
    assert!(!log.contains("failed:"), "{log}");
    assert!(tree.join("build/generated").is_file());
    assert_eq!(rig.read("state/worktrees.json"), registration);
    let (_, status) = rig.json(&["status"]);
    assert_eq!(status["sweep"]["failures"], json!([]), "{status}");
    let hold = &status["sweep"]["unlinked"][0];
    assert_eq!(hold["removable"], false);
    assert_eq!(hold["kind"], "unlinked");
    let detail = hold["detail"].as_str().unwrap();
    assert!(detail.contains(&reason), "{detail}");
    assert!(detail.contains("move or delete the directory yourself"));
    let human = String::from_utf8(rig.run(&["status"]).stdout).unwrap();
    assert!(
        human.contains(&format!("hold: {}", tree.display())),
        "{human}"
    );
    // Only the agent that launched work into the worktree hears about it.
    assert_eq!(gate(&rig, "w1:p1"), "");
    launched_into(&rig, "creator", "w1:p1", &tree);
    assert_eq!(gate(&rig, "w2:p1"), "");
    let message = gate(&rig, "w1:p1");
    assert!(message.contains(&reason), "{message}");
    assert!(message.contains("do not delete it yourself"));
    // The same hold speaks once, and later passes neither fail nor touch the directory.
    assert_eq!(gate(&rig, "w1:p1"), "");
    assert!(!serve(&rig).contains("failed:"));
    assert_eq!(gate(&rig, "w1:p1"), "");
    assert!(tree.join("build/generated").is_file());

    // The owner checked the directory and deleted it: the next pass drops the registration.
    fs::remove_dir_all(&tree).unwrap();
    gate(&rig, "w1:p1");
    let log = serve(&rig);
    assert!(!log.contains("failed:"), "{log}");
    assert_eq!(rig.read("state/worktrees.json"), json!([]));
}

#[test]
fn missing_configured_repository_stays_in_status_and_does_not_stop_other_cleanup() {
    let (rig, tree) = close_rig("stop-missing-repo");
    let missing = rig.dir.join("gone-repo");
    let repo = rig.dir.join("home/repo");
    rig.put("state/repositories.json", &json!([missing, repo]));
    rig.config(&format!("[cleanup]\nrepositories = [{}]\n", json!(missing)));
    gate(&rig, "w1:p1");
    serve(&rig);
    assert!(
        !tree.exists(),
        "one missing repository must not stop cleanup"
    );
    assert_eq!(rig.read("state/repositories.json"), json!([repo]));
    assert!(
        fs::read_to_string(rig.dir.join("config.toml"))
            .unwrap()
            .contains(missing.to_str().unwrap())
    );
    // No agent created a configured repository: the warning stays in status only.
    assert_eq!(gate(&rig, "w1:p1"), "");
    let (_, status) = rig.json(&["status"]);
    assert_eq!(status["sweep"]["repositories"][0]["id"], json!(missing));
    let human = String::from_utf8(rig.run(&["status"]).stdout).unwrap();
    assert!(human.contains(missing.to_str().unwrap()));
}

#[test]
fn cleanup_failure_report_preserves_worktree_identity_for_notice_routing() {
    let (rig, tree) = close_rig("stop-failure-identity");
    rig.put(
        "state/repositories.json",
        &json!([rig.dir.join("home/repo")]),
    );
    fs::write(rig.dir.join("hook-fails"), "").unwrap();
    gate(&rig, "w1:p1");
    serve(&rig);
    assert!(tree.exists());
    let report = rig.read("state/sweep.json");
    let failed = report["failure_entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|failed| failed["entry"]["id"] == json!(tree))
        .unwrap();
    assert_eq!(failed["entry"]["class"], "worktrees");
    assert_eq!(failed["entry"]["tree"]["path"], json!(tree));
    assert!(
        report["failures"]
            .as_array()
            .unwrap()
            .contains(&failed["detail"])
    );
}

#[test]
fn cleanup_notice_reaches_only_the_agent_that_created_the_work() {
    let rig = Rig::new("cleanup-notice-routing");
    let checkout = rig.dir.join("home/child-checkout");
    let elsewhere = rig.dir.join("home/elsewhere");
    fs::create_dir_all(&checkout).unwrap();
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[
            {"pane_id":"w1:p1","terminal_id":"operator"},
            {"pane_id":"w2:p1","terminal_id":"unrelated"},
            {"pane_id":"w3:p1","terminal_id":"owner"},
            {"pane_id":"w4:p1","terminal_id":"worker","cwd":checkout}
        ]}}),
    );
    // w3 launched the worker into the checkout. w1 launched w2 and w3 elsewhere, so it created
    // agents but not this work. Dogsled launched none of w1's own work.
    launched_into(&rig, "unrelated", "w1:p1", &elsewhere);
    launched_into(&rig, "owner", "w1:p1", &elsewhere);
    launched_into(&rig, "worker", "w3:p1", &checkout);
    rig.put(
        "state/items.json",
        &json!([
            {"id":"worker","kind":"agent","host":"local","pane":"w4:p1","owner_pane":"w3:p1",
                "name":"worker","created_ms":1,"closed_ms":null}
        ]),
    );
    let entry = json!({"class":"worktrees","id":checkout,"detail":"teardown refused","removable":true,
        "tree":{"repo":rig.dir.join("home/repo"),"path":checkout,"branch":"feat/child"}});
    rig.put(
        "state/sweep.json",
        &json!({"started_ms":1,"finished_ms":2,"results":[],
        "failures":["child teardown refused","host survey failed"],
        "failure_entries":[{"entry":entry,"detail":"child teardown refused"}],
        "repositories":[{"class":"repositories","id":"/host/repository","detail":"cannot inspect",
            "removable":false,"tree":null}],"landed_dirty":[],
        "unlinked":[
            {"class":"worktrees","id":checkout,"detail":"held: child checkout unlinked","removable":false,
                "kind":"unlinked","tree":{"repo":rig.dir.join("home/repo"),"path":checkout,"branch":"feat/child"}},
            {"class":"worktrees","id":"/host/orphan","detail":"held: orphan unlinked","removable":false,
                "kind":"unlinked","tree":{"repo":"/host","path":"/host/orphan","branch":"feat/orphan"}}
        ]}),
    );
    // Neither a pane Dogsled did not launch, an unrelated child, nor the child working in the
    // checkout created it. None of them hears, and none consumes the owner's history.
    assert_eq!(gate(&rig, "w1:p1"), "");
    assert_eq!(gate(&rig, "w2:p1"), "");
    assert_eq!(gate(&rig, "w4:p1"), "");
    let context = |pane: &str| {
        let out = rig
            .cmd(&["hook", "context", "--harness", "pi"])
            .env("HERDR_ENV", "1")
            .env("HERDR_PANE_ID", pane)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    };
    rig.snapshot("state/host.json", now(), 0.0, 8);
    assert!(!context("w1:p1").contains("Cleanup at"));
    assert!(
        context("w3:p1").contains("Cleanup at 2 ms: 1 failures and 1 held worktrees in your work")
    );
    let owner = gate(&rig, "w3:p1");
    assert!(owner.contains("child teardown refused"), "{owner}");
    assert!(!owner.contains("host survey failed"), "{owner}");
    assert!(!owner.contains("/host/repository"), "{owner}");
    assert!(owner.contains("child checkout unlinked"), "{owner}");
    assert!(!owner.contains("orphan unlinked"), "{owner}");
    assert_eq!(gate(&rig, "w3:p1"), "");
    // After the worker's pane closes, the launch record still names the creator.
    let mut panes = rig.read("panes.json");
    panes["result"]["panes"]
        .as_array_mut()
        .unwrap()
        .retain(|pane| pane["pane_id"] != "w4:p1");
    rig.put("panes.json", &panes);
    assert_eq!(gate(&rig, "w1:p1"), "");
    assert_eq!(gate(&rig, "w3:p1"), "");
    fs::remove_file(rig.dir.join("state/gate.json")).unwrap();
    assert!(gate(&rig, "w3:p1").contains("child teardown refused"));
    assert_eq!(gate(&rig, "w1:p1"), "");
}

#[test]
fn landed_dirty_reaches_only_the_agent_that_created_the_worktree() {
    let (rig, tree) = close_rig("stop-landed-dirty");
    let repo = rig.dir.join("home/repo");
    let alias = rig.dir.join("repo-link");
    std::os::unix::fs::symlink(&repo, &alias).unwrap();
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[
            {"pane_id":"w1:p1","terminal_id":"creator","cwd":alias},
            {"pane_id":"w3:p1","terminal_id":"researcher","cwd":repo}
        ]}}),
    );
    rig.put("state/repositories.json", &json!([repo]));
    // The creator recorded the launch through a symlinked path; it still names the worktree.
    launched_into(&rig, "creator", "w1:p1", &alias.join(".worktrees/x"));
    fs::write(tree.join("notes.txt"), "keep this work").unwrap();
    gate(&rig, "w1:p1");
    serve(&rig);
    assert!(tree.exists());
    // An agent in the same repository that created nothing there hears nothing.
    assert_eq!(gate(&rig, "w3:p1"), "");
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
    // A pane that later works inside the worktree did not create it either.
    let mut panes = rig.read("panes.json");
    panes["result"]["panes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"pane_id":"w4:p1","terminal_id":"linked","cwd":tree}));
    rig.put("panes.json", &panes);
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
    let owner = rig.dir.join("home/owner-tree");
    let child = rig.dir.join("home/child-tree");
    launched_into(&rig, "owner-work", "w1:p1", &owner);
    launched_into(&rig, "child-work", "w2:p1", &child);
    failed_pass(
        &rig,
        &[
            (&owner, "survey unavailable: w1:p1"),
            (&child, "survey unavailable: w2:p1"),
        ],
    );
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
        let reason = value["reason"].as_str().unwrap();
        assert!(
            reason.contains(&format!("survey unavailable: {pane}")),
            "{reason}"
        );
        assert_eq!(reason.matches("survey unavailable").count(), 1, "{reason}");
        assert!(rig.dir.join("state/sweep-request.json").exists());
        assert_eq!(run().stdout, [] as [u8; 0]);
    }
}

/// The service the test owns. The guard kills the service and every process it started, and
/// waits for it to leave, so no service outlives its test.
struct Service(#[allow(dead_code)] Owned);

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
    // Keep the real private directory, but address the socket relative to it.
    // macOS temporary-directory prefixes can exceed sockaddr_un's path limit.
    let command = |args: &[&str]| {
        let mut cmd = rig.cmd(args);
        cmd.current_dir(&rig.dir).env("DOGSLED_STATE", "state");
        cmd
    };
    let request = || {
        let out = command(&["hook", "gate"])
            .env("HERDR_ENV", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    let mut service = command(&["serve", "--interval", "60"]);
    service.stdout(Stdio::null()).stderr(Stdio::null());
    let _service = Service(Owned::spawn(&mut service));
    until(|| rig.dir.join("state/host.json").exists());
    assert!(
        fs::metadata(rig.dir.join("state/sweep.sock"))
            .unwrap()
            .file_type()
            .is_socket()
    );
    assert!(!rig.dir.join("state/sweep.json").exists());
    request();
    until(|| rig.dir.join("state/sweep.json").exists());
    until(|| !rig.dir.join("state/sweep-request.json").exists());
    let first = rig.read("state/sweep.json");
    request();
    until(|| rig.read("state/sweep.json") != first);
}

#[test]
fn long_state_path_without_a_socket_still_consumes_stop_requests() {
    let rig = Rig::new("stop-long-path");
    let state = rig
        .dir
        .join("state")
        .join("long-state-directory-".repeat(8));
    fs::create_dir_all(&state).unwrap();
    let journal_path = rig.dir.join("service.log");
    let journal = fs::File::create(&journal_path).unwrap();
    let mut service = rig.cmd(&["serve", "--interval", "1"]);
    service
        .env("DOGSLED_STATE", &state)
        .stdout(Stdio::null())
        .stderr(journal);
    let _service = Service(Owned::spawn(&mut service));
    until(|| state.join("host.json").exists());
    assert!(!state.join("sweep.sock").exists());
    assert!(!state.join("sweep.json").exists());
    assert!(
        fs::read_to_string(&journal_path)
            .unwrap()
            .contains("cleanup wake socket unavailable")
    );
    // Requests arrive after startup, and again after a completed pass. Neither can rely
    // on the socket or on service restart to reach the worker.
    for count in 1..=2 {
        let out = rig
            .cmd(&["hook", "gate"])
            .env("HERDR_ENV", "1")
            .env("DOGSLED_STATE", &state)
            .output()
            .unwrap();
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("cleanup request failed"));
        until(|| state.join("sweep.json").exists() && !state.join("sweep-request.json").exists());
        assert_eq!(
            fs::read_to_string(state.join("sweeps.jsonl"))
                .unwrap()
                .lines()
                .count(),
            count
        );
    }
}

#[test]
fn request_write_failure_warns_without_losing_gate_or_inbox_delivery() {
    for harness in ["pi", "claude", "codex"] {
        let rig = Rig::new(&format!("stop-write-failure-{harness}"));
        rig.put(
            "agents.json",
            &json!({"result":{"agents":[
                {"pane_id":"w1:p1","agent":harness,"agent_status":"idle","terminal_id":"term_owner"}
            ]}}),
        );
        let tree = rig.dir.join("home/owned-tree");
        launched_into(&rig, "owned", "w1:p1", &tree);
        failed_pass(&rig, &[(&tree, "previous survey failed")]);
        if harness != "pi" {
            rig.snapshot("state/host.json", now(), 0.0, 8);
            let mut snapshot = rig.read("state/host.json");
            snapshot["probe"]["agent_panes"][0]["terminal_id"] = json!("term_owner");
            rig.put("state/host.json", &snapshot);
            let registered = rig
                .cmd(&[
                    "hook",
                    "inbox",
                    "--harness",
                    harness,
                    "--event",
                    "UserPromptSubmit",
                ])
                .env("HERDR_ENV", "1")
                .output()
                .unwrap();
            assert!(registered.status.success());
            assert_eq!(
                rig.json(&["send", "w1:p1", "--message", "inbox delivery survives"])
                    .0,
                0
            );
        }
        // A directory at the request file path makes atomic replacement fail while
        // the inbox, latest report, and gate history remain writable.
        fs::create_dir(rig.dir.join("state/sweep-request.json")).unwrap();
        let args = if harness == "pi" {
            vec!["hook", "gate"]
        } else {
            vec!["hook", "inbox", "--harness", harness, "--event", "Stop"]
        };
        let out = rig.cmd(&args).env("HERDR_ENV", "1").output().unwrap();
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("cleanup request failed"));
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("previous survey failed"), "{harness}: {text}");
        if harness != "pi" {
            let output: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(output["decision"], "block");
            assert!(text.contains("inbox delivery survives"));
            assert_eq!(
                rig.read("state/inbox/term_owner.json")[0]["state"],
                "delivered"
            );
        }
        let repeated = rig.cmd(&args).env("HERDR_ENV", "1").output().unwrap();
        assert_eq!(repeated.stdout, [] as [u8; 0]);
    }
}

#[test]
fn pass_history_keeps_only_seven_days_including_the_boundary() {
    let rig = Rig::new("stop-history-window");
    let week = 7 * 24 * 60 * 60 * 1000u64;
    // Legacy entries without timestamps also leave the retained window.
    fs::write(
        rig.dir.join("state/sweeps.jsonl"),
        "{\"started_ms\":0,\"finished_ms\":0}\n",
    )
    .unwrap();
    for time in [1, 2, week + 1, week + 2] {
        fs::write(rig.dir.join("clock"), time.to_string()).unwrap();
        gate(&rig, "w1:p1");
        serve(&rig);
    }
    let history = fs::read_to_string(rig.dir.join("state/sweeps.jsonl")).unwrap();
    let reports: Vec<Value> = history
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        reports
            .iter()
            .map(|r| r["ts_ms"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [2, week + 1, week + 2]
    );
    let latest = rig.read("state/sweep.json");
    assert_eq!(latest["ts_ms"], latest["finished_ms"]);
    assert_eq!(latest["ts_ms"], week + 2);
}
