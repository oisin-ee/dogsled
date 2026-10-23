use super::*;

fn fail_peer(rig: &Rig, step: &str, response: &Value) {
    rig.put("failure.json", response);
    let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
    rig.script(
        "ssh",
        &script.replace(
            &format!("  *'host {step}'*)"),
            &format!("  *'host {step}'*) cat \"$RIG/failure.json\"; exit 2;"),
        ),
    );
}

fn envelope(cause: &str) -> Value {
    json!({"error":"PRIVATE_SENTINEL", "diagnostic": {
        "version":1,"operation":"prepare","cause":cause,"context":{}
    }})
}

#[test]
fn cli_preserves_known_causes_and_never_peer_text() {
    for cause in [
        "base_unavailable",
        "branch_exists",
        "workspace_occupied",
        "setup_failed",
    ] {
        let rig = Rig::new(&format!("diagnostic-cli-{cause}"));
        let repo = rig.dir.join("home/repo");
        repository(&repo);
        rig.peer();
        fail_peer(&rig, "prepare", &envelope(cause));
        let (code, error) = rig.json(&[
            "agent",
            "--host",
            "netcup",
            "--cwd",
            repo.to_str().unwrap(),
            "--branch",
            "feat/new",
            "--base",
            "missing",
            "--name",
            "worker",
            "--message",
            "TASK_SECRET",
        ]);
        assert_eq!(code, 2, "{error}");
        assert_eq!(error["diagnostic"]["cause"], cause, "{error}");
        assert_eq!(error["diagnostic"]["operation"], "prepare");
        assert_eq!(error["diagnostic"]["status"], 2);
        assert!(error["error"].as_str().unwrap().contains(cause), "{error}");
        assert!(!error.to_string().contains("PRIVATE_SENTINEL"));
        assert!(!error.to_string().contains("TASK_SECRET"));
        assert!(!rig.dir.join("state/queue.json").exists());
        let admission = fs::read_dir(rig.dir.join("state/admissions"))
            .unwrap()
            .flatten()
            .find(|f| f.path().extension().is_some_and(|e| e == "json"))
            .unwrap();
        let attempt: Value = serde_json::from_slice(&fs::read(admission.path()).unwrap()).unwrap();
        assert_eq!(attempt["diagnostic"], error["diagnostic"]);
        assert_eq!(attempt["host"]["label"], "netcup");
        assert!(!rig.log("ssh.log").contains("host launch"));
        let text = rig.run(&[
            "place",
            "--host",
            "netcup",
            "--repo",
            repo.to_str().unwrap(),
            "--branch",
            "feat/new",
            "--base",
            "missing",
        ]);
        let text = String::from_utf8(text.stderr).unwrap();
        assert!(text.contains(cause) && text.contains("netcup"), "{text}");
        assert!(!text.contains("PRIVATE_SENTINEL"));
    }
}

/// Run the real peer command against private test state, not a live host.
fn real_peer_prepare(rig: &Rig) {
    let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
    let handler = format!(
        r#"  *'host prepare'*)
    for remote do :; done
    remote=${{remote#* host }}
    DOGSLED_STATE="$RIG/peer-state" sh -c '{} host '"$remote" ;;"#,
        env!("CARGO_BIN_EXE_dogsled")
    );
    rig.script("ssh", &script.replace(
        "  *'host prepare'*) printf '%s\\n' '{\"pane_id\":\"w9:p1\",\"terminal_id\":\"term_remote\",\"path\":\"/remote/tree\"}' ;;", &handler));
}

#[test]
fn real_preflight_is_read_only_and_existing_branch_refusal_still_wins() {
    let (rig, repo) = place::place_rig("diagnostic-preflight");
    let git = |args: &[&str]| {
        let out = isolated("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    };
    let before = git(&["show-ref"]);
    let trees = git(&["worktree", "list", "--porcelain"]);
    for (branch, cause) in [("new-work", "base_unavailable"), ("main", "branch_exists")] {
        let (code, error) = rig.json(&[
            "host",
            "prepare",
            "--repo",
            repo.to_str().unwrap(),
            "--branch",
            branch,
            "--base",
            "not-present",
            "--request-id",
            branch,
        ]);
        assert_eq!(code, 2, "{error}");
        assert_eq!(error["diagnostic"]["cause"], cause);
        assert_eq!(error["diagnostic"]["request_id"], branch);
        assert_eq!(error["diagnostic"]["context"]["branch"], branch);
    }
    assert_eq!(git(&["show-ref"]), before);
    assert_eq!(git(&["worktree", "list", "--porcelain"]), trees);
    assert!(!repo.join(".worktrees").exists());
    assert!(!rig.dir.join("state/preparations.json").exists());
    assert_eq!(rig.log("setup.log"), "");
    assert!(!rig.log("herdr.log").contains("worktree open"));
}

#[test]
fn real_remote_preparation_causes_are_safe_through_transport() {
    for (case, base, cause) in [
        ("missing", "not-present", "base_unavailable"),
        ("exists", "main", "branch_exists"),
        ("occupied", "main", "workspace_occupied"),
        ("setup", "main", "setup_failed"),
    ] {
        let (rig, repo) = place::place_rig(&format!("diagnostic-real-{case}"));
        rig.peer();
        real_peer_prepare(&rig);
        let branch = if case == "exists" { "main" } else { "worker" };
        if case == "occupied" {
            fs::create_dir_all(repo.join(".worktrees/worker")).unwrap();
        }
        if case == "setup" {
            fs::write(
                repo.join("dogsled.toml"),
                "[worktree]\nsetup = [\"echo SCRIPT_SECRET >&2; exit 1\"]\n",
            )
            .unwrap();
        }
        let (code, error) = rig.json(&[
            "agent",
            "--host",
            "netcup",
            "--cwd",
            repo.to_str().unwrap(),
            "--branch",
            branch,
            "--base",
            base,
            "--name",
            "worker",
            "--message",
            "TASK_SECRET",
        ]);
        assert_eq!(code, 2, "{case}: {error}");
        assert_eq!(error["diagnostic"]["cause"], cause, "{case}: {error}");
        assert_eq!(error["diagnostic"]["context"]["branch"], branch);
        assert_eq!(error["diagnostic"]["context"]["base"], base);
        assert!(!error.to_string().contains("SCRIPT_SECRET"));
        assert!(!error.to_string().contains("TASK_SECRET"));
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("untrusted detail withheld")
        );
        assert!(!rig.log("ssh.log").contains("host launch"));
    }
}

#[test]
fn malformed_unknown_and_secret_envelopes_never_import_peer_text() {
    for (index, diagnostic) in [
        json!("SECRET_SENTINEL"),
        json!({"version":99,"operation":"prepare","cause":"base_unavailable","context":{}}),
        json!({"version":1,"operation":"prepare","cause":"SECRET_SENTINEL","context":{}}),
        json!({"version":1,"operation":"prepare","cause":"base_unavailable","context":{"origin":"https://user:SECRET_SENTINEL@host/repo"}}),
        json!({"version":1,"operation":"prepare","cause":"base_unavailable","context":{"base":"\u{1b}[31mSECRET_SENTINEL"}}),
        json!({"version":1,"operation":"prepare","cause":"base_unavailable","context":{"base":"https://user:SECRET_SENTINEL@host/repo"}}),
        json!({"version":1,"operation":"prepare","cause":"base_unavailable","context":{"base":"SECRET_SENTINEL"},"request_id":"forged","status":999}),
    ].into_iter().enumerate() {
        let rig = Rig::new(&format!("diagnostic-invalid-{index}"));
        let repo = rig.dir.join("home/repo"); repository(&repo); rig.peer();
        fail_peer(&rig,"prepare",&json!({"error":"SECRET_SENTINEL","diagnostic":diagnostic}));
        let (code, error) = rig.json(&["agent","--host","netcup","--cwd",repo.to_str().unwrap(),
            "--branch","worker","--base","missing","--name","worker","--message","TASK_SECRET"]);
        assert_eq!(code,2,"{error}");
        assert_eq!(error["diagnostic"]["operation"],"prepare");
        assert_eq!(error["diagnostic"]["status"],2);
        assert_ne!(error["diagnostic"]["request_id"], "forged");
        assert!(error["diagnostic"]["request_id"].is_string());
        assert!(error["error"].as_str().unwrap().contains("untrusted detail withheld"));
        assert!(!error.to_string().contains("SECRET_SENTINEL"),"{error}");
        assert!(!error.to_string().contains("TASK_SECRET"),"{error}");
        assert!(!error.to_string().contains("\\u001b"),"{error}");
        assert!(!rig.dir.join("state/queue.json").exists());
    }
}

#[test]
fn parse_errors_do_not_format_launch_payloads_or_echo_remote_output() {
    let rig = Rig::new("diagnostic-parse");
    rig.peer();
    let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
    rig.script(
        "ssh",
        &script.replace(
            "  *'host launch'*)",
            "  *'host launch'*) echo 'PAYLOAD_SECRET'; exit 0;",
        ),
    );
    let (code, error) = rig.json(&[
        "agent",
        "--host",
        "netcup",
        "--name",
        "worker",
        "--message",
        "TASK_SECRET",
    ]);
    assert_eq!(code, 2, "{error}");
    assert_eq!(error["diagnostic"]["operation"], "launch");
    assert_eq!(error["diagnostic"]["status"], 0);
    assert!(error["diagnostic"]["request_id"].is_string());
    assert!(!error.to_string().contains("TASK_SECRET"));
    assert!(!error.to_string().contains("PAYLOAD_SECRET"));
    assert!(!rig.dir.join("state/queue.json").exists());
}

#[test]
fn remote_deadline_keeps_operation_and_correlation_without_launch_payload() {
    let rig = Rig::new("diagnostic-deadline");
    rig.peer();
    let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
    rig.script(
        "ssh",
        &script.replace("  *'host launch'*)", "  *'host launch'*) printf '%s\\n' '{\"deadline\":true,\"error\":\"TASK_SECRET\"}'; exit 2;"),
    );
    let (code, error) = rig.json(&[
        "agent",
        "--host",
        "netcup",
        "--name",
        "worker",
        "--message",
        "TASK_SECRET",
    ]);
    assert_eq!(code, 2, "{error}");
    assert_eq!(error["deadline"], true);
    assert_eq!(error["diagnostic"]["operation"], "launch", "{error}");
    assert_eq!(error["diagnostic"]["host"], "netcup");
    assert!(error["diagnostic"]["request_id"].is_string());
    assert_eq!(error["diagnostic"]["status"], 2);
    assert!(!error.to_string().contains("TASK_SECRET"));
    assert!(!rig.dir.join("state/queue.json").exists());
}

#[test]
fn remote_checkout_causes_and_legacy_clone_code_keep_the_diagnostic() {
    for cause in ["clone_failed", "checkout_identity_mismatch"] {
        let rig = Rig::new(&format!("diagnostic-remote-checkout-{cause}"));
        let repo = rig.dir.join("home/repo");
        repository(&repo);
        rig.peer();
        let mut response = envelope(cause);
        response["diagnostic"]["operation"] = json!("checkout");
        if cause == "clone_failed" {
            response["code"] = json!("CLONE_FAILED");
        }
        fail_peer(&rig, "checkout", &response);
        let (code, error) = rig.json(&[
            "agent",
            "--host",
            "netcup",
            "--cwd",
            repo.to_str().unwrap(),
            "--branch",
            "worker",
            "--name",
            "worker",
            "--message",
            "TASK_SECRET",
        ]);
        assert_eq!(code, 2, "{error}");
        assert_eq!(error["diagnostic"]["cause"], cause);
        assert_eq!(error["diagnostic"]["operation"], "checkout");
        assert_eq!(error["diagnostic"]["host"], "netcup");
        assert!(error["diagnostic"]["request_id"].is_string());
        assert!(!error.to_string().contains("PRIVATE_SENTINEL"));
        assert!(!rig.log("ssh.log").contains("host prepare"));
        assert!(!rig.dir.join("state/queue.json").exists());
    }
}

#[test]
fn mcp_place_preserves_structured_cause_and_safe_text() {
    let rig = Rig::new("diagnostic-mcp");
    let repo = rig.dir.join("home/repo");
    repository(&repo);
    rig.peer();
    fail_peer(&rig, "prepare", &envelope("branch_exists"));
    let mut client = mcp::Client::start(&rig);
    let (error, failed) = client.call(
        "place",
        &json!({"host":"netcup","cwd":repo,
        "branch":"worker","base":"main"}),
    );
    assert!(failed, "{error}");
    assert_eq!(error["diagnostic"]["cause"], "branch_exists");
    assert_eq!(error["diagnostic"]["context"]["branch"], "worker");
    assert!(error["error"].as_str().unwrap().contains("omit --base"));
    assert!(!error.to_string().contains("PRIVATE_SENTINEL"));
}

#[test]
fn unit_commit_error_and_observe_keep_diagnostic() {
    use super::unit_requests::*;
    let rig = rig("diagnostic-unit", false);
    let repo = rig.dir.join("home/repo");
    repository(&repo);
    rig.peer();
    fail_peer(&rig, "prepare", &envelope("base_unavailable"));
    let input = json!({"origin":exported(0)["contract"]["ref"],"placement":{
        "host":"netcup","branch":"worker","base":"missing"}});
    let out = rig
        .cmd(&[
            "unit",
            "prepare",
            "--input",
            &input.to_string(),
            "--tool-call-id",
            "diag-call",
            "--json",
        ])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let execution: Value = serde_json::from_slice(&out.stdout).unwrap();
    let (code, error) = operation(&rig, "commit", &commit_input(&execution, 0));
    assert_eq!(code, 2, "{error}");
    assert_eq!(error["error"]["diagnostic"]["cause"], "base_unavailable");
    let (code, observed) = operation(&rig, "observe", &execution_input(&execution));
    assert_eq!(code, 0, "{observed}");
    assert_eq!(observed["diagnostic"], error["error"]["diagnostic"]);
    assert_eq!(observed["phase"], "unresolved");
    assert!(!observed.to_string().contains("PRIVATE_SENTINEL"));
}

#[test]
fn queued_unit_receipt_retains_diagnostic_after_service_admission_fails() {
    use super::unit_requests::*;
    let rig = rig("diagnostic-unit-queued", false);
    let repo = rig.dir.join("home/repo");
    repository(&repo);
    rig.peer();
    rig.put("reserve.json", &json!({"reserved":false,"headroom":null}));
    let input = json!({"origin":exported(0)["contract"]["ref"],"placement":{
        "host":"netcup","branch":"worker","base":"missing"}});
    let out = rig
        .cmd(&[
            "unit",
            "prepare",
            "--input",
            &input.to_string(),
            "--tool-call-id",
            "diag-call",
            "--json",
        ])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(out.status.success());
    let execution: Value = serde_json::from_slice(&out.stdout).unwrap();
    let (code, receipt) = operation(&rig, "commit", &commit_input(&execution, 0));
    assert_eq!(code, 0, "{receipt}");
    assert_eq!(receipt["disposition"], "queued");
    fail_peer(&rig, "prepare", &envelope("base_unavailable"));
    rig.put("reserve.json", &json!({"reserved":true,"headroom":null}));
    let ticket = advance(&rig);
    assert_eq!(ticket["state"], "failed");
    let (code, observed) = operation(&rig, "observe", &execution_input(&execution));
    assert_eq!(code, 0, "{observed}");
    assert_eq!(observed["receipt"]["diagnostic"], ticket["diagnostic"]);
    assert_eq!(observed["diagnostic"], ticket["diagnostic"]);
    assert_eq!(observed["phase"], "not_started");
    assert!(!observed.to_string().contains("PRIVATE_SENTINEL"));
}

#[test]
fn checkout_identity_and_clone_failures_never_show_credential_origins() {
    for cause in ["checkout_identity_mismatch", "clone_failed"] {
        let rig = Rig::new(&format!("diagnostic-checkout-{cause}"));
        let path = rig.dir.join("home/repo");
        if cause == "checkout_identity_mismatch" {
            fs::create_dir_all(&path).unwrap();
        }
        rig.script("git", "#!/bin/sh\ncase \"$*\" in\n *'remote get-url origin') echo https://user:ACTUAL_SECRET@other/repo ;;\n *'rev-parse --show-toplevel') printf '%s\\n' \"$2\" ;;\n *) echo CLONE_SECRET >&2; exit 1 ;;\nesac\n");
        let (code, error) = rig.json(&[
            "host",
            "checkout",
            "--path",
            "repo",
            "--origin",
            "https://user:ORIGIN_SECRET@expected/repo",
            "--host-label",
            "peer",
        ]);
        assert_eq!(code, 2, "{error}");
        assert_eq!(error["diagnostic"]["cause"], cause);
        for secret in ["ACTUAL_SECRET", "ORIGIN_SECRET", "CLONE_SECRET"] {
            assert!(!error.to_string().contains(secret), "{error}");
        }
    }
}

fn advance(rig: &Rig) -> Value {
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    rig.read("state/queue.json")[0].clone()
}

#[test]
fn waiting_caller_receives_failed_ticket_diagnostic() {
    let rig = Rig::new("diagnostic-wait");
    let repo = rig.dir.join("home/repo");
    repository(&repo);
    rig.peer();
    rig.put("reserve.json", &json!({"reserved":false,"headroom":null}));
    fail_peer(&rig, "prepare", &envelope("base_unavailable"));
    let mut child = rig
        .cmd(&[
            "agent",
            "--wait",
            "--max",
            "30s",
            "--host",
            "netcup",
            "--cwd",
            repo.to_str().unwrap(),
            "--branch",
            "worker",
            "--base",
            "missing",
            "--name",
            "worker",
            "--message",
            "go",
            "--json",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !rig.dir.join("state/queue.json").exists() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("caller did not enqueue");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    rig.put("reserve.json", &json!({"reserved":true,"headroom":null}));
    assert_eq!(advance(&rig)["state"], "failed");
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    let error: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
    assert_eq!(error["diagnostic"]["cause"], "base_unavailable");
    assert!(!text.contains("PRIVATE_SENTINEL"));
}

#[test]
fn queued_failure_keeps_cause_in_ticket_admission_and_owner_inbox() {
    let rig = Rig::new("diagnostic-queue");
    let repo = rig.dir.join("home/repo");
    repository(&repo);
    rig.peer();
    rig.put("agents.json",&json!({"result":{"agents":[{"pane_id":"w1:p1","terminal_id":"term_owner","agent":"pi","agent_status":"idle"}]}}));
    assert_eq!(rig.json(&["inbox", "claim", "--harness", "pi"]).0, 0);
    rig.put("reserve.json", &json!({"reserved":false,"headroom":null}));
    let (code, queued) = rig.json(&[
        "agent",
        "--host",
        "netcup",
        "--cwd",
        repo.to_str().unwrap(),
        "--branch",
        "worker",
        "--base",
        "missing",
        "--name",
        "worker",
        "--message",
        "TASK_SECRET",
    ]);
    assert_eq!(code, 1, "{queued}");
    rig.put("reserve.json", &json!({"reserved":true,"headroom":null}));
    fail_peer(&rig, "prepare", &envelope("base_unavailable"));
    let ticket = advance(&rig);
    assert_eq!(ticket["state"], "failed", "{ticket}");
    assert_eq!(ticket["retry"]["attempts"], 0);
    assert_eq!(ticket["diagnostic"]["cause"], "base_unavailable");
    let id = ticket["request"]["id"].as_str().unwrap();
    let attempt = rig.read(&format!("state/admissions/{id}.json"));
    assert_eq!(attempt["diagnostic"], ticket["diagnostic"]);
    let inbox = rig.read("state/inbox/term_owner.json");
    let text = inbox[0]["text"].as_str().unwrap();
    assert!(
        text.contains("on netcup") && text.contains("base_unavailable"),
        "{text}"
    );
    assert!(!text.contains("unknown") && !text.contains("PRIVATE_SENTINEL"));
    assert!(!text.contains("TASK_SECRET"));
    let (code, observed) = rig.json(&["inbox", "list"]);
    assert_eq!(code, 0);
    assert!(observed.to_string().contains("base_unavailable"));
}
