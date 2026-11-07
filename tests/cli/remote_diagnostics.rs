use super::*;

fn launch_peer(rig: &Rig) {
    rig.peer();
    let mut snapshot = rig.read("remote.json");
    snapshot["job_validity"] = json!(2);
    rig.put("remote.json", &snapshot);
    rig.ingest_peers();
}

fn run_json(rig: &Rig, args: &[&str]) -> (i32, Value) {
    let mut args = args.to_vec();
    args.insert(0, "--json");
    rig.json(&args)
}

// The caller accepts only typed envelopes and binds context to its own launch request.
fn assert_remote_launch_cause(cause: &str) {
    let rig = Rig::new(&format!("diagnostic-launch-{cause}"));
    launch_peer(&rig);
    fail_peer(
        &rig,
        "launch",
        &json!({"error":"PRIVATE_SENTINEL", "diagnostic":{
        "version":1,"operation":"launch","cause":cause,
        "context":{"workspace":"PEER_SECRET","directory":"PEER_SECRET"},"host":"FORGED_HOST","request_id":"forged"}}),
    );
    let _observer = super::place::peer_observer(&rig);
    let args = [
        "run",
        "--host",
        "netcup",
        "--cwd",
        "/Users/oisin",
        "--",
        "true",
    ];
    let (code, error) = run_json(&rig, &args);
    assert_eq!(code, 2, "{error}");
    assert_eq!(error["diagnostic"]["cause"], cause, "{error}");
    assert_eq!(error["diagnostic"]["host"], "netcup");
    assert_eq!(error["diagnostic"]["context"]["directory"], "/Users/oisin");
    assert_ne!(error["diagnostic"]["request_id"], "forged");
    let text = error["error"].as_str().unwrap();
    assert!(text.contains(cause) && text.contains("netcup"), "{error}");
    assert!(!text.contains("ssh "), "{error}");
    let plain = rig.run(&args);
    assert_eq!(plain.status.code(), Some(2));
    let stderr = String::from_utf8(plain.stderr).unwrap();
    assert!(
        stderr.contains(cause) && stderr.contains("netcup"),
        "{stderr}"
    );
    for secret in ["PRIVATE_SENTINEL", "PEER_SECRET", "FORGED_HOST"] {
        assert!(!error.to_string().contains(secret), "{error}");
        assert!(!stderr.contains(secret), "{stderr}");
    }
}

#[test]
fn remote_launch_execution_directory_missing() {
    assert_remote_launch_cause("execution_directory_missing");
}

#[test]
fn remote_launch_checkout_unavailable() {
    assert_remote_launch_cause("checkout_unavailable");
}

#[test]
fn remote_launch_request_changed() {
    assert_remote_launch_cause("request_changed");
}

#[test]
fn remote_launch_protocol_unsupported() {
    assert_remote_launch_cause("protocol_unsupported");
}

#[test]
fn remote_launch_preparation_binding_missing() {
    assert_remote_launch_cause("preparation_binding_missing");
}

#[test]
fn remote_launch_preparation_binding_mismatch() {
    assert_remote_launch_cause("preparation_binding_mismatch");
}

#[test]
fn remote_launch_preparation_held() {
    assert_remote_launch_cause("preparation_held");
}

#[test]
fn remote_launch_preparation_interrupted() {
    assert_remote_launch_cause("preparation_interrupted");
}

#[test]
fn remote_launch_preparation_contradictory() {
    assert_remote_launch_cause("preparation_contradictory");
}

#[test]
fn remote_launch_launch_receipt_missing() {
    assert_remote_launch_cause("launch_receipt_missing");
}

#[test]
fn remote_launch_launch_receipt_mismatch() {
    assert_remote_launch_cause("launch_receipt_mismatch");
}

#[test]
fn remote_launch_job_claim_unresolved() {
    assert_remote_launch_cause("job_claim_unresolved");
}

#[test]
fn remote_launch_job_claim_exists() {
    assert_remote_launch_cause("job_claim_exists");
}

#[test]
fn remote_launch_pane_unavailable() {
    assert_remote_launch_cause("pane_unavailable");
}

#[test]
fn remote_launch_pane_identity_mismatch() {
    assert_remote_launch_cause("pane_identity_mismatch");
}

#[test]
fn remote_launch_pane_busy() {
    assert_remote_launch_cause("pane_busy");
}

#[test]
fn remote_launch_workspace_unavailable() {
    assert_remote_launch_cause("workspace_unavailable");
}

#[test]
fn remote_launch_branch_invalid() {
    assert_remote_launch_cause("branch_invalid");
}

#[test]
fn remote_launch_profile_invalid() {
    assert_remote_launch_cause("profile_invalid");
}

#[test]
fn remote_launch_command_too_long() {
    assert_remote_launch_cause("command_too_long");
}

#[test]
fn remote_launch_startup_uncertain() {
    assert_remote_launch_cause("startup_uncertain");
}

#[test]
fn remote_launch_launch_refused() {
    assert_remote_launch_cause("launch_refused");
}

#[test]
fn remote_launch_invalid_request() {
    assert_remote_launch_cause("invalid_request");
}

#[test]
fn remote_launch_validity_unprovable() {
    assert_remote_launch_cause("validity_unprovable");
}

#[test]
fn remote_launch_request_expired() {
    assert_remote_launch_cause("request_expired");
}

#[test]
fn remote_launch_request_from_future() {
    assert_remote_launch_cause("request_from_future");
}

#[test]
fn remote_launch_validity_clock_unavailable() {
    assert_remote_launch_cause("validity_clock_unavailable");
}

#[test]
fn remote_launch_io_failed() {
    assert_remote_launch_cause("io_failed");
}

#[test]
fn remote_launch_state_failed() {
    assert_remote_launch_cause("state_failed");
}

#[test]
fn remote_launch_invalid_payload() {
    assert_remote_launch_cause("invalid_payload");
}

#[test]
fn remote_launch_config_invalid() {
    assert_remote_launch_cause("config_invalid");
}

#[test]
fn remote_launch_command_failed() {
    assert_remote_launch_cause("command_failed");
}

#[test]
fn remote_launch_herdr_unavailable() {
    assert_remote_launch_cause("herdr_unavailable");
}

#[test]
fn real_remote_missing_directory_names_the_path_and_host() {
    let rig = Rig::new("diagnostic-real-missing-cwd");
    let missing = rig.dir.join("missing");
    let missing = missing.to_str().unwrap();
    assert!(!Path::new(missing).exists());
    let expected = format!("execution directory {missing} does not exist on netcup");
    launch_peer(&rig);
    real_peer_prepare(&rig);
    let _observer = super::place::peer_observer(&rig);
    let args = ["run", "--host", "netcup", "--cwd", missing, "--", "true"];
    let (code, error) = run_json(&rig, &args);
    assert_eq!(code, 2, "{error}");
    assert_eq!(error["diagnostic"]["cause"], "execution_directory_missing");
    let text = error["error"].as_str().unwrap();
    assert!(text.contains(&expected), "{error}");
    assert!(!text.contains("ssh "), "{error}");
    let plain = rig.run(&args);
    let stderr = String::from_utf8(plain.stderr).unwrap();
    assert_eq!(plain.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains(&expected), "{stderr}");
    assert!(!stderr.contains("ssh "), "{stderr}");
    assert!(!rig.log("herdr.log").contains("pane run"));
}

#[test]
fn real_remote_missing_directory_reaches_ticket_inbox_and_status() {
    let rig = Rig::new("diagnostic-missing-cwd-ticket");
    let missing = rig.dir.join("missing");
    let missing = missing.to_str().unwrap();
    assert!(!Path::new(missing).exists());
    let expected = format!("execution directory {missing} does not exist on netcup");
    launch_peer(&rig);
    real_peer_prepare(&rig);
    rig.put("agents.json", &json!({"result":{"agents":[{"pane_id":"w1:p1","terminal_id":"term_owner","agent":"pi","agent_status":"idle"}]}}));
    assert_eq!(run_json(&rig, &["inbox", "claim", "--harness", "pi"]).0, 0);
    rig.put("reserve.json", &json!({"reserved":false,"headroom":null}));
    let observer = super::place::peer_observer(&rig);
    let (code, queued) = run_json(
        &rig,
        &["run", "--host", "netcup", "--cwd", missing, "--", "true"],
    );
    drop(observer);
    assert_eq!(code, 1, "{queued}");
    rig.put("reserve.json", &json!({"reserved":true,"headroom":null}));
    let ticket = advance(&rig);
    assert_eq!(ticket["state"], "failed", "{ticket}");
    assert_eq!(ticket["diagnostic"]["cause"], "execution_directory_missing");
    assert_eq!(ticket["diagnostic"]["context"]["directory"], missing);
    let error = ticket["error"].as_str().unwrap();
    assert!(error.contains(&expected), "{ticket}");
    assert!(!error.contains("ssh "), "{ticket}");
    let id = ticket["request"]["id"].as_str().unwrap();
    assert_eq!(rig.admission(id)["diagnostic"], ticket["diagnostic"]);
    let inbox = rig.inbox("term_owner");
    assert!(
        inbox[0]["text"].as_str().unwrap().contains(error),
        "{inbox}"
    );
    let _observer = rig.serve();
    super::wait_publication(&rig, |frame| {
        frame["views"]["status"]["data"]["queue"]
            .as_array()
            .is_some_and(|queue| {
                queue
                    .iter()
                    .any(|ticket| ticket["diagnostic"]["cause"] == "execution_directory_missing")
            })
    });
    let status = rig.run(&["status"]);
    assert!(status.status.success(), "{status:?}");
    let text = String::from_utf8(status.stdout).unwrap();
    assert!(
        text.contains(missing) && text.contains("execution_directory_missing"),
        "{text}"
    );
}

#[test]
fn launch_ssh_exit_255_stays_a_transport_failure_even_with_a_peer_envelope() {
    let rig = Rig::new("diagnostic-launch-transport");
    launch_peer(&rig);
    let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
    rig.script("ssh", &script.replace("  *'host launch'*)", "  *'host launch'*) echo 'Permission denied (publickey)' >&2; cat \"$RIG/transport-envelope.json\"; exit 255;"));
    rig.put("transport-envelope.json", &json!({"error":"PRIVATE_SENTINEL","deadline":true,"diagnostic":{"version":1,"operation":"launch","cause":"execution_directory_missing","context":{}}}));
    let observer = super::place::peer_observer(&rig);
    let (code, error) = run_json(
        &rig,
        &[
            "run",
            "--host",
            "netcup",
            "--cwd",
            "/Users/oisin",
            "--",
            "true",
        ],
    );
    drop(observer);
    assert_eq!(code, 2, "{error}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("ssh netcup: authentication failed"),
        "{error}"
    );
    assert!(error["diagnostic"].is_null(), "{error}");
    assert!(!error.to_string().contains("PRIVATE_SENTINEL"));
}

#[test]
fn waiting_remote_job_receives_the_real_missing_directory_diagnostic() {
    let rig = Rig::new("diagnostic-run-wait-missing-cwd");
    let missing = rig.dir.join("missing");
    let missing = missing.to_str().unwrap();
    assert!(!Path::new(missing).exists());
    let expected = format!("execution directory {missing} does not exist on netcup");
    launch_peer(&rig);
    real_peer_prepare(&rig);
    rig.put("reserve.json", &json!({"reserved":false,"headroom":null}));
    let observer = super::place::peer_observer(&rig);
    let mut command = rig.start(&[
        "--json", "run", "--wait", "--max", "30s", "--host", "netcup", "--cwd", missing, "--",
        "true",
    ]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while rig.tickets().as_array().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "waiting caller did not enqueue");
        std::thread::sleep(Duration::from_millis(10));
    }
    super::place::stop_observer(&rig, observer);
    rig.put("reserve.json", &json!({"reserved":true,"headroom":null}));
    assert_eq!(advance(&rig)["state"], "failed");
    assert_eq!(command.wait().code(), Some(2));
    let (stdout, stderr) = command.output();
    let error: Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    assert_eq!(
        error["diagnostic"]["cause"], "execution_directory_missing",
        "{stdout} {stderr}"
    );
    assert_eq!(error["diagnostic"]["context"]["directory"], missing);
    assert!(
        error["error"].as_str().unwrap().contains(&expected),
        "{error}"
    );
}

#[test]
fn jobs_and_agents_share_repository_relative_remote_cwd_mapping() {
    assert_repository_relative_remote_cwd_mapping(false);
}

#[test]
fn jobs_and_agents_map_symlinked_repository_cwd_to_the_remote_checkout() {
    assert_repository_relative_remote_cwd_mapping(true);
}

fn assert_repository_relative_remote_cwd_mapping(symlinked: bool) {
    for action in ["run", "agent"] {
        let rig = Rig::new(&format!("diagnostic-cwd-mapping-{action}-{symlinked}"));
        let repo = rig.dir.join("home/repo");
        repository(&repo);
        fs::create_dir_all(repo.join("nested")).unwrap();
        let cwd = if symlinked {
            let alias = rig.dir.join("alias-home");
            std::os::unix::fs::symlink(rig.dir.join("home"), &alias).unwrap();
            alias.join("repo/nested")
        } else {
            repo.join("nested")
        };
        launch_peer(&rig);
        let mut args = vec![action, "--host", "netcup", "--cwd", cwd.to_str().unwrap()];
        if action == "run" {
            args.extend(["--", "true"]);
        } else {
            args.extend(["--name", "worker", "--message", "go"]);
        }
        let observer = super::place::peer_observer(&rig);
        let (code, result) = run_json(&rig, &args);
        drop(observer);
        assert_eq!(code, 0, "{action}: {result}");
        let log = rig.log("ssh.log");
        assert!(
            log.contains("\"cwd\":\"/remote/repo/nested\""),
            "{action}: {log}"
        );
    }
}

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
        rig.ingest_peers();
        let observer = super::place::peer_observer(&rig);
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
        assert_eq!(rig.tickets(), json!([]));
        let attempt = rig.admissions()[0].clone();
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
        drop(observer);
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
    let begin = script.find("  *'host prepare'*)").unwrap();
    let end = script[begin..].find("  *) echo unexpected").unwrap() + begin;
    let routing = format!(
        r#"  *'host launch'*|*'host prepared'*)
    for remote do :; done
    remote=${{remote#* host }}
    DOGSLED_STATE="$RIG/peer-state" sh -c '{} host '"$remote" ;;
"#,
        env!("CARGO_BIN_EXE_dogsled")
    );
    let script = format!("{}{}{}", &script[..begin], handler, &script[end..]);
    rig.script(
        "ssh",
        &script.replacen("case \"$*\" in", &format!("case \"$*\" in\n{routing}"), 1),
    );
}

/// The peer runs the real prepare command, so the cause it sends is the real one.
#[test]
fn a_real_remote_unknown_base_names_the_base_in_the_ticket() {
    let (rig, repo) = place::place_rig("diagnostic-real-unknown-base");
    rig.peer();
    real_peer_prepare(&rig);
    rig.ingest_peers();
    let observer = super::place::peer_observer(&rig);
    let (code, error) = rig.json(&[
        "agent",
        "--host",
        "netcup",
        "--cwd",
        repo.to_str().unwrap(),
        "--branch",
        "worker",
        "--base",
        "origin/absent",
        "--name",
        "worker",
        "--message",
        "TASK_SECRET",
    ]);
    drop(observer);
    assert_eq!(code, 2, "{error}");
    let text = error["error"].as_str().unwrap();
    assert!(text.contains("origin/absent"), "{error}");
    assert!(text.contains("netcup"), "{error}");
    assert!(!error.to_string().contains("TASK_SECRET"), "{error}");
}

/// A bare origin that holds one branch the clone has never fetched.
fn origin_with_branch(rig: &Rig, repo: &Path, branch: &str) {
    let bare = rig.dir.join("origin.git");
    let out = isolated("git")
        .args(["init", "-q", "--bare", "-b", "main"])
        .arg(&bare)
        .output()
        .unwrap();
    assert!(out.status.success(), "{:?}", out.status);
    let out = isolated("git")
        .arg("-C")
        .arg(repo)
        .args(["push", "-q"])
        .arg(&bare)
        .arg(format!("main:refs/heads/{branch}"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = isolated("git")
        .arg("-C")
        .arg(repo)
        .args(["remote", "set-url", "origin"])
        .arg(&bare)
        .output()
        .unwrap();
    assert!(out.status.success(), "{:?}", out.status);
}

/// A base that the origin holds, but the clone has never fetched, resolves after one
/// fetch of that branch.
#[test]
fn a_base_pushed_after_the_clone_is_fetched_and_resolves() {
    let (rig, repo) = place::place_rig("diagnostic-fetch-base");
    origin_with_branch(&rig, &repo, "feature");
    let unresolved = isolated("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "--verify", "origin/feature"])
        .output()
        .unwrap();
    assert!(!unresolved.status.success());
    let (code, prepared) = rig.json(&[
        "host",
        "prepare",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "worker",
        "--base",
        "origin/feature",
        "--request-id",
        "fetched",
    ]);
    assert_eq!(code, 0, "{prepared}");
    let tree = repo.join(".worktrees/worker");
    assert!(tree.is_dir(), "{tree:?}");
    let head = isolated("git")
        .arg("-C")
        .arg(&tree)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(head.status.success());
    let base = isolated("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "main"])
        .output()
        .unwrap();
    assert_eq!(head.stdout, base.stdout);
    // The remote branch is now a remote-tracking ref of this clone.
    let fetched = isolated("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "--verify", "origin/feature"])
        .output()
        .unwrap();
    assert!(fetched.status.success());
}

/// A clone that fetches one branch resolves a base on another branch, because the fetch
/// names its own destination.
#[test]
fn a_single_branch_clone_resolves_a_base_on_another_branch() {
    let (rig, repo) = place::place_rig("diagnostic-single-branch-fetch");
    origin_with_branch(&rig, &repo, "feature");
    // The clone follows one branch only, so a fetch by name creates no remote-tracking ref.
    let narrowed = isolated("git")
        .arg("-C")
        .arg(&repo)
        .args([
            "config",
            "remote.origin.fetch",
            "+refs/heads/main:refs/remotes/origin/main",
        ])
        .output()
        .unwrap();
    assert!(narrowed.status.success());
    let (code, prepared) = rig.json(&[
        "host",
        "prepare",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "worker",
        "--base",
        "origin/feature",
        "--request-id",
        "narrow",
    ]);
    assert_eq!(code, 0, "{prepared}");
    let tree = repo.join(".worktrees/worker");
    assert!(tree.is_dir(), "{tree:?}");
    let fetched = isolated("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "--verify", "origin/feature"])
        .output()
        .unwrap();
    assert!(fetched.status.success());
}

/// The same fetch happens on a remote peer: a base the peer's clone has not fetched
/// resolves there too.
#[test]
fn a_remote_peer_fetches_a_base_its_clone_has_not_fetched() {
    let (rig, repo) = place::place_rig("diagnostic-remote-fetch-base");
    origin_with_branch(&rig, &repo, "feature");
    rig.peer();
    real_peer_prepare(&rig);
    rig.ingest_peers();
    let observer = super::place::peer_observer(&rig);
    let (code, prepared) = rig.json(&[
        "agent",
        "--host",
        "netcup",
        "--cwd",
        repo.to_str().unwrap(),
        "--branch",
        "worker",
        "--base",
        "origin/feature",
        "--name",
        "worker",
        "--message",
        "go",
    ]);
    drop(observer);
    assert_eq!(code, 0, "{prepared}");
    let tree = repo.join(".worktrees/worker");
    assert!(tree.is_dir(), "{tree:?}");
    let fetched = isolated("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "--verify", "origin/feature"])
        .output()
        .unwrap();
    assert!(fetched.status.success());
}

/// A base the origin does not hold still fails, and the failure names the base.
#[test]
fn a_base_missing_on_origin_still_fails_with_its_name() {
    let (rig, repo) = place::place_rig("diagnostic-missing-base");
    origin_with_branch(&rig, &repo, "feature");
    let (code, error) = rig.json(&[
        "host",
        "prepare",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "worker",
        "--base",
        "origin/absent",
        "--request-id",
        "absent",
    ]);
    assert_eq!(code, 2, "{error}");
    assert_eq!(error["diagnostic"]["cause"], "base_unavailable", "{error}");
    assert_eq!(
        error["diagnostic"]["context"]["base"], "origin/absent",
        "{error}"
    );
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("requested base unavailable"),
        "{error}"
    );
    assert!(!repo.join(".worktrees/worker").exists());
    let absent = isolated("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "--verify", "origin/absent"])
        .output()
        .unwrap();
    assert!(!absent.status.success());
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
    assert!(!rig.preparation_record("state", "claims", "").exists());
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
        let name = format!("diagnostic-real-{case}");
        let (rig, repo) = if case == "setup" {
            place::place_rig_with(
                &name,
                "[worktree]\nsetup = [\"echo SCRIPT_SECRET >&2; exit 1\"]\n",
            )
        } else {
            place::place_rig(&name)
        };
        rig.peer();
        real_peer_prepare(&rig);
        let branch = if case == "exists" { "main" } else { "worker" };
        if case == "occupied" {
            fs::create_dir_all(repo.join(".worktrees/worker")).unwrap();
        }
        rig.ingest_peers();
        let observer = super::place::peer_observer(&rig);
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
        drop(observer);
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

/// A remote preparation failure that names its cause reaches the ticket. The peer's other
/// text does not.
#[test]
fn a_remote_prepare_failure_names_its_cause_in_the_ticket() {
    let rig = Rig::new("diagnostic-remote-cause");
    let repo = rig.dir.join("home/repo");
    repository(&repo);
    rig.peer();
    fail_peer(&rig, "prepare", &envelope("base_unavailable"));
    rig.ingest_peers();
    let observer = super::place::peer_observer(&rig);
    let (code, error) = rig.json(&[
        "agent",
        "--host",
        "netcup",
        "--cwd",
        repo.to_str().unwrap(),
        "--branch",
        "worker",
        "--base",
        "origin/x",
        "--name",
        "worker",
        "--message",
        "TASK_SECRET",
    ]);
    drop(observer);
    assert_eq!(code, 2, "{error}");
    let text = error["error"].as_str().unwrap();
    assert!(text.contains("base origin/x"), "{error}");
    assert!(text.contains("netcup"), "{error}");
    assert!(!error.to_string().contains("TASK_SECRET"), "{error}");
    // The cause is the caller's own base, not the peer's text.
    assert!(!error.to_string().contains("PRIVATE_SENTINEL"), "{error}");
    let attempt = rig.admissions()[0].clone();
    assert!(
        attempt["error"].as_str().unwrap().contains("base origin/x"),
        "{attempt}"
    );
}

/// The peer fails to prepare and cannot record the outcome. The caller on the other side of
/// the transport must see that persistence failure itself, with its own correlation, and
/// without any of the peer's own text.
#[test]
fn real_remote_unrecorded_failure_preserves_warning_and_correlation() {
    // The peer's private setup fails, and its receipt store cannot hold an outcome.
    let (rig, repo) = place::place_rig_with(
        "diagnostic-real-unrecorded",
        r#"[worktree]
setup = ["printf 'https://user:SCRIPT_SECRET@private/PATH_SECRET' >&2; \"$DOGSLED_BIN\" state fault preparation-receipts --write; exit 1"]
"#,
    );
    rig.peer();
    real_peer_prepare(&rig);
    rig.ingest_peers();
    let observer = super::place::peer_observer(&rig);
    let (code, error) = rig.json(&[
        "agent",
        "--host",
        "netcup",
        "--cwd",
        repo.to_str().unwrap(),
        "--branch",
        "worker",
        "--base",
        "main",
        "--name",
        "worker",
        "--message",
        "TASK_SECRET",
    ]);
    drop(observer);
    assert_eq!(code, 2, "{error}");
    assert_eq!(
        error["diagnostic"]["cause"], "outcome_unrecorded",
        "{error}"
    );
    assert_eq!(error["diagnostic"]["operation"], "prepare");
    assert_eq!(error["diagnostic"]["host"], "netcup");
    assert_eq!(error["diagnostic"]["status"], 2);
    assert_eq!(error["diagnostic"]["context"]["branch"], "worker");
    assert_eq!(error["diagnostic"]["context"]["base"], "main");
    let request = error["diagnostic"]["request_id"].as_str().unwrap();
    assert!(!request.is_empty(), "{error}");
    // The peer recorded the intent, and no outcome: the failure stayed unrecorded.
    rig.cmd(&["state", "fault", "clear-preparation-receipts", "--write"])
        .env("DOGSLED_STATE", rig.dir.join("peer-state"))
        .status()
        .unwrap();
    let intent = rig
        .preparation_record("peer-state", "receipt", request)
        .read()
        .to_string();
    assert!(intent.contains("\"prepared\":null"), "{intent}");
    assert!(!intent.contains("\"failure\""), "{intent}");

    let text = error["error"].as_str().unwrap();
    assert!(text.contains("cause was not recorded"), "{error}");
    assert!(
        text.contains("inspect this host before resubmitting"),
        "{error}"
    );
    assert!(text.contains("untrusted detail withheld"), "{error}");
    for private in [
        "SCRIPT_SECRET",
        "PATH_SECRET",
        "TASK_SECRET",
        "https://user:",
        "blocked",
        repo.to_str().unwrap(),
    ] {
        assert!(!error.to_string().contains(private), "{error}");
    }
    assert!(!rig.log("ssh.log").contains("host launch"));
    assert_eq!(rig.tickets(), json!([]));
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
        rig.ingest_peers();
        let observer = super::place::peer_observer(&rig);
        let (code, error) = rig.json(&["agent","--host","netcup","--cwd",repo.to_str().unwrap(),
            "--branch","worker","--base","missing","--name","worker","--message","TASK_SECRET"]);
        drop(observer);
        assert_eq!(code,2,"{error}");
        assert_eq!(error["diagnostic"]["operation"],"prepare");
        assert_eq!(error["diagnostic"]["status"],2);
        assert_ne!(error["diagnostic"]["request_id"], "forged");
        assert!(error["diagnostic"]["request_id"].is_string());
        assert!(error["error"].as_str().unwrap().contains("untrusted detail withheld"));
        assert!(!error.to_string().contains("SECRET_SENTINEL"),"{error}");
        assert!(!error.to_string().contains("TASK_SECRET"),"{error}");
        assert!(!error.to_string().contains("\\u001b"),"{error}");
        assert_eq!(rig.tickets(), json!([]));
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
    rig.ingest_peers();
    let observer = super::place::peer_observer(&rig);
    let (code, error) = rig.json(&[
        "agent",
        "--host",
        "netcup",
        "--name",
        "worker",
        "--message",
        "TASK_SECRET",
    ]);
    drop(observer);
    assert_eq!(code, 2, "{error}");
    assert_eq!(error["diagnostic"]["operation"], "launch");
    assert_eq!(error["diagnostic"]["status"], 0);
    assert!(error["diagnostic"]["request_id"].is_string());
    assert!(!error.to_string().contains("TASK_SECRET"));
    assert!(!error.to_string().contains("PAYLOAD_SECRET"));
    assert_eq!(rig.tickets(), json!([]));
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
    rig.ingest_peers();
    let observer = super::observation::seeded(&rig);
    let (code, error) = rig.json(&[
        "agent",
        "--host",
        "netcup",
        "--name",
        "worker",
        "--message",
        "TASK_SECRET",
    ]);
    drop(observer);
    assert_eq!(code, 2, "{error}");
    assert_eq!(error["deadline"], true);
    assert_eq!(error["diagnostic"]["operation"], "launch", "{error}");
    assert_eq!(error["diagnostic"]["host"], "netcup");
    assert!(error["diagnostic"]["request_id"].is_string());
    assert_eq!(error["diagnostic"]["status"], 2);
    assert!(!error.to_string().contains("TASK_SECRET"));
    assert_eq!(rig.tickets(), json!([]));
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
        rig.ingest_peers();
        let observer = super::place::peer_observer(&rig);
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
        drop(observer);
        assert_eq!(code, 2, "{error}");
        assert_eq!(error["diagnostic"]["cause"], cause);
        assert_eq!(error["diagnostic"]["operation"], "checkout");
        assert_eq!(error["diagnostic"]["host"], "netcup");
        assert!(error["diagnostic"]["request_id"].is_string());
        assert!(!error.to_string().contains("PRIVATE_SENTINEL"));
        assert!(!rig.log("ssh.log").contains("host prepare"));
        assert_eq!(rig.tickets(), json!([]));
    }
}

#[test]
fn mcp_place_preserves_structured_cause_and_safe_text() {
    let rig = Rig::new("diagnostic-mcp");
    let repo = rig.dir.join("home/repo");
    repository(&repo);
    rig.peer();
    fail_peer(&rig, "prepare", &envelope("branch_exists"));
    rig.ingest_peers();
    let _observer = super::place::peer_observer(&rig);
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
    rig.ingest_peers();
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
    let producer = super::place::peer_observer(&rig);
    drop(producer);
    // Preserve the peer's fresh sample without live lanes settling its refusal.
    fs::write(
        rig.dir.join("clock"),
        rig.read("remote.json")["sampled_ms"].to_string(),
    )
    .unwrap();
    rig.placement_capacity();
    let producer = super::observation::seeded(&rig);
    let (code, error) = operation(&rig, "commit", &commit_input(&execution, 0));
    drop(producer);
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
    rig.ingest_peers();
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
    let producer = super::place::peer_observer(&rig);
    let (code, receipt) = operation(&rig, "commit", &commit_input(&execution, 0));
    drop(producer);
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
            "--request-validity",
            "1",
            "--request-id",
            "checkout-diagnostic",
            "--request-created-ms",
            &rig.origin_ms.to_string(),
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

/// A returned preparation failure is retained on the host. The same request replays the
/// original safe cause, and the replay runs no preparation work.
#[test]
fn a_retained_failure_replays_its_cause_without_preparing_again() {
    let (rig, repo) = place::place_rig("diagnostic-retained-failure");
    let args = [
        "host",
        "prepare",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "worker",
        "--base",
        "missing",
        "--request-id",
        "retained",
    ];
    let (code, original) = rig.json(&args);
    assert_eq!(code, 2, "{original}");
    assert_eq!(original["diagnostic"]["cause"], "base_unavailable");
    assert_eq!(original["diagnostic"]["request_id"], "retained");
    let receipt = rig
        .preparation_record("state", "receipt", "retained")
        .read();
    assert!(receipt["prepared"].is_null(), "{receipt}");
    assert_eq!(receipt["failure"]["kind"], "other", "{receipt}");
    assert_eq!(
        receipt["failure"]["diagnostic"]["cause"], "base_unavailable",
        "{receipt}"
    );
    // The host state now allows the preparation to succeed. A replay must not use it.
    let git = isolated("git")
        .arg("-C")
        .arg(&repo)
        .args(["branch", "missing"])
        .output()
        .unwrap();
    assert!(git.status.success());
    let herdr = rig.log("herdr.log");
    let setup = rig.log("setup.log");
    let (code, replay) = rig.json(&args);
    assert_eq!(code, 2, "{replay}");
    assert_eq!(replay["diagnostic"], original["diagnostic"]);
    assert_eq!(rig.log("herdr.log"), herdr, "the replay prepared again");
    assert_eq!(rig.log("setup.log"), setup);
    assert!(!repo.join(".worktrees/worker").exists());
}

/// A successful preparation still replays as a success, with no second preparation.
#[test]
fn a_successful_preparation_still_replays_as_a_success() {
    let (rig, repo) = place::place_rig("diagnostic-retained-success");
    let args = [
        "host",
        "prepare",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "worker",
        "--request-id",
        "successful",
    ];
    let (code, original) = rig.json(&args);
    assert_eq!(code, 0, "{original}");
    let herdr = rig.log("herdr.log");
    let setup = rig.log("setup.log");
    let (code, replay) = rig.json(&args);
    assert_eq!(code, 0, "{replay}");
    assert_eq!(replay, original, "{replay}");
    assert_eq!(rig.log("herdr.log"), herdr, "the replay prepared again");
    assert_eq!(rig.log("setup.log"), setup);
}

/// The same request with a changed input is a conflict, not a replay.
#[test]
fn a_retained_failure_still_refuses_a_changed_request() {
    let (rig, repo) = place::place_rig("diagnostic-retained-conflict");
    let base = [
        "host",
        "prepare",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "worker",
        "--base",
        "missing",
        "--request-id",
        "retained",
    ];
    let (code, original) = rig.json(&base);
    assert_eq!(code, 2, "{original}");
    let herdr = rig.log("herdr.log");
    let mut changed = base;
    changed[7] = "main";
    let (code, conflict) = rig.json(&changed);
    assert_eq!(code, 2, "{conflict}");
    assert_eq!(
        conflict["diagnostic"]["cause"], "request_changed",
        "{conflict}"
    );
    assert_eq!(rig.log("herdr.log"), herdr);
}

/// An old receipt without an outcome proves nothing. It stays uncertain, and it is not
/// read as a success or as a recorded failure.
#[test]
fn a_receipt_without_an_outcome_stays_uncertain() {
    let (rig, repo) = place::place_rig("diagnostic-legacy-receipt");
    let args = [
        "host",
        "prepare",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "worker",
        "--base",
        "missing",
        "--request-id",
        "legacy",
    ];
    let (code, original) = rig.json(&args);
    assert_eq!(code, 2, "{original}");
    // A receipt written before failures were retained holds no outcome.
    let receipt = rig.preparation_record("state", "receipt", "legacy").read();
    let mut legacy = receipt.clone();
    legacy.as_object_mut().unwrap().remove("failure");
    rig.preparation_record("state", "receipt", "legacy")
        .put(&legacy);
    let herdr = rig.log("herdr.log");
    let (code, uncertain) = rig.json(&args);
    assert_eq!(code, 2, "{uncertain}");
    assert!(
        uncertain["error"]
            .as_str()
            .unwrap()
            .contains("stopped during preparation"),
        "{uncertain}"
    );
    assert_eq!(
        uncertain["diagnostic"]["cause"], "preparation_interrupted",
        "{uncertain}"
    );
    assert_eq!(rig.log("herdr.log"), herdr);
}

/// A receipt cannot hold a success and a failure at once.
#[test]
fn a_contradictory_receipt_is_refused() {
    let (rig, repo) = place::place_rig("diagnostic-contradictory");
    let args = [
        "host",
        "prepare",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "worker",
        "--base",
        "missing",
        "--request-id",
        "contradictory",
    ];
    let (code, original) = rig.json(&args);
    assert_eq!(code, 2, "{original}");
    let mut receipt = rig
        .preparation_record("state", "receipt", "contradictory")
        .read();
    receipt["prepared"] = json!({"pane_id":"w8:p1","terminal_id":"term_tree"});
    let original_receipt = rig
        .preparation_record("state", "receipt", "contradictory")
        .read();
    let out = rig.put_preparation_refused("state", "receipt", "contradictory", &receipt);
    assert!(!out.status.success());
    assert_eq!(
        rig.preparation_record("state", "receipt", "contradictory")
            .read(),
        original_receipt
    );
    // The contradictory write was refused before it could replace the outcome.
    receipt = original_receipt;
    assert!(receipt["prepared"].is_null());
    let (code, replay) = rig.json(&args);
    assert_eq!((code, replay), (2, original));
}

/// A failure without a typed diagnostic keeps its safe classification and correlation, and
/// withholds its detail. The secret in the failing command never reaches the receipt.
#[test]
fn a_failure_without_a_diagnostic_keeps_only_safe_information() {
    let (rig, repo) = place::place_rig_with(
        "diagnostic-untyped-failure",
        "[worktree]\nsetup = [\"printf 'SETUP_SECRET' >&2; exit 1\"]\n",
    );
    let args = [
        "host",
        "prepare",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "worker",
        "--request-id",
        "untyped",
    ];
    let (code, original) = rig.json(&args);
    assert_eq!(code, 2, "{original}");
    let receipt = rig.preparation_record("state", "receipt", "untyped").read();
    assert!(!receipt.to_string().contains("SETUP_SECRET"), "{receipt}");
    assert_eq!(receipt["failure"]["detail_withheld"], true, "{receipt}");
    assert_eq!(receipt["failure"]["request_id"], "untyped", "{receipt}");
    let herdr = rig.log("herdr.log");
    let (code, replay) = rig.json(&args);
    assert_eq!(code, 2, "{replay}");
    assert!(
        replay["error"]
            .as_str()
            .unwrap()
            .contains("untrusted detail withheld"),
        "{replay}"
    );
    assert!(!replay.to_string().contains("SETUP_SECRET"), "{replay}");
    assert_eq!(rig.log("herdr.log"), herdr);
}

/// A failure whose outcome cannot be written is reported as unrecorded. The report must not
/// claim that the original cause is retained.
#[test]
fn a_failure_whose_outcome_cannot_be_written_is_reported_as_unrecorded() {
    let (rig, repo) = place::place_rig_with(
        "diagnostic-unrecorded",
        r#"[worktree]
setup = ['"$DOGSLED_BIN" state fault preparation-receipts --write; exit 1']
"#,
    );
    let (code, error) = rig.json(&[
        "host",
        "prepare",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "worker",
        "--request-id",
        "unrecorded",
    ]);
    assert_eq!(code, 2, "{error}");
    let text = error["error"].as_str().unwrap();
    assert!(text.contains("cause was not recorded"), "{error}");
    assert!(text.contains("inspect this host"), "{error}");
}

fn advance(rig: &Rig) -> Value {
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    rig.tickets()[0].clone()
}

#[test]
fn waiting_caller_receives_failed_ticket_diagnostic() {
    let rig = Rig::new("diagnostic-wait");
    let repo = rig.dir.join("home/repo");
    repository(&repo);
    rig.peer();
    rig.put("reserve.json", &json!({"reserved":false,"headroom":null}));
    fail_peer(&rig, "prepare", &envelope("base_unavailable"));
    rig.ingest_peers();
    let observer = super::place::peer_observer(&rig);
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
    while rig.tickets().as_array().unwrap().is_empty() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("caller did not enqueue");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    super::place::stop_observer(&rig, observer);
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

/// A typed prepare failure names its cause in the ticket, admission, and owner inbox.
#[test]
fn a_typed_remote_prepare_failure_names_its_cause_in_the_ticket_and_inbox() {
    let rig = Rig::new("diagnostic-plain-ticket");
    let repo = rig.dir.join("home/repo");
    repository(&repo);
    rig.peer();
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{"pane_id":"w1:p1","terminal_id":"term_owner","agent":"pi","agent_status":"idle"}]}}),
    );
    assert_eq!(rig.json(&["inbox", "claim", "--harness", "pi"]).0, 0);
    rig.put("reserve.json", &json!({"reserved":false,"headroom":null}));
    rig.ingest_peers();
    let observer = super::place::peer_observer(&rig);
    let (code, queued) = rig.json(&[
        "agent",
        "--host",
        "netcup",
        "--cwd",
        repo.to_str().unwrap(),
        "--branch",
        "worker",
        "--base",
        "origin/absent",
        "--name",
        "worker",
        "--message",
        "TASK_SECRET",
    ]);
    drop(observer);
    assert_eq!(code, 1, "{queued}");
    rig.put("reserve.json", &json!({"reserved":true,"headroom":null}));
    fail_peer(&rig, "prepare", &envelope("base_unavailable"));
    let ticket = advance(&rig);
    assert_eq!(ticket["state"], "failed", "{ticket}");
    let error = ticket["error"].as_str().unwrap();
    assert!(error.contains("base origin/absent"), "{ticket}");
    assert!(error.contains("netcup"), "{ticket}");
    assert!(!error.contains("PRIVATE_SENTINEL"), "{ticket}");
    let id = ticket["request"]["id"].as_str().unwrap();
    let attempt = rig.admission(id);
    assert!(
        attempt["error"]
            .as_str()
            .unwrap()
            .contains("base origin/absent"),
        "{attempt}"
    );
    let inbox = rig.inbox("term_owner");
    let text = inbox[0]["text"].as_str().unwrap();
    assert!(text.contains("base origin/absent"), "{text}");
    assert!(!text.contains("PRIVATE_SENTINEL"), "{text}");
    assert!(!text.contains("TASK_SECRET"), "{text}");
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
    rig.ingest_peers();
    let producer = super::place::peer_observer(&rig);
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
    drop(producer);
    assert_eq!(code, 1, "{queued}");
    rig.put("reserve.json", &json!({"reserved":true,"headroom":null}));
    fail_peer(&rig, "prepare", &envelope("base_unavailable"));
    let ticket = advance(&rig);
    assert_eq!(ticket["state"], "failed", "{ticket}");
    assert_eq!(ticket["retry"]["attempts"], 0);
    assert_eq!(ticket["diagnostic"]["cause"], "base_unavailable");
    let id = ticket["request"]["id"].as_str().unwrap();
    let attempt = rig.admission(id);
    assert_eq!(attempt["diagnostic"], ticket["diagnostic"]);
    let inbox = rig.inbox("term_owner");
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
