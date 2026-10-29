use super::*;
use std::io::Write as _;

#[test]
fn c3a_bound_final_launch_replays_but_stripped_new_launch_refuses() {
    for job in [false, true] {
        for progress in ["released", "fenced", "missing"] {
            for historical in [true, false] {
                c3a_replay_case(historical, job, progress);
            }
        }
    }
}

fn c3a_replay_case(historical: bool, job: bool, progress: &str) {
    let (rig, repo) = short_disposition_rig("c3a-replay");
    if job {
        let herdr = fs::read_to_string(rig.dir.join("bin/herdr")).unwrap();
        rig.script("herdr", &herdr.replace("case \"$*\" in", "case \"$*\" in\n  'tab create'*) printf '%s\\n' '{\"result\":{\"root_pane\":{\"pane_id\":\"w9:p1\",\"terminal_id\":\"job-terminal\"}}}' ;;\n  'pane run'*) printf '{}\\n' ;;"));
    }
    let prepared = disposition_prepared(&rig, &repo, "recover-one", "saved", true);
    let mut launch = if job {
        job_launch_request(&rig, "recover-one", &["true"])
    } else {
        host_request(&rig)
    };
    launch["cwd"] = prepared["path"].clone();
    if !job {
        launch["pane"] = prepared["pane_id"].clone();
        launch["terminal"] = prepared["terminal_id"].clone();
    }
    launch["preparation"] = prepared["binding"].clone();
    launch["preparation_protocol"] = json!(1);
    if !historical && progress == "released" {
        assert_unbound_launch_refuses(&rig, &launch);
    }
    let (code, result) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &launch.to_string(),
    ]);
    assert_eq!(code, 0, "{result}");
    if historical {
        write_c3a_launch_shape(&rig, job);
    }
    let file = "state/preparation-receipts/recover-one.disposition.json";
    if progress == "missing" {
        fs::remove_file(rig.dir.join(file)).unwrap();
    } else if progress == "fenced" {
        let mut fence = rig.read(file);
        fence["progress"] = json!(progress);
        rig.put(file, &fence);
    }
    launch.as_object_mut().unwrap().remove("preparation");
    launch
        .as_object_mut()
        .unwrap()
        .remove("preparation_protocol");
    let fence = rig
        .dir
        .join("state/preparation-receipts/recover-one.disposition.json");
    let before = fs::read(&fence).ok();
    let calls = rig.log("herdr.log");
    let (code, replay) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &launch.to_string(),
    ]);
    assert_eq!(
        code,
        if historical { 0 } else { 2 },
        "historical={historical} job={job} progress={progress}: {replay}"
    );
    if historical {
        let mut expected = result;
        if job {
            expected["request"]
                .as_object_mut()
                .unwrap()
                .remove("preparation_protocol");
        }
        assert_eq!(replay, expected);
    }
    assert_eq!(fs::read(fence).ok(), before);
    assert_eq!(rig.log("herdr.log"), calls);
}

fn assert_unbound_launch_refuses(rig: &Rig, launch: &Value) {
    let mut stripped = launch.clone();
    stripped.as_object_mut().unwrap().remove("preparation");
    stripped
        .as_object_mut()
        .unwrap()
        .remove("preparation_protocol");
    let claims = rig.read("state/preparations.json");
    let calls = rig.log("herdr.log");
    assert_eq!(
        rig.json(&[
            "host",
            "launch",
            "--job-validity",
            "2",
            "--request",
            &stripped.to_string()
        ])
        .0,
        2
    );
    assert_eq!(rig.read("state/preparations.json"), claims);
    assert_eq!(rig.log("herdr.log"), calls);
    assert!(!rig.dir.join("state/launches/recover-one.json").exists());
    for version in [Value::Null, json!(99)] {
        let mut unsupported = launch.clone();
        unsupported["preparation_protocol"] = version;
        assert_eq!(
            rig.json(&[
                "host",
                "launch",
                "--job-validity",
                "2",
                "--request",
                &unsupported.to_string()
            ])
            .0,
            2
        );
        assert_eq!(rig.read("state/preparations.json"), claims);
        assert_eq!(rig.log("herdr.log"), calls);
    }
}

fn write_c3a_launch_shape(rig: &Rig, job: bool) {
    let file = if job {
        "state/launches/recover-one.json"
    } else {
        "state/launches/recover-one.progress.json"
    };
    let mut saved = rig.read(file);
    saved["request"]
        .as_object_mut()
        .unwrap()
        .remove("preparation");
    saved["request"]
        .as_object_mut()
        .unwrap()
        .remove("preparation_protocol");
    rig.put(file, &saved);
    let file = "state/preparation-receipts/recover-one.disposition.json";
    let mut fence = rig.read(file);
    fence["request"]["policy"]["launch"]
        .as_object_mut()
        .unwrap()
        .remove("preparation");
    fence["request"]["policy"]["launch"]
        .as_object_mut()
        .unwrap()
        .remove("preparation_protocol");
    rig.put(file, &fence);
}

#[test]
fn aliased_preparation_rows_merge_only_for_the_same_request() {
    use std::os::unix::fs::symlink;
    let (rig, repo) = short_disposition_rig("aliased-owners");
    let storage = rig.dir.join("trees");
    fs::create_dir_all(&storage).unwrap();
    symlink(&storage, repo.join(".worktrees")).unwrap();
    disposition_prepared(&rig, &repo, "first", "alias", true);
    disposition_prepared(&rig, &repo, "second", "alias", false);
    let claims = rig.read("state/preparations.json");
    assert_eq!(claims.as_array().unwrap().len(), 2, "{claims}");
    assert_eq!(claims[0]["request_id"], "first");
    assert_eq!(claims[1]["request_id"], "second");
    assert_eq!(claims[0]["identity"], claims[1]["identity"]);
    let request = rig.disposition_request("first");
    assert_eq!(rig.json(&["host", "prepared", "--input", &request]).0, 0);
    assert_eq!(rig.read("state/preparations.json"), json!([claims[1]]));
}

#[test]
fn remote_preparation_accepts_owning_hosts_symlink_equivalence() {
    use std::os::unix::fs::symlink;
    let (rig, repo) = short_disposition_rig("remote-symlink");
    rig.peer();
    fs::create_dir_all(rig.dir.join("peer-state")).unwrap();
    rig.snapshot("peer-state/host.json", rig.origin_ms, 0.0, 16);
    let storage = rig.dir.join("trees");
    fs::create_dir_all(&storage).unwrap();
    symlink(&storage, repo.join(".worktrees")).unwrap();
    let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
    let handler = format!(
        r#"  *'host reserve '*|*'host prepare '*|*'host prepared '*)
    for remote do :; done
    remote=${{remote#* host }}
    DOGSLED_STATE="$RIG/peer-state" sh -c '{} host '"$remote" > "$RIG/peer-output" 2> "$RIG/peer-error"
    status=$?; cat "$RIG/peer-output"; exit "$status" ;;
"#,
        env!("CARGO_BIN_EXE_dogsled")
    );
    rig.script(
        "ssh",
        &script.replacen("case \"$*\" in", &format!("case \"$*\" in\n{handler}"), 1),
    );
    let (code, placed) = rig.json(&[
        "place",
        "--host",
        "netcup",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "alias",
        "--base",
        "main",
    ]);
    assert_eq!(
        code,
        0,
        "{placed}: {} {}",
        rig.log("peer-error"),
        rig.log("peer-output")
    );
    let path = PathBuf::from(placed["path"].as_str().unwrap());
    assert_eq!(path.canonicalize().unwrap(), storage.join("alias"));
    let receipt = fs::read_dir(rig.dir.join("peer-state/preparation-receipts"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(".disposition.json")
        })
        .unwrap();
    let receipt: Value = serde_json::from_slice(&fs::read(receipt).unwrap()).unwrap();
    assert_eq!(receipt["progress"], "released");
}

fn short_disposition_rig(name: &str) -> (Rig, PathBuf) {
    let (rig, repo) = place::place_rig(name);
    let relative = repo
        .strip_prefix(rig.dir.canonicalize().unwrap())
        .unwrap()
        .to_path_buf();
    let rig = rig.shortened();
    let repo = rig.dir.join(relative);
    (rig, repo)
}

#[test]
fn remote_preparation_release_requires_correlated_ack() {
    for (case, filter) in [
        ("empty", "printf '{}\n'"),
        ("legacy", "printf '{\"prepared\":true}\n'"),
        (
            "request",
            "sed 's/\"request_id\":\"[^\"]*\"/\"request_id\":\"foreign\"/g'",
        ),
        (
            "identity",
            "sed 's/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/g'",
        ),
        ("policy", "sed 's/placement_returned/preparation_failed/g'"),
        ("version", "sed 's/\"version\":1/\"version\":99/g'"),
        ("released", "sed 's/\"released\":true/\"released\":false/g'"),
        (
            "outcome",
            "sed 's/\"outcome\":\"unknown\"/\"outcome\":\"not_started\"/g'",
        ),
        ("exact", "cat"),
    ] {
        let rig = Rig::short(&format!("ack-{case}"));
        let repo = rig.dir.join("home/repo");
        repository(&repo);
        rig.peer();
        let other = json!([{ "request_id":"other", "path":"/other", "repo":"/other", "branch":"other", "head":"old" }]);
        rig.put("state/preparations.json", &other);
        // Keep the filter outside the protocol fixture: the response is untrusted wire data.
        let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
        let start = script.find("  *'host prepared'*)").unwrap();
        let end = script[start..].find("  *'host prepare'*)").unwrap() + start;
        let handler = format!(
            r#"  *'host prepared'*)
    for remote do :; done
    sh -c "$RIG/bin/peer-fixture $remote" > "$RIG/exact-ack"
    {filter} < "$RIG/exact-ack" ;;
"#
        );
        rig.script(
            "ssh",
            &format!("{}{}{}", &script[..start], handler, &script[end..]),
        );
        let (code, result) = rig.json(&[
            "place",
            "--host",
            "netcup",
            "--repo",
            repo.to_str().unwrap(),
            "--branch",
            "wire",
            "--base",
            "main",
        ]);
        assert_eq!(code == 0, case == "exact", "{case}: {result}");
        assert_eq!(rig.read("state/preparations.json"), other);
        assert!(!rig.dir.join("state/unit-requests").exists());
        let log = rig.log("ssh.log");
        assert_eq!(log.matches("host prepared ").count(), 1, "{log}");
        assert!(!log.contains("host launch "), "{log}");
    }
}

#[test]
fn lost_disposition_ack_replays_only_the_same_release() {
    let (rig, repo) = short_disposition_rig("lost-disposition");
    disposition_prepared(&rig, &repo, "release", "shared", true);
    disposition_prepared(&rig, &repo, "other", "shared", false);
    let input = rig.disposition_request("release");
    let request: Value = serde_json::from_str(&input).unwrap();
    let args = ["host", "prepared", "--input", &input];
    let calls = rig.log("herdr.log");
    let setup = rig.log("setup.log");
    let out = rig
        .cmd(&args)
        .env("DOGSLED_TEST_UNIT_CRASH", "preparation-claim-released")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(86));
    let claims = rig.read("state/preparations.json");
    assert_eq!(claims.as_array().unwrap().len(), 1);
    assert_eq!(claims[0]["request_id"], "other");
    let (code, ack) = rig.json(&args);
    assert_eq!(code, 0, "{ack}");
    let mut expected = request;
    expected["released"] = json!(true);
    assert_eq!(ack, expected);
    assert_eq!(rig.json(&args), (0, ack));
    assert_eq!(rig.read("state/preparations.json"), claims);
    assert_eq!(rig.log("herdr.log"), calls);
    assert_eq!(rig.log("setup.log"), setup);
    assert!(!rig.dir.join("ssh.log").exists());
}

#[test]
fn old_preparation_peer_refuses_at_reserve_before_preparation() {
    let (rig, repo) = short_disposition_rig("old-disposition-peer");
    rig.peer();
    let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
    rig.script(
        "ssh",
        &script.replace(
            "  *'host reserve'*)",
            "  *'host reserve'*) echo 'unexpected argument --preparation-protocol' >&2; exit 2;",
        ),
    );
    let (code, error) = remote_agent(&rig, Some(&repo));
    assert_eq!(code, 2, "{error}");
    let log = rig.log("ssh.log");
    assert!(
        log.contains("host reserve --preparation-protocol 1"),
        "{log}"
    );
    for command in ["host checkout ", "host prepare ", "host launch "] {
        assert!(!log.contains(command), "{log}");
    }
    assert!(!rig.dir.join("state/preparation-receipts").exists());
    assert!(!repo.join(".worktrees/remote-complete").exists());
    assert!(!rig.dir.join("state/placements.json").exists());
}

fn disposition_prepared(rig: &Rig, repo: &Path, id: &str, branch: &str, base: bool) -> Value {
    let mut args = vec![
        "host",
        "prepare",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        branch,
        "--request-id",
        id,
    ];
    if base {
        args.extend(["--base", "main"]);
    }
    let (code, prepared) = rig.json(&args);
    assert_eq!(code, 0, "{prepared}");
    prepared
}

#[test]
fn historical_preparation_without_outcome_cannot_rebind_to_todays_checkout() {
    let (rig, repo) = place::place_rig("historical-preparation-rebind");
    let tree = repo.join(".worktrees/historical");
    close::git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "historical",
            tree.to_str().unwrap(),
        ],
    );
    let admin = PathBuf::from(close::git(&tree, &["rev-parse", "--absolute-git-dir"]).trim());
    rig.put("state/preparations.json", &json!([{ "repo":repo, "path":tree, "branch":"historical", "head":"old-head", "request_id":"historical" }]));
    let original = fs::read(rig.dir.join("state/preparations.json")).unwrap();
    let herdr = rig.log("herdr.log");
    let (code, error) = rig.json(&[
        "host",
        "prepare",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "historical",
        "--request-id",
        "historical",
    ]);
    assert_ne!(code, 0, "{error}");
    assert_eq!(
        fs::read(rig.dir.join("state/preparations.json")).unwrap(),
        original
    );
    assert_eq!(rig.log("herdr.log"), herdr);
    assert!(
        !rig.dir
            .join("state/preparation-receipts/historical.json")
            .exists()
    );
    assert!(!admin.join("dogsled-checkout-id").exists());
}

#[test]
fn prohibited_preparation_cannot_replay_into_new_startup() {
    let (rig, repo) = place::place_rig("disposition-prohibited");
    let prepared = disposition_prepared(&rig, &repo, "recover-one", "fenced", true);
    let path = prepared["path"].as_str().unwrap();
    let receipt = fs::read(rig.dir.join("state/preparation-receipts/recover-one.json")).unwrap();
    let (code, ack) = rig.json(&[
        "host",
        "prepared",
        "--input",
        &rig.disposition_request("recover-one"),
    ]);
    assert_eq!(code, 0, "{ack}");
    assert_eq!(ack["released"], true);
    assert_eq!(ack["policy"]["kind"], "prohibit");
    let calls = rig.log("herdr.log");
    let setup = rig.log("setup.log");
    let (code, error) = rig.json(&[
        "host",
        "prepare",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "fenced",
        "--base",
        "main",
        "--request-id",
        "recover-one",
    ]);
    assert_ne!(code, 0, "{error}");
    // A replacement with no marker cannot acquire a generation through the old request.
    close::git(&repo, &["worktree", "remove", "-f", "-f", path]);
    close::git(&repo, &["worktree", "add", path, "fenced"]);
    let admin =
        PathBuf::from(close::git(Path::new(path), &["rev-parse", "--absolute-git-dir"]).trim());
    assert!(!admin.join("dogsled-checkout-id").exists());
    let mut launch = host_request(&rig);
    launch["cwd"] = prepared["path"].clone();
    launch["pane"] = prepared["pane_id"].clone();
    launch["terminal"] = prepared["terminal_id"].clone();
    launch["preparation"] = prepared["binding"].clone();
    launch["preparation_protocol"] = json!(1);
    let (code, error) = rig.json(&["host", "launch", "--request", &launch.to_string()]);
    assert_ne!(code, 0, "{error}");
    assert!(!admin.join("dogsled-checkout-id").exists());
    assert_eq!(rig.log("herdr.log"), calls);
    assert_eq!(rig.log("setup.log"), setup);
    assert_eq!(rig.read("state/preparations.json"), json!([]));
    assert_eq!(
        fs::read(rig.dir.join("state/preparation-receipts/recover-one.json")).unwrap(),
        receipt
    );
    assert!(
        !rig.dir
            .join("state/launches/recover-one.progress.json")
            .exists()
    );
}

#[test]
fn disposition_recovers_fence_before_release_and_release_before_ack() {
    for point in ["preparation-fenced", "preparation-claim-released"] {
        let (rig, repo) = place::place_rig(point);
        disposition_prepared(&rig, &repo, "a", "shared", true);
        disposition_prepared(&rig, &repo, "b", "shared", false);
        let claims = rig.read("state/preparations.json");
        let other = claims
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["request_id"] == "b")
            .unwrap()
            .clone();
        let outcome = fs::read(rig.dir.join("state/preparation-receipts/a.json")).unwrap();
        let args = ["host", "prepared", "--input", &rig.disposition_request("a")];
        let out = rig
            .cmd(&args)
            .env("DOGSLED_TEST_UNIT_CRASH", point)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(86),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let fence = rig.read("state/preparation-receipts/a.disposition.json");
        assert_eq!(fence["progress"], "fenced");
        if point == "preparation-fenced" {
            assert_eq!(rig.read("state/preparations.json"), claims);
        } else {
            assert_eq!(rig.read("state/preparations.json"), json!([other.clone()]));
        }
        let (code, ack) = rig.json(&args);
        assert_eq!(code, 0, "{ack}");
        assert_eq!(ack["released"], true);
        assert_eq!(ack["binding"], fence["request"]["binding"]);
        assert_eq!(rig.json(&args), (0, ack));
        let released = rig.read("state/preparation-receipts/a.disposition.json");
        assert_eq!(released["request"], fence["request"]);
        assert_eq!(released["progress"], "released");
        assert_eq!(rig.read("state/preparations.json"), json!([other]));
        assert_eq!(
            fs::read(rig.dir.join("state/preparation-receipts/a.json")).unwrap(),
            outcome
        );
    }
}

#[test]
fn historical_preparation_replays_final_launch_without_releasing_claim() {
    let (rig, repo) = place::place_rig("historical-final-replay");
    let prepared = disposition_prepared(&rig, &repo, "recover-one", "historical", true);
    let mut launch = host_request(&rig);
    launch["cwd"] = prepared["path"].clone();
    launch["pane"] = prepared["pane_id"].clone();
    launch["terminal"] = prepared["terminal_id"].clone();
    launch["preparation"] = prepared["binding"].clone();
    launch["preparation_protocol"] = json!(1);
    let request = launch.to_string();
    let args = ["host", "launch", "--request", &request];
    let out = rig
        .cmd(&args)
        .env("DOGSLED_TEST_UNIT_CRASH", "preparation-before-fence")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(86), "{out:?}");
    let outcome_path = "state/preparation-receipts/recover-one.json";
    let current = rig.read(outcome_path);
    assert_eq!(current["version"], 1);
    // Main's pre-disposition format had only these outcome fields.
    let outcome = json!({
        "request": current["request"],
        "prepared": current["prepared"],
        "failure": current["failure"],
    });
    rig.put(outcome_path, &outcome);
    // Historical launch records also predate transported preparation authority.
    launch.as_object_mut().unwrap().remove("preparation");
    let progress_file = "state/launches/recover-one.progress.json";
    let mut progress = rig.read(progress_file);
    progress["request"]
        .as_object_mut()
        .unwrap()
        .remove("preparation");
    rig.put(progress_file, &progress);
    let request = launch.to_string();
    let args = ["host", "launch", "--request", &request];
    let claims = rig.read("state/preparations.json");
    let receipt = rig.read("state/launches/recover-one.json");
    let calls = rig.log("herdr.log");
    assert_eq!(rig.json(&args), (0, receipt));
    assert_eq!(rig.read("state/preparations.json"), claims);
    assert_eq!(rig.read(outcome_path), outcome);
    assert!(
        !rig.dir
            .join("state/preparation-receipts/recover-one.disposition.json")
            .exists()
    );
    assert_eq!(rig.log("herdr.log"), calls);
}

#[test]
fn versioned_preparation_without_binding_refuses_final_launch_replay() {
    let (rig, repo) = place::place_rig("versioned-final-replay");
    let prepared = disposition_prepared(&rig, &repo, "recover-one", "corrupt", true);
    let mut launch = host_request(&rig);
    launch["cwd"] = prepared["path"].clone();
    launch["pane"] = prepared["pane_id"].clone();
    launch["terminal"] = prepared["terminal_id"].clone();
    launch["preparation"] = prepared["binding"].clone();
    launch["preparation_protocol"] = json!(1);
    let request = launch.to_string();
    let args = ["host", "launch", "--request", &request];
    let out = rig
        .cmd(&args)
        .env("DOGSLED_TEST_UNIT_CRASH", "preparation-before-fence")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(86), "{out:?}");
    let path = "state/preparation-receipts/recover-one.json";
    let mut outcome = rig.read(path);
    outcome["version"] = json!(1);
    outcome.as_object_mut().unwrap().remove("binding");
    rig.put(path, &outcome);
    let claims = rig.read("state/preparations.json");
    let receipt = rig.read("state/launches/recover-one.json");
    let calls = rig.log("herdr.log");
    let out = rig.run(&args);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("missing its binding"),
        "{out:?}"
    );
    assert_eq!(rig.read("state/preparations.json"), claims);
    assert_eq!(rig.read("state/launches/recover-one.json"), receipt);
    assert_eq!(rig.read(path), outcome);
    assert!(
        !rig.dir
            .join("state/preparation-receipts/recover-one.disposition.json")
            .exists()
    );
    assert_eq!(rig.log("herdr.log"), calls);
}

#[test]
fn over_budget_agent_refusal_releases_preparation_before_dispatch_and_on_replay() {
    for interrupted in [false, true] {
        let (rig, repo) = place::place_rig(&format!("budget-agent-{interrupted}"));
        let prepared = disposition_prepared(&rig, &repo, "recover-one", "budget", true);
        let mut launch = host_request(&rig);
        launch["cwd"] = prepared["path"].clone();
        launch["pane"] = prepared["pane_id"].clone();
        launch["terminal"] = prepared["terminal_id"].clone();
        launch["preparation"] = prepared["binding"].clone();
        launch["preparation_protocol"] = json!(1);
        launch["action"]["name"] = json!("n".repeat(1000));
        let request = launch.to_string();
        let args = ["host", "launch", "--request", &request];
        let mut command = rig.cmd(&args);
        if interrupted {
            command.env("DOGSLED_TEST_UNIT_CRASH", "preflight-refusal-persisted");
        }
        let out = command.output().unwrap();
        assert_eq!(
            out.status.code(),
            Some(if interrupted { 86 } else { 2 }),
            "{out:?}"
        );
        let calls = rig.log("herdr.log");
        assert!(!calls.contains("agent start "));
        let outcome = rig.read("state/preparation-receipts/recover-one.json");
        let out = rig.run(&args);
        assert_eq!(out.status.code(), Some(2), "{out:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("960-byte budget"),
            "{out:?}"
        );
        assert_eq!(rig.read("state/preparations.json"), json!([]));
        assert_eq!(
            rig.read("state/preparation-receipts/recover-one.json"),
            outcome
        );
        assert_eq!(
            rig.read("state/preparation-receipts/recover-one.disposition.json")["progress"],
            "released"
        );
        assert_eq!(rig.run(&args).status.code(), Some(2));
        assert_eq!(rig.read("state/preparations.json"), json!([]));
        assert!(!rig.log("herdr.log").contains("agent start "));
    }
}

#[test]
fn released_claim_replay_acknowledges_without_repository() {
    let (rig, repo) = place::place_rig("released-replay-no-repository");
    let prepared = disposition_prepared(&rig, &repo, "recover-one", "released", true);
    let mut launch = host_request(&rig);
    launch["cwd"] = prepared["path"].clone();
    launch["pane"] = prepared["pane_id"].clone();
    launch["terminal"] = prepared["terminal_id"].clone();
    launch["preparation"] = prepared["binding"].clone();
    launch["preparation_protocol"] = json!(1);
    let request = launch.to_string();
    let args = ["host", "launch", "--request", &request];
    let out = rig
        .cmd(&args)
        .env("DOGSLED_TEST_UNIT_CRASH", "preparation-claim-released")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(86), "{out:?}");
    assert_eq!(rig.read("state/preparations.json"), json!([]));
    let fence = rig.read("state/preparation-receipts/recover-one.disposition.json");
    assert_eq!(fence["progress"], "fenced");
    let receipt = rig.read("state/launches/recover-one.json");
    let calls = rig.log("herdr.log");
    fs::remove_dir_all(&repo).unwrap();
    assert_eq!(rig.json(&args), (0, receipt));
    let released = rig.read("state/preparation-receipts/recover-one.disposition.json");
    assert_eq!(released["request"], fence["request"]);
    assert_eq!(released["progress"], "released");
    assert_eq!(rig.read("state/preparations.json"), json!([]));
    assert_eq!(rig.log("herdr.log"), calls);
}

#[test]
fn agent_receipt_replay_completes_fenced_preparation_transfer() {
    for point in [
        "preparation-before-fence",
        "preparation-fenced",
        "preparation-claim-released",
    ] {
        let (rig, repo) = place::place_rig(&format!("agent-transfer-{point}"));
        let prepared = disposition_prepared(&rig, &repo, "recover-one", "transferred", true);
        let mut launch = host_request(&rig);
        launch["cwd"] = prepared["path"].clone();
        launch["pane"] = prepared["pane_id"].clone();
        launch["terminal"] = prepared["terminal_id"].clone();
        launch["preparation"] = prepared["binding"].clone();
        launch["preparation_protocol"] = json!(1);
        let request = launch.to_string();
        let args = ["host", "launch", "--request", &request];
        let out = rig
            .cmd(&args)
            .env("DOGSLED_TEST_UNIT_CRASH", point)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(86), "{out:?}");
        let fence = if point == "preparation-before-fence" {
            assert!(
                !rig.dir
                    .join("state/preparation-receipts/recover-one.disposition.json")
                    .exists()
            );
            None
        } else {
            let fence = rig.read("state/preparation-receipts/recover-one.disposition.json");
            assert_eq!(fence["progress"], "fenced");
            Some(fence)
        };
        if point == "preparation-fenced" {
            assert_eq!(
                rig.read("state/preparations.json")
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
        }
        let receipt = rig.read("state/launches/recover-one.json");
        let calls = rig.log("herdr.log");
        for _ in 0..2 {
            let (code, replay) = rig.json(&args);
            assert_eq!(code, 0, "{replay}");
            assert_eq!(replay, receipt);
            assert_eq!(rig.read("state/preparations.json"), json!([]));
            let transferred = rig.read("state/preparation-receipts/recover-one.disposition.json");
            assert_eq!(transferred["progress"], "released");
            if let Some(fence) = &fence {
                assert_eq!(transferred["request"], fence["request"]);
            }
            assert_eq!(transferred["request"]["policy"]["kind"], "transfer");
            assert_eq!(rig.log("herdr.log"), calls);
        }
    }
}

#[test]
fn prepared_agent_refusal_releases_only_known_not_started_claims() {
    for final_refusal in [true, false] {
        let (rig, repo) = place::place_rig(if final_refusal {
            "prepared-refusal"
        } else {
            "prepared-uncertain"
        });
        let prepared = disposition_prepared(&rig, &repo, "recover-one", "refusal", true);
        let mut launch = host_request(&rig);
        launch["cwd"] = prepared["path"].clone();
        launch["pane"] = prepared["pane_id"].clone();
        launch["terminal"] = prepared["terminal_id"].clone();
        launch["preparation"] = prepared["binding"].clone();
        launch["preparation_protocol"] = json!(1);
        if final_refusal {
            let script = fs::read_to_string(rig.dir.join("bin/herdr")).unwrap();
            rig.script("herdr", &script.replace("  'agent start'*)\n", "  'agent start'*) echo '{\"error\":{\"code\":\"agent_name_taken\",\"message\":\"already used\"}}' >&2; exit 1 ;;\n  'unused start'*)\n"));
        } else {
            fs::write(rig.dir.join("start-fails"), "").unwrap();
        }
        let pending = rig.read("state/preparations.json");
        let request = launch.to_string();
        let args = ["host", "launch", "--request", &request];
        for _ in 0..2 {
            let (code, error) = rig.json(&args);
            assert_ne!(code, 0, "{error}");
            if final_refusal {
                assert_eq!(rig.read("state/preparations.json"), json!([]));
                let disposition =
                    rig.read("state/preparation-receipts/recover-one.disposition.json");
                assert_eq!(disposition["progress"], "released");
                assert_eq!(
                    disposition["request"]["policy"]["basis"]["agent_refused"]["launch"],
                    launch
                );
                assert_eq!(disposition["request"]["outcome"], "not_started");
            } else {
                assert_eq!(rig.read("state/preparations.json"), pending);
                assert!(
                    !rig.dir
                        .join("state/preparation-receipts/recover-one.disposition.json")
                        .exists()
                );
            }
        }
        assert_eq!(
            rig.log("herdr.log").matches("agent start worker ").count(),
            1
        );
    }
}

#[test]
fn unsupported_preparation_disposition_refuses_job_ingress_without_writes() {
    let (rig, repo) = place::place_rig("disposition-ingress");
    let prepared = disposition_prepared(&rig, &repo, "ingress", "ingress", true);
    let path = prepared["path"].as_str().unwrap();
    assert_eq!(
        rig.json(&[
            "host",
            "prepared",
            "--input",
            &rig.disposition_request("ingress")
        ])
        .0,
        0
    );
    let disposition = "state/preparation-receipts/ingress.disposition.json";
    let mut unsupported = rig.read(disposition);
    unsupported["request"]["version"] = json!(2);
    rig.put(disposition, &unsupported);
    let original = fs::read(rig.dir.join(disposition)).unwrap();
    let herdr = rig.log("herdr.log");
    let mut request = job_launch_request(&rig, "ingress", &["true"]);
    request["cwd"] = json!(path);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    assert_eq!(fs::read(rig.dir.join(disposition)).unwrap(), original);
    assert!(!rig.dir.join("state/job-validity/ingress.json").exists());
    assert!(!rig.dir.join("state/job-validity-clock.json").exists());
    assert!(!rig.dir.join("state/jobs/ingress.json").exists());
    assert!(!rig.dir.join("state/launches/ingress.json").exists());
    assert_eq!(rig.log("herdr.log"), herdr);
}

#[test]
fn preparation_failure_is_durable_before_ownership_release() {
    let (rig, repo) = place::place_rig("disposition-failure");
    fs::write(rig.dir.join("setup-fails"), "").unwrap();
    let args = [
        "host",
        "prepare",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "failed",
        "--base",
        "main",
        "--request-id",
        "failed",
    ];
    let out = rig
        .cmd(&args)
        .env("DOGSLED_TEST_UNIT_CRASH", "preparation-failure-recorded")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(86),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!rig.read("state/preparation-receipts/failed.json")["failure"].is_null());
    assert_eq!(
        rig.read("state/preparations.json")
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        !rig.dir
            .join("state/preparation-receipts/failed.disposition.json")
            .exists()
    );
    let setup = rig.log("setup.log");
    let (code, error) = rig.json(&args);
    assert_ne!(code, 0, "{error}");
    assert_eq!(rig.read("state/preparations.json"), json!([]));
    assert_eq!(
        rig.read("state/preparation-receipts/failed.disposition.json")["progress"],
        "released"
    );
    assert_eq!(rig.log("setup.log"), setup);
}

/// The test process holds the same kernel mutex that separate host processes must acquire.
fn repository_lifecycle_lock(repo: &Path) -> fs::File {
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(repo.join(".git/dogsled-lifecycle.lock"))
        .unwrap();
    lock.lock().unwrap();
    lock
}

fn lifecycle_git_barrier(rig: &Rig) -> Fifo {
    let script = fs::read_to_string(rig.dir.join("bin/herdr")).unwrap();
    rig.script("herdr", &script.replace("case \"$*\" in", "case \"$*\" in\n  'tab create'*|'workspace create'*) printf '%s\\n' '{\"result\":{\"root_pane\":{\"pane_id\":\"w8:p1\",\"terminal_id\":\"term_tree\"}}}' ;;\n  'pane run'*) printf '{}\\n' ;;"));
    let ready = Fifo::new(&rig.dir, "lifecycle-git-ready");
    let out = isolated("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let git = String::from_utf8(out.stdout).unwrap();
    rig.script("git", &format!(r#"#!/bin/sh
case "$*" in
  *'rev-parse --path-format=absolute --git-common-dir') printf 'ready\n' > "$RIG/lifecycle-git-ready" ;;
esac
exec '{}' "$@"
"#, git.trim()));
    ready
}

#[test]
fn prepare_and_launch_wait_for_repository_lifecycle_lock() {
    for kind in ["prepare", "agent", "job"] {
        let (rig, repo) = place::place_rig(&format!("lifecycle-lock-{kind}"));
        let ready = lifecycle_git_barrier(&rig);
        let _ready_hold = ready.hold();
        let lock = repository_lifecycle_lock(&repo);
        let mut request = if kind == "job" {
            job_launch_request(&rig, "c1-wait-job", &["true"])
        } else {
            host_request(&rig)
        };
        request["cwd"] = json!(repo);
        let text = request.to_string();
        let arguments = if kind == "prepare" {
            vec![
                "--json",
                "--budget-ms",
                "1500",
                "host",
                "prepare",
                "--repo",
                repo.to_str().unwrap(),
                "--branch",
                "c1-wait",
                "--base",
                "main",
                "--request-id",
                "c1-wait",
            ]
        } else {
            vec![
                "--json",
                "--budget-ms",
                "1500",
                "host",
                "launch",
                "--job-validity",
                "2",
                "--request",
                &text,
            ]
        };
        let mut contender = rig.start(&arguments);
        read_fifo(ready.path(), "repository lookup");
        let status = contender.wait();
        let (stdout, stderr) = contender.output();
        assert!(!status.success(), "{kind}: {stdout} {stderr}");
        let error: Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(error["deadline"], true, "{kind}: {error}");
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("dogsled-lifecycle.lock"),
            "{kind}: {error}"
        );
        assert!(!rig.dir.join("setup.log").exists());
        assert!(!rig.dir.join("state/preparations.json").exists());
        assert!(!rig.dir.join("state/jobs/c1-wait-job.json").exists());
        let log = rig.log("herdr.log");
        for effect in [
            "worktree open",
            "tab create",
            "workspace create",
            "pane run",
            "agent start",
        ] {
            assert!(!log.contains(effect), "{kind}: {log}");
        }
        lock.unlock().unwrap();
        // A new preparation request can proceed; failed receipt replay remains read-only.
        let mut released = arguments.clone();
        released[2] = "10000";
        if kind == "prepare" {
            *released.last_mut().unwrap() = "c1-after-release";
        }
        let mut contender = rig.start(&released);
        read_fifo(ready.path(), "released repository lookup");
        let status = contender.wait();
        let (stdout, stderr) = contender.output();
        assert!(status.success(), "{kind}: {stdout} {stderr}");
    }
}

fn replace_unmarked_checkout(repo: &Path, tree: &Path, branch: &str) -> PathBuf {
    close::git(
        repo,
        &["worktree", "remove", "-f", "-f", tree.to_str().unwrap()],
    );
    close::git(
        repo,
        &["worktree", "add", "-q", tree.to_str().unwrap(), branch],
    );
    let admin = PathBuf::from(close::git(tree, &["rev-parse", "--absolute-git-dir"]).trim());
    assert!(!admin.join("dogsled-checkout-id").exists());
    admin
}

fn start_gate_disposition(rig: &Rig, input: &str, name: &str) -> Owned {
    let mut command = rig.cmd(&[
        "--json",
        "--budget-ms",
        "15000",
        "host",
        "prepared",
        "--input",
        input,
    ]);
    command.env(
        "DOGSLED_TEST_LAUNCH_DISPOSITION_BEFORE_GATE",
        rig.dir.join(name),
    );
    command.stdout(fs::File::create(rig.dir.join(format!("{name}-output"))).unwrap());
    command.stderr(fs::File::create(rig.dir.join(format!("{name}-error"))).unwrap());
    Owned::spawn(&mut command)
}

#[test]
fn disposition_and_dispatch_obey_both_gate_orders() {
    for release_first in [true, false] {
        let (rig, repo) = short_disposition_rig(if release_first {
            "release-first"
        } else {
            "dispatch-first"
        });
        let prepared = disposition_prepared(&rig, &repo, "recover-one", "ordered", true);
        let mut launch = host_request(&rig);
        launch["preparation"] = prepared["binding"].clone();
        launch["preparation_protocol"] = json!(1);
        launch["cwd"] = prepared["path"].clone();
        launch["pane"] = prepared["pane_id"].clone();
        launch["terminal"] = prepared["terminal_id"].clone();
        let input = rig.disposition_request("recover-one");
        let (release_ready, release_go) = launch_gate_barriers(&rig, "release");
        let _release_ready = release_ready.hold();
        let _release_go = release_go.hold();
        let (launch_ready, launch_go) = launch_gate_barriers(&rig, "dispatch");
        let _launch_ready = launch_ready.hold();
        let _launch_go = launch_go.hold();
        let phase = if release_first {
            "BEFORE_GATE"
        } else {
            "ENTERED_GATE"
        };
        let mut dispatch = start_gate_launch(&rig, &launch, "dispatch", &[(phase, "dispatch")]);
        read_fifo(launch_ready.path(), "dispatch barrier");
        let mut release = start_gate_disposition(&rig, &input, "release");
        read_fifo(release_ready.path(), "disposition ingress barrier");
        if release_first {
            // Disposition owns the preparation gate while it waits for repository protection.
            let git_ready = lifecycle_git_barrier(&rig);
            let _git_ready = git_ready.hold();
            let repository = repository_lifecycle_lock(&repo);
            writeln!(release_go.hold(), "go").unwrap();
            read_fifo(
                git_ready.path(),
                "disposition entered preparation gate before repository",
            );
            probe_free_lock(&rig.dir.join("state/done.lock"));
            probe_free_lock(&rig.dir.join("state/launches/recover-one.lock"));
            writeln!(launch_go.hold(), "go").unwrap();
            repository.unlock().unwrap();
            assert!(release.wait().success(), "{}", rig.log("release-error"));
            assert!(!dispatch.wait().success(), "{}", rig.log("dispatch-output"));
            assert_eq!(
                rig.read("state/preparation-receipts/recover-one.disposition.json")["request"]["policy"]
                    ["kind"],
                "prohibit"
            );
            assert!(!rig.log("herdr.log").contains("agent start"));
        } else {
            // The already-entered dispatch gate wins; the delayed prohibition cannot replace it.
            writeln!(release_go.hold(), "go").unwrap();
            probe_free_lock(&rig.dir.join("state/done.lock"));
            probe_free_lock(&rig.dir.join("state/launches/recover-one.lock"));
            writeln!(launch_go.hold(), "go").unwrap();
            assert!(dispatch.wait().success(), "{}", rig.log("dispatch-error"));
            assert!(!release.wait().success(), "{}", rig.log("release-output"));
            let saved = rig.read("state/launches/recover-one.json");
            let fence = rig.read("state/preparation-receipts/recover-one.disposition.json");
            assert_eq!(fence["request"]["policy"]["kind"], "transfer");
            assert_eq!(fence["request"]["outcome"], "possibly_executed");
            let transfer = fence["request"].to_string();
            assert_eq!(rig.json(&["host", "prepared", "--input", &transfer]).0, 0);
            assert_eq!(
                rig.json(&["host", "launch", "--request", &launch.to_string()]),
                (0, saved)
            );
            let mut other = launch.clone();
            other["owner"] = json!("foreign");
            assert_eq!(
                rig.json(&["host", "launch", "--request", &other.to_string()])
                    .0,
                2
            );
            assert_eq!(
                rig.log("herdr.log").matches("agent start worker ").count(),
                1
            );
        }
        assert_eq!(rig.read("state/preparations.json"), json!([]));
    }
}

fn launch_gate_barriers(rig: &Rig, name: &str) -> (Fifo, Fifo) {
    (
        Fifo::new(&rig.dir, &format!("{name}.ready")),
        Fifo::new(&rig.dir, &format!("{name}.go")),
    )
}

fn start_gate_launch(rig: &Rig, request: &Value, name: &str, barriers: &[(&str, &str)]) -> Owned {
    let mut command = rig.cmd(&[
        "--json",
        "--budget-ms",
        "15000",
        "host",
        "launch",
        "--request",
        &request.to_string(),
    ]);
    for (phase, barrier) in barriers {
        command.env(
            format!("DOGSLED_TEST_LAUNCH_{phase}"),
            rig.dir.join(barrier),
        );
    }
    command.stdout(fs::File::create(rig.dir.join(format!("{name}-output"))).unwrap());
    command.stderr(fs::File::create(rig.dir.join(format!("{name}-error"))).unwrap());
    Owned::spawn(&mut command)
}

#[test]
fn launch_rechecks_final_ownership_after_waiting_for_preparation_gate() {
    for complete in [false, true] {
        let (rig, repo) = place::place_rig(&format!("same-request-gate-{complete}"));
        let git_ready = lifecycle_git_barrier(&rig);
        let _git_ready_hold = git_ready.hold();
        let tree = repo.join(".worktrees/same-request");
        close::git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "same-request",
                tree.to_str().unwrap(),
            ],
        );
        let mut request = host_request(&rig);
        request["cwd"] = json!(tree);
        let (entered, enter_go) = launch_gate_barriers(&rig, "first");
        let (completed, complete_go) = launch_gate_barriers(&rig, "completed");
        let (second_ready, second_go) = launch_gate_barriers(&rig, "second");
        let _entered_hold = entered.hold();
        let _completed_hold = completed.hold();
        let _second_hold = second_ready.hold();
        let _enter_go_hold = enter_go.hold();
        let _complete_go_hold = complete_go.hold();
        let _second_go_hold = second_go.hold();
        let mut first = start_gate_launch(
            &rig,
            &request,
            "first",
            &[("ENTERED_GATE", "first"), ("COMPLETED_GATE", "completed")],
        );
        read_fifo(entered.path(), "first launch owns preparation gate");
        let prep = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(rig.dir.join("state/preparation-receipts/recover-one.lock"))
            .unwrap();
        assert!(matches!(prep.try_lock(), Err(fs::TryLockError::WouldBlock)));
        // An outer preparation wait never holds done, repository, or launch protection.
        probe_free_lock(&rig.dir.join("state/launches/recover-one.lock"));
        probe_free_lock(&rig.dir.join("state/done.lock"));
        let repository = repository_lifecycle_lock(&repo);
        repository.unlock().unwrap();
        let mut second = start_gate_launch(&rig, &request, "second", &[("BEFORE_GATE", "second")]);
        read_fifo(
            second_ready.path(),
            "second launch observed no final receipt before gate",
        );
        writeln!(second_go.hold(), "go").unwrap();
        if !complete {
            fs::write(rig.dir.join("start-fails"), "").unwrap();
        }
        writeln!(enter_go.hold(), "go").unwrap();
        read_fifo(
            completed.path(),
            "first launch saved its outcome while retaining preparation gate",
        );
        assert!(matches!(prep.try_lock(), Err(fs::TryLockError::WouldBlock)));
        let admin = replace_unmarked_checkout(&repo, &tree, "same-request");
        fs::write(tree.join("replacement"), "keep").unwrap();
        assert!(!admin.join("dogsled-checkout-id").exists());
        writeln!(complete_go.hold(), "go").unwrap();
        assert_eq!(
            first.wait().success(),
            complete,
            "{}",
            rig.log("first-error")
        );
        assert_eq!(
            second.wait().success(),
            complete,
            "{}",
            rig.log("second-error")
        );
        if complete {
            let first: Value = serde_json::from_str(&rig.log("first-output")).unwrap();
            let second: Value = serde_json::from_str(&rig.log("second-output")).unwrap();
            assert_eq!(second, first);
        } else {
            assert!(rig.log("second-output").contains("identity changed"));
        }
        assert!(!admin.join("dogsled-checkout-id").exists());
        assert_eq!(
            fs::read_to_string(tree.join("replacement")).unwrap(),
            "keep"
        );
        assert_eq!(
            rig.log("herdr.log").matches("agent start worker ").count(),
            1
        );
    }
}

#[test]
fn preparation_gate_budget_refuses_second_launch_without_inner_locks_or_side_effects() {
    let (rig, repo) = place::place_rig("preparation-gate-budget");
    let git_ready = lifecycle_git_barrier(&rig);
    let _git_ready_hold = git_ready.hold();
    let mut request = host_request(&rig);
    request["cwd"] = json!(repo);
    let (ready, go) = launch_gate_barriers(&rig, "first");
    let _ready_hold = ready.hold();
    let _go_hold = go.hold();
    let mut first = start_gate_launch(&rig, &request, "first", &[("ENTERED_GATE", "first")]);
    read_fifo(ready.path(), "first launch preparation gate");
    let (code, error) = rig.json(&[
        "--budget-ms",
        "500",
        "host",
        "launch",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    assert_eq!(error["deadline"], true);
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("preparation-receipts/recover-one.lock"),
        "{error}"
    );
    probe_free_lock(&rig.dir.join("state/launches/recover-one.lock"));
    probe_free_lock(&rig.dir.join("state/done.lock"));
    let repository = repository_lifecycle_lock(&repo);
    repository.unlock().unwrap();
    assert!(
        !rig.dir
            .join("state/launches/recover-one.progress.json")
            .exists()
    );
    assert!(!rig.log("herdr.log").contains("agent start"));
    writeln!(go.hold(), "go").unwrap();
    assert!(first.wait().success(), "{}", rig.log("first-error"));
    assert_eq!(
        rig.log("herdr.log").matches("agent start worker ").count(),
        1
    );
}

#[test]
fn non_repository_launch_skips_repository_protection() {
    let rig = Rig::new("launch-outside-repository");
    let request = host_request(&rig);
    let (code, result) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 0, "{result}");
    assert_eq!(
        rig.log("herdr.log").matches("agent start worker ").count(),
        1
    );
    assert!(!rig.dir.join("home/dogsled-lifecycle.lock").exists());
    assert!(!rig.dir.join("state/dogsled-lifecycle.lock").exists());
    assert!(!rig.dir.join("home/.git").exists());
    assert!(result.get("checkout").is_none());
}

fn probe_free_lock(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    file.try_lock().unwrap_or_else(|error| {
        panic!(
            "{} was held before repository protection: {error}",
            path.display()
        )
    });
    file.unlock().unwrap();
}

#[test]
fn repository_lock_precedes_launch_and_store_locks() {
    for prepare in [true, false] {
        let (rig, repo) = place::place_rig(&format!("lifecycle-order-{prepare}"));
        let ready = lifecycle_git_barrier(&rig);
        let _ready_hold = ready.hold();
        let lock = repository_lifecycle_lock(&repo);
        let mut request = host_request(&rig);
        request["cwd"] = json!(repo);
        let text = request.to_string();
        let arguments = if prepare {
            vec![
                "--json",
                "--budget-ms",
                "1500",
                "host",
                "prepare",
                "--repo",
                repo.to_str().unwrap(),
                "--branch",
                "c1-order",
                "--base",
                "main",
                "--request-id",
                "c1-order",
            ]
        } else {
            vec![
                "--json",
                "--budget-ms",
                "1500",
                "host",
                "launch",
                "--request",
                &text,
            ]
        };
        let mut contender = rig.start(&arguments);
        read_fifo(ready.path(), "lock-order repository lookup");
        for name in [
            "done.lock",
            "preparations.lock",
            "worktrees.lock",
            "jobs/c1-order.lock",
            "launches/recover-one.lock",
        ] {
            probe_free_lock(&rig.dir.join("state").join(name));
        }
        let status = contender.wait();
        let (stdout, stderr) = contender.output();
        assert!(!status.success(), "{stdout} {stderr}");
        let error: Value = serde_json::from_str(&stdout).unwrap();
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("dogsled-lifecycle.lock"),
            "{error}"
        );
        lock.unlock().unwrap();
        let mut released = arguments.clone();
        released[2] = "10000";
        if prepare {
            *released.last_mut().unwrap() = "c1-order-after";
        }
        let mut contender = rig.start(&released);
        read_fifo(ready.path(), "released lock-order lookup");
        let status = contender.wait();
        let (stdout, stderr) = contender.output();
        assert!(status.success(), "{stdout} {stderr}");
    }
}

#[test]
fn incomplete_launch_keeps_saved_checkout_identity_after_preparation_retires() {
    let (rig, repo) = place::place_rig("launch-replacement-after-retirement");
    let ready = lifecycle_git_barrier(&rig);
    let _ready_hold = ready.hold();
    let (code, prepared) = rig.json(&[
        "host",
        "prepare",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "c1-recovery",
        "--base",
        "main",
        "--request-id",
        "recover-one",
    ]);
    assert_eq!(code, 0, "{prepared}");
    let tree = repo.join(".worktrees/c1-recovery");
    let mut request = host_request(&rig);
    request["cwd"] = json!(tree);
    request["pane"] = prepared["pane_id"].clone();
    request["terminal"] = prepared["terminal_id"].clone();
    request["preparation"] = prepared["binding"].clone();
    request["preparation_protocol"] = json!(1);
    fs::write(rig.dir.join("start-fails"), "").unwrap();
    assert_ne!(
        rig.json(&["host", "launch", "--request", &request.to_string()])
            .0,
        0
    );
    let saved = rig.read("state/launches/recover-one.progress.json");
    rig.put("state/preparations.json", &json!([]));
    close::git(
        &repo,
        &["worktree", "remove", "-f", "-f", tree.to_str().unwrap()],
    );
    close::git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            tree.to_str().unwrap(),
            "c1-recovery",
        ],
    );
    let admin = PathBuf::from(close::git(&tree, &["rev-parse", "--absolute-git-dir"]).trim());
    fs::write(
        admin.join("dogsled-checkout-id"),
        "11111111111111111111111111111111\n",
    )
    .unwrap();
    fs::write(tree.join("replacement"), "keep").unwrap();
    let (code, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_ne!(code, 0, "{error}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("identity changed"),
        "{error}"
    );
    assert_eq!(rig.read("state/launches/recover-one.progress.json"), saved);
    assert_eq!(
        fs::read_to_string(tree.join("replacement")).unwrap(),
        "keep"
    );
    assert_eq!(
        rig.log("herdr.log").matches("agent start worker ").count(),
        1
    );
}

#[test]
fn main_checkout_launch_recovery_preserves_the_same_captured_identity() {
    let (rig, repo) = place::place_rig("launch-main-recovery");
    let ready = lifecycle_git_barrier(&rig);
    let _ready_hold = ready.hold();
    let mut request = host_request(&rig);
    request["cwd"] = json!(repo);
    fs::write(rig.dir.join("start-fails"), "").unwrap();
    assert_ne!(
        rig.json(&["host", "launch", "--request", &request.to_string()])
            .0,
        0
    );
    let captured =
        rig.read("state/launches/recover-one.progress.json")["result"]["checkout"].clone();
    assert_eq!(
        captured["identity"]["common"],
        captured["identity"]["gitdir"]
    );
    assert!(captured["identity"]["generation"].is_string(), "{captured}");
    rig.put("agents.json", &json!({"result":{"agents":[{"pane_id":"w8:p1","terminal_id":"term_tree","name":"worker","agent":"pi","agent_status":"idle"}]}}));
    let (code, recovered) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 0, "{recovered}");
    assert_eq!(recovered["checkout"], captured);
    assert_eq!(
        rig.log("herdr.log").matches("agent start worker ").count(),
        1
    );
}

#[test]
fn unborn_main_checkout_recovers_an_interrupted_same_request_launch() {
    let (rig, _) = place::place_rig("launch-unborn-recovery");
    let repo = rig.dir.join("home/unborn");
    fs::create_dir_all(&repo).unwrap();
    close::git(&repo, &["init", "-q", "-b", "main"]);
    let ready = lifecycle_git_barrier(&rig);
    let _ready_hold = ready.hold();
    let mut request = host_request(&rig);
    request["cwd"] = json!(repo);
    fs::write(rig.dir.join("start-fails"), "").unwrap();
    let (code, failed) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_ne!(code, 0, "{failed}");
    let captured =
        rig.read("state/launches/recover-one.progress.json")["result"]["checkout"].clone();
    assert_eq!(captured["head"], "", "{captured}");
    assert_eq!(captured["branch"], "main", "{captured}");
    assert_eq!(
        std::path::Path::new(captured["identity"]["gitdir"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        repo.join(".git").canonicalize().unwrap()
    );
    assert!(captured["identity"]["generation"].is_string(), "{captured}");
    rig.put("agents.json", &json!({"result":{"agents":[{"pane_id":"w8:p1","terminal_id":"term_tree","name":"worker","agent":"pi","agent_status":"idle"}]}}));
    let (code, recovered) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 0, "{recovered}");
    assert_eq!(recovered["checkout"], captured);
    assert_eq!(
        rig.log("herdr.log").matches("agent start worker ").count(),
        1
    );
}

#[test]
fn setup_failure_preserves_replacement_checkout() {
    let (rig, repo) = place::place_rig("setup-replacement");
    let script = "git -C {main} worktree remove -f -f {path} && git -C {main} worktree add --lock --reason agent:worktree {path} {branch} && printf replacement > {path}/replacement; exit 1";
    fs::write(
        repo.join("dogsled.toml"),
        format!(
            "[worktree]\nsetup = [{}]\n",
            serde_json::to_string(script).unwrap()
        ),
    )
    .unwrap();
    let (code, error) = rig.json(&[
        "host",
        "prepare",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "c1-replace",
        "--base",
        "main",
        "--request-id",
        "c1-replace",
    ]);
    assert_ne!(code, 0, "{error}");
    let path = repo.join(".worktrees/c1-replace");
    assert_eq!(
        fs::read_to_string(path.join("replacement")).unwrap(),
        "replacement"
    );
    assert!(close::git(&repo, &["branch", "--list", "c1-replace"]).contains("c1-replace"));
    assert!(
        close::git(&repo, &["worktree", "list", "--porcelain"]).contains("locked agent:worktree")
    );
    assert!(!rig.log("herdr.log").contains("worktree open"));
}

#[test]
fn handoff_reconciles_removal_before_closed_mark_save() {
    let (rig, tree) = close::close_rig("handoff-prior-removal");
    let script = fs::read_to_string(rig.dir.join("bin/herdr")).unwrap();
    rig.script(
        "herdr",
        &script.replace(
            "case \"$*\" in",
            "case \"$*\" in\n  'agent start'*) printf '{}\\n' ;;",
        ),
    );
    rig.put("panes.json", &json!({"result":{"panes":[{"pane_id":"w2:p1","terminal_id":"term_child","workspace_id":"w2","cwd":tree}]}}));
    let mut request = host_request(&rig);
    request["id"] = json!("c1-handoff");
    request["cwd"] = json!(tree);
    request["pane"] = json!("w2:p1");
    request["terminal"] = json!("term_child");
    let (code, launched) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 0, "{launched}");
    assert!(launched["checkout"]["identity"].is_object(), "{launched}");
    let reply = json!({"seq":10,"text":"sealed native reply","captured_ms":2,"done":true});
    rig.put("state/items.json", &json!([{"id":"c1-handoff","kind":"agent","host":"local","pane":"w2:p1","owner_pane":"w1:p1","name":"worker","created_ms":1,"closed_ms":null}]));
    let mark_key = json!(["c1-handoff", "w2:p1", null]).to_string();
    rig.put(
        "state/done.json",
        &json!({mark_key:{
            "pane":"w2:p1","item":"c1-handoff","after":9,"sealed":10,"marked_ms":1,
            "native_done":true,"reply":reply,
            "branch":{"repo":rig.dir.join("home/repo"),"name":"feat/x","ahead":0,"landed":true}
        }}),
    );
    let (code, removed) = rig.json(&["host", "close", "w2:p1"]);
    assert_eq!(code, 0, "{removed}");
    assert!(!tree.exists());
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_null());
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let marks = rig.read("state/done.json");
    let mark = marks.as_object().unwrap().values().next().unwrap();
    assert!(mark["closed_ms"].is_u64(), "{mark}");
    assert_eq!(mark["reply"], reply);
    assert_eq!(mark["sealed"], 10);
    assert_eq!(mark["native_done"], true);
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_u64());
    assert_eq!(rig.log("hook.log").lines().count(), 1);
}

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
        assert_eq!(
            log.lines()
                .filter(|line| line.contains(&format!("host {step} ")))
                .count(),
            2,
            "{log}"
        );
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
fn queued_placement_validates_branch_under_symlinked_temp_root() {
    let rig = Rig::short("queued-invalid-alias");
    let temp_root = rig.dir.join("tmp-alias");
    std::os::unix::fs::symlink(rig.dir.join("home"), &temp_root).unwrap();
    let repo = temp_root.join("repo");
    repository(&repo);
    assert_ne!(repo, repo.canonicalize().unwrap());
    rig.snapshot("state/host.json", now(), 0.0, 8);
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
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
            .contains("not a valid branch name"),
        "{ticket}"
    );
    assert!(!repo.join(".worktrees").exists());
    assert!(!rig.log("herdr.log").contains("worktree open"));
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
fn expired_agent_preparation_reservations_revalidate_on_the_same_host_and_keep_refusals() {
    let rig = Rig::short("expired-preparation-agent");
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
            .replace("  *'host reserve'*)\n    for remote do :; done\n    sh -c \"$RIG/bin/peer-fixture $remote\" ;;", &reserve)
            .replace("  *'host checkout'*)", "  *'host checkout'*) if ! test -f \"$RIG/failed-once\"; then touch \"$RIG/failed-once\"; echo reset >&2; exit 255; fi;"));
    let (code, queued) = remote_agent(&rig, Some(&repo));
    assert_eq!(code, 1, "{queued}");
    let mut holds = rig.read("peer-state/placements.json");
    holds[0]["created_ms"] = json!(1);
    rig.put("peer-state/placements.json", &holds);
    // Admission now refuses this host despite the caller's eligible cached snapshot.
    let mut snapshot = rig.read("peer-state/host.json");
    snapshot["probe"]["disk_free_percent"] = json!(-1.0);
    rig.put("peer-state/host.json", &snapshot);
    let ticket = advance(&rig);
    assert_eq!(ticket["state"], "pending", "{ticket}");
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
    assert_eq!(ticket["state"], "admitted", "{ticket}");
    assert!(ticket.get("wait_reason").is_none(), "{ticket}");
    assert_eq!(
        rig.read("peer-state/placements.json")
            .as_array()
            .unwrap()
            .len(),
        1
    );
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
        &script.replace("  *'host reserve'*)\n    for remote do :; done\n    sh -c \"$RIG/bin/peer-fixture $remote\" ;;", &handler),
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
    let script = script.replacen("case \"$*\" in", &format!("case \"$*\" in\n{routing}"), 1);
    rig.script("ssh", &script);
    let (code, queued) = remote_agent(&rig, Some(&repo));
    assert_eq!(code, 1, "{queued}");
    assert_eq!(rig.read("state/queue.json")[0]["retry"]["attempts"], 1);
    let ticket = advance(&rig);
    assert_eq!(ticket["state"], "admitted", "{ticket}");
    assert_eq!(rig.log("setup.log").lines().count(), 1);
    assert_eq!(rig.log("herdr.log").matches("worktree open").count(), 1);
    assert_eq!(rig.log("ssh.log").matches("host reserve").count(), 2);
    assert_eq!(rig.log("ssh.log").matches("host prepare ").count(), 2);
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
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
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
        let (code, error) = rig.json(&[
            "host",
            "launch",
            "--job-validity",
            "2",
            "--request",
            &request.to_string(),
        ]);
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
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
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
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
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
        rig.json(&[
            "host",
            "launch",
            "--job-validity",
            "2",
            "--request",
            &request.to_string()
        ])
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
        let (code, error) = rig.json(&[
            "host",
            "launch",
            "--job-validity",
            "2",
            "--request",
            &request.to_string(),
        ]);
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
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
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
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    let message = error["error"].as_str().unwrap();
    assert!(message.contains("agent_name_taken"), "{message}");
    assert!(!message.contains("outcome is unknown"), "{message}");
    let log = rig.log("herdr.log");
    assert!(log.contains("pane close w2:p1"), "{log}");
    // The refusal is the receipt: a resubmission neither allocates nor starts again.
    let (code, again) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
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
    let (code, result) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
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
        let (code, error) = rig.json(&[
            "host",
            "launch",
            "--job-validity",
            "2",
            "--request",
            &request.to_string(),
        ]);
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
            rig.json(&[
                "host",
                "launch",
                "--job-validity",
                "2",
                "--request",
                &request.to_string()
            ])
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
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
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
    let (code, result) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
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
        rig.json(&[
            "host",
            "launch",
            "--job-validity",
            "2",
            "--request",
            &request.to_string()
        ])
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
        rig.json(&[
            "host",
            "launch",
            "--job-validity",
            "2",
            "--request",
            &different.to_string()
        ])
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
    let (_, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
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
    let (_, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
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
    let (_, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
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
        assert_eq!(
            log.lines()
                .filter(|line| line.contains(&format!("{command} ")))
                .count(),
            1,
            "{log}"
        );
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
    let action = json!({"type":"job","cmd":["true"],"timeout_ms":null,"idle_timeout_ms":null,"validity":rig.job_validity()});
    let request = json!({"id":"interrupted-job","action":action,"host":null,"branch":null,"base":null,
        "cwd":rig.dir.join("home"),"repository":null,"workspace":"w1","workspace_name":"caller","owner":"w1:p1"});
    let launch = json!({"id":"interrupted-job","action":action,"host":"local","cwd":rig.dir.join("home"),
        "pane":null,"workspace":"w1","workspace_name":"caller","owner":"w1:p1"});
    fs::create_dir_all(rig.dir.join("state/admissions")).unwrap();
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    rig.put("state/admissions/interrupted-job.json", &json!({"request":request,"host":{"id":"local","label":"local"},"launch":launch,"error":null}));
    let job = json!({"id":"interrupted-job","cmd":["true"],"cwd":rig.dir.join("home"),"host":"local","owner":"w1:p1",
        "pane":"w2:p1","state":"running","exit":null,"signal":null,"reason":null,"started_ms":123,"ended_ms":null,
        "timeout_ms":null,"idle_timeout_ms":null,"validity":rig.job_validity()});
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
    job_launch_request_at(rig, id, cmd, rig.origin_ms)
}

/// The host launch request for a job whose request was created at `created_ms`.
fn job_launch_request_at(rig: &Rig, id: &str, cmd: &[&str], created_ms: u64) -> Value {
    json!({
        "id": id,
        "action": {"type":"job","cmd":cmd,"timeout_ms":null,"idle_timeout_ms":null,
            "validity":{"version":1,"created_ms":created_ms}},
        "host": "local",
        "cwd": rig.dir.join("home"),
        "pane": Value::Null,
        "workspace": "w1",
        "workspace_name": "caller",
        "owner": "w1:p1",
    })
}

/// A host replay whose receipt is missing must not allocate a second pane and must not
/// dispatch the command again when the job already holds a claim. A claim is not a
/// dispatch receipt: without one the replay fails closed.
#[test]
fn a_host_replay_without_a_receipt_does_not_dispatch_an_existing_claim_again() {
    let rig = Rig::new("replay-existing-claim");
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    let job = json!({"id":"replayed","cmd":["true"],"cwd":rig.dir.join("home"),"host":"local","owner":"w1:p1",
        "pane":"w2:p1","state":"pending","exit":null,"signal":null,"reason":null,"started_ms":null,"ended_ms":null,
        "timeout_ms":null,"idle_timeout_ms":null,"validity":rig.job_validity()});
    rig.put("state/jobs/replayed.json", &job);
    let request = job_launch_request(&rig, "replayed", &["true"]);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("no saved launch receipt"),
        "{error}"
    );
    let log = rig.log("herdr.log");
    // No pane was allocated, and the command was never dispatched again.
    assert!(!log.contains("tab create"), "{log}");
    assert!(!log.contains("pane run"), "{log}");
    // The claim keeps its own pane and its own state, and no receipt was invented.
    assert_eq!(rig.read("state/jobs/replayed.json"), job);
    assert!(!rig.dir.join("state/launches/replayed.json").exists());
}

/// A replay with a saved receipt that matches the request and the recorded job replays its
/// result without allocating a pane or dispatching the command again.
#[test]
fn a_host_replay_with_a_matching_receipt_replays_without_side_effects() {
    let rig = Rig::new("replay-matching-receipt");
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    fs::create_dir_all(rig.dir.join("state/launches")).unwrap();
    let job = json!({"id":"replayed","cmd":["true"],"cwd":rig.dir.join("home"),"host":"local","owner":"w1:p1",
        "pane":"w2:p1","state":"pending","exit":null,"signal":null,"reason":null,"started_ms":null,"ended_ms":null,
        "timeout_ms":null,"idle_timeout_ms":null,"validity":rig.job_validity()});
    rig.put("state/jobs/replayed.json", &job);
    rig.put(
        "state/launches/replayed.json",
        &job_receipt(
            &rig,
            "replayed",
            "local",
            "w2:p1",
            &["true"],
            &rig.dir.join("home").display().to_string(),
            "w1:p1",
        ),
    );
    let request = job_launch_request(&rig, "replayed", &["true"]);
    let (code, result) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 0, "{result}");
    assert_eq!(result["pane"], "w2:p1", "{result}");
    let log = rig.log("herdr.log");
    assert!(!log.contains("tab create"), "{log}");
    assert!(!log.contains("pane run"), "{log}");
    assert_eq!(rig.read("state/jobs/replayed.json"), job);
}

/// A legacy receipt still replays while its job record survives. The record carries the
/// same request facts the receipt now keeps, so the older receipt is not a second reader of
/// a different truth: the record is the evidence, and the receipt is the proof of dispatch.
#[test]
fn a_legacy_receipt_replays_while_its_job_record_survives() {
    let rig = Rig::new("legacy-receipt-with-record");
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    fs::create_dir_all(rig.dir.join("state/launches")).unwrap();
    let job = json!({"id":"replayed","cmd":["true"],"cwd":rig.dir.join("home"),"host":"local","owner":"w1:p1",
        "pane":"w2:p1","state":"pending","exit":null,"signal":null,"reason":null,"started_ms":null,"ended_ms":null,
        "timeout_ms":null,"idle_timeout_ms":null,"validity":rig.job_validity()});
    rig.put("state/jobs/replayed.json", &job);
    // The receipt as it was written before the request evidence existed.
    rig.put(
        "state/launches/replayed.json",
        &json!({"id":"replayed","host":"local","pane":"w2:p1"}),
    );
    let request = job_launch_request(&rig, "replayed", &["true"]);
    let (code, result) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 0, "{result}");
    assert_eq!(result["pane"], "w2:p1", "{result}");
    let log = rig.log("herdr.log");
    assert!(!log.contains("tab create"), "{log}");
    assert!(!log.contains("pane run"), "{log}");
    assert_eq!(rig.read("state/jobs/replayed.json"), job);
}

/// A replay with a conflicting same-id request refuses. The saved receipt belongs to the
/// request that made it, so a different command, host, directory, options, or pane never
/// replays.
#[test]
fn a_host_replay_with_a_conflicting_receipt_refuses() {
    // The conflict in the request, and the conflict in the saved receipt.
    type Conflict = (&'static str, fn(&mut Value, &mut Value));
    let conflicts: [Conflict; 5] = [
        ("command", |request: &mut Value, _saved: &mut Value| {
            request["action"]["cmd"] = json!(["false"]);
        }),
        ("host", |request: &mut Value, _saved: &mut Value| {
            request["host"] = json!("other-host");
        }),
        ("cwd", |request: &mut Value, _saved: &mut Value| {
            request["cwd"] = json!("/somewhere/else");
        }),
        ("options", |request: &mut Value, _saved: &mut Value| {
            request["action"]["timeout_ms"] = json!(5000);
        }),
        ("pane", |_request: &mut Value, saved: &mut Value| {
            saved["pane"] = json!("w9:p1");
        }),
    ];
    for (name, conflict) in conflicts {
        let rig = Rig::new(&format!("replay-conflict-{name}"));
        fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
        fs::create_dir_all(rig.dir.join("state/launches")).unwrap();
        let job = json!({"id":"replayed","cmd":["true"],"cwd":rig.dir.join("home"),"host":"local","owner":"w1:p1",
            "pane":"w2:p1","state":"pending","exit":null,"signal":null,"reason":null,"started_ms":null,"ended_ms":null,
            "timeout_ms":null,"idle_timeout_ms":null,"validity":rig.job_validity()});
        rig.put("state/jobs/replayed.json", &job);
        let mut saved = job_receipt(
            &rig,
            "replayed",
            "local",
            "w2:p1",
            &["true"],
            &rig.dir.join("home").display().to_string(),
            "w1:p1",
        );
        let mut request = job_launch_request(&rig, "replayed", &["true"]);
        conflict(&mut request, &mut saved);
        rig.put("state/launches/replayed.json", &saved);
        let (code, error) = rig.json(&[
            "host",
            "launch",
            "--job-validity",
            "2",
            "--request",
            &request.to_string(),
        ]);
        assert_eq!(code, 2, "{name}: {error}");
        let log = rig.log("herdr.log");
        assert!(!log.contains("tab create"), "{name}: {log}");
        assert!(!log.contains("pane run"), "{name}: {log}");
        assert_eq!(rig.read("state/jobs/replayed.json"), job, "{name}");
        assert_eq!(rig.read("state/launches/replayed.json"), saved, "{name}");
    }
}

/// A conflicting same-id request without a saved receipt refuses too: the claim alone
/// authorizes nothing.
#[test]
fn a_host_replay_with_a_conflicting_request_and_no_receipt_refuses() {
    let rig = Rig::new("replay-conflict-no-receipt");
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    let job = json!({"id":"replayed","cmd":["true"],"cwd":rig.dir.join("home"),"host":"local","owner":"w1:p1",
        "pane":"w2:p1","state":"pending","exit":null,"signal":null,"reason":null,"started_ms":null,"ended_ms":null,
        "timeout_ms":null,"idle_timeout_ms":null,"validity":rig.job_validity()});
    rig.put("state/jobs/replayed.json", &job);
    let mut request = job_launch_request(&rig, "replayed", &["true"]);
    request["action"]["cmd"] = json!(["false"]);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("no saved launch receipt"),
        "{error}"
    );
    let log = rig.log("herdr.log");
    assert!(!log.contains("tab create"), "{log}");
    assert!(!log.contains("pane run"), "{log}");
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
        "started_ms":null,"ended_ms":null,"timeout_ms":null,"idle_timeout_ms":null,"validity":rig.job_validity()});
    rig.put("state/jobs/unproved.json", &job);
    let request = job_launch_request(&rig, "unproved", &["true"]);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
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

/// The job launch receipt, as a job launch writes it.
fn job_receipt(
    rig: &Rig,
    id: &str,
    host: &str,
    pane: &str,
    cmd: &[&str],
    cwd: &str,
    owner: &str,
) -> Value {
    json!({"id":id,"host":host,"pane":pane,
        "request":{"cmd":cmd,"cwd":cwd,"owner":owner,
            "timeout_ms":null,"idle_timeout_ms":null,
            "validity":rig.job_validity()}})
}

/// A matching receipt replays after the prune removed the job record. The prune keeps the
/// receipt, so the receipt's own evidence carries the replay: no second pane, no second
/// dispatch, and no invented job record.
#[test]
fn a_matching_receipt_replays_after_the_prune_removed_the_job_record() {
    let rig = Rig::new("replay-after-prune");
    // The job ends long enough ago that the prune removes its record.
    let ended = now() - 2 * 86_400_000;
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    fs::create_dir_all(rig.dir.join("state/launches")).unwrap();
    let job = json!({"id":"pruned","cmd":["true"],"cwd":rig.dir.join("home"),"host":"local","owner":"w1:p1",
        "pane":"w2:p1","state":"idle","exit":0,"signal":null,"reason":null,
        "started_ms":ended,"ended_ms":ended,"timeout_ms":null,"idle_timeout_ms":null,"validity":rig.job_validity()});
    rig.put("state/jobs/pruned.json", &job);
    rig.put(
        "state/launches/pruned.json",
        &job_receipt(
            &rig,
            "pruned",
            "local",
            "w2:p1",
            &["true"],
            &rig.dir.join("home").display().to_string(),
            "w1:p1",
        ),
    );
    // The prune removes the job record and its log, and keeps the receipt.
    let out = rig.run(&["prune", "--yes", "jobs"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("removed job pruned"), "{stdout}");
    assert!(!rig.dir.join("state/jobs/pruned.json").exists());
    assert!(rig.dir.join("state/launches/pruned.json").exists());
    // The replay returns the recorded result. It allocates nothing and dispatches nothing.
    let request = job_launch_request(&rig, "pruned", &["true"]);
    let (code, result) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 0, "{result}");
    assert_eq!(result["id"], "pruned", "{result}");
    assert_eq!(result["pane"], "w2:p1", "{result}");
    let log = rig.log("herdr.log");
    assert!(!log.contains("tab create"), "{log}");
    assert!(!log.contains("pane run"), "{log}");
    // The prune did not come back, and the receipt is unchanged.
    assert!(!rig.dir.join("state/jobs/pruned.json").exists());
    assert_eq!(
        rig.read("state/launches/pruned.json"),
        job_receipt(
            &rig,
            "pruned",
            "local",
            "w2:p1",
            &["true"],
            &rig.dir.join("home").display().to_string(),
            "w1:p1",
        )
    );
}

/// A receipt that predates the request evidence fails closed once the prune removed the job
/// record. Without the record, the receipt has nothing to prove which command it dispatched,
/// so the replay refuses instead of dispatching again.
#[test]
fn a_legacy_receipt_without_request_evidence_fails_closed_after_the_prune() {
    let rig = Rig::new("legacy-receipt-after-prune");
    let ended = now() - 2 * 86_400_000;
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    fs::create_dir_all(rig.dir.join("state/launches")).unwrap();
    let job = json!({"id":"legacy","cmd":["true"],"cwd":rig.dir.join("home"),"host":"local","owner":"w1:p1",
        "pane":"w2:p1","state":"idle","exit":0,"signal":null,"reason":null,
        "started_ms":ended,"ended_ms":ended,"timeout_ms":null,"idle_timeout_ms":null,"validity":rig.job_validity()});
    rig.put("state/jobs/legacy.json", &job);
    // The receipt as it was written before the request evidence existed.
    rig.put(
        "state/launches/legacy.json",
        &json!({"id":"legacy","host":"local","pane":"w2:p1"}),
    );
    let out = rig.run(&["prune", "--yes", "jobs"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(!rig.dir.join("state/jobs/legacy.json").exists());
    // The replay refuses: it dispatches nothing and invents nothing.
    let request = job_launch_request(&rig, "legacy", &["true"]);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("legacy launch receipt without request evidence"),
        "{error}"
    );
    let log = rig.log("herdr.log");
    assert!(!log.contains("tab create"), "{log}");
    assert!(!log.contains("pane run"), "{log}");
    assert!(!rig.dir.join("state/jobs/legacy.json").exists());
}

/// A receipt that carries the request evidence but no pane refuses after the prune removed
/// the job record. A receipt that names no pane proves no dispatch, so the replay refuses
/// instead of dispatching again.
#[test]
fn a_receipt_without_a_pane_fails_closed_after_the_prune() {
    for (name, pane) in [
        ("missing", Value::Null),
        ("empty", json!("")),
        ("number", json!(7)),
    ] {
        let rig = Rig::new(&format!("receipt-without-pane-after-prune-{name}"));
        let ended = now() - 2 * 86_400_000;
        fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
        fs::create_dir_all(rig.dir.join("state/launches")).unwrap();
        let job = json!({"id":"incomplete","cmd":["true"],"cwd":rig.dir.join("home"),"host":"local","owner":"w1:p1",
            "pane":"w2:p1","state":"idle","exit":0,"signal":null,"reason":null,
            "started_ms":ended,"ended_ms":ended,"timeout_ms":null,"idle_timeout_ms":null,"validity":rig.job_validity()});
        rig.put("state/jobs/incomplete.json", &job);
        let mut saved = job_receipt(
            &rig,
            "incomplete",
            "local",
            "w2:p1",
            &["true"],
            &rig.dir.join("home").display().to_string(),
            "w1:p1",
        );
        match name {
            "missing" => {
                saved.as_object_mut().unwrap().remove("pane");
            }
            _ => saved["pane"] = pane,
        }
        rig.put("state/launches/incomplete.json", &saved);
        let out = rig.run(&["prune", "--yes", "jobs"]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{name}: {stdout}");
        assert!(!rig.dir.join("state/jobs/incomplete.json").exists());
        // The receipt names no pane, so the replay refuses.
        let request = job_launch_request(&rig, "incomplete", &["true"]);
        let (code, error) = rig.json(&[
            "host",
            "launch",
            "--job-validity",
            "2",
            "--request",
            &request.to_string(),
        ]);
        assert_eq!(code, 2, "{name}: {error}");
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("launch receipt without a pane"),
            "{name}: {error}"
        );
        let log = rig.log("herdr.log");
        assert!(!log.contains("tab create"), "{name}: {log}");
        assert!(!log.contains("pane run"), "{name}: {log}");
        assert!(!rig.dir.join("state/jobs/incomplete.json").exists());
    }
}

/// A receipt whose evidence names another command refuses after the prune removed the job
/// record. The evidence is the check that a pruned record can no longer make.
#[test]
fn a_conflicting_receipt_fails_closed_after_the_prune() {
    for (name, conflict) in [("command", "cmd"), ("owner", "owner")] {
        let rig = Rig::new(&format!("conflicting-receipt-after-prune-{name}"));
        let ended = now() - 2 * 86_400_000;
        fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
        fs::create_dir_all(rig.dir.join("state/launches")).unwrap();
        let job = json!({"id":"conflicted","cmd":["true"],"cwd":rig.dir.join("home"),"host":"local","owner":"w1:p1",
            "pane":"w2:p1","state":"idle","exit":0,"signal":null,"reason":null,
            "started_ms":ended,"ended_ms":ended,"timeout_ms":null,"idle_timeout_ms":null,"validity":rig.job_validity()});
        rig.put("state/jobs/conflicted.json", &job);
        let mut saved = job_receipt(
            &rig,
            "conflicted",
            "local",
            "w2:p1",
            &["true"],
            &rig.dir.join("home").display().to_string(),
            "w1:p1",
        );
        match conflict {
            "cmd" => saved["request"]["cmd"] = json!(["false"]),
            _ => saved["request"]["owner"] = json!("w9:p9"),
        }
        rig.put("state/launches/conflicted.json", &saved);
        let out = rig.run(&["prune", "--yes", "jobs"]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{name}: {stdout}");
        assert!(!rig.dir.join("state/jobs/conflicted.json").exists());
        // The receipt names another command or another owner, so the replay refuses.
        let request = job_launch_request(&rig, "conflicted", &["true"]);
        let (code, error) = rig.json(&[
            "host",
            "launch",
            "--job-validity",
            "2",
            "--request",
            &request.to_string(),
        ]);
        assert_eq!(code, 2, "{name}: {error}");
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("another command or request"),
            "{name}: {error}"
        );
        let log = rig.log("herdr.log");
        assert!(!log.contains("tab create"), "{name}: {log}");
        assert!(!log.contains("pane run"), "{name}: {log}");
        assert!(!rig.dir.join("state/jobs/conflicted.json").exists());
    }
}

/// How the fake Herdr orders the real wrapper and the dispatch failure.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DispatchOrder {
    /// The wrapper runs to completion, and the dispatch reports its failure afterwards.
    After,
    /// The wrapper is still running when the dispatch reports its failure.
    During,
    /// The dispatch reports its failure before the wrapper ever starts.
    Before,
    /// The wrapper runs, and its pane never closes. The close marker never arrives.
    NoClose,
}

/// The fake Herdr for a dispatch-order test. It records the wrapper's command and then
/// waits for the test to release it, so the test owns the wrapper as its own child. The
/// wrapper is the real `dogsled exec`, so its publication is its own.
fn dispatch_order_herdr(rig: &Rig, order: DispatchOrder) {
    // The fake Herdr writes the wrapper's command to a file and waits for the test. The
    // test reads that command and spawns the wrapper itself, so the test is the wrapper's
    // parent and can reap it. The fake Herdr waits on a FIFO, so the order of the wrapper
    // and the dispatch failure is forced by the test, not by a sleep.
    let run = match order {
        DispatchOrder::Before => {
            r"
    shift 3
    echo 'pane refused' >&2
    exit 1 ;;"
        }
        _ => {
            r#"
    shift 3
    # Dispatch enqueues; the test owns the delayed wrapper after gates release.
    printf '%s\n' "$*" > "$RIG/wrapper-command"
    printf '{}\n' ;;"#
        }
    };
    // A wrapper that never closes its pane leaves the close marker unwritten, so the
    // test's wait must end on its own bound.
    let close = match order {
        DispatchOrder::NoClose => {
            r"
    # The pane never closes, so the close marker never arrives.
    printf '{{}}\n' ;;
"
        }
        _ => {
            r#"
    # The wrapper's close is the last thing it does. The marker lets the test wait for the
    # close itself. The wrapper's own exit status is the proof that the wrapper ended.
    ( printf 'closed\n' > "$RIG/wrapper-closed" ) &
    printf '{}\n' ;;
"#
        }
    };
    rig.script(
        "herdr",
        &format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/herdr.log"
case "$*" in
  'machine list --json') cat "$RIG/machines.json" ;;
  'agent list') cat "${{RIG_AGENTS:-$RIG/agents.json}}" ;;
  'pane list') cat "$RIG/panes.json" ;;
  'workspace list') printf '%s\n' '{{"result":{{"workspaces":[{{"workspace_id":"w1","label":"caller"}}]}}}}' ;;
  'worktree list --cwd'*) printf '%s\n' '{{"result":{{"worktrees":[]}}}}' ;;
  'tab create'*|'workspace create'*) printf '%s\n' '{{"result":{{"root_pane":{{"pane_id":"w2:p1","terminal_id":"term_child"}}}}}}' ;;
  'pane run'*)
    # The fake Herdr's own process id lets the test own the process that waits for the
    # dispatch to end. The test is not its parent, so the test cannot reap it.
    echo $$ > "$RIG/herdr-pid"
{run}
  'pane close '*)
    mkdir -p "$RIG/at-close"
    cp "$DOGSLED_STATE"/jobs/*.json "$RIG/at-close/"
{close}  'agent start'*|'agent prompt'*|'pane report-agent'*|'pane report-metadata'*) printf '{{}}\n' ;;
  *) echo unexpected >&2; exit 2 ;;
esac
"#,
        ),
    );
}

/// A dispatch that fails after the real wrapper published its terminal record keeps that
/// record. The wrapper runs to completion before the dispatch reports its failure, so the
/// order is forced by the process, not by a sleep.
#[test]
fn a_failed_replay_after_a_real_wrapper_ended_keeps_its_terminal_record() {
    let rig = Rig::new("dispatch-after-wrapper-ended");
    // The fake Herdr waits for this FIFO. The test writes to it only after the wrapper
    // ended, so the dispatch failure comes after the wrapper's terminal record.
    let release = Fifo::new(&rig.dir, "wrapper-release");
    let _release_hold = release.hold();
    dispatch_order_herdr(&rig, DispatchOrder::After);
    let command = "echo out; exit 7";
    let request = job_launch_request(&rig, "dispatched", &["sh", "-c", command]);
    let mut launch = rig.start(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    // The test spawns the wrapper as its own child, so the test is the wrapper's parent
    // and can reap it. A failed assertion kills it and its group.
    let mut wrapper = rig.spawn_recorded(&rig.dir.join("wrapper-command"), "the wrapper");
    // The wrapper ended on its own. Its reaped exit status is the proof, and the wait is
    // bounded: a wrapper that never ends fails the test instead of hanging it.
    assert_eq!(wrapper.wait().code(), Some(7));
    // Startup cannot precede dispatch completion. A conflicting host replay exercises
    // the equivalent failure boundary without weakening the live ownership assertion.
    let status = launch.wait();
    let (stdout, stderr) = launch.output();
    assert!(status.success(), "{stdout}{stderr}");
    let mut conflict = request.clone();
    conflict["action"]["cmd"] = json!(["false"]);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &conflict.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    assert_eq!(rig.log("herdr.log").matches("pane run ").count(), 1);
    // The wrapper's own terminal record survives the dispatch failure.
    let job = rig.read("state/jobs/dispatched.json");
    assert!(job["started_ms"].is_number(), "{job}");
    assert!(job["ended_ms"].is_number(), "{job}");
    assert_eq!(job["exit"], 7, "{job}");
    assert_eq!(job["state"], "blocked", "{job}");
    // The real wrapper's output is preserved.
    let output = rig.log("wrapper-output");
    assert!(output.contains("out\n"), "{output}");
    assert!(rig.log("herdr.log").contains("pane close w2:p1"));
}

/// A dispatch that fails while the real wrapper is running keeps the wrapper's running
/// record. The wrapper's command signals a FIFO, which proves its started record is
/// published before the dispatch reports its failure.
#[test]
fn a_failed_replay_during_a_real_wrapper_run_keeps_its_running_record() {
    let rig = Rig::new("dispatch-during-wrapper-run");
    // The ready FIFO is the command's own barrier. The test holds its ends open, so a
    // reader on either side never waits for an open that may never come.
    let ready = Fifo::new(&rig.dir, "wrapper-ready");
    let go = Fifo::new(&rig.dir, "wrapper-go");
    let closed = Fifo::new(&rig.dir, "wrapper-closed");
    // The release FIFO lets the fake Herdr report its failure at the point this order
    // requires. The test holds its end open, so the fake Herdr never waits for an open.
    let release = Fifo::new(&rig.dir, "wrapper-release");
    let _ready_hold = ready.hold();
    let _go_hold = go.hold();
    let _closed_hold = closed.hold();
    let _release_hold = release.hold();
    dispatch_order_herdr(&rig, DispatchOrder::During);
    let marker = rig.dir.join("marker");
    let command = format!(
        "echo $$ > \"$RIG/command-pid\"; echo ready > \"$RIG/wrapper-ready\"; read -r go < \"$RIG/wrapper-go\"; touch {}",
        marker.display()
    );
    let request = job_launch_request(&rig, "dispatched", &["sh", "-c", &command]);
    let mut launch = rig.start(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    // The test spawns the wrapper as its own child, so the test is the wrapper's parent
    // and can reap it. A failed assertion kills it and its group.
    let mut wrapper = rig.spawn_recorded(&rig.dir.join("wrapper-command"), "the wrapper");
    // The command leads a group of its own, so the wrapper's group does not cover it.
    // The test owns that group too, or the command outlives the test.
    let command_pid = read_pid(&rig.dir.join("command-pid"), "the command");
    let _command = Owned::adopt(command_pid);
    // The command is running: it published its own process id and signalled readiness.
    assert_eq!(
        read_fifo(ready.path(), "the command's readiness"),
        "ready\n"
    );
    // Startup cannot precede dispatch completion. A conflicting host replay exercises
    // the equivalent failure boundary without weakening the live ownership assertion.
    let status = launch.wait();
    let (stdout, stderr) = launch.output();
    assert!(status.success(), "{stdout}{stderr}");
    let mut conflict = request.clone();
    conflict["action"]["cmd"] = json!(["false"]);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &conflict.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    assert_eq!(rig.log("herdr.log").matches("pane run ").count(), 1);
    // The wrapper is running: it published its started record, and the dispatch failure
    // did not overwrite it or invent an end.
    let job = rig.read("state/jobs/dispatched.json");
    assert!(job["started_ms"].is_number(), "{job}");
    assert!(job["ended_ms"].is_null(), "{job}");
    assert_eq!(job["state"], "working", "{job}");
    // The blocked command has not run.
    assert!(!marker.exists());
    // The wrapper is released and finishes its own record. The rendezvous is bounded: a
    // wrapper that never answers fails the test instead of hanging it. The writer stays
    // open through the acknowledgement, so the reader never sees an early end.
    let mut go_writer = open_fifo(go.path());
    go_writer.write_all(b"go\n").unwrap();
    go_writer.flush().unwrap();
    // The close marker only proves the close helper ran. The wrapper's own reaped exit
    // proves the wrapper ended, and the wait is bounded.
    assert_eq!(wrapper.wait().code(), Some(0));
    assert_eq!(read_fifo(closed.path(), "the wrapper's close"), "closed\n");
    let job = rig.read("state/jobs/dispatched.json");
    assert!(job["ended_ms"].is_number(), "{job}");
    assert_eq!(job["exit"], 0, "{job}");
    assert!(marker.exists());
}

/// The wrapper's guard covers the command group the wrapper started. A wrapper that never
/// closes its pane leaves both the wrapper and its command running, and the guard kills
/// both. The proof is bounded: the test ends on its own bound, and no process outlives it.
#[test]
fn the_wrapper_guard_covers_the_command_group_it_started() {
    let rig = Rig::new("wrapper-guard-covers-command");
    // The ready FIFO is the command's own barrier. The test holds its ends open, so a
    // reader on either side never waits for an open that may never come.
    let ready = Fifo::new(&rig.dir, "wrapper-ready");
    let go = Fifo::new(&rig.dir, "wrapper-go");
    let closed = Fifo::new(&rig.dir, "wrapper-closed");
    // The release FIFO lets the fake Herdr report its failure while the wrapper runs.
    let release = Fifo::new(&rig.dir, "wrapper-release");
    let _ready_hold = ready.hold();
    let _go_hold = go.hold();
    let _closed_hold = closed.hold();
    let _release_hold = release.hold();
    dispatch_order_herdr(&rig, DispatchOrder::During);
    let marker = rig.dir.join("marker");
    // The command records its own process id, so the test can name the process the
    // wrapper started.
    let command = format!(
        "echo $$ > \"$RIG/command-pid\"; echo ready > \"$RIG/wrapper-ready\"; read -r go < \"$RIG/wrapper-go\"; touch {}",
        marker.display()
    );
    let request = job_launch_request(&rig, "dispatched", &["sh", "-c", &command]);
    let mut launch = rig.start(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    // The fake Herdr waits for the test's release. The test owns that process, so a
    // failed assertion kills it and it never outlives the test.
    // The test spawns the wrapper as its own child, so the test is the wrapper's parent
    // and can reap it.
    let mut wrapper = rig.spawn_recorded(&rig.dir.join("wrapper-command"), "the wrapper");
    let command_pid = read_pid(&rig.dir.join("command-pid"), "the command");
    // The command leads a group of its own, so the wrapper's group does not cover it.
    // The test owns that group too, or the command outlives the test.
    let command = Owned::adopt(command_pid);
    assert_eq!(
        read_fifo(ready.path(), "the command's readiness"),
        "ready\n"
    );
    let mut release_writer = open_fifo(release.path());
    release_writer.write_all(b"release\n").unwrap();
    release_writer.flush().unwrap();
    let status = launch.wait();
    let (stdout, stderr) = launch.output();
    assert!(status.success(), "{stdout}{stderr}");
    assert_ne!(process_group(command_pid), None, "the command has no group");
    // The guard leaves nothing behind: the command group dies first. The wrapper records
    // the command's death, so it ends on its own, and the test reaps that end. Both are
    // gone before the test ends.
    drop(command);
    // The command was killed, so the wrapper recorded the signal and ended with it.
    assert_eq!(wrapper.wait().code(), Some(137));
    assert!(!marker.exists(), "the killed command ran its own command");
    drop(wrapper);
    assert!(
        wait_gone(command_pid, PROCESS_DEADLINE),
        "the command outlived its test"
    );
}

/// A wrapper that never closes its pane fails the race test on the bound instead of
/// hanging it. The close marker never arrives, so the wait ends on the bound, and the
/// guard leaves no wrapper or command behind.
#[test]
fn a_wrapper_that_never_closes_its_pane_fails_the_race_on_the_bound() {
    let rig = Rig::new("wrapper-never-closes");
    let ready = Fifo::new(&rig.dir, "wrapper-ready");
    let go = Fifo::new(&rig.dir, "wrapper-go");
    let closed = Fifo::new(&rig.dir, "wrapper-closed");
    // The release FIFO lets the fake Herdr report its failure while the wrapper runs.
    let release = Fifo::new(&rig.dir, "wrapper-release");
    let _ready_hold = ready.hold();
    let _go_hold = go.hold();
    let _closed_hold = closed.hold();
    let _release_hold = release.hold();
    dispatch_order_herdr(&rig, DispatchOrder::NoClose);
    // The wrapper's close never happens, so the close marker never arrives.
    let marker = rig.dir.join("marker");
    let command = format!(
        "echo $$ > \"$RIG/command-pid\"; echo ready > \"$RIG/wrapper-ready\"; read -r go < \"$RIG/wrapper-go\"; touch {}",
        marker.display()
    );
    let request = job_launch_request(&rig, "dispatched", &["sh", "-c", &command]);
    let mut launch = rig.start(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    // The fake Herdr waits for the test's release. The test owns that process, so a
    // failed assertion kills it and it never outlives the test.
    // The test spawns the wrapper as its own child, so the test is the wrapper's parent
    // and can reap it.
    let mut wrapper = rig.spawn_recorded(&rig.dir.join("wrapper-command"), "the wrapper");
    let command_pid = read_pid(&rig.dir.join("command-pid"), "the command");
    // The command leads a group of its own, so the test owns that group too.
    let command = Owned::adopt(command_pid);
    assert_eq!(
        read_fifo(ready.path(), "the command's readiness"),
        "ready\n"
    );
    let mut release_writer = open_fifo(release.path());
    release_writer.write_all(b"release\n").unwrap();
    release_writer.flush().unwrap();
    let status = launch.wait();
    let (stdout, stderr) = launch.output();
    assert!(status.success(), "{stdout}{stderr}");
    let mut go_writer = open_fifo(go.path());
    go_writer.write_all(b"go\n").unwrap();
    go_writer.flush().unwrap();
    // The close never arrives, so the wait ends on its own bound instead of hanging.
    let bound = Duration::from_millis(300);
    let started = Instant::now();
    let closed_text = read_fifo_within(closed.path(), "the wrapper's close", bound);
    assert!(closed_text.is_none(), "the close arrived unexpectedly");
    assert!(
        started.elapsed() < PROCESS_DEADLINE,
        "the wait outlived the test bound"
    );
    // The wrapper ended on its own. Its reaped exit status is the proof that it ended,
    // and the guard leaves no wrapper or command behind.
    assert_eq!(wrapper.wait().code(), Some(0));
    drop(command);
    assert!(
        wait_gone(command_pid, PROCESS_DEADLINE),
        "the command outlived its test"
    );
}

/// A dispatch that fails before the wrapper starts leaves the claim uncertain. A wrapper
/// that starts later refuses that claim, and its command never runs.
#[test]
fn a_dispatch_failure_before_a_real_wrapper_starts_makes_the_late_wrapper_refuse() {
    let rig = Rig::new("dispatch-before-wrapper-starts");
    dispatch_order_herdr(&rig, DispatchOrder::Before);
    let marker = rig.dir.join("marker");
    let command = format!("touch {}", marker.display());
    let request = job_launch_request(&rig, "dispatched", &["sh", "-c", &command]);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    // The dispatch outcome was never proved, so the claim stays uncertain.
    let job = rig.read("state/jobs/dispatched.json");
    assert_eq!(job["state"], "uncertain", "{job}");
    assert!(job["ended_ms"].is_null(), "{job}");
    assert!(job["exit"].is_null(), "{job}");
    // The wrapper starts now. It refuses the uncertain claim, and runs nothing.
    let output = rig.run(&["exec", "dispatched", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not executable"), "{stderr}");
    assert!(!marker.exists());
    assert!(!rig.dir.join("state/jobs/dispatched.log").exists());
    // The uncertainty is kept: nothing invents an end or a retry.
    let job = rig.read("state/jobs/dispatched.json");
    assert_eq!(job["state"], "uncertain", "{job}");
    assert!(job["ended_ms"].is_null(), "{job}");
    // The dispatch happened once. The late wrapper dispatched nothing.
    assert_eq!(rig.log("herdr.log").matches("pane run").count(), 1);
}

/// A launch that stops after the claim is published but before the dispatch never runs the
/// command. The retry fails closed: it keeps the claim, saves no receipt, and neither
/// allocates nor dispatches.
#[test]
fn an_interrupted_launch_after_claim_publication_fails_closed_on_retry() {
    let rig = Rig::new("interrupted-after-claim");
    // The fake Herdr refuses the dispatch, so the claim is published and the launch stops
    // before the wrapper runs.
    rig.script(
        "herdr",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/herdr.log"
case "$*" in
  'machine list --json') cat "$RIG/machines.json" ;;
  'agent list') cat "${RIG_AGENTS:-$RIG/agents.json}" ;;
  'pane list') cat "$RIG/panes.json" ;;
  'workspace list') printf '%s\n' '{"result":{"workspaces":[{"workspace_id":"w1","label":"caller"}]}}' ;;
  'worktree list --cwd'*) printf '%s\n' '{"result":{"worktrees":[]}}' ;;
  'tab create'*|'workspace create'*) printf '%s\n' '{"result":{"root_pane":{"pane_id":"w2:p1","terminal_id":"term_child"}}}' ;;
  'pane run'*) echo 'pane refused' >&2; exit 1 ;;
  *) echo unexpected >&2; exit 2 ;;
esac
"#,
    );
    let request = job_launch_request(&rig, "interrupted", &["true"]);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    // The claim is published, and the dispatch that was never proved keeps it uncertain.
    let job = rig.read("state/jobs/interrupted.json");
    assert_eq!(job["state"], "uncertain", "{job}");
    assert_eq!(job["pane"], "w2:p1", "{job}");
    // No receipt was saved: nothing proves a dispatch.
    assert!(!rig.dir.join("state/launches/interrupted.json").exists());
    // The retry fails closed. It keeps the claim, allocates nothing, and dispatches nothing.
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("unresolved claim"),
        "{error}"
    );
    let log = rig.log("herdr.log");
    // The dispatch happened once. The retry neither allocated a pane nor dispatched again.
    assert_eq!(log.matches("pane run").count(), 1, "{log}");
    assert_eq!(log.matches("tab create").count(), 1, "{log}");
    assert!(!rig.dir.join("state/launches/interrupted.json").exists());
    // The claim is kept exactly as the failed dispatch left it.
    assert_eq!(rig.read("state/jobs/interrupted.json"), job);
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
            "timeout_ms":null,"idle_timeout_ms":null,"validity":rig.job_validity()});
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
        let (code, error) = rig.json(&[
            "host",
            "launch",
            "--job-validity",
            "2",
            "--request",
            &request.to_string(),
        ]);
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
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
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
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
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
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
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
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
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
    let (code, result) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
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
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
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
    let (code, result) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 0, "{result}");
    let log = rig.log("herdr.log");
    assert_eq!(log.matches("tab create").count(), 1, "{log}");
    assert!(!log.contains("pane close"), "{log}");
}

/// The retention pass removes a terminal job's metadata after 30 days. A request that
/// may not start new execution is still refused afterwards, and it starts nothing. The
/// refusal comes from the request's own age, not from the record the pass deleted.
#[test]
fn a_replay_after_the_retention_pass_removed_the_metadata_refuses_new_execution() {
    let rig = Rig::new("replay-after-retention");
    let ended = now() - 40 * 86_400_000;
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    fs::create_dir_all(rig.dir.join("state/launches")).unwrap();
    // The request was created 40 days ago, so it may not start new execution.
    let created = now() - 40 * 86_400_000;
    let job = json!({"id":"expired","cmd":["true"],"cwd":rig.dir.join("home"),"host":"local","owner":"w1:p1",
        "pane":"w2:p1","state":"idle","exit":0,"signal":null,"reason":null,
        "started_ms":ended,"ended_ms":ended,"timeout_ms":null,"idle_timeout_ms":null,
        "validity":{"version":1,"created_ms":created}});
    rig.put("state/jobs/expired.json", &job);
    fs::write(rig.dir.join("state/jobs/expired.log"), "the output\n").unwrap();
    // The service's retention pass removes the metadata of a job that ended 40 days ago.
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!rig.dir.join("state/jobs/expired.json").exists());
    assert!(!rig.dir.join("state/jobs/expired.log").exists());
    // The request is older than 30 days, so it may not start new execution.
    let request = job_launch_request_at(&rig, "expired", &["true"], created);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("older than 30 days"),
        "{error}"
    );
    // Nothing was dispatched, and no job record came back.
    let log = rig.log("herdr.log");
    assert!(!log.contains("pane run"), "{log}");
    assert!(!log.contains("tab create"), "{log}");
    assert!(!rig.dir.join("state/jobs/expired.json").exists());
}

/// Deleting a record advances the validity fence before a wall-clock rollback.
#[test]
fn retention_clock_deletion_still_fences_a_replay_after_rollback() {
    let rig = Rig::new("retention-replay-rollback");
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    let created = now();
    let deletion = created + 30 * 86_400_000 + 1;
    rig.put(
        "state/job-validity-clock.json",
        &json!({"version":1,"high_water_ms":created}),
    );
    rig.put(
        "state/jobs/expired.json",
        &json!({
            "id":"expired","cmd":["true"],"cwd":rig.dir.join("home"),"host":"local","owner":"w1:p1",
            "pane":"w2:p1","state":"idle","exit":0,"signal":null,"reason":null,
            "started_ms":created,"ended_ms":created,"timeout_ms":null,"idle_timeout_ms":null,
            "validity":{"version":1,"created_ms":created}
        }),
    );
    fs::write(rig.dir.join("state/jobs/expired.log"), "old output\n").unwrap();
    fs::write(rig.dir.join("clock"), deletion.to_string()).unwrap();
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(out.status.success(), "{out:?}");
    assert!(!rig.dir.join("state/jobs/expired.json").exists());
    assert_eq!(
        rig.read("state/job-validity-clock.json")["high_water_ms"],
        deletion
    );
    fs::write(rig.dir.join("clock"), (created + 1_000).to_string()).unwrap();
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(out.status.success(), "{out:?}");
    let request = job_launch_request_at(&rig, "expired", &["true"], created);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    assert_eq!(error["code"], "JOB_EXPIRED", "{error}");
    assert!(!rig.dir.join("state/jobs/expired.json").exists());
    assert_eq!(
        rig.read("state/job-validity-clock.json")["high_water_ms"],
        deletion
    );
    let log = rig.log("herdr.log");
    assert!(!log.contains("pane run"), "{log}");
    assert!(!log.contains("tab create"), "{log}");
}

/// A correlated replay after the retention pass removed the record stays read-only. It
/// returns the saved result and dispatches nothing, and the pass never brings the record
/// back.
#[test]
fn a_correlated_replay_after_the_retention_pass_stays_read_only() {
    let rig = Rig::new("replay-after-retention-read-only");
    let ended = now() - 40 * 86_400_000;
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    fs::create_dir_all(rig.dir.join("state/launches")).unwrap();
    let job = json!({"id":"expired","cmd":["true"],"cwd":rig.dir.join("home"),"host":"local","owner":"w1:p1",
        "pane":"w2:p1","state":"idle","exit":0,"signal":null,"reason":null,
        "started_ms":ended,"ended_ms":ended,"timeout_ms":null,"idle_timeout_ms":null,"validity":rig.job_validity()});
    rig.put("state/jobs/expired.json", &job);
    fs::write(rig.dir.join("state/jobs/expired.log"), "the output\n").unwrap();
    rig.put(
        "state/launches/expired.json",
        &job_receipt(
            &rig,
            "expired",
            "local",
            "w2:p1",
            &["true"],
            &rig.dir.join("home").display().to_string(),
            "w1:p1",
        ),
    );
    // The service's retention pass removes the record and its log, and keeps the receipt.
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!rig.dir.join("state/jobs/expired.json").exists());
    assert!(!rig.dir.join("state/jobs/expired.log").exists());
    assert!(rig.dir.join("state/launches/expired.json").exists());
    // The replay returns the recorded result. It allocates nothing and dispatches nothing.
    let request = job_launch_request(&rig, "expired", &["true"]);
    let (code, result) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 0, "{result}");
    assert_eq!(result["id"], "expired", "{result}");
    assert_eq!(result["pane"], "w2:p1", "{result}");
    let log = rig.log("herdr.log");
    assert!(!log.contains("pane run"), "{log}");
    assert!(!log.contains("tab create"), "{log}");
    // The pass did not bring the record back.
    assert!(!rig.dir.join("state/jobs/expired.json").exists());
}
