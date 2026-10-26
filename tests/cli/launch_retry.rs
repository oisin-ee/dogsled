use super::*;

fn remote_agent(rig: &Rig, repo: Option<&Path>) -> (i32, Value) {
    let mut args = vec![
        "agent",
        "--host",
        "netcup",
        "--name",
        "worker",
        "--message",
        "go",
    ];
    if let Some(repo) = repo {
        args.extend([
            "--cwd",
            repo.to_str().unwrap(),
            "--branch",
            "remote-complete",
            "--base",
            "main",
        ]);
    }
    rig.json(&args)
}

#[test]
fn reservation_refusal_overrides_an_eligible_snapshot_and_clears_on_admission() {
    let rig = Rig::new("reservation-refusal");
    rig.peer();
    rig.snapshot("state/host.json", now(), 0.0, 8);
    rig.put(
        "reserve.json",
        &json!({"reserved":false,"headroom":{
            "host":"8103","headroom":0.0,"eligible":false,"live_agents":0,
            "reason":"reservation raced with another request"
        }}),
    );
    let (code, result) = remote_agent(&rig, None);
    assert_eq!(code, 1, "{result}");
    let ticket = rig.read("state/queue.json")[0].clone();
    assert_eq!(
        ticket["wait_reason"],
        "netcup: reservation raced with another request"
    );
    let (_, board) = rig.json(&["board", "--all"]);
    assert_eq!(board["hosts"][1]["eligible"], true, "{board}");
    assert_eq!(
        board["rows"][0]["activity"],
        "for netcup: reservation raced with another request"
    );
    assert!(!ticket["wait_reason"].as_str().unwrap().contains("local:"));
    rig.put("reserve.json", &json!({"reserved":true,"headroom":null}));
    let ticket = advance(&rig);
    assert_eq!(ticket["state"], "admitted", "{ticket}");
    assert!(ticket.get("wait_reason").is_none(), "{ticket}");
}

#[test]
fn unpinned_refusal_keeps_one_line_per_host() {
    let rig = Rig::new("all-host-refusals");
    rig.peer();
    rig.snapshot("state/host.json", now(), 100.0, 8);
    rig.put("reserve.json", &json!({"reserved":false,"headroom":{
        "host":"8103","headroom":0.0,"eligible":false,"live_agents":0,"reason":"peer reservation refused"
    }}));
    let (code, queued) = rig.json(&["agent", "--name", "worker", "--message", "go"]);
    assert_eq!(code, 1, "{queued}");
    let ticket = rig.read("state/queue.json")[0].clone();
    let lines: Vec<_> = ticket["wait_reason"].as_str().unwrap().lines().collect();
    assert_eq!(lines.len(), 2, "{ticket}");
    assert!(lines[0].starts_with("local: "), "{ticket}");
    assert_eq!(lines[1], "netcup: peer reservation refused");
}

#[test]
fn legacy_ticket_without_saved_reason_keeps_the_snapshot_fallback_and_pin() {
    let rig = Rig::new("legacy-wait-reason");
    rig.peer();
    rig.snapshot("state/host.json", now(), 20.0, 8);
    rig.put("reserve.json", &json!({"reserved":false,"headroom":null}));
    assert_eq!(remote_agent(&rig, None).0, 1);
    let mut queue = rig.read("state/queue.json");
    queue[0].as_object_mut().unwrap().remove("wait_reason");
    rig.put("state/queue.json", &queue);
    let (_, board) = rig.json(&["board", "--all"]);
    assert_eq!(
        board["rows"][0]["activity"], "for a host with room",
        "{board}"
    );
    rig.snapshot("remote.json", now(), 100.0, 16);
    rig.expire_cache();
    let (_, board) = rig.json(&["board", "--all"]);
    let reason = board["rows"][0]["activity"].as_str().unwrap();
    assert!(reason.starts_with("for netcup: "), "{board}");
    assert!(!reason.contains("local:"), "{board}");
}

#[test]
fn ssh_failure_before_launch_retries_reservation_checkout_and_preparation() {
    for step in ["reserve", "checkout", "prepare"] {
        let rig = Rig::new(&format!("early-ssh-{step}"));
        let repo = rig.dir.join("home/repo");
        repository(&repo);
        rig.peer();
        rig.snapshot("state/host.json", now(), 0.0, 8);
        let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
        rig.script("ssh", &script.replace(
            &format!("  *'host {step}'*)"),
            &format!("  *'host {step}'*) if ! test -f \"$RIG/failed-once\"; then touch \"$RIG/failed-once\"; echo 'connection reset' >&2; exit 255; fi;"),
        ));
        let (code, result) = remote_agent(&rig, Some(&repo));
        assert_eq!(code, 1, "{step}: {result}");
        let ticket = rig.read("state/queue.json")[0].clone();
        assert_eq!(ticket["state"], "pending", "{step}: {ticket}");
        assert_eq!(ticket["retry"]["attempts"], 1, "{step}: {ticket}");
        let ticket = advance(&rig);
        assert_eq!(ticket["state"], "admitted", "{step}: {ticket}");
        let log = rig.log("ssh.log");
        assert_eq!(log.matches("host launch").count(), 1, "{log}");
        assert_eq!(log.matches(&format!("host {step}")).count(), 2, "{log}");
        if step == "reserve" {
            let calls: Vec<_> = log.lines().filter(|s| s.contains("host reserve")).collect();
            let ids: Vec<_> = calls
                .iter()
                .map(|s| {
                    s.split(" --id ")
                        .nth(1)
                        .unwrap()
                        .split_whitespace()
                        .next()
                        .unwrap()
                })
                .collect();
            assert_eq!(ids[0], ids[1], "same reservation ID: {log}");
            assert!(calls.iter().all(|s| s.contains("netcup-dev")), "{log}");
        } else {
            assert_eq!(log.matches("host reserve").count(), 2, "{log}");
        }
    }
}

#[test]
fn persistent_prelaunch_ssh_failure_exhausts_the_launch_attempt_budget() {
    for step in ["reserve", "prepare"] {
        let rig = Rig::new(&format!("early-ssh-exhausted-{step}"));
        let repo = rig.dir.join("home/repo");
        repository(&repo);
        rig.peer();
        rig.snapshot("state/host.json", now(), 0.0, 8);
        let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
        rig.script(
            "ssh",
            &script.replace(
                &format!("  *'host {step}'*)"),
                &format!("  *'host {step}'*) echo 'connection refused' >&2; exit 255;"),
            ),
        );
        assert_eq!(remote_agent(&rig, Some(&repo)).0, 1);
        assert_eq!(advance(&rig)["retry"]["attempts"], 2);
        let ticket = advance(&rig);
        assert_eq!(ticket["state"], "failed", "{ticket}");
        assert!(
            ticket["error"]
                .as_str()
                .unwrap()
                .ends_with("(after 3 attempts)"),
            "{ticket}"
        );
        assert_eq!(
            rig.log("ssh.log").matches(&format!("host {step}")).count(),
            3
        );
        assert!(!rig.log("ssh.log").contains("host launch"));
    }
}

#[test]
fn prelaunch_deadlines_structured_refusals_and_invalid_requests_are_terminal() {
    for (case, response) in [
        (
            "deadline",
            r#"{"error":"deadline reached","deadline":true}"#,
        ),
        (
            "refusal",
            r#"{"error":"private remote detail","code":"CLONE_FAILED"}"#,
        ),
    ] {
        let rig = Rig::new(&format!("early-terminal-{case}"));
        rig.peer();
        rig.snapshot("state/host.json", now(), 0.0, 8);
        let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
        rig.script(
            "ssh",
            &script.replace(
                "  *'host reserve'*)",
                &format!("  *'host reserve'*) echo '{response}'; exit 2;"),
            ),
        );
        let (code, result) = remote_agent(&rig, None);
        assert_eq!(code, 2, "{case}: {result}");
        assert!(!rig.dir.join("state/queue.json").exists(), "{case}");
    }
    let rig = Rig::new("queued-invalid");
    rig.snapshot("state/host.json", now(), 0.0, 8);
    let repo = rig.dir.join("home/repo");
    repository(&repo);
    rig.put(
        "state/queue.json",
        &json!([{
            "id":"invalid-ticket","request":{"id":"invalid-request","action":{"type":"place"},
                "host":"local","branch":"bad..branch","base":null,"cwd":repo,
                "repository":{"path":repo,"origin":"https://example.invalid/repo.git","relative":"repo","working_root":repo},
                "workspace":null,"workspace_name":null,"owner":"w1:p1"},
            "created_ms":1,"state":"pending","waiting":false,"waiting_until_ms":null,
            "notified":false,"result":null,"error":null,"retry":{"attempts":0,"next_ms":1}
        }]),
    );
    let ticket = advance(&rig);
    assert_eq!(ticket["state"], "failed", "{ticket}");
    assert_eq!(ticket["retry"]["attempts"], 0, "{ticket}");
    assert!(
        ticket["error"]
            .as_str()
            .unwrap()
            .contains("not a valid branch name")
    );
}

#[test]
fn terminal_ssh_categories_never_enter_the_retry_queue() {
    for step in ["reserve", "checkout", "prepare", "launch"] {
        for (case, response, status) in [
            ("command", "echo 'private remote failure' >&2", 2),
            ("auth", "echo 'Permission denied (publickey)' >&2", 255),
            (
                "invalid",
                "echo '{\"error\":\"private invalid request\"}'",
                2,
            ),
            (
                "conflict",
                "echo '{\"error\":\"private conflict\",\"code\":\"CONFLICT\"}'",
                2,
            ),
            (
                "deadline",
                "echo '{\"error\":\"private deadline\",\"deadline\":true}'",
                2,
            ),
        ] {
            let rig = Rig::new(&format!("terminal-{step}-{case}"));
            let repo = rig.dir.join("home/repo");
            repository(&repo);
            rig.peer();
            rig.snapshot("state/host.json", now(), 0.0, 8);
            let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
            rig.script(
                "ssh",
                &script.replace(
                    &format!("  *'host {step}'*)"),
                    &format!("  *'host {step}'*) {response}; exit {status};"),
                ),
            );
            let (code, error) = remote_agent(&rig, Some(&repo));
            assert_eq!(code, 2, "{step}/{case}: {error}");
            assert!(!rig.dir.join("state/queue.json").exists(), "{step}/{case}");
            assert!(!error.to_string().contains("private"), "{error}");
            assert_eq!(
                rig.log("ssh.log").matches(&format!("host {step}")).count(),
                1
            );
        }
    }
}

#[test]
fn expired_preparation_reservations_revalidate_on_the_same_host_and_keep_refusals() {
    for action in ["agent", "place"] {
        let rig = Rig::new(&format!("expired-preparation-{action}"));
        let repo = rig.dir.join("home/repo");
        repository(&repo);
        rig.peer();
        rig.snapshot("state/host.json", now(), 0.0, 8);
        fs::create_dir_all(rig.dir.join("peer-state")).unwrap();
        rig.snapshot("peer-state/host.json", now(), 0.0, 16);
        let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
        let reserve = format!(
            r#"  *'host reserve'*)
    for remote do :; done
    remote=${{remote#* host }}
    DOGSLED_STATE="$RIG/peer-state" sh -c '{} host '"$remote" ;;"#,
            env!("CARGO_BIN_EXE_dogsled")
        );
        rig.script("ssh", &script
            .replace("case \"$*\" in", "case \"$*\" in\n  *'host prepared'*) printf '{}\\n' ;;")
            .replace("  *'host reserve'*) cat \"$RIG/reserve.json\" ;;", &reserve)
            .replace("  *'host checkout'*)", "  *'host checkout'*) if ! test -f \"$RIG/failed-once\"; then touch \"$RIG/failed-once\"; echo reset >&2; exit 255; fi;"));
        let (code, queued) = if action == "agent" {
            remote_agent(&rig, Some(&repo))
        } else {
            rig.json(&[
                "place",
                "--host",
                "netcup",
                "--repo",
                repo.to_str().unwrap(),
                "--branch",
                "remote-complete",
                "--base",
                "main",
            ])
        };
        assert_eq!(code, 1, "{action}: {queued}");
        let mut holds = rig.read("peer-state/placements.json");
        holds[0]["created_ms"] = json!(1);
        rig.put("peer-state/placements.json", &holds);
        // Admission now refuses this host despite the caller's eligible cached snapshot.
        let mut snapshot = rig.read("peer-state/host.json");
        snapshot["probe"]["disk_free_percent"] = json!(-1.0);
        rig.put("peer-state/host.json", &snapshot);
        let ticket = advance(&rig);
        assert_eq!(ticket["state"], "pending", "{action}: {ticket}");
        let reason = ticket["wait_reason"].as_str().unwrap();
        assert!(reason.starts_with("netcup: "), "{ticket}");
        assert!(!reason.contains("local:"), "{ticket}");
        assert_eq!(ticket["retry"]["attempts"], 1, "{ticket}");
        let log = rig.log("ssh.log");
        assert_eq!(log.matches("host reserve").count(), 2, "{log}");
        assert_eq!(log.matches("host checkout").count(), 1, "{log}");
        assert!(!log.contains("host prepare"), "{log}");
        assert!(!log.contains("host launch"), "{log}");
        let calls: Vec<_> = log.lines().filter(|s| s.contains("host reserve")).collect();
        let ids: Vec<_> = calls
            .iter()
            .map(|call| {
                call.split(" --id ")
                    .nth(1)
                    .unwrap()
                    .split_whitespace()
                    .next()
                    .unwrap()
            })
            .collect();
        assert_eq!(ids[0], ids[1], "{log}");
        assert!(
            calls.iter().all(|call| call.contains("netcup-dev")),
            "{log}"
        );
        rig.snapshot("peer-state/host.json", now(), 0.0, 16);
        let ticket = advance(&rig);
        assert_eq!(ticket["state"], "admitted", "{action}: {ticket}");
        assert!(ticket.get("wait_reason").is_none(), "{ticket}");
        assert_eq!(
            rig.read("peer-state/placements.json")
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
}

#[test]
fn interrupted_preparation_refuses_to_adopt_or_rerun_incomplete_setup() {
    for base in [Some("main"), None] {
        let (rig, repo) = place::place_rig(if base.is_some() {
            "interrupted-with-base"
        } else {
            "interrupted-without-base"
        });
        let config = fs::read_to_string(repo.join("dogsled.toml")).unwrap();
        fs::write(repo.join("dogsled.toml"), config.replace("setup = [", "setup = [\n  \"if test -f \\\"$RIG/state/preparation-receipts/interrupted.json\\\"; then cp \\\"$RIG/state/preparation-receipts/interrupted.json\\\" \\\"$RIG/intent-at-setup.json\\\"; fi; kill -KILL $PPID\"," )).unwrap();
        let mut args = vec![
            "host",
            "prepare",
            "--repo",
            repo.to_str().unwrap(),
            "--branch",
            "incomplete",
            "--request-id",
            "interrupted",
        ];
        if let Some(base) = base {
            args.extend(["--base", base]);
        }
        let out = rig.run(&args);
        assert!(!out.status.success());
        let tree = repo.join(".worktrees/incomplete");
        assert!(
            tree.is_dir(),
            "preparation intent must exist before worktree creation: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            rig.dir.join("intent-at-setup.json").exists(),
            "preparation intent must precede setup"
        );
        let receipt = rig.read("intent-at-setup.json");
        assert!(receipt["prepared"].is_null(), "{receipt}");
        let pending = rig.read("state/preparations.json");
        fs::write(tree.join("keep-me"), "unfinished setup output").unwrap();
        fs::write(repo.join("dogsled.toml"), config).unwrap();
        let (code, error) = rig.json(&args);
        assert_eq!(code, 2, "{error}");
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("stopped during preparation"),
            "{error}"
        );
        assert_eq!(
            fs::read_to_string(tree.join("keep-me")).unwrap(),
            "unfinished setup output"
        );
        assert_eq!(rig.read("state/preparations.json"), pending);
        assert!(!rig.dir.join("setup.log").exists());
        assert!(!rig.log("herdr.log").contains("worktree open"));
    }
}

#[test]
fn lost_successful_reservation_reply_reuses_one_hold_on_the_selected_host() {
    let rig = Rig::new("reserve-response-lost");
    rig.peer();
    rig.snapshot("state/host.json", now(), 0.0, 8);
    fs::create_dir_all(rig.dir.join("peer-state")).unwrap();
    rig.snapshot("peer-state/host.json", now(), 0.0, 16);
    let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
    let handler = format!(
        r#"  *'host reserve'*)
    for remote do :; done
    remote=${{remote#* host }}
    DOGSLED_STATE="$RIG/peer-state" sh -c '{} host '"$remote" > "$RIG/reserve-response"
    status=$?
    test "$status" = 0 || exit "$status"
    if ! test -f "$RIG/reply-lost"; then touch "$RIG/reply-lost"; echo lost >&2; exit 255; fi
    cat "$RIG/reserve-response" ;;"#,
        env!("CARGO_BIN_EXE_dogsled")
    );
    rig.script(
        "ssh",
        &script.replace("  *'host reserve'*) cat \"$RIG/reserve.json\" ;;", &handler),
    );
    let (code, queued) = remote_agent(&rig, None);
    assert_eq!(code, 1, "{queued}");
    assert_eq!(rig.read("state/queue.json")[0]["retry"]["attempts"], 1);
    let before = rig.read("peer-state/placements.json");
    assert_eq!(before.as_array().unwrap().len(), 1);
    // Replay must not be new admission, even when a fresh status has no headroom.
    rig.snapshot("peer-state/host.json", now(), 100.0, 16);
    let ticket = advance(&rig);
    assert_eq!(ticket["state"], "admitted", "{ticket}");
    assert_eq!(rig.read("peer-state/placements.json"), before);
    assert_eq!(rig.log("ssh.log").matches("host reserve").count(), 2);
    assert_eq!(rig.log("ssh.log").matches("host launch").count(), 1);
}

#[test]
fn completed_preparation_receipt_replays_only_the_same_request() {
    let (rig, repo) = place::place_rig("preparation-receipt");
    let args = [
        "host",
        "prepare",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "receipt-owned",
        "--base",
        "main",
        "--request-id",
        "same-request",
    ];
    let (code, prepared) = rig.json(&args);
    assert_eq!(code, 0, "{prepared}");
    fs::write(rig.dir.join("already-open"), "").unwrap();
    let (code, replay) = rig.json(&args);
    assert_eq!(code, 0, "{replay}");
    assert_eq!(replay, prepared);
    assert_eq!(rig.log("setup.log").lines().count(), 1);
    assert_eq!(rig.log("herdr.log").matches("worktree open").count(), 1);
    let mut changed = args;
    changed[7] = "HEAD~1";
    let (code, error) = rig.json(&changed);
    assert_eq!(code, 2, "{error}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("preparation request changed")
    );
    changed = args;
    changed[9] = "another-request";
    let (code, error) = rig.json(&changed);
    assert_eq!(code, 2, "{error}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("exists; omit --base")
    );
}

#[test]
fn lost_successful_preparation_reply_reuses_the_completed_worktree_and_pane() {
    let (rig, repo) = place::place_rig("prepare-response-lost");
    rig.peer();
    let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
    let handler = format!(
        r#"  *'host prepare'*)
    for remote do :; done
    remote=${{remote#* host }}
    DOGSLED_STATE="$RIG/peer-state" sh -c '{} host '"$remote" > "$RIG/prepare-response"
    status=$?
    test "$status" = 0 || exit "$status"
    if ! test -f "$RIG/reply-lost"; then touch "$RIG/reply-lost"; touch "$RIG/already-open"; echo lost >&2; exit 255; fi
    cat "$RIG/prepare-response" ;;"#,
        env!("CARGO_BIN_EXE_dogsled")
    );
    // The peer runs real preparation in its own ledger; SSH drops its first successful reply.
    rig.script("ssh", &script.replace("  *'host prepare'*) printf '%s\\n' '{\"pane_id\":\"w9:p1\",\"terminal_id\":\"term_remote\",\"path\":\"/remote/tree\"}' ;;", &handler));
    let (code, queued) = remote_agent(&rig, Some(&repo));
    assert_eq!(code, 1, "{queued}");
    assert_eq!(rig.read("state/queue.json")[0]["retry"]["attempts"], 1);
    let ticket = advance(&rig);
    assert_eq!(ticket["state"], "admitted", "{ticket}");
    assert_eq!(rig.log("setup.log").lines().count(), 1);
    assert_eq!(rig.log("herdr.log").matches("worktree open").count(), 1);
    assert_eq!(rig.log("ssh.log").matches("host reserve").count(), 2);
    assert_eq!(rig.log("ssh.log").matches("host prepare").count(), 2);
    let id = ticket["request"]["id"].as_str().unwrap();
    let attempt = rig.read(&format!("state/admissions/{id}.json"));
    assert_eq!(attempt["launch"]["terminal"], "term_tree");
    assert_eq!(attempt["launch"]["pane"], "w8:p1");
}

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

pub(super) const NPM_FAILURE: &str =
    "npm error ETARGET No matching version found for @oisin-ee/pi-dogsled@0.19.0";

pub(super) fn npm_start_fails(rig: &Rig) {
    fs::write(rig.dir.join("startup-output"), NPM_FAILURE).unwrap();
    rig.script(
        "herdr",
        &fs::read_to_string(rig.dir.join("bin/herdr"))
            .unwrap()
            .replace(
                "  'agent start'*|",
                "  'agent start'*) echo '{\"error\":{\"code\":\"timeout\",\"message\":\"timed out waiting for agent startup\"}}' >&2; exit 1 ;;\n  ",
            )
            .replace(
                "case \"$*\" in",
                "case \"$*\" in\n  'pane read '*) cat \"$RIG/startup-output\" ;;",
            ),
    );
}

#[test]
fn failed_start_records_the_child_output_without_changing_unknown_outcome() {
    let rig = Rig::new("startup-output");
    npm_start_fails(&rig);
    let request = host_request(&rig);
    let (code, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 2, "{error}");
    let message = error["error"].as_str().unwrap();
    assert!(message.contains(NPM_FAILURE), "{message}");
    assert!(
        message.contains("timed out waiting for agent startup"),
        "{message}"
    );
    assert_eq!(error["deadline"], false);
    let progress = rig.read("state/launches/recover-one.progress.json");
    assert_eq!(progress["error"], error["error"]);
    assert_eq!(progress["attempted"], true);
    assert_eq!(progress["confirmed"], false);
    assert!(!rig.dir.join("state/launches/recover-one.json").exists());
    for _ in 0..2 {
        let (code, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
        assert_eq!(code, 2, "{error}");
        let message = error["error"].as_str().unwrap();
        assert!(message.contains("startup outcome is unknown"), "{message}");
        assert_eq!(message.matches(NPM_FAILURE).count(), 1, "{message}");
    }
    let log = rig.log("herdr.log");
    assert_eq!(log.matches("agent start worker ").count(), 1, "{log}");
    assert!(
        log.contains("pane read w2:p1 --source recent-unwrapped --lines 120 --format text"),
        "{log}"
    );
    assert!(!log.contains("pane close"), "{log}");
}

#[test]
fn cli_spawn_error_and_progress_include_the_child_startup_failure() {
    let rig = Rig::new("cli-startup-output");
    rig.snapshot("state/host.json", now(), 0.0, 8);
    npm_start_fails(&rig);
    let (code, queued) = rig.json(&["agent", "--name", "worker", "--message", "go"]);
    assert_eq!(code, 1, "{queued}");
    assert!(
        queued["retrying"].as_str().unwrap().contains(NPM_FAILURE),
        "{queued}"
    );
    let ticket = rig.read("state/queue.json")[0].clone();
    let id = ticket["request"]["id"].as_str().unwrap();
    let progress = rig.read(&format!("state/launches/{id}.progress.json"));
    assert!(
        progress["error"].as_str().unwrap().contains(NPM_FAILURE),
        "{progress}"
    );
    assert!(
        ticket["error"].as_str().unwrap().contains(NPM_FAILURE),
        "{ticket}"
    );
}

#[test]
fn interrupted_start_can_recover_diagnostics_on_confirmation() {
    let rig = Rig::new("startup-late-output");
    let request = host_request(&rig);
    failed_host_start(&rig, &request);
    npm_start_fails(&rig);
    let (code, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 2, "{error}");
    let message = error["error"].as_str().unwrap();
    assert!(message.contains("startup outcome is unknown"), "{message}");
    assert!(message.contains("startup response lost"), "{message}");
    assert!(message.contains(NPM_FAILURE), "{message}");
    let progress = rig.read("state/launches/recover-one.progress.json");
    assert!(
        progress["error"].as_str().unwrap().contains(NPM_FAILURE),
        "{progress}"
    );
    assert_eq!(
        rig.log("herdr.log").matches("agent start worker ").count(),
        1
    );
}

#[test]
fn startup_diagnostics_do_not_turn_a_native_refusal_into_an_unknown_launch() {
    let rig = Rig::new("refused-with-output");
    refusing_start(&rig);
    rig.script(
        "herdr",
        &fs::read_to_string(rig.dir.join("bin/herdr"))
            .unwrap()
            .replace(
                "case \"$*\" in",
                "case \"$*\" in\n  'pane read '*) printf 'startup detail\\n' ;;",
            ),
    );
    let request = host_request(&rig);
    let (code, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 2, "{error}");
    let message = error["error"].as_str().unwrap();
    assert!(message.contains("agent_name_taken"), "{message}");
    assert!(message.contains("startup detail"), "{message}");
    assert!(!message.contains("outcome is unknown"), "{message}");
    assert!(rig.log("herdr.log").contains("pane close w2:p1"));
    assert!(
        rig.read("state/launches/recover-one.json")["failed"]
            .as_str()
            .unwrap()
            .contains("startup detail")
    );
    assert_eq!(
        rig.json(&["host", "launch", "--request", &request.to_string()])
            .0,
        2
    );
    assert_eq!(
        rig.log("herdr.log").matches("agent start worker ").count(),
        1
    );
}

#[test]
fn empty_or_unavailable_pane_output_preserves_the_native_failure() {
    for empty in [true, false] {
        let rig = Rig::new(&format!("startup-missing-output-{empty}"));
        npm_start_fails(&rig);
        if empty {
            fs::write(rig.dir.join("startup-output"), " \n").unwrap();
        } else {
            fs::remove_file(rig.dir.join("startup-output")).unwrap();
        }
        let request = host_request(&rig);
        let (code, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
        assert_eq!(code, 2, "{error}");
        let message = error["error"].as_str().unwrap();
        assert!(
            message.contains("timed out waiting for agent startup"),
            "{message}"
        );
        assert!(!message.contains("startup output from"), "{message}");
        assert!(!message.contains("cat:"), "{message}");
        assert_eq!(
            rig.read("state/launches/recover-one.progress.json")["error"],
            error["error"]
        );
    }
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

/// Herdr refuses `agent start` before it types anything into the pane.
fn refusing_start(rig: &Rig) {
    rig.script(
        "herdr",
        &fs::read_to_string(rig.dir.join("bin/herdr"))
            .unwrap()
            .replace(
                "  'agent start'*|",
                "  'agent start'*) echo '{\"error\":{\"code\":\"agent_name_taken\",\"message\":\"agent name worker is already used\"},\"id\":\"cli:agent:start\"}' >&2; exit 1 ;;\n  ",
            ),
    );
}

#[test]
fn a_refused_start_fails_at_once_closes_its_pane_and_releases_its_checkout() {
    let rig = Rig::new("refused-start");
    let request = host_request(&rig);
    refusing_start(&rig);
    let (code, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 2, "{error}");
    let message = error["error"].as_str().unwrap();
    assert!(message.contains("agent_name_taken"), "{message}");
    assert!(!message.contains("outcome is unknown"), "{message}");
    let log = rig.log("herdr.log");
    assert!(log.contains("pane close w2:p1"), "{log}");
    // The refusal is the receipt: a resubmission neither allocates nor starts again.
    let (code, again) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 2, "{again}");
    assert!(
        again["error"]
            .as_str()
            .unwrap()
            .contains("agent_name_taken")
    );
    let log = rig.log("herdr.log");
    assert_eq!(log.matches("agent start worker ").count(), 1, "{log}");
    assert_eq!(log.matches("tab create").count(), 1, "{log}");
    let receipt = rig.read("state/launches/recover-one.json");
    assert!(
        receipt["failed"]
            .as_str()
            .unwrap()
            .contains("agent_name_taken")
    );
}

#[test]
fn shell_initialization_is_owned_by_native_agent_start() {
    let rig = Rig::new("native-shell-startup");
    rig.script("herdr", &fs::read_to_string(rig.dir.join("bin/herdr")).unwrap().replace(
        "[{\"pid\":10,\"name\":\"zsh\",\"argv\":[\"zsh\"]}]",
        "[{\"pid\":10,\"name\":\"zsh\",\"argv\":[\"zsh\"]},{\"pid\":11,\"name\":\"bash\",\"cmdline\":\"brew shellenv\"}]",
    ));
    let request = host_request(&rig);
    let (code, result) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 0, "{result}");
    let log = rig.log("herdr.log");
    assert_eq!(log.matches("agent start worker ").count(), 1, "{log}");
    assert!(!log.contains("pane process-info"), "{log}");
    assert!(!log.contains("pane close"), "{log}");
}

#[test]
fn refused_start_preserves_occupied_uninspectable_and_replaced_panes() {
    for case in [
        "busy",
        "foreign",
        "uninspectable",
        "empty",
        "replaced",
        "focused",
    ] {
        let rig = Rig::new(&format!("refused-preserve-{case}"));
        refusing_start(&rig);
        let mutation = match case {
            "foreign" => {
                "printf '%s\\n' '{\"result\":{\"agents\":[{\"pane_id\":\"w2:p1\",\"terminal_id\":\"term_child\",\"name\":\"foreign\",\"agent\":\"pi\"}]}}' > \"$RIG/agents.json\"; "
            }
            "replaced" => {
                "printf '%s\\n' '{\"result\":{\"panes\":[{\"pane_id\":\"w2:p1\",\"terminal_id\":\"replacement\"}]}}' > \"$RIG/panes.json\"; "
            }
            "focused" => {
                "printf '%s\\n' '{\"result\":{\"panes\":[{\"pane_id\":\"w2:p1\",\"terminal_id\":\"term_child\",\"focused\":true}]}}' > \"$RIG/panes.json\"; "
            }
            _ => "",
        };
        let script = fs::read_to_string(rig.dir.join("bin/herdr"))
            .unwrap()
            .replace(
                "'agent start'*) echo",
                &format!("'agent start'*) {mutation}echo"),
            );
        let script = match case {
            "busy" => script.replace(
                "[{\"pid\":10,\"name\":\"zsh\",\"argv\":[\"zsh\"]}]",
                "[{\"pid\":22,\"name\":\"vim\",\"argv\":[\"vim\"]}]",
            ),
            "uninspectable" => script.replace(
                "'pane process-info'*) printf",
                "'pane process-info'*) exit 2 ;;\n  'unused'*) printf",
            ),
            "empty" => script.replace("[{\"pid\":10,\"name\":\"zsh\",\"argv\":[\"zsh\"]}]", "[]"),
            _ => script,
        };
        rig.script("herdr", &script);
        let request = host_request(&rig);
        let (code, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
        assert_eq!(code, 2, "{case}: {error}");
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("agent_name_taken"),
            "{case}: {error}"
        );
        let receipt = rig.read("state/launches/recover-one.json");
        assert!(
            receipt["failed"]
                .as_str()
                .unwrap()
                .contains("agent_name_taken")
        );
        assert_eq!(
            rig.json(&["host", "launch", "--request", &request.to_string()])
                .0,
            2
        );
        let log = rig.log("herdr.log");
        assert_eq!(
            log.matches("agent start worker ").count(),
            1,
            "{case}: {log}"
        );
        assert!(!log.contains("pane close"), "{case}: {log}");
    }
}

#[test]
fn refused_start_closes_its_idle_prepared_pane_and_records_the_refusal() {
    let rig = Rig::new("refused-prepared");
    refusing_start(&rig);
    let mut request = host_request(&rig);
    request["pane"] = json!("w2:p1");
    request["terminal"] = json!("term_child");
    let (code, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 2, "{error}");
    assert!(
        rig.read("state/launches/recover-one.json")["failed"]
            .as_str()
            .unwrap()
            .contains("agent_name_taken")
    );
    let log = rig.log("herdr.log");
    assert!(log.contains("pane close w2:p1"), "{log}");
    assert!(!log.contains("tab create"), "{log}");
}

#[test]
fn a_refused_start_is_not_retried_by_the_service() {
    let rig = Rig::new("refused-retry");
    rig.snapshot("state/host.json", now(), 0.0, 8);
    refusing_start(&rig);
    let (code, error) = rig.json(&["agent", "--name", "w", "--message", "go"]);
    assert_eq!(code, 2, "{error}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("agent_name_taken")
    );
    // No ticket waits to retry it.
    assert!(!rig.dir.join("state/queue.json").exists());
    assert_eq!(rig.log("herdr.log").matches("agent start w ").count(), 1);
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
                "[{\"pid\":10,\"name\":\"zsh\",\"argv\":[\"zsh\"]}]",
                "[{\"pid\":11,\"name\":\"pi\",\"argv\":[\"pi\"]}]",
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

/// The host launch request for a job, as the peer receives it.
fn job_launch_request(rig: &Rig, id: &str, cmd: &[&str]) -> Value {
    json!({
        "id": id,
        "action": {"type":"job","cmd":cmd,"timeout_ms":null,"idle_timeout_ms":null},
        "host": "local",
        "cwd": rig.dir.join("home"),
        "pane": Value::Null,
        "workspace": "w1",
        "workspace_name": "caller",
        "owner": "w1:p1",
    })
}

/// A host replay whose receipt is missing must not allocate a second pane and must not
/// dispatch the command again when the job already holds a claim.
#[test]
fn a_host_replay_without_a_receipt_does_not_dispatch_an_existing_claim_again() {
    let rig = Rig::new("replay-existing-claim");
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    let job = json!({"id":"replayed","cmd":["true"],"cwd":rig.dir.join("home"),"host":"local","owner":"w1:p1",
        "pane":"w2:p1","state":"pending","exit":null,"signal":null,"reason":null,"started_ms":null,"ended_ms":null,
        "timeout_ms":null,"idle_timeout_ms":null});
    rig.put("state/jobs/replayed.json", &job);
    let request = job_launch_request(&rig, "replayed", &["true"]);
    let (code, result) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 0, "{result}");
    assert_eq!(result["pane"], "w2:p1", "{result}");
    let log = rig.log("herdr.log");
    // No pane was allocated, and the command was never dispatched again.
    assert!(!log.contains("tab create"), "{log}");
    assert!(!log.contains("pane run"), "{log}");
    // The claim keeps its own pane and its own state.
    assert_eq!(rig.read("state/jobs/replayed.json"), job);
}

/// A job whose dispatch outcome was never proved stays uncertain. No replay allocates a
/// pane, dispatches the command, or invents an end.
#[test]
fn a_host_replay_of_an_uncertain_claim_refuses_and_keeps_the_uncertainty() {
    let rig = Rig::new("replay-uncertain-claim");
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    let job = json!({"id":"unproved","cmd":["true"],"cwd":rig.dir.join("home"),"host":"local","owner":"w1:p1",
        "pane":"w2:p1","state":"uncertain","exit":null,"signal":null,"reason":"dispatch outcome unknown",
        "started_ms":null,"ended_ms":null,"timeout_ms":null,"idle_timeout_ms":null});
    rig.put("state/jobs/unproved.json", &job);
    let request = job_launch_request(&rig, "unproved", &["true"]);
    let (code, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 2, "{error}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("unresolved claim"),
        "{error}"
    );
    let log = rig.log("herdr.log");
    assert!(!log.contains("tab create"), "{log}");
    assert!(!log.contains("pane run"), "{log}");
    // The uncertainty is kept: nothing invents an end or a retry.
    assert_eq!(rig.read("state/jobs/unproved.json"), job);
}

/// A failed `pane run` leaves the job uncertain, and a wrapper that already started or ended
/// keeps its own record. Neither order overwrites the other.
#[test]
fn a_failed_dispatch_keeps_a_running_or_terminal_wrapper_record() {
    for (name, state, started, ended) in [
        ("running", "working", json!(123), Value::Null),
        ("terminal", "idle", json!(123), json!(124)),
    ] {
        let rig = Rig::new(&format!("dispatch-{name}"));
        // The wrapper publishes its own record while the dispatch fails. The order is forced
        // by the fake Herdr, so the test needs no sleep.
        let wrapper = json!({"id":"dispatched","cmd":["true"],"cwd":rig.dir.join("home"),"host":"local",
            "owner":"w1:p1","pane":"w2:p1","state":state,"exit":if ended.is_null() {Value::Null} else {json!(0)},
            "signal":null,"reason":null,"started_ms":started,"ended_ms":ended,
            "timeout_ms":null,"idle_timeout_ms":null});
        rig.put("wrapper.json", &wrapper);
        rig.script(
            "herdr",
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/herdr.log"
case "$*" in
  'workspace list') printf '%s\n' '{"result":{"workspaces":[{"workspace_id":"w1","label":"caller"}]}}' ;;
  'worktree list --cwd'*) cat "$RIG/worktrees.json" ;;
  'tab create'*|'workspace create'*) printf '%s\n' '{"result":{"root_pane":{"pane_id":"w2:p1","terminal_id":"term_child"}}}' ;;
  'pane run'*)
    mkdir -p "$DOGSLED_STATE/jobs"
    cp "$RIG/wrapper.json" "$DOGSLED_STATE/jobs/dispatched.json"
    echo 'pane refused' >&2; exit 1 ;;
  *) echo unexpected >&2; exit 2 ;;
esac
"#,
        );
        let request = job_launch_request(&rig, "dispatched", &["true"]);
        let (code, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
        assert_eq!(code, 2, "{error}");
        // The wrapper's own record survives the transport failure.
        let after = rig.read("state/jobs/dispatched.json");
        assert_eq!(after["started_ms"], started, "{after}");
        assert_eq!(after["ended_ms"], ended, "{after}");
        assert_eq!(after["state"], state, "{after}");
    }
}

/// A dispatch that failed with a pending claim and no wrapper record stays uncertain. It does
/// not end, and it does not become executable again.
#[test]
fn a_failed_dispatch_without_a_wrapper_record_stays_uncertain() {
    let rig = Rig::new("dispatch-uncertain");
    rig.script(
        "herdr",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/herdr.log"
case "$*" in
  'workspace list') printf '%s\n' '{"result":{"workspaces":[{"workspace_id":"w1","label":"caller"}]}}' ;;
  'worktree list --cwd'*) cat "$RIG/worktrees.json" ;;
  'tab create'*|'workspace create'*) printf '%s\n' '{"result":{"root_pane":{"pane_id":"w2:p1","terminal_id":"term_child"}}}' ;;
  'pane run'*) echo 'pane refused' >&2; exit 1 ;;
  *) echo unexpected >&2; exit 2 ;;
esac
"#,
    );
    let request = job_launch_request(&rig, "unproved", &["true"]);
    let (code, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 2, "{error}");
    let job = rig.read("state/jobs/unproved.json");
    assert_eq!(job["state"], "uncertain", "{job}");
    // Nothing invents an end, and nothing is executable again.
    assert!(job["ended_ms"].is_null(), "{job}");
    assert!(job["exit"].is_null(), "{job}");
    assert!(
        job["reason"].as_str().unwrap().contains("pane refused"),
        "{job}"
    );
    // A second launch refuses the uncertain claim.
    let (code, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 2, "{error}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("unresolved claim"),
        "{error}"
    );
    assert_eq!(rig.log("herdr.log").matches("pane run").count(), 1);
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

/// Herdr reports the shell busy for the first `busy` starts, then accepts the start. A negative
/// count means it stays busy.
fn busy_start(rig: &Rig, busy: i32) {
    fs::write(rig.dir.join("busy-left"), busy.to_string()).unwrap();
    rig.script(
        "herdr",
        &fs::read_to_string(rig.dir.join("bin/herdr"))
            .unwrap()
            .replace(
                "  'agent start'*|",
                "  'agent start'*) left=$(cat \"$RIG/busy-left\"); if test \"$left\" != 0; then test \"$left\" -gt 0 && echo $((left - 1)) > \"$RIG/busy-left\"; echo '{\"error\":{\"code\":\"agent_pane_busy\",\"message\":\"agent target pane w2:p1 is not an available shell\"},\"id\":\"cli:agent:start\"}' >&2; exit 1; fi; printf '{}\\n' ;;\n  ",
            ),
    );
}

#[test]
fn a_busy_start_is_repeated_on_the_same_pane_until_the_shell_is_ready() {
    let rig = Rig::new("busy-then-ready");
    busy_start(&rig, 2);
    let request = host_request(&rig);
    let (code, result) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 0, "{result}");
    assert_eq!(result["pane"], "w2:p1");
    let log = rig.log("herdr.log");
    assert_eq!(log.matches("agent start worker ").count(), 3, "{log}");
    assert_eq!(log.matches("tab create").count(), 1, "{log}");
    assert!(!log.contains("pane close"), "{log}");
}

#[test]
fn a_start_that_stays_busy_times_out_without_closing_its_pane_and_can_be_retried() {
    let rig = Rig::new("busy-timeout");
    fs::write(rig.dir.join("busy-window-ms"), "600").unwrap();
    busy_start(&rig, -1);
    let request = host_request(&rig);
    let (code, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_ne!(code, 0, "{error}");
    let message = error["error"].as_str().unwrap();
    assert!(message.contains("w2:p1"), "{message}");
    assert!(
        message.contains("not an available shell after"),
        "{message}"
    );
    assert!(message.contains("stays open"), "{message}");
    assert!(!message.contains("outcome is unknown"), "{message}");
    let log = rig.log("herdr.log");
    assert!(log.matches("agent start worker ").count() >= 2, "{log}");
    assert!(!log.contains("pane close"), "{log}");
    assert!(!log.contains("workspace close"), "{log}");
    // No final receipt: the launch may run again, and it has not been attempted.
    assert!(!rig.dir.join("state/launches/recover-one.json").exists());
    assert_eq!(
        rig.read("state/launches/recover-one.progress.json")["attempted"],
        false
    );
    // The shell finishes starting. Cleanup must not retire its retryable launch before replay.
    busy_start(&rig, 0);
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[{
            "pane_id":"w2:p1","terminal_id":"term_child",
            "cwd":rig.dir.join("home"),"workspace_id":"w1"
        }]}}),
    );
    rig.script(
        "herdr",
        &fs::read_to_string(rig.dir.join("bin/herdr"))
            .unwrap()
            .replace(
                "\"shell_pid\":10,\"foreground_processes\"",
                "\"shell_pid\":10,\"foreground_process_group_id\":10,\"foreground_processes\"",
            ),
    );
    let cleanup = rig.run(&["prune", "--yes", "panes", "workspaces"]);
    assert!(
        cleanup.status.success(),
        "{}",
        String::from_utf8_lossy(&cleanup.stderr)
    );
    let log = rig.log("herdr.log");
    assert!(!log.contains("pane close"), "{log}");
    assert!(!log.contains("workspace close"), "{log}");
    assert_eq!(
        rig.read("state/launches/recover-one.progress.json")["attempted"],
        false
    );
    let (code, result) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 0, "{result}");
    let log = rig.log("herdr.log");
    assert_eq!(log.matches("tab create").count(), 1, "{log}");
    assert!(!log.contains("pane close"), "{log}");
}
