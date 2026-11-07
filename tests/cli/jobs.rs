use super::*;
use std::io::Write as _;

fn transferred_job(variant: &str) -> (Rig, Value) {
    let (rig, repo) = place::place_rig(&format!("disposition-wrapper-{variant}"));
    let (code, prepared) = rig.json(&[
        "host",
        "prepare",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "wrapper",
        "--base",
        "main",
        "--request-id",
        "startup",
    ]);
    assert_eq!(code, 0, "{prepared}");
    startup_herdr(&rig);
    let tree = PathBuf::from(prepared["path"].as_str().unwrap());
    let mut request = startup_request(&rig, &tree);
    request["preparation"] = prepared["binding"].clone();
    request["preparation_protocol"] = json!(1);
    request["action"]["cmd"] = json!(["sh", "-c", "echo once >> \"$RIG/command-marker\""]);
    let args = [
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ];
    if variant == "fenced" || variant == "before-fence" {
        let out = rig
            .cmd(&args)
            .env(
                "DOGSLED_TEST_UNIT_CRASH",
                if variant == "fenced" {
                    "preparation-fenced"
                } else {
                    "preparation-before-fence"
                },
            )
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(86));
    } else {
        let (code, result) = rig.json(&args);
        assert_eq!(code, 0, "{result}");
    }
    (rig, request)
}

#[test]
fn transferred_preparation_allows_only_its_existing_job_owner() {
    for variant in [
        "matching",
        "fenced",
        "before-fence",
        "owner",
        "identity",
        "unrelated",
        "missing-transfer",
    ] {
        let (rig, request) = transferred_job(variant);
        let text = request.to_string();
        let args = ["host", "launch", "--job-validity", "2", "--request", &text];
        if variant == "before-fence" {
            assert!(
                !rig.preparation_record("state", "disposition", "startup")
                    .exists()
            );
            let (code, result) = rig.json(&args);
            assert_eq!(code, 0, "{result}");
            assert_eq!(rig.preparations(), json!([]));
        }
        let transfer = rig
            .preparation_record("state", "disposition", "startup")
            .read();
        assert_eq!(transfer["request"]["policy"]["kind"], "transfer");
        let mut job = rig.job("startup");
        assert_eq!(job["state"], "pending");
        assert_eq!(transfer["request"]["policy"]["job"], job);
        match variant {
            "owner" => {
                job["owner"] = json!("replacement");
                rig.put_job("startup", &job);
            }
            "identity" => {
                job["startup"]["checkout"]["identity"]["generation"] = json!("0".repeat(32));
                rig.put_job("startup", &job);
            }
            "unrelated" => {
                job["id"] = json!("unrelated");
                rig.put_job("unrelated", &job);
            }
            "missing-transfer" => rig
                .preparation_record("state", "disposition", "startup")
                .delete(),
            _ => (),
        }
        let mut wrapper = if variant == "unrelated" {
            rig.start(&[
                "exec",
                "unrelated",
                "--",
                "sh",
                "-c",
                "echo once >> \"$RIG/command-marker\"",
            ])
        } else {
            // Herdr saved a real pending wrapper; only its process start is delayed.
            let mut wrapper =
                rig.spawn_recorded(&rig.dir.join("startup-wrapper"), "transferred wrapper");
            let status = wrapper.wait();
            if variant == "matching" || variant == "fenced" || variant == "before-fence" {
                assert!(status.success(), "{}", rig.log("wrapper-error"));
                assert_eq!(rig.log("command-marker"), "once\n");
                assert_transferred_job_replay(&rig, &request, &transfer);
                let disposition = rig
                    .preparation_record("state", "disposition", "startup")
                    .read();
                let receipt = rig.launch_record("startup", "receipt");
                let (code, result) = rig.json(&args);
                assert_eq!(code, 0, "{result}");
                assert_eq!(
                    rig.preparation_record("state", "disposition", "startup")
                        .read(),
                    disposition
                );
                assert_eq!(rig.launch_record("startup", "receipt"), receipt);
                let mut duplicate = rig.spawn_recorded(
                    &rig.dir.join("startup-wrapper"),
                    "duplicate transferred wrapper",
                );
                assert!(!duplicate.wait().success());
                assert_eq!(rig.log("command-marker"), "once\n");
            } else {
                assert!(!status.success(), "{variant}");
                assert!(!rig.dir.join("command-marker").exists(), "{variant}");
                assert_eq!(rig.job("startup"), job);
            }
            continue;
        };
        assert!(!wrapper.wait().success());
        assert!(!rig.dir.join("command-marker").exists());
    }
}

fn assert_transferred_job_replay(rig: &Rig, request: &Value, transfer: &Value) {
    let calls = rig.log("herdr.log");
    let text = request.to_string();
    let (code, result) = rig.json(&["host", "launch", "--job-validity", "2", "--request", &text]);
    assert_eq!(code, 0, "{result}");
    assert_eq!(rig.preparations(), json!([]));
    let completed = rig
        .preparation_record("state", "disposition", "startup")
        .read();
    assert_eq!(completed["progress"], "released");
    assert_eq!(completed["request"], transfer["request"]);
    assert_eq!(rig.log("herdr.log"), calls);
}

fn startup_herdr(rig: &Rig) {
    let script = fs::read_to_string(rig.dir.join("bin/herdr")).unwrap();
    rig.script("herdr", &script.replace("case \"$*\" in", r#"case "$*" in
  'tab create'*|'workspace create'*) printf '%s\n' '{"result":{"root_pane":{"pane_id":"w2:p1","terminal_id":"term_job"}}}' ;;
  'pane run'*) shift 3; printf '%s\n' "$*" > "$RIG/startup-wrapper"; printf '{}\n' ;;
  'pane report-agent'*|'pane report-metadata'*) printf '{}\n' ;;
"#));
}

fn startup_request(rig: &Rig, cwd: &Path) -> Value {
    json!({"id":"startup","action":{"type":"job","cmd":["sh","-c","touch \"$RIG/command-marker\""],
        "timeout_ms":null,"idle_timeout_ms":null,"validity":rig.job_validity()},
        "host":"local","cwd":cwd,"pane":null,"workspace":"w1","workspace_name":"caller","owner":"w1:p1"})
}

fn pending_startup(rig: &Rig, tree: &Path) -> Value {
    startup_herdr(rig);
    let request = startup_request(rig, tree);
    let (code, result) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 0, "{result}");
    register_startup_tree(rig, &result["checkout"]);
    let job = rig.job("startup");
    assert_eq!(job["startup"]["checkout"], result["checkout"]);
    job
}

fn register_startup_tree(rig: &Rig, checkout: &Value) {
    rig.put_state(&["state", "worktrees", "--put"], &json!([checkout]));
    close::git(
        &PathBuf::from(checkout["repo"].as_str().unwrap()),
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
}

fn close_job_hold(rig: &Rig, tree: &Path) -> (i32, Value) {
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[{"pane_id":"w2:p1","workspace_id":"w2","cwd":tree}]}}),
    );
    rig.json(&["host", "close", "w2:p1"])
}

fn job_repository_lock(repo: &Path) -> fs::File {
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

fn job_git_barrier(rig: &Rig) -> Fifo {
    let ready = Fifo::new(&rig.dir, "startup-git-ready");
    let actual = isolated("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    assert!(actual.status.success());
    let actual = String::from_utf8(actual.stdout).unwrap();
    rig.script("git", &format!(r#"#!/bin/sh
case "$*" in
  *'rev-parse --path-format=absolute --git-common-dir') printf 'ready\n' > "$RIG/startup-git-ready" ;;
esac
exec '{}' "$@"
"#, actual.trim()));
    ready
}

#[test]
fn dispatch_before_removal_keeps_pending_job_checkout() {
    let (rig, tree) = close::close_rig("startup-dispatch-hold");
    startup_herdr(&rig);
    let request = startup_request(&rig, &tree);
    let (code, result) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 0, "{result}");
    register_startup_tree(&rig, &result["checkout"]);
    let job = rig.job("startup");
    assert_eq!(job["startup"]["checkout"], result["checkout"], "{job}");
    rig.put("panes.json", &json!({"result":{"panes":[]}}));
    // Apply a pre-dispatch plan, not merely a survey that already sees the Job hold.
    let checkout = &result["checkout"];
    let plan = json!({"entries":[{"class":"worktrees","id":tree,"detail":"captured before dispatch","removable":true,"tree":checkout}]});
    let (code, applied) = rig.json(&["host", "prune", "--plan", &plan.to_string()]);
    assert_eq!(code, 0, "{applied}");
    let text = applied.to_string();
    assert!(tree.exists(), "{text}");
    assert!(text.contains("job startup"), "{text}");
    assert!(!rig.dir.join("hook.log").exists());
    let (code, refusal) = close_job_hold(&rig, &tree);
    assert_eq!(code, 2, "{refusal}");
    assert!(refusal.to_string().contains("job startup"), "{refusal}");
    let mut wrapper = rig.spawn_recorded(&rig.dir.join("startup-wrapper"), "pending wrapper");
    assert!(wrapper.wait().success(), "{}", rig.log("wrapper-error"));
    assert!(rig.dir.join("command-marker").exists());
}

#[test]
fn hook_environment_cannot_redirect_a_job_command() {
    let (rig, tree) = close::close_rig("startup-git-env");
    let hook = rig.dir.join("home/hook");
    fs::create_dir_all(&hook).unwrap();
    close::git(&hook, &["init", "-q", "-b", "main"]);
    startup_herdr(&rig);
    let mut request = startup_request(&rig, &tree);
    request["action"]["cmd"] = json!([
        "sh",
        "-c",
        "git rev-parse --absolute-git-dir > \"$RIG/job-git-dir\" && env > \"$RIG/job-env\""
    ]);
    let (code, result) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 0, "{result}");
    register_startup_tree(&rig, &result["checkout"]);
    let mut wrapper = isolated("sh");
    wrapper.args([
        "-c",
        &read_command(&rig.dir.join("startup-wrapper"), "git env wrapper"),
    ]);
    rig.rig_env(&mut wrapper);
    wrapper
        .env("GIT_DIR", hook.join(".git"))
        .env("GIT_WORK_TREE", &hook)
        .env("GIT_INDEX_FILE", hook.join(".git/index"))
        .stdout(fs::File::create(rig.dir.join("wrapper-output")).unwrap())
        .stderr(fs::File::create(rig.dir.join("wrapper-error")).unwrap());
    assert!(
        Owned::spawn(&mut wrapper).wait().success(),
        "{}",
        rig.log("wrapper-error")
    );
    assert_eq!(
        rig.log("job-git-dir").trim(),
        close::git(&tree, &["rev-parse", "--absolute-git-dir"]).trim()
    );
    let env = rig.log("job-env");
    for key in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
        assert!(
            !env.lines().any(|line| line.starts_with(&format!("{key}="))),
            "{key}"
        );
    }
}

#[test]
fn removal_before_dispatch_refuses_stale_job_startup() {
    let (rig, tree) = close::close_rig("startup-removed");
    startup_herdr(&rig);
    let request = startup_request(&rig, &tree);
    let repo = rig.dir.join("home/repo");
    close::git(
        &repo,
        &["worktree", "remove", "-f", "-f", tree.to_str().unwrap()],
    );
    assert!(!tree.exists());
    let (code, refusal) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{refusal}");
    assert!(rig.job("startup").is_null());
    assert!(rig.launch_record("startup", "receipt").is_null());
    assert!(!rig.dir.join("startup-wrapper").exists());
    assert!(!rig.log("herdr.log").contains("tab create"));
}

#[test]
fn exec_refuses_replacement_checkout_at_same_path() {
    for post_report in [false, true] {
        let (rig, tree) = close::close_rig(&format!("startup-replaced-{post_report}"));
        pending_startup(&rig, &tree);
        let repo = rig.dir.join("home/repo");
        let admin = PathBuf::from(close::git(&tree, &["rev-parse", "--absolute-git-dir"]).trim());
        let mut blocked = None;
        let release = Fifo::new(&rig.dir, "report-release");
        let ready = Fifo::new(&rig.dir, "report-ready");
        let _ready_hold = ready.hold();
        let _release_hold = release.hold();
        if post_report {
            let script = fs::read_to_string(rig.dir.join("bin/herdr")).unwrap();
            rig.script("herdr", &script.replace("'pane report-agent'*|'pane report-metadata'*) printf '{}\\n' ;;", "'pane report-agent'*|'pane report-metadata'*) if ! test -f \"$RIG/report-entered\"; then touch \"$RIG/report-entered\"; echo ready > \"$RIG/report-ready\"; read -r go < \"$RIG/report-release\"; fi; printf '{}\\n' ;;"));
            blocked = Some(rig.spawn_recorded(&rig.dir.join("startup-wrapper"), "report wrapper"));
            read_fifo(ready.path(), "pane report");
        }
        close::git(
            &repo,
            &["worktree", "remove", "-f", "-f", tree.to_str().unwrap()],
        );
        close::git(
            &repo,
            &["worktree", "add", "-q", tree.to_str().unwrap(), "feat/x"],
        );
        let replacement_admin =
            PathBuf::from(close::git(&tree, &["rev-parse", "--absolute-git-dir"]).trim());
        assert_eq!(admin, replacement_admin);
        let marker = admin.join("dogsled-checkout-id");
        assert!(!marker.exists());
        let mut wrapper = if let Some(wrapper) = blocked {
            writeln!(release.hold(), "go").unwrap();
            wrapper
        } else {
            rig.spawn_recorded(&rig.dir.join("startup-wrapper"), "replacement wrapper")
        };
        assert_eq!(
            wrapper.wait().code(),
            Some(2),
            "{}",
            rig.log("wrapper-error")
        );
        assert!(!rig.dir.join("command-marker").exists());
        assert!(!rig.dir.join("state/jobs/startup.log").exists());
        assert!(rig.job("startup")["ended_ms"].is_null());
        assert!(
            !marker.exists(),
            "refusal manufactured replacement identity"
        );
        assert_eq!(close::git(&tree, &["status", "--porcelain"]), "");
    }
}

#[test]
fn exec_waits_for_repository_gate_before_starting() {
    let (rig, tree) = close::close_rig("startup-repository-gate");
    let pending = pending_startup(&rig, &tree);
    let ready = job_git_barrier(&rig);
    let _ready_hold = ready.hold();
    let lock = job_repository_lock(&rig.dir.join("home/repo"));
    let mut wrapper = rig.start(&[
        "--budget-ms",
        "1000",
        "exec",
        "startup",
        "--",
        "sh",
        "-c",
        "touch \"$RIG/command-marker\"",
    ]);
    read_fifo(ready.path(), "wrapper repository lookup");
    assert_eq!(wrapper.wait().code(), Some(2));
    let (out, err) = wrapper.output();
    assert!(
        format!("{out}{err}").contains("dogsled-lifecycle.lock"),
        "{out}{err}"
    );
    assert_eq!(rig.job("startup"), pending);
    assert!(!rig.dir.join("state/jobs/startup.log").exists());
    assert!(!rig.dir.join("command-marker").exists());
    lock.unlock().unwrap();
    let mut wrapper = rig.start(&[
        "exec",
        "startup",
        "--",
        "sh",
        "-c",
        "touch \"$RIG/command-marker\"",
    ]);
    read_fifo(ready.path(), "released wrapper repository lookup");
    assert!(wrapper.wait().success());
    // A running command owns exec, not the preparation/repository gates.
    let running = Fifo::new(&rig.dir, "runtime-ready");
    let go = Fifo::new(&rig.dir, "runtime-go");
    let _running_hold = running.hold();
    let _go_hold = go.hold();
    let command = "echo $$ > \"$RIG/runtime-pid\"; echo ready > \"$RIG/runtime-ready\"; read -r go < \"$RIG/runtime-go\"";
    let mut request = startup_request(&rig, &tree);
    request["id"] = json!("runtime");
    request["action"]["cmd"] = json!(["sh", "-c", command]);
    let (code, result) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 0, "{result}");
    let mut wrapper = rig.spawn_recorded(&rig.dir.join("startup-wrapper"), "running wrapper");
    read_fifo(running.path(), "running command");
    let _command = Owned::adopt(read_pid(&rig.dir.join("runtime-pid"), "running command"));
    lock.try_lock().unwrap();
    lock.unlock().unwrap();
    let preparation = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(rig.dir.join("state/preparation-receipts/runtime.lock"))
        .unwrap();
    preparation.try_lock().unwrap();
    preparation.unlock().unwrap();
    let job = rig.job("runtime");
    let mut duplicate = rig.start(&[
        "--budget-ms",
        "5000",
        "exec",
        "runtime",
        "--",
        "sh",
        "-c",
        command,
    ]);
    read_fifo(ready.path(), "duplicate repository lookup");
    assert_eq!(duplicate.wait().code(), Some(2));
    let (out, err) = duplicate.output();
    assert!(
        format!("{out}{err}").contains("execution wrapper"),
        "{out}{err}"
    );
    assert_eq!(rig.job("runtime"), job);
    writeln!(go.hold(), "go").unwrap();
    assert!(wrapper.wait().success());
}

#[test]
fn nonterminal_job_holds_survive_failed_ticket_and_missing_pane() {
    for state in [
        "claiming",
        "pending",
        "working",
        "uncertain",
        "future",
        "legacy",
        "contradictory",
        "unsupported",
        "reversed-time",
        "idle",
    ] {
        let (rig, tree) = close::close_rig(&format!("startup-hold-{state}"));
        let mut job = pending_startup(&rig, &tree);
        job["state"] = json!(if state == "legacy" {
            "pending"
        } else if state == "contradictory" {
            "idle"
        } else {
            state
        });
        if state == "claiming" {
            job["pane"] = json!("");
        }
        if state == "working" || state == "idle" || state == "contradictory" {
            job["started_ms"] = json!(100);
        }
        if state == "idle" {
            job["ended_ms"] = json!(101);
            job["exit"] = json!(0);
        }
        if state == "unsupported" {
            job["startup"]["version"] = json!(2);
        }
        if state == "reversed-time" {
            job["state"] = json!("idle");
            job["started_ms"] = json!(102);
            job["ended_ms"] = json!(101);
        }
        if state == "legacy" {
            job.as_object_mut().unwrap().remove("startup");
            job["cwd"] = json!(tree.join("subdir"));
        }
        rig.put_job("startup", &job);
        rig.put("panes.json", &json!({"result":{"panes":[]}}));
        let request = startup_request(&rig, &tree);
        rig.put_tickets(&json!([{"request":request,"id":"startup","state":"failed","created_ms":rig.now_ms(),"waiting":false,"waiting_until_ms":null,"notified":true,"result":null,"error":"dispatch failed"}]));
        let out = rig.run(&["prune", "--yes", "worktrees"]);
        let text = String::from_utf8_lossy(&out.stdout);
        assert_eq!(tree.exists(), state != "idle", "{state}: {text}");
        if state != "idle" {
            assert!(text.contains("job startup"), "{state}: {text}");
            assert!(!rig.dir.join("hook.log").exists());
            let (code, refusal) = close_job_hold(&rig, &tree);
            assert_eq!(code, 2, "{state}: {refusal}");
            assert!(
                refusal.to_string().contains("job startup"),
                "{state}: {refusal}"
            );
        }
    }
}

#[test]
fn historical_claim_without_startup_identity_is_readable_but_cannot_run() {
    let rig = Rig::new("startup-legacy-claim");
    rig.claim("legacy", &["sh", "-c", "touch \"$RIG/command-marker\""]);
    let mut job = rig.job("legacy");
    job.as_object_mut().unwrap().remove("startup");
    rig.put_job("legacy", &job);
    let out = rig.run(&[
        "exec",
        "legacy",
        "--",
        "sh",
        "-c",
        "touch \"$RIG/command-marker\"",
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!rig.dir.join("command-marker").exists());
    assert!(!rig.dir.join("state/jobs/legacy.log").exists());
    assert_eq!(rig.job("legacy"), job);
}

#[test]
fn first_unavailable_peer_ingestion_keeps_unknown_job_capability_refused() {
    let rig = Rig::new("startup-unknown-peer");
    rig.peer();
    rig.script("ssh", "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$RIG/ssh.log\"\necho 'Connection refused' >&2\nexit 255\n");
    rig.ingest_peers();
    assert!(rig.state(&["peer", "8103"])["job_validity"].is_null());
    let _observer = rig.serve();
    wait_publication(&rig, |frame| {
        frame["views"]["hosts"]["data"]
            .as_array()
            .is_some_and(|hosts| {
                hosts
                    .iter()
                    .any(|host| host["id"] == "8103" && host["reachable"] == false)
            })
    });
    let output = rig.run(&["run", "--host", "netcup", "--", "true"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("host too old for job startup ownership")
    );
    let log = rig.log("ssh.log");
    assert!(!log.contains("host reserve"), "{log}");
    assert!(!log.contains("host launch"), "{log}");
}

#[test]
fn v1_only_host_refuses_jobs_before_dispatch() {
    let rig = Rig::new("startup-v1-only-peer");
    rig.peer();
    rig.snapshot("remote.json", rig.now_ms(), 0.0, 8);
    let mut peer = rig.read("remote.json");
    peer["job_validity"] = json!(1);
    rig.put("remote.json", &peer);
    rig.ingest_peers();
    let observer = super::place::peer_observer(&rig);
    let out = rig.run(&["run", "--host", "netcup", "--", "true"]);
    drop(observer);
    assert_eq!(out.status.code(), Some(2));
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(
        error.contains("host too old for job startup ownership; upgrade it"),
        "{error}"
    );
    let log = rig.log("ssh.log");
    assert!(!log.contains("host reserve"), "{log}");
    assert!(!log.contains("host launch"), "{log}");
    assert!(!rig.dir.join("state/jobs").exists());
    // The reverse direction also refuses v1 ingress before claim publication.
    let request = startup_request(&rig, &rig.dir.join("home"));
    let (code, refused) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "1",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{refused}");
    assert_eq!(refused["code"], "JOB_VALIDITY_UNSUPPORTED", "{refused}");
    assert!(rig.job("startup").is_null());
    assert!(!rig.log("herdr.log").contains("pane run"));
}

fn complete(id: &str) -> Value {
    json!({"id":id,"cmd":["echo","hi"],"cwd":"/tmp","host":"local","owner":"w1:p1","pane":"w2:p1",
        "state":"idle","exit":0,"signal":null,"reason":null,"started_ms":100,"ended_ms":101,"timeout_ms":null,"idle_timeout_ms":null})
}

/// The job retention windows: the output goes after a day, the record after 30 days.
const LOG_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;
const RECORD_WINDOW_MS: u64 = 30 * 24 * 60 * 60 * 1000;
/// What a reader sees when a job's output expired.
const EXPIRED: &str = "output expired";

/// A terminal job record that ended at `ended_ms`.
fn terminal(id: &str, ended_ms: u64) -> Value {
    let mut job = complete(id);
    job["started_ms"] = json!(ended_ms);
    job["ended_ms"] = json!(ended_ms);
    job["cwd"] = json!("/tmp");
    job
}

/// Write a job record and its log into a rig.
fn put_job(rig: &Rig, id: &str, job: &Value, output: &str) {
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    rig.put_job(id, job);
    fs::write(
        rig.dir.join(format!("state/jobs/{id}.log")),
        format!("{output}\n"),
    )
    .unwrap();
}

fn prune_old_job(state: &str, reversed: bool, kept: bool) {
    let rig = watchdog::watchdog_rig(&format!("prune-old-job-{state}-{reversed}"));
    let tree = rig.dir.join("home/repo/tree");
    let mut job = terminal("old", 2);
    job["state"] = json!(state);
    job["cwd"] = json!(tree);
    if reversed {
        job["started_ms"] = json!(3);
    }
    put_job(&rig, "old", &job, "old output");
    let (code, plan) = rig.json(&["doctor"]);
    assert_eq!(code, 0, "{plan}");
    assert_eq!(
        plan["entries"].as_array().unwrap().iter().any(|entry| {
            entry["class"] == "jobs" && entry["id"] == "old" && entry["removable"] == true
        }),
        !kept,
        "{plan}"
    );
    let out = rig.run(&["prune", "--yes", "jobs"]);
    assert!(out.status.success(), "{out:?}");
    let record = rig.job("old");
    // A terminal job releases its checkout after a day, not its thirty-day record.
    assert!(!record.is_null(), "{out:?}");
    assert!(rig.dir.join("state/jobs/old.log").exists());
    if !kept {
        job["output_expired"] = json!(true);
    }
    assert_eq!(rig.job("old"), job);
    let out = rig.run(&["prune", "--yes", "worktrees"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(tree.exists(), kept, "{out:?}");
    assert_eq!(rig.dir.join("removed").exists(), !kept);
}

#[test]
fn prune_old_job_with_unknown_state_keeps_record_and_checkout() {
    prune_old_job("future-state", false, true);
}

#[test]
fn prune_old_job_with_reversed_time_keeps_record_and_checkout() {
    prune_old_job("idle", true, true);
}

#[test]
fn prune_old_job_with_terminal_evidence_expires_output_but_keeps_record() {
    prune_old_job("idle", false, false);
}

/// Set the rig's host clock.
fn set_clock(rig: &Rig, at: u64) {
    fs::write(rig.dir.join("clock"), at.to_string()).unwrap();
}

/// One service pass. The retention lane runs on the timer, so a pass expires the jobs
/// whose windows passed.
fn serve(rig: &Rig) {
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The job's log, when it is still there.
fn log_of(rig: &Rig, id: &str) -> Option<String> {
    fs::read_to_string(rig.dir.join(format!("state/jobs/{id}.log"))).ok()
}

#[test]
fn stop_ends_a_pending_job_that_never_started() {
    let rig = Rig::new("stop-pending-job");
    let marker = rig.dir.join("marker");
    let command = format!("touch {}", marker.display());
    rig.claim("pending", &["sh", "-c", &command]);

    let output = rig.run(&["stop", "pending"]);
    assert!(output.status.success(), "{output:?}");
    let stopped = rig.job("pending");
    assert_eq!(stopped["state"], "blocked");
    assert_eq!(stopped["reason"], "stopped");
    assert!(stopped["ended_ms"].as_u64().is_some());
    assert!(stopped["started_ms"].is_null());
    assert!(stopped["exit"].is_null());
    assert!(stopped["signal"].is_null());
    assert_eq!(rig.stop_requested("pending"), json!(true));

    let output = rig.run(&["exec", "pending", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("job already ended"),
        "{output:?}"
    );
    assert!(!marker.exists());
    assert_eq!(rig.job("pending"), stopped);

    // A started job must let its capture loop stop the process and record its end.
    rig.claim("started", &["sh", "-c", &command]);
    let mut started = rig.job("started");
    started["state"] = json!("working");
    started["started_ms"] = json!(100);
    rig.put_job("started", &started);
    let output = rig.run(&["stop", "started"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(rig.job("started"), started);
    assert!(started["ended_ms"].is_null());
    assert_eq!(rig.stop_requested("started"), json!(true));
}

/// A wrapper runs nothing when its claim is missing, corrupt, for another command, already
/// started, ended, or uncertain. No process, no log, and no pane exist for such a job.
#[test]
fn exec_without_an_executable_claim_never_runs_the_command() {
    let rig = Rig::new("exec-no-claim");
    let marker = rig.dir.join("marker");
    let command = format!("touch {}", marker.display());

    // No record at all: the old behavior ran this.
    let output = rig.run(&["exec", "missing", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no claim to run"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!marker.exists());

    // Malformed input cannot become an executable claim.
    assert!(
        !rig.write_state(&["state", "job", "corrupt", "--put"], &json!("not a job"))
            .status
            .success()
    );
    assert!(rig.job("corrupt").is_null());
    let output = rig.run(&["exec", "corrupt", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!marker.exists());

    // A claim for another command does not cover this one.
    rig.claim("other-command", &["sh", "-c", "true"]);
    let output = rig.run(&["exec", "other-command", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!marker.exists());

    // A claim that already started never runs a second time.
    rig.claim("already-started", &["sh", "-c", &command]);
    let mut started = rig.job("already-started");
    started["started_ms"] = json!(100);
    started["state"] = json!("working");
    rig.put_job("already-started", &started);
    let output = rig.run(&["exec", "already-started", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!marker.exists());

    // A claim that ended never runs again.
    rig.claim("already-ended", &["sh", "-c", &command]);
    let mut ended = rig.job("already-ended");
    ended["ended_ms"] = json!(101);
    ended["state"] = json!("idle");
    rig.put_job("already-ended", &ended);
    let output = rig.run(&["exec", "already-ended", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!marker.exists());

    // An uncertain claim authorizes nothing.
    rig.claim("uncertain-claim", &["sh", "-c", &command]);
    let mut uncertain = rig.job("uncertain-claim");
    uncertain["state"] = json!("uncertain");
    rig.put_job("uncertain-claim", &uncertain);
    let output = rig.run(&["exec", "uncertain-claim", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!marker.exists());

    // A pre-allocation intent is not executable either.
    rig.claim("intent-only", &["sh", "-c", &command]);
    let mut intent = rig.job("intent-only");
    intent["state"] = json!("claiming");
    rig.put_job("intent-only", &intent);
    let output = rig.run(&["exec", "intent-only", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!marker.exists());

    // Not one of them left a log, a pane, or a running record behind.
    let jobs = fs::read_dir(rig.dir.join("state/jobs")).unwrap();
    for entry in jobs {
        let entry = entry.unwrap();
        assert!(
            entry.path().extension().is_none_or(|e| e != "log"),
            "{entry:?}"
        );
    }
    assert!(!rig.log("herdr.log").contains("pane close"));
}

/// A record that carries another job's identity never runs. The requested id, the record's
/// own id, and the record the wrapper reads must all name the same job. A record that
/// embeds another id is refused before any lock, process, log, or pane.
#[test]
fn exec_refuses_a_record_that_embeds_another_job() {
    let rig = Rig::new("exec-embedded-id");
    let marker = rig.dir.join("marker");
    let command = format!("touch {}", marker.display());
    let home = rig.dir.join("home").display().to_string();
    // B's own directory exists, so B is a valid, executable claim of its own.
    fs::create_dir_all(rig.dir.join("home/other")).unwrap();
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    // The typed publication refuses B's identity under A before either can execute.
    let a = json!({"id":"B","cmd":["sh","-c",&command],"cwd":home,"host":"local","owner":"w1:p1",
        "pane":"w2:p1","state":"pending","exit":Value::Null,"signal":Value::Null,"reason":Value::Null,
        "started_ms":Value::Null,"ended_ms":Value::Null,"timeout_ms":Value::Null,"idle_timeout_ms":Value::Null});
    let refused = rig.write_state(&["state", "job", "A", "--put"], &a);
    assert_eq!(refused.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("identity changed"));
    // B.json is a valid, executable claim of its own, with its own cwd, options, and pane.
    let b = json!({"id":"B","cmd":["sh","-c",&command],"cwd":format!("{home}/other"),"host":"local",
        "owner":"w1:p1","pane":"w3:p1","state":"pending","exit":Value::Null,"signal":Value::Null,
        "reason":Value::Null,"started_ms":Value::Null,"ended_ms":Value::Null,"timeout_ms":json!(1000),
        "idle_timeout_ms":Value::Null});
    rig.put_job("B", &b);
    let output = rig.run(&["exec", "A", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    // Both records are unchanged: A is not B, and B was never started.
    assert!(rig.job("A").is_null());
    assert_eq!(rig.job("B"), b);
    // No process, log, or pane ran for either job.
    assert!(!marker.exists());
    assert!(!rig.dir.join("state/jobs/A.log").exists());
    assert!(!rig.dir.join("state/jobs/B.log").exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no claim to run"), "{stderr}");
    let herdr = rig.log("herdr.log");
    assert!(!herdr.contains("pane run"), "{herdr}");
    assert!(!herdr.contains("pane close"), "{herdr}");
}

/// A pending claim is executable only when it is complete. An empty pane, a result it
/// already carries, a missing field, or a record that changed since it was read is refused
/// without mutation.
#[test]
fn exec_refuses_an_incomplete_or_conflicting_pending_claim() {
    let rig = Rig::new("exec-incomplete-claim");
    let marker = rig.dir.join("marker");
    let command = format!("touch {}", marker.display());

    // A pending claim with no allocated pane proves nothing.
    rig.claim_with(
        "empty-pane",
        &["sh", "-c", &command],
        &json!(null),
        &json!(null),
        "",
    );
    // A pending claim that already carries a terminal result is not executable.
    rig.claim("carried-exit", &["sh", "-c", &command]);
    let mut carried = rig.job("carried-exit");
    carried["exit"] = json!(0);
    rig.put_job("carried-exit", &carried);
    rig.claim("carried-signal", &["sh", "-c", &command]);
    let mut carried = rig.job("carried-signal");
    carried["signal"] = json!(15);
    rig.put_job("carried-signal", &carried);
    rig.claim("carried-reason", &["sh", "-c", &command]);
    let mut carried = rig.job("carried-reason");
    carried["reason"] = json!("timeout");
    rig.put_job("carried-reason", &carried);
    // A pending claim without a directory, a host, or an owner is incomplete.
    for (id, field) in [
        ("no-cwd", "cwd"),
        ("no-host", "host"),
        ("no-owner", "owner"),
    ] {
        rig.claim(id, &["sh", "-c", &command]);
        let mut claim = rig.job(id);
        claim[field] = json!("");
        rig.put_job(id, &claim);
    }

    for id in [
        "empty-pane",
        "carried-exit",
        "carried-signal",
        "carried-reason",
        "no-cwd",
        "no-host",
        "no-owner",
    ] {
        let before = rig.job(id);
        let output = rig.run(&["exec", id, "--", "sh", "-c", &command]);
        assert_eq!(output.status.code(), Some(2), "{id}");
        // The record is unchanged: nothing started, nothing ended, nothing ran.
        assert_eq!(rig.job(id), before, "{id}");
        assert!(
            !rig.dir.join(format!("state/jobs/{id}.log")).exists(),
            "{id}"
        );
    }
    assert!(!marker.exists());
    let herdr = rig.log("herdr.log");
    assert!(!herdr.contains("pane run"), "{herdr}");
    assert!(!herdr.contains("pane close"), "{herdr}");
}

/// A request that names different options than its claim asks for a different job.
#[test]
fn exec_refuses_options_that_differ_from_the_claim() {
    let rig = Rig::new("exec-options");
    rig.claim_with("claimed", &["true"], &json!(null), &json!(null), "w1:p1");
    let output = rig.run(&["exec", "claimed", "--timeout", "40ms", "--", "true"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("options differ from the launch record"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let job = rig.job("claimed");
    assert!(job["started_ms"].is_null());
    assert!(job["ended_ms"].is_null());
}

#[test]
fn run_wait_captures_exit_output_metadata_and_owner() {
    let rig = Rig::new("run-wait");
    rig.snapshot("state", now(), 0.0, 8);
    let observer = super::place::placement_observer(&rig);
    let output = rig.run(&[
        "run",
        "--wait",
        "--",
        "sh",
        "-c",
        "echo hi; echo err >&2; exit 3",
    ]);
    drop(observer);
    assert_eq!(
        output.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    let id = result["id"].as_str().unwrap();
    // wait returns the terminal record; the asynchronous wrapper still reports/closes.
    let deadline = Instant::now() + PROCESS_DEADLINE;
    let at_close = loop {
        if let Ok(bytes) = fs::read(rig.dir.join("at-close/jobs.json"))
            && let Ok(records) = serde_json::from_slice::<Vec<Value>>(&bytes)
            && let Some(record) = records.into_iter().find(|job| job["id"] == id)
        {
            break record;
        }
        assert!(Instant::now() < deadline, "wrapper did not close its pane");
        std::thread::yield_now();
    };
    let job = rig.job(id);
    assert_eq!(job["exit"], 3);
    assert_eq!(job["state"], "blocked");
    let log = rig.log(&format!("state/jobs/{id}.log"));
    assert!(log.contains("hi\n") && log.contains("err\n"));
    assert_eq!(rig.log("job-output"), log);
    assert_eq!(rig.items()[0]["owner_pane"], "w1:p1");
    let herdr = rig.log("herdr.log");
    assert!(herdr.contains("tab create --workspace w1"));
    assert!(herdr.contains("--source custom:dogsled --agent dogsled-job --state working"));
    assert!(herdr.contains("--token dogsled=exit 3"));
    assert_eq!(at_close, job);
    assert!(job["ended_ms"].is_number());
    assert!(herdr.ends_with("pane close w2:p1\n"));
    assert_eq!(
        rig.run(&["wait", id, "--max", "30s"]).status.code(),
        Some(3)
    );
    assert_eq!(rig.run(&["logs", id, "--follow"]).stdout, log.as_bytes());
    let observer = rig.serve();
    super::wait_publication(&rig, |frame| frame["views"]["jobs"]["data"][0]["exit"] == 3);
    assert_eq!(rig.json(&["ps", "--all"]).1[0]["exit"], 3);
    assert_eq!(rig.json(&["ps"]).1, json!([]));
    assert_eq!(
        rig.json(&["items", "--owner", "other"]).1["items"],
        json!([])
    );
    assert_eq!(
        rig.json(&["items", "--owner", "w1:p1"]).1["items"][0]["id"],
        id
    );
    drop(observer);
    assert!(rig.run(&["serve", "--once"]).status.success());
    assert_eq!(rig.host()["jobs"][0]["exit"], 3);
    assert!(rig.items()[0]["closed_ms"].is_number());
}

#[test]
fn exec_signal_and_timeouts_persist_terminal_result_and_kill_descendants() {
    let rig = Rig::new("exec");
    rig.claim("signal", &["sh", "-c", "kill -TERM $$"]);
    let out = rig.run(&["exec", "signal", "--", "sh", "-c", "kill -TERM $$"]);
    assert_eq!(out.status.code(), Some(143));
    assert_eq!(rig.job("signal")["signal"], 15);
    for (id, flag, reason) in [
        ("timed", "--timeout", "timeout"),
        ("silent", "--idle-timeout", "idle-timeout"),
    ] {
        // Both parent and descendant block on child-driven timers. No test sleeps.
        let (timeout_ms, idle_timeout_ms) = if flag == "--timeout" {
            (json!(40), json!(null))
        } else {
            (json!(null), json!(40))
        };
        rig.claim_with(
            id,
            &["sh", "-c", "sleep 30 & echo $!; wait"],
            &timeout_ms,
            &idle_timeout_ms,
            "w1:p1",
        );
        let out = rig.run(&[
            "exec",
            id,
            flag,
            "40ms",
            "--",
            "sh",
            "-c",
            "sleep 30 & echo $!; wait",
        ]);
        assert_eq!(
            out.status.code(),
            Some(124),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let result = rig.job(id);
        assert_eq!(result["reason"], reason);
        assert_eq!(result["signal"], 9);
        let pid = String::from_utf8(out.stdout).unwrap().trim().to_owned();
        let ps = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid])
            .output()
            .unwrap();
        // A killed orphan can remain a zombie until the host's init reaps it.
        assert!(
            !ps.status.success() || String::from_utf8_lossy(&ps.stdout).trim().starts_with('Z')
        );
        assert!(rig.log("herdr.log").contains(&format!("dogsled={reason}")));
    }
    rig.claim("ok", &["true"]);
    assert!(rig.run(&["exec", "ok", "--", "true"]).status.success());
    assert_eq!(rig.job("ok")["exit"], 0);
    // A claim that already started never runs a second time.
    assert_eq!(
        rig.run(&["exec", "ok", "--", "true"]).status.code(),
        Some(2)
    );
    rig.claim("bad", &["missing-command-xyz"]);
    assert_eq!(
        rig.run(&["exec", "bad", "--", "missing-command-xyz"])
            .status
            .code(),
        Some(2)
    );
    assert!(rig.job("bad")["ended_ms"].is_number());
}

#[test]
fn exec_close_failure_preserves_final_record_exit_and_logs() {
    let rig = Rig::new("exec-close-failure");
    fs::write(rig.dir.join("fail-close"), "").unwrap();
    for (id, command, code) in [
        ("success", "echo kept", 0),
        ("failure", "echo kept; exit 7", 7),
    ] {
        rig.claim(id, &["sh", "-c", command]);
        let output = rig.run(&["exec", id, "--", "sh", "-c", command]);
        assert_eq!(
            output.status.code(),
            Some(code),
            "{id}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("dogsled pane close:"), "{stderr}");
        assert!(stderr.contains("close-refused"), "{stderr}");
        let job = rig.job(id);
        assert_eq!(job["exit"], code);
        assert_eq!(job["reason"], Value::Null);
        assert!(job["ended_ms"].is_number());
        assert_eq!(rig.at_close(id), job);
        assert_eq!(rig.run(&["wait", id]).status.code(), Some(code));
        assert_eq!(rig.run(&["logs", id]).stdout, b"kept\n");
    }
}

#[test]
fn exec_closes_after_a_spawn_error_and_skips_close_without_a_pane() {
    let rig = Rig::new("exec-close-error");
    rig.claim_with(
        "spawn-error",
        &["missing-command-xyz"],
        &json!(null),
        &json!(null),
        "w1:p1",
    );
    let output = rig.run(&["exec", "spawn-error", "--", "missing-command-xyz"]);
    assert_eq!(output.status.code(), Some(2));
    let job = rig.job("spawn-error");
    assert_eq!(rig.at_close("spawn-error"), job);
    assert!(job["ended_ms"].is_number());
    assert!(job["reason"].is_string());
    assert!(rig.log("herdr.log").ends_with("pane close w1:p1\n"));
    fs::remove_file(rig.dir.join("herdr.log")).unwrap();
    // A claim without an allocated pane is not executable. It closes no pane, and it leaves
    // its own record untouched.
    rig.claim_with("no-pane", &["true"], &json!(null), &json!(null), "");
    let output = rig
        .cmd(&["exec", "no-pane", "--", "true"])
        .env_remove("HERDR_PANE_ID")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(!rig.log("herdr.log").contains("pane close"));
    let job = rig.job("no-pane");
    assert!(job["started_ms"].is_null(), "{job}");
    assert!(job["ended_ms"].is_null(), "{job}");
    assert!(!rig.dir.join("state/jobs/no-pane.log").exists());
    // A valid claim runs without a current HERDR pane in its own environment. It closes
    // the pane its claim names, and nothing else.
    rig.claim_with(
        "no-current-pane",
        &["true"],
        &json!(null),
        &json!(null),
        "w2:p1",
    );
    let output = rig
        .cmd(&["exec", "no-current-pane", "--", "true"])
        .env_remove("HERDR_PANE_ID")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(rig.job("no-current-pane")["ended_ms"].is_number());
    assert!(rig.log("herdr.log").contains("pane close w2:p1"));
}

#[test]
fn wait_max_and_stop_use_durable_local_job_state() {
    let rig = Rig::new("stop-wait");
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    let mut job = complete("running");
    job["ended_ms"] = Value::Null;
    job["exit"] = Value::Null;
    rig.put_job("running", &job);
    let out = rig.run(&["wait", "running", "--max", "0ms"]);
    assert_eq!(out.status.code(), Some(124));
    assert_eq!(out.stdout, b"still running\n");
    assert!(rig.run(&["stop", "running"]).status.success());
    assert_eq!(rig.stop_requested("running"), true);
    assert_eq!(rig.run(&["stop", "../escape"]).status.code(), Some(2));
}

#[test]
fn agent_places_starts_with_its_task_and_tracks_any_kind() {
    let rig = Rig::new("agent");
    let confirmed = now();
    fs::write(rig.dir.join("clock"), confirmed.to_string()).unwrap();
    rig.snapshot("state", confirmed, 0.0, 8);
    let observer = super::place::placement_observer(&rig);
    let (code, result) = rig.json(&[
        "agent",
        "--kind",
        "codex",
        "--name",
        "worker",
        "--message",
        "literal ' message",
    ]);
    drop(observer);
    assert_eq!(code, 0, "{result}");
    assert_eq!(
        result,
        json!({"host":"local","pane":"w2:p1","name":"worker","launch_confirmed_ms":confirmed})
    );
    let log = rig.log("herdr.log");
    // The task is read from its file, named on a line that also holds the child contract as
    // Codex developer instructions; nothing is typed into the pane.
    let task = log
        .lines()
        .find(|l| l.starts_with("agent start worker "))
        .and_then(|l| l.split("Your task is in ").nth(1))
        .and_then(|l| l.split(". Read that file").next())
        .unwrap()
        .to_owned();
    assert_eq!(fs::read_to_string(&task).unwrap(), "literal ' message");
    assert!(
        log.contains(&format!(
            "agent start worker --kind codex --pane w2:p1 -- -c developer_instructions=\"{CONTRACT}\" Your task is in {task}. Read that file and do the task.\n"
        )),
        "{log}"
    );
    assert!(!log.contains("agent prompt"), "{log}");
    assert_eq!(rig.items()[0]["kind"], "agent");
    assert_eq!(rig.items()[0]["launch_confirmed_ms"], confirmed);
    assert_eq!(
        rig.json(&["track", "local", "w3:p1", "--name", "other"]).0,
        0
    );
    assert_eq!(rig.items().as_array().unwrap().len(), 2);
    let _observer = rig.serve();
    super::wait_publication(&rig, |frame| {
        frame["views"]["status"]["data"]["items"]
            .as_array()
            .is_some_and(|items| items.len() == 2)
    });
    assert!(
        String::from_utf8(rig.run(&["status"]).stdout)
            .unwrap()
            .contains("owner w1:p1")
    );
}

#[test]
fn concurrent_item_and_queue_writers_keep_every_record() {
    let rig = Rig::new("concurrent-items-queue");
    let barrier = std::sync::Barrier::new(8);
    rig.snapshot("state", now(), 99.0, 2);
    let _publisher = super::observation::seeded(&rig);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let rig = &rig;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    assert_eq!(rig.json(&["track", "local", &format!("w3:p{i}")]).0, 0);
                    barrier.wait();
                    assert_eq!(rig.json(&["place"]).0, 1);
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
    });
    assert_eq!(rig.items().as_array().unwrap().len(), 8);
    assert_eq!(rig.tickets().as_array().unwrap().len(), 8);
}

#[test]
fn queue_admission_is_fifo_and_the_owner_hears_through_its_inbox() {
    let rig = Rig::new("queue-admit");
    rig.config("free_core_fraction_min = 2\n");
    rig.snapshot("state", now(), 0.0, 8);
    let publisher = super::observation::seeded(&rig);
    let (_, first) = rig.json(&["place"]);
    let (_, second) = rig.json(&[
        "agent",
        "--kind",
        "pi",
        "--name",
        "queued",
        "--message",
        "hello",
    ]);
    // --max counts from command entry, so it must cover the caller and workspace lookups
    // that run before the ticket is written, even on a loaded host.
    assert_eq!(
        rig.run(&["run", "--wait", "--max", "5s", "--", "true"])
            .status
            .code(),
        Some(124)
    );
    let tickets = rig.tickets();
    assert_eq!(tickets.as_array().unwrap().len(), 3);
    drop(publisher);
    let observer = rig.serve();
    super::wait_publication(&rig, |frame| {
        frame["views"]["inventory"]["data"]["queue"][2]["waiting"] == false
    });
    assert_eq!(rig.json(&["items"]).1["queue"][2]["waiting"], false);
    assert_eq!(
        rig.run(&["cancel", second["ticket"].as_str().unwrap()])
            .status
            .code(),
        Some(0)
    );
    super::wait_publication(&rig, |frame| {
        frame["views"]["status_display"]["data"]["counts"]["queued"] == 2
    });
    let brief = rig.run(&["status", "--brief"]);
    assert!(
        String::from_utf8_lossy(&brief.stdout).contains("2 queued"),
        "status --brief exited {:?}\nstdout: {}\nstderr: {}",
        brief.status.code(),
        String::from_utf8_lossy(&brief.stdout),
        String::from_utf8_lossy(&brief.stderr)
    );
    drop(observer);
    // Admission behavior is independent of the CI runner's real probe load/disk.
    // Exact capacity thresholds and FIFO blocking use the injected-time unit fixture.
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    let owner = json!({"result":{"agents":[{"pane_id":"w1:p1","agent":"pi",
        "agent_status":"working","terminal_id":"term_owner"}]}});
    rig.put("agents.json", &owner);
    assert!(rig.run(&["serve", "--once"]).status.success());
    let tickets = rig.tickets();
    assert_eq!(tickets[0]["id"], first["ticket"]);
    assert_eq!(tickets[0]["state"], "admitted", "{tickets}");
    assert_eq!(tickets[1]["state"], "cancelled");
    assert_eq!(tickets[2]["state"], "admitted", "{tickets}");
    // The owner has no inbox reader yet, so nothing tells it, and nothing types into it.
    assert_eq!(tickets[0]["notified"], false);
    // Once it reads its inbox, the notice arrives there, while the owner still works.
    assert_eq!(rig.json(&["inbox", "claim", "--harness", "pi"]).0, 0);
    assert!(rig.run(&["serve", "--once"]).status.success());
    assert_eq!(rig.tickets()[0]["notified"], true);
    let (_, claimed) = rig.json(&["inbox", "claim", "--harness", "pi"]);
    assert!(
        claimed[0]["text"].as_str().unwrap().starts_with(&format!(
            "dogsled: ticket {} admitted on local",
            first["ticket"].as_str().unwrap()
        )),
        "{claimed}"
    );
    assert!(!rig.log("herdr.log").contains("agent prompt"));
    assert_eq!(
        rig.run(&["cancel", first["ticket"].as_str().unwrap()])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn remote_job_controls_use_configured_transport_and_owner_records() {
    let rig = Rig::new("remote-controls");
    rig.peer();
    rig.config("remote_command = ['custom-dogsled']\n");
    let mut job = complete("remote-job");
    job["host"] = json!("8103");
    job["exit"] = json!(3);
    rig.put("job.json", &job);
    rig.put("jobs.json", &json!([job.clone()]));
    let mut snapshot = rig.read("remote.json");
    snapshot["jobs"] = json!([job]);
    rig.put("remote.json", &snapshot);
    rig.put("logs.json", &json!({"bytes":[],"next":0,"done":true}));
    rig.put_items(&json!([{
        "id":"remote-job","kind":"job","host":"8103","pane":"w9:p1","owner_pane":"w1:p1","created_ms":0,"closed_ms":null,"name":null
    }]));
    rig.ingest_peers();
    let observer = super::place::peer_observer(&rig);
    assert_eq!(
        rig.run(&["wait", "remote-job", "--max", "30s"])
            .status
            .code(),
        Some(3)
    );
    assert!(rig.run(&["stop", "remote-job"]).status.success());
    assert!(
        rig.run(&["logs", "remote-job", "--follow"])
            .status
            .success()
    );
    drop(observer);
    let _observer = rig.serve();
    super::wait_publication(&rig, |frame| {
        frame["views"]["jobs"]["data"][0]["host"] == "8103"
    });
    assert_eq!(rig.json(&["ps", "--all"]).1[0]["host"], "8103");
    assert!(
        rig.log("ssh.log")
            .contains("custom-dogsled host job remote-job --budget-ms")
    );
}

#[test]
fn checkout_clones_missing_path_accepts_ssh_origin_rejects_mismatch_and_reports_clone_failure() {
    let rig = Rig::new("checkout");
    rig.script("git", r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/git.log"
case "$*" in
  'clone '*) if test -f "$RIG/fail-clone"; then echo credentials >&2; exit 1; fi; mkdir -p "$4"; printf '[tools]\n' > "$4/mise.toml" ;;
  *'remote get-url origin') if test -f "$RIG/origin"; then cat "$RIG/origin"; else printf '%s\n' 'https://other.invalid/repo.git'; fi ;;
  *'rev-parse --show-toplevel') printf '%s\n' "$2" ;;
  *) exit 2 ;;
esac
"#);
    rig.script(
        "mise",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/mise.log"
case "$1" in
 trust) touch "$RIG/trusted" ;;
 *) if ! test -f "$RIG/trusted"; then echo 'config is not trusted' >&2; exit 1; fi; printf '{}\n' ;;
esac
"#,
    );
    let created = rig.origin_ms.to_string();
    let args = [
        "host",
        "checkout",
        "--request-validity",
        "1",
        "--request-id",
        "checkout-request",
        "--request-created-ms",
        &created,
        "--path",
        "repo",
        "--origin",
        "https://expected.invalid/repo.git",
        "--host-label",
        "peer",
    ];
    assert_eq!(rig.json(&args).0, 0);
    assert!(
        rig.log("git.log")
            .contains("clone -- https://expected.invalid/repo.git")
    );
    assert!(
        rig.log("mise.log")
            .contains(&format!("trust {}/home/repo/mise.toml", rig.dir.display()))
    );
    let (code, error) = rig.json(&args);
    assert_eq!(code, 2);
    let text = error["error"].as_str().unwrap();
    assert_eq!(error["diagnostic"]["cause"], "checkout_identity_mismatch");
    assert_eq!(
        error["diagnostic"]["context"]["workspace"],
        json!(rig.dir.join("home/repo"))
    );
    assert!(text.contains("checkout identity mismatch"));
    assert!(!text.contains("https://expected.invalid/repo.git"));
    assert!(!text.contains("https://other.invalid/repo.git"));
    assert_eq!(rig.log("git.log").matches("clone --").count(), 1);
    fs::write(rig.dir.join("origin"), "git@expected.invalid:repo.git\n").unwrap();
    assert_eq!(rig.json(&args).0, 0);
    fs::remove_file(rig.dir.join("origin")).unwrap();
    fs::write(rig.dir.join("fail-clone"), "").unwrap();
    let (code, error) = rig.json(&[
        "host",
        "checkout",
        "--request-validity",
        "1",
        "--request-id",
        "missing-checkout",
        "--request-created-ms",
        &created,
        "--path",
        "missing",
        "--origin",
        "https://expected.invalid/repo.git",
        "--host-label",
        "peer",
    ]);
    assert_eq!(code, 2);
    assert_eq!(error["diagnostic"]["cause"], "clone_failed");
    assert_eq!(error["code"], "CLONE_FAILED");
    assert!(error["error"].as_str().unwrap().contains("could not clone"));
    assert!(!error.to_string().contains("credentials"));
    assert!(
        !error
            .to_string()
            .contains("https://expected.invalid/repo.git")
    );
}

/// A job's output goes after a day. It stays at exactly 24 hours, and it goes at 24
/// hours and one millisecond.
#[test]
fn a_job_log_expires_after_a_day() {
    let rig = Rig::new("retention-log");
    let ended = now();
    set_clock(&rig, ended);
    put_job(&rig, "aged", &terminal("aged", ended), "the output");
    // Exactly 24 hours: the log stays.
    set_clock(&rig, ended + LOG_WINDOW_MS);
    serve(&rig);
    assert_eq!(log_of(&rig, "aged").as_deref(), Some("the output\n"));
    assert!(!rig.job("aged").is_null());
    // One millisecond more: the log is gone and the record says so.
    set_clock(&rig, ended + LOG_WINDOW_MS + 1);
    serve(&rig);
    assert_eq!(log_of(&rig, "aged").as_deref(), Some("output expired\n"));
    let job = rig.job("aged");
    assert_eq!(job["output_expired"], true, "{job}");
    // The record itself is still there: only a day passed.
    assert!(!rig.job("aged").is_null());
}

/// A job's record goes after 30 days. It stays at exactly 30 days, and it goes at 30
/// days and one millisecond.
#[test]
fn a_job_record_expires_after_thirty_days() {
    let rig = Rig::new("retention-record");
    let ended = now();
    set_clock(&rig, ended);
    put_job(&rig, "aged", &terminal("aged", ended), "the output");
    // Exactly 30 days: the record stays.
    set_clock(&rig, ended + RECORD_WINDOW_MS);
    serve(&rig);
    assert!(!rig.job("aged").is_null());
    // One millisecond more: the record and its log are gone.
    set_clock(&rig, ended + RECORD_WINDOW_MS + 1);
    serve(&rig);
    assert!(rig.job("aged").is_null());
    assert!(!rig.dir.join("state/jobs/aged.log").exists());
}

/// Retention uses the durable high-water time after the wall clock moves back.
#[test]
fn retention_clock_rollback_preserves_exact_boundaries() {
    let rig = Rig::new("retention-clock-rollback");
    let ended = now();
    let high_water = ended + LOG_WINDOW_MS + 1;
    rig.put_clock(&json!({"version":1,"high_water_ms":high_water}));
    set_clock(&rig, ended - 1_000);
    put_job(&rig, "aged", &terminal("aged", ended), "old output");
    put_job(
        &rig,
        "day-boundary",
        &terminal("day-boundary", high_water - LOG_WINDOW_MS),
        "day output",
    );
    put_job(
        &rig,
        "month-boundary",
        &terminal("month-boundary", high_water - RECORD_WINDOW_MS),
        "month output",
    );
    put_job(
        &rig,
        "future",
        &terminal("future", high_water + 1),
        "future output",
    );
    serve(&rig);
    assert_eq!(log_of(&rig, "aged").as_deref(), Some("output expired\n"));
    assert_eq!(
        log_of(&rig, "day-boundary").as_deref(),
        Some("day output\n")
    );
    assert_eq!(log_of(&rig, "future").as_deref(), Some("future output\n"));
    assert!(!rig.job("month-boundary").is_null());
    assert_eq!(rig.clock()["high_water_ms"], high_water);
}

/// A running job never expires. Its age changes nothing.
#[test]
fn a_running_job_never_expires() {
    let rig = Rig::new("retention-running");
    let started = now() - 90 * 86_400_000;
    set_clock(&rig, now());
    let mut job = complete("running");
    job["started_ms"] = json!(started);
    job["ended_ms"] = Value::Null;
    job["exit"] = Value::Null;
    job["state"] = json!("working");
    put_job(&rig, "running", &job, "the output");
    serve(&rig);
    assert!(!rig.job("running").is_null());
    assert_eq!(log_of(&rig, "running").as_deref(), Some("the output\n"));
}

/// An uncertain job never expires. Its age changes nothing.
#[test]
fn an_uncertain_job_never_expires() {
    let rig = Rig::new("retention-uncertain");
    set_clock(&rig, now());
    let mut job = complete("uncertain");
    job["started_ms"] = Value::Null;
    job["ended_ms"] = Value::Null;
    job["exit"] = Value::Null;
    job["state"] = json!("uncertain");
    job["reason"] = json!("dispatch outcome unknown");
    put_job(&rig, "uncertain", &job, "the output");
    serve(&rig);
    assert!(!rig.job("uncertain").is_null());
    assert_eq!(log_of(&rig, "uncertain").as_deref(), Some("the output\n"));
}

/// A reader sees the expiry instead of a missing output. The job list says it, and the
/// tail of the job says it.
#[test]
fn an_expired_output_is_reported_not_missing() {
    let rig = Rig::new("retention-readers");
    let ended = now();
    set_clock(&rig, ended);
    put_job(&rig, "aged", &terminal("aged", ended), "the output");
    set_clock(&rig, ended + LOG_WINDOW_MS + 1);
    serve(&rig);
    // The list carries the expiry marker, so a reader never sees a job that never ran.
    let listed = rig.json(&["host", "jobs"]).1;
    let job = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["id"] == "aged")
        .expect("the record is still listed");
    assert_eq!(job["output_expired"], true, "{job}");
    // The log read reports the expiry and does not fail.
    let logs = rig.json(&["host", "logs", "aged"]).1;
    assert_eq!(logs["expired"], true, "{logs}");
    assert_eq!(logs["bytes"], json!([]), "{logs}");
    assert_eq!(logs["done"], true, "{logs}");
    // The tail says it too.
    let tail = rig.json(&["host", "tail", "job", "aged"]).1;
    assert_eq!(tail, json!(EXPIRED), "{tail}");
}

/// Expired chunks carry a terminal signal, not bytes from the output cursor.
#[test]
fn expired_logs_end_after_one_marker() {
    let rig = Rig::new("retention-log-cursor");
    let mut job = terminal("aged", now());
    job["output_expired"] = json!(true);
    put_job(&rig, "aged", &job, EXPIRED);
    let marker = format!("{EXPIRED}\n");
    let (code, first) = rig.json(&["host", "logs", "aged"]);
    assert_eq!(code, 0, "{first}");
    assert_eq!(first["bytes"], json!([]), "{first}");
    assert_eq!(first["next"], 0, "{first}");
    assert_eq!(first["expired"], true, "{first}");
    assert_eq!(first["done"], true, "{first}");
    for offset in [3, marker.len() as u64, u64::MAX] {
        let (code, end) = rig.json(&["host", "logs", "aged", "--offset", &offset.to_string()]);
        assert_eq!(code, 0, "{end}");
        assert_eq!(end["bytes"], json!([]), "{end}");
        assert_eq!(end["next"], offset, "{end}");
        assert_eq!(end["done"], true, "{end}");
        assert_eq!(end["expired"], true, "{end}");
    }
    for args in [vec!["logs", "aged"], vec!["logs", "aged", "--follow"]] {
        let mut command = rig.start(&args);
        assert!(command.wait().success());
        let (stdout, stderr) = command.output();
        assert_eq!(stdout, marker, "{stderr}");
    }
}

/// Start a following reader and wait until it has consumed real output.
fn partially_read_logs(rig: &Rig) -> Owned {
    let mut command = rig.cmd(&["logs", "aged", "--follow"]);
    command.stdout(fs::File::create(rig.dir.join("stream-output")).unwrap());
    command.stderr(fs::File::create(rig.dir.join("stream-error")).unwrap());
    let reader = Owned::spawn(&mut command);
    assert_eq!(
        read_command(&rig.dir.join("stream-output"), "the log reader"),
        "output already consumed before retention"
    );
    reader
}

fn assert_stream_expired_once(rig: &Rig, reader: &mut Owned) {
    assert!(reader.wait().success(), "{}", rig.log("stream-error"));
    assert_eq!(
        rig.log("stream-output"),
        format!("output already consumed before retention\n{EXPIRED}\n")
    );
}

#[test]
fn local_stream_expiry_after_partial_output_prints_the_full_marker_once() {
    let rig = Rig::new("local-stream-expiry");
    let mut running = terminal("aged", now());
    running["ended_ms"] = Value::Null;
    running["exit"] = Value::Null;
    running["state"] = json!("working");
    put_job(
        &rig,
        "aged",
        &running,
        "output already consumed before retention",
    );
    let mut reader = partially_read_logs(&rig);
    // Publish the same transition as retention atomically, while the reader follows.
    let mut expired = terminal("aged", now());
    expired["output_expired"] = json!(true);
    rig.put_job("aged", &expired);
    assert_stream_expired_once(&rig, &mut reader);
}

#[test]
fn remote_stream_expiry_after_partial_output_prints_the_full_marker_once() {
    let rig = remote_expired_item_rig("remote-stream-expiry");
    fs::create_dir_all(rig.dir.join("peer-state/jobs")).unwrap();
    let mut running = terminal("aged", now());
    running["ended_ms"] = Value::Null;
    running["exit"] = Value::Null;
    running["state"] = json!("working");
    rig.put_peer_state(&["job", "aged", "--put"], &running);
    fs::write(
        rig.dir.join("peer-state/jobs/aged.log"),
        "output already consumed before retention\n",
    )
    .unwrap();
    let observer = super::place::peer_observer(&rig);
    let mut reader = partially_read_logs(&rig);
    drop(observer);
    // The owner keeps its item after remote retention removes the record.
    rig.peer_state(&["job", "aged", "--delete"]);
    assert_stream_expired_once(&rig, &mut reader);
    assert!(!rig.log("ssh.log").contains("host launch"));
}

/// A returning owner retains its item after retention removes the job record and log.
fn expired_item_rig(name: &str) -> Rig {
    let rig = Rig::new(name);
    let ended = now();
    put_job(&rig, "aged", &terminal("aged", ended), "old output");
    set_clock(&rig, ended + RECORD_WINDOW_MS + 1);
    serve(&rig);
    assert!(rig.job("aged").is_null());
    assert!(!rig.dir.join("state/jobs/aged.log").exists());
    rig.put_items(&json!([{
        "id":"aged","kind":"job","host":"local","pane":"w2:p1",
        "owner_pane":"w1:p1","created_ms":ended,"closed_ms":ended,"name":null
    }]));
    rig
}

#[test]
fn expired_record_read_reports_expiry_to_the_owner() {
    let rig = expired_item_rig("retention-gone-read");
    let (code, read) = rig.json(&["read", "aged"]);
    assert_eq!(code, 0, "{read}");
    assert_eq!(read["tail"], EXPIRED, "{read}");
    assert!(rig.job("aged").is_null());
}

#[test]
fn expired_record_tail_reports_expiry_to_the_owner() {
    let rig = expired_item_rig("retention-gone-tail");
    let (code, tail) = rig.json(&["host", "tail", "job", "aged"]);
    assert_eq!(code, 0, "{tail}");
    assert_eq!(tail, EXPIRED, "{tail}");
    assert!(rig.job("aged").is_null());
}

#[test]
fn expired_record_logs_report_expiry_to_the_owner() {
    let rig = expired_item_rig("retention-gone-logs");
    let marker = format!("{EXPIRED}\n");
    let (code, chunk) = rig.json(&["host", "logs", "aged"]);
    assert_eq!(code, 0, "{chunk}");
    assert_eq!(chunk["bytes"], json!([]), "{chunk}");
    assert_eq!(chunk["expired"], true, "{chunk}");
    assert_eq!(chunk["done"], true, "{chunk}");
    assert_eq!(chunk["next"], 0, "{chunk}");
    let (code, end) = rig.json(&[
        "host",
        "logs",
        "aged",
        "--offset",
        &marker.len().to_string(),
    ]);
    assert_eq!(code, 0, "{end}");
    assert_eq!(end["bytes"], json!([]), "{end}");
    assert_eq!(end["done"], true, "{end}");
    assert_eq!(rig.json(&["host", "logs", "unknown"]).0, 2);
    assert!(rig.job("aged").is_null());
    assert!(!rig.dir.join("state/jobs/aged.log").exists());
}

/// Remote retention leaves only the owner item; the execution host keeps nothing.
fn remote_expired_item_rig(name: &str) -> Rig {
    let rig = Rig::new(name);
    rig.peer();
    fs::create_dir_all(rig.dir.join("peer-state")).unwrap();
    let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
    let remote = format!(
        r#"case "$*" in
  *'host tail'*|*'host logs'*)
    for remote do :; done
    remote=${{remote#* host }}
    DOGSLED_STATE="$RIG/peer-state" sh -c '{} host '"$remote" ;;"#,
        env!("CARGO_BIN_EXE_dogsled")
    );
    rig.script("ssh", &script.replace("case \"$*\" in", &remote));
    rig.put_items(&json!([{
        "id":"aged","kind":"job","host":"8103","pane":"w9:p1",
        "owner_pane":"w1:p1","created_ms":0,"closed_ms":1,"name":null
    }]));
    rig
}

#[test]
fn remote_expired_record_read_reports_expiry_to_the_owner() {
    let rig = remote_expired_item_rig("remote-expired-read");
    let _observer = super::place::peer_observer(&rig);
    let (code, read) = rig.json(&["read", "aged"]);
    assert_eq!(code, 0, "{read}");
    assert_eq!(read["tail"], EXPIRED, "{read}");
    assert!(rig.peer_state(&["job", "aged"]).is_null());
    assert!(!rig.log("ssh.log").contains("host launch"));
}

#[test]
fn remote_expired_record_tail_reports_expiry_to_the_owner() {
    let rig = remote_expired_item_rig("remote-expired-tail");
    let _observer = super::place::peer_observer(&rig);
    let output = rig.run(&["read", "w9:p1", "--host", "netcup"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains(EXPIRED));
    assert!(!rig.dir.join("peer-state/jobs/aged.log").exists());
    assert!(!rig.log("ssh.log").contains("host launch"));
}

#[test]
fn remote_expired_record_logs_report_expiry_to_the_owner_once() {
    let rig = remote_expired_item_rig("remote-expired-logs");
    let _observer = super::place::peer_observer(&rig);
    for args in [vec!["logs", "aged"], vec!["logs", "aged", "--follow"]] {
        let mut command = rig.start(&args);
        assert!(command.wait().success());
        let (stdout, stderr) = command.output();
        assert_eq!(stdout, format!("{EXPIRED}\n"), "{stderr}");
    }
    assert!(rig.peer_state(&["job", "aged"]).is_null());
    assert!(!rig.dir.join("peer-state/jobs/aged.log").exists());
    assert!(!rig.log("ssh.log").contains("host launch"));
}

#[test]
fn remote_expired_chunks_ignore_wire_bytes() {
    let rig = remote_expired_item_rig("remote-expired-wire-bytes");
    // A peer may still send a legacy marker fragment; the reader owns the message.
    rig.put(
        "logs.json",
        &json!({"bytes":b"expired\n","next":15,"done":true,"expired":true}),
    );
    let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
    rig.script(
        "ssh",
        &script.replace(
            "case \"$*\" in",
            "case \"$*\" in\n  *'host logs'*) cat \"$RIG/logs.json\" ;;",
        ),
    );
    let _observer = super::place::peer_observer(&rig);
    let mut reader = rig.start(&["logs", "aged", "--follow"]);
    assert!(reader.wait().success());
    let (stdout, stderr) = reader.output();
    assert_eq!(stdout, format!("{EXPIRED}\n"), "{stderr}");
}

#[test]
fn unknown_jobs_without_an_owner_item_still_error() {
    let rig = remote_expired_item_rig("remote-expired-unknown");
    for args in [vec!["read", "unknown"], vec!["logs", "unknown"]] {
        assert_eq!(rig.json(&args).0, 2);
    }
    for args in [
        vec!["host", "tail", "job", "unknown"],
        vec!["host", "logs", "unknown"],
    ] {
        let (code, error) = rig.json(&args);
        assert_eq!(code, 2, "{error}");
        assert_eq!(error["code"], "UNKNOWN_JOB", "{error}");
    }
}

#[test]
fn a_record_without_output_still_reports_no_log_yet() {
    let rig = Rig::new("pending-no-log");
    rig.claim("pending", &["true"]);
    let (code, tail) = rig.json(&["host", "tail", "job", "pending"]);
    assert_eq!(code, 0, "{tail}");
    assert_eq!(tail, "(no log yet)");
}

/// A non-marker-sized log is replaced without opening or reading its content.
#[test]
fn retention_skips_reading_a_non_marker_sized_log() {
    let rig = Rig::new("retention-log-no-read");
    let ended = now();
    put_job(&rig, "aged", &terminal("aged", ended), "old output");
    let log = rig.dir.join("state/jobs/aged.log");
    fs::remove_file(&log).unwrap();
    assert!(Command::new("mkfifo").arg(&log).status().unwrap().success());
    // A FIFO has length zero. Opening it would wait for a writer; metadata alone
    // proves it cannot contain the marker, so retention must replace it directly.
    assert_eq!(fs::metadata(&log).unwrap().len(), 0);
    set_clock(&rig, ended + LOG_WINDOW_MS + 1);
    let mut service = rig.start(&["serve", "--once", "--interval", "1"]);
    assert!(service.wait().success());
    assert!(fs::metadata(&log).unwrap().is_file());
    assert_eq!(log_of(&rig, "aged").as_deref(), Some("output expired\n"));
}

#[test]
fn retention_replaces_a_large_log_with_the_marker() {
    let rig = Rig::new("retention-large-log");
    let ended = now();
    put_job(&rig, "aged", &terminal("aged", ended), "old output");
    fs::write(
        rig.dir.join("state/jobs/aged.log"),
        vec![b'x'; 4 * 1024 * 1024],
    )
    .unwrap();
    set_clock(&rig, ended + LOG_WINDOW_MS + 1);
    serve(&rig);
    assert_eq!(log_of(&rig, "aged").as_deref(), Some("output expired\n"));
}

/// The expiry pass repeats safely. A second pass changes nothing.
#[test]
fn a_second_expiry_pass_changes_nothing() {
    let rig = Rig::new("retention-repeat");
    let ended = now();
    set_clock(&rig, ended);
    put_job(&rig, "aged", &terminal("aged", ended), "the output");
    set_clock(&rig, ended + RECORD_WINDOW_MS + 1);
    serve(&rig);
    assert!(rig.job("aged").is_null());
    // A second pass at the same time is a no-op.
    serve(&rig);
    assert!(rig.job("aged").is_null());
    assert!(!rig.dir.join("state/jobs/aged.log").exists());
    // And a pass at a later time is a no-op too.
    set_clock(&rig, ended + 100 * RECORD_WINDOW_MS);
    serve(&rig);
    assert!(rig.job("aged").is_null());
}

/// A list that runs while the expiry runs does not fail. The record is either there or
/// gone, never half read.
#[test]
fn a_list_during_the_expiry_does_not_fail() {
    let rig = Rig::new("retention-concurrent");
    let ended = now();
    set_clock(&rig, ended);
    for index in 0..20 {
        put_job(
            &rig,
            &format!("aged-{index}"),
            &terminal(&format!("aged-{index}"), ended),
            "the output",
        );
    }
    set_clock(&rig, ended + RECORD_WINDOW_MS + 1);
    let out = rig.run(&["host", "jobs"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let listed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 20, "{listed}");
    serve(&rig);
    let out = rig.run(&["host", "jobs"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn settle_job_resources(rig: &Rig) {
    for _ in 0..2 {
        let mut service = rig.start(&["serve", "--once", "--interval", "1"]);
        assert!(service.wait().success(), "{:?}", service.output());
    }
}

fn assert_job_holds_cleared(rig: &Rig, id: &str) {
    wait_publication(rig, |frame| {
        frame["local"]["host"]["holds"]
            .as_array()
            .is_some_and(Vec::is_empty)
            && frame["views"]["boards"]["data"]["w1:p1"]["rows"]
                .as_array()
                .is_none_or(|rows| {
                    rows.iter()
                        .all(|row| row.get("holds").is_none_or(|holds| holds == &json!([])))
                })
    });
    let (code, board) = rig.json(&["board", "--owner", "w1:p1"]);
    assert_eq!(code, 0, "{board}");
    for row in board["rows"].as_array().unwrap() {
        assert!(
            row.get("holds").is_none_or(|holds| holds == &json!([])),
            "{board}"
        );
    }
    let (code, snapshot) = rig.json(&["host", "snapshot"]);
    assert_eq!(code, 0, "{snapshot}");
    assert!(
        snapshot["holds"].as_array().unwrap().is_empty(),
        "{id}: {snapshot}"
    );
}

#[test]
fn settled_local_jobs_release_holds_without_discarding_replay_evidence() {
    for exit in [0, 7] {
        let rig = Rig::short(&format!("settled-job-{exit}"));
        rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
        rig.snapshot("state", now(), 0.0, 8);
        let observer = super::place::placement_observer(&rig);
        let mut command = rig.start(&[
            "run",
            "--host",
            "local",
            "--wait",
            "--",
            "sh",
            "-c",
            &format!("echo result; exit {exit}"),
        ]);
        let status = command.wait();
        let (stdout, stderr) = command.output();
        assert_eq!(status.code(), Some(exit), "{stderr}");
        let result: Value = serde_json::from_str(&stdout).unwrap();
        let id = result["id"].as_str().unwrap();
        drop(observer);
        settle_job_resources(&rig);
        {
            let _observer = rig.serve_at(&rig.dir.join("state"), "3600");
            assert_job_holds_cleared(&rig, id);
        }
        assert!(rig.admission(id)["finished_ms"].is_number());
        assert!(rig.admission(id)["ordinary_release"]["acknowledgement"].is_object());
        assert!(!rig.validity(id).is_null());
        assert!(!rig.launch_record(id, "receipt").is_null());
    }
}

fn peer_unavailable(frame: &Value) -> bool {
    frame["sources"]["peers"]["8103"]["availability"] == "unavailable"
        && frame["sources"]["peers"]["8103"]["received_ms"]
            .as_u64()
            .is_some_and(|time| time > 0)
        && frame["views"]["status"]["data"]["hosts"]
            .as_array()
            .is_some_and(|hosts| {
                hosts.iter().any(|host| {
                    host["id"] == "8103"
                        && host["reachable"] == false
                        && host["headroom"]["eligible"] == false
                })
            })
}

fn settled_peer_rig(name: &str) -> (Rig, Background) {
    let rig = Rig::short(name);
    rig.peer();
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    rig.snapshot("state", now(), 0.0, 8);
    // Give the peer its own zero-counter CPU baseline, as the origin already has.
    // Real capture must derive eligibility on its first tick, not depend on a second wake.
    rig.snapshot("peer-state", now(), 0.0, 8);
    let herdr = fs::read_to_string(rig.dir.join("bin/herdr")).unwrap();
    rig.script("herdr", &herdr.replace("case \"$*\" in", r#"if test "$DOGSLED_STATE" = "$RIG/peer-state" && test "$*" = 'machine list --json'; then printf '[]\n'; exit; fi
case "$*" in"#));
    rig.script(
        "ssh",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/ssh.log"
case "$*" in
  *)
    for remote do :; done
    remote=${remote#* host }
    DOGSLED_STATE="$RIG/peer-state" sh -c '"$RIG_DOGSLED" --json host '"$remote"
    exit $? ;;
esac
"#,
    );
    let service = rig.serve_at(&rig.dir.join("peer-state"), "3600");
    wait_publication_at(&rig, &rig.dir.join("peer-state"), |frame| {
        frame["local"]["host"]["running"] == true && frame["local"]["headroom"]["eligible"] == true
    });
    (rig, service)
}

#[test]
fn peer_ingestion_never_corrupts_the_live_peer_probe_source() {
    let (rig, _peer) = settled_peer_rig("ingestion-source-isolation");
    let original = fs::read(rig.dir.join("agents.json")).unwrap();
    let script = rig.log("bin/herdr");
    rig.script(
        "herdr",
        &script.replacen(
            "case \"$*\" in",
            r#"if test "$DOGSLED_STATE" = "$RIG/state" && test "$*" = 'agent list'; then
  cp "$RIG/agents.json" "$RIG/peer-source-during-origin.json"
fi
case "$*" in"#,
            1,
        ),
    );
    rig.ingest_peers();
    assert_eq!(
        fs::read(rig.dir.join("peer-source-during-origin.json")).unwrap(),
        original
    );
}

#[test]
fn settled_remote_job_releases_holds_on_both_hosts() {
    let (rig, _peer_service) = settled_peer_rig("settled-peer-job");
    rig.ingest_peers();
    let observer = rig.serve();
    wait_publication(&rig, |frame| {
        frame["views"]["hosts"]["data"]
            .as_array()
            .is_some_and(|hosts| {
                hosts.iter().any(|host| {
                    host["id"] == "8103"
                        && host["sampled_ms"] == rig.host_at("peer-state")["sampled_ms"]
                })
            })
    });
    let mut command = rig.start(&[
        "run",
        "--host",
        "netcup",
        "--wait",
        "--max",
        "10s",
        "--",
        "sh",
        "-c",
        "echo peer-result",
    ]);
    let status = command.wait();
    let (stdout, stderr) = command.output();
    assert_eq!(status.code(), Some(0), "{stderr}");
    let job: Value = serde_json::from_str(&stdout).unwrap();
    let id = job["id"].as_str().unwrap();
    drop(observer);
    settle_job_resources(&rig);
    {
        let _observer = rig.serve_at(&rig.dir.join("state"), "3600");
        assert_job_holds_cleared(&rig, id);
    }
    let out = rig
        .cmd(&["host", "snapshot"])
        .env("DOGSLED_STATE", rig.dir.join("peer-state"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    wait_publication_at(&rig, &rig.dir.join("peer-state"), |frame| {
        frame["local"]["host"]["holds"] == json!([])
    });
    let snapshot: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(snapshot["holds"], json!([]), "{snapshot}");
    assert!(rig.peer_launch_record(id, "release").is_object());
}

#[test]
fn peer_refused_job_reports_once_and_releases_holds() {
    let (rig, peer_service) = settled_peer_rig("settled-refused-job");
    rig.ingest_peers();
    drop(peer_service);
    rig.expire_cache();
    {
        let _observer = rig.serve_at(&rig.dir.join("state"), "3600");
        wait_publication(&rig, peer_unavailable);
    }
    let missing = rig.dir.join("missing");
    // A shared non-repository cwd is removed on the peer after reservation, before ingress.
    fs::create_dir(&missing).unwrap();
    let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
    rig.script(
        "ssh",
        &script.replace(
            "case \"$*\" in",
            r#"case "$*" in
  *'host launch'*)
    rmdir "$RIG/missing"
    for remote do :; done
    remote=${remote#* host }
    DOGSLED_STATE="$RIG/peer-state" sh -c '"$RIG_DOGSLED" host '"$remote"
    exit $? ;;
"#,
        ),
    );
    let observer = rig.serve();
    wait_publication(&rig, peer_unavailable);
    let mut command = rig.start(&[
        "run",
        "--host",
        "netcup",
        "--cwd",
        missing.to_str().unwrap(),
        "--max",
        "10s",
        "--",
        "true",
    ]);
    let status = command.wait();
    let (stdout, stderr) = command.output();
    assert_eq!(status.code(), Some(1), "{stdout}\n{stderr}");
    drop(observer);
    let _peer_service = rig.serve_at(&rig.dir.join("peer-state"), "3600");
    wait_publication_at(&rig, &rig.dir.join("peer-state"), |frame| {
        frame["local"]["host"]["running"] == true && frame["local"]["headroom"]["eligible"] == true
    });
    rig.expire_cache();
    settle_job_resources(&rig);
    let tickets = rig.tickets();
    let ticket = &tickets[0];
    assert_eq!(ticket["state"], "failed", "{tickets}");
    let id = ticket["request"]["id"].as_str().unwrap();
    assert_eq!(
        ticket["diagnostic"]["cause"], "execution_directory_missing",
        "{ticket}"
    );
    assert!(
        ticket["error"]
            .as_str()
            .unwrap()
            .contains("netcup refused the launch: execution_directory_missing"),
        "{ticket}"
    );
    assert!(
        rig.peer_launch_record(id, "progress")["error"]
            .as_str()
            .unwrap()
            .contains("execution directory disappeared")
    );
    settle_job_resources(&rig);
    {
        let _observer = rig.serve_at(&rig.dir.join("state"), "3600");
        assert_job_holds_cleared(&rig, id);
    }
    let out = rig
        .cmd(&["host", "snapshot"])
        .env("DOGSLED_STATE", rig.dir.join("peer-state"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let snapshot: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(snapshot["holds"], json!([]), "{snapshot}");
    let before = rig.admission(id);
    settle_job_resources(&rig);
    assert_eq!(rig.admission(id), before);
}

fn peer_publication_diagnostic(rig: &Rig) -> String {
    let output = rig
        .cmd(&["observation", "current", "--json"])
        .env("DOGSLED_STATE", rig.dir.join("peer-state"))
        .output()
        .unwrap();
    format!(
        "publication={} cache={}",
        String::from_utf8_lossy(&output.stdout),
        rig.state(&["peer", "8103"])
    )
}

#[test]
fn peer_allocation_refusal_settles_its_non_executable_intent() {
    let (rig, _peer_service) = settled_peer_rig("settled-allocation-refusal");
    let script = fs::read_to_string(rig.dir.join("bin/herdr")).unwrap();
    rig.script(
        "herdr",
        &script.replace(
            "case \"$*\" in",
            r#"case "$*" in
  'tab create'*|'workspace create'*) echo allocation-refused >&2; exit 2 ;;
"#,
        ),
    );
    rig.ingest_peers();
    let observer = super::observation::seeded(&rig);
    let output = rig.run(&["run", "--host", "netcup", "--", "true"]);
    drop(observer);
    assert_eq!(
        output.status.code(),
        Some(2),
        "{output:?}; {}",
        peer_publication_diagnostic(&rig)
    );
    let tickets = rig.tickets();
    assert_eq!(tickets[0]["state"], "failed", "{tickets}");
    let id = tickets[0]["request"]["id"].as_str().unwrap();
    let job = rig.peer_state(&["job", id]);
    assert_eq!(job["state"], "blocked", "{job}");
    assert!(job["started_ms"].is_null(), "{job}");
    assert!(job["ended_ms"].is_number(), "{job}");
    settle_job_resources(&rig);
    {
        let _observer = rig.serve_at(&rig.dir.join("state"), "3600");
        assert_job_holds_cleared(&rig, id);
    }
    let output = rig
        .cmd(&["host", "snapshot"])
        .env("DOGSLED_STATE", rig.dir.join("peer-state"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let snapshot: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(snapshot["holds"], json!([]), "{snapshot}");
}

#[test]
fn peer_reserve_failure_releases_unstarted_evidence_and_fences_dispatch() {
    for queued in [false, true] {
        let (rig, peer_service) = settled_peer_rig(&format!("settled-reserve-refusal-{queued}"));
        let mut peer_service = Some(peer_service);
        if queued {
            rig.ingest_peers();
            drop(peer_service.take());
            rig.expire_cache();
        }
        let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
        rig.script(
            "ssh",
            &script.replace(
                "case \"$*\" in",
                r#"case "$*" in
  *'host reserve'*)
    for remote do :; done
    remote=${remote#* host }
    DOGSLED_STATE="$RIG/peer-state" sh -c '"$RIG_DOGSLED" host '"$remote" > /dev/null
    echo peer-refused >&2
    exit 2 ;;
"#,
            ),
        );
        if queued {
            // Phase B: a real origin publication has ingested the unavailable peer.
            let origin = rig.serve_at(&rig.dir.join("state"), "3600");
            wait_publication(&rig, peer_unavailable);
            drop(origin);
        } else {
            rig.ingest_peers();
        }
        let observer = super::observation::seeded(&rig);
        let output = rig.run(&["run", "--host", "netcup", "--", "true"]);
        drop(observer);
        assert_eq!(
            output.status.code(),
            Some(if queued { 1 } else { 2 }),
            "{output:?}; {}",
            peer_publication_diagnostic(&rig)
        );
        if queued {
            peer_service = Some(rig.serve_at(&rig.dir.join("peer-state"), "3600"));
        }
        rig.expire_cache();
        if !queued {
            let healthy = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
            rig.script(
                "ssh",
                &healthy.replace(
                    "case \"$*\" in",
                    r#"case "$*" in
          *'host release-unstarted'*) echo transport-ambiguous >&2; exit 255 ;;
        "#,
                ),
            );
            settle_job_resources(&rig);
            let _observer = rig.serve_at(&rig.dir.join("state"), "3600");
            let (code, snapshot) = rig.json(&["host", "snapshot"]);
            assert_eq!(code, 0, "{snapshot}");
            assert_ne!(snapshot["holds"], json!([]));
            assert_eq!(
                rig.peer_state(&["reservations"]).as_array().unwrap().len(),
                1
            );
            assert_eq!(
                rig.run(&["prune", "--yes", "resources"]).status.code(),
                Some(2)
            );
            rig.script("ssh", &healthy);
        }
        settle_job_resources(&rig);
        let tickets = rig.tickets();
        let ticket = &tickets[0];
        assert_eq!(ticket["state"], "failed", "{ticket}");
        let id = ticket["request"]["id"].as_str().unwrap();
        {
            let _observer = rig.serve_at(&rig.dir.join("state"), "3600");
            assert_job_holds_cleared(&rig, id);
        }
        let out = rig
            .cmd(&["host", "snapshot"])
            .env("DOGSLED_STATE", rig.dir.join("peer-state"))
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let snapshot: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(snapshot["holds"], json!([]), "{snapshot}");
        assert_eq!(rig.peer_state(&["reservations"]), json!([]));
        assert_peer_dispatch_fenced(&rig, ticket);
        drop(peer_service);
    }
}

fn assert_peer_dispatch_fenced(rig: &Rig, ticket: &Value) {
    let id = ticket["request"]["id"].as_str().unwrap();
    let validity = ticket["request"]["action"]["validity"].clone();
    let launch = json!({"id":id,"validity":validity,"action":ticket["request"]["action"],"host":"8103",
    "cwd":rig.dir.join("home"),"workspace":null,"workspace_name":null,"owner":"w1:p1","owner_host":ticket["request"]["owner_host"]});
    let out = rig
        .cmd(&[
            "host",
            "launch",
            "--job-validity",
            "2",
            "--request",
            &launch.to_string(),
        ])
        .env("DOGSLED_STATE", rig.dir.join("peer-state"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("future dispatch prohibited"),
        "{out:?}"
    );
    let created = validity["created_ms"].as_u64().unwrap().to_string();
    let out = rig
        .cmd(&[
            "host",
            "reserve",
            "--id",
            id,
            "--job-validity",
            "2",
            "--job-created-ms",
            &created,
        ])
        .env("DOGSLED_STATE", rig.dir.join("peer-state"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("future dispatch prohibited"),
        "{out:?}"
    );
}

#[test]
fn stopped_job_releases_holds_after_terminal_capture() {
    let rig = Rig::short("settled-stopped-job");
    startup_herdr(&rig);
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    rig.snapshot("state", now(), 0.0, 8);
    let observer = super::place::placement_observer(&rig);
    let (code, result) = rig.json(&["run", "--host", "local", "--", "sh", "-c", "sleep 90"]);
    drop(observer);
    assert_eq!(code, 0, "{result}");
    let id = result["id"].as_str().unwrap();
    let mut wrapper = rig.spawn_recorded(&rig.dir.join("startup-wrapper"), "stopped job wrapper");
    let _observer = rig.serve_at(&rig.dir.join("state"), "3600");
    wait_publication(&rig, |frame| {
        frame["local"]["host"]["holds"]
            .as_array()
            .is_some_and(|holds| !holds.is_empty())
    });
    let (code, snapshot) = rig.json(&["host", "snapshot"]);
    assert_eq!(code, 0, "{snapshot}");
    assert_ne!(snapshot["holds"], json!([]));
    for hold in snapshot["holds"].as_array().unwrap() {
        assert_eq!(hold["owner"], "w1:p1", "{hold}");
        assert_eq!(hold["owner_pane"], "w1:p1", "{hold}");
    }
    let (code, doctor) = rig.json(&["doctor"]);
    assert_eq!(code, 0, "{doctor}");
    for hold in snapshot["holds"].as_array().unwrap() {
        assert!(
            doctor["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| { entry["class"] == "resources" && entry["id"] == hold["resource"] }),
            "{hold}: {doctor}"
        );
    }
    assert_eq!(
        rig.run(&["prune", "--yes", "resources"]).status.code(),
        Some(2)
    );
    assert!(rig.job(id)["ended_ms"].is_null());
    assert!(rig.run(&["stop", id]).status.success());
    let mut command = rig.start(&["wait", id, "--max", "10s"]);
    assert!(!command.wait().success(), "{:?}", command.output());
    assert_eq!(rig.job(id)["reason"], "stopped");
    assert!(!wrapper.wait().success());
    let output = rig.run(&["prune", "--yes", "resources"]);
    assert!(output.status.success(), "{output:?}");
    assert_job_holds_cleared(&rig, id);
}

#[test]
fn peer_reservation_records_agent_owner_and_refuses_missing_owner() {
    let rig = Rig::new("reserve-job-owner");
    rig.snapshot("state", now(), 0.0, 8);
    let created = rig.now_ms().to_string();
    let args = [
        "--json",
        "host",
        "reserve",
        "--preparation-protocol",
        "1",
        "--id",
        "owned",
        "--job-validity",
        "2",
        "--job-created-ms",
        &created,
    ];
    let mut command = Command::new(env!("CARGO_BIN_EXE_dogsled"));
    rig.rig_env(&mut command);
    let out = command.args(args).output().unwrap();
    assert!(!out.status.success(), "{out:?}");
    assert!(rig.validity("owned").is_null());
    let (code, _) = rig.json(&[
        "host",
        "reserve",
        "--id",
        "owned",
        "--job-validity",
        "2",
        "--job-created-ms",
        &created,
    ]);
    assert!([0, 3].contains(&code));
    let _observer = rig.serve_at(&rig.dir.join("state"), "3600");
    wait_publication(&rig, |frame| {
        frame["local"]["host"]["holds"]
            .as_array()
            .is_some_and(|holds| !holds.is_empty())
    });
    let (code, snapshot) = rig.json(&["host", "snapshot"]);
    assert_eq!(code, 0, "{snapshot}");
    assert_ne!(snapshot["holds"], json!([]));
    for hold in snapshot["holds"].as_array().unwrap() {
        assert_eq!(hold["owner"], "w1:p1", "{hold}");
        assert_eq!(hold["owner_pane"], "w1:p1", "{hold}");
    }
}
