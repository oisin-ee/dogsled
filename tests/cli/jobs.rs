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
                !rig.dir
                    .join("state/preparation-receipts/startup.disposition.json")
                    .exists()
            );
            let (code, result) = rig.json(&args);
            assert_eq!(code, 0, "{result}");
            assert_eq!(rig.read("state/preparations.json"), json!([]));
        }
        let transfer = rig.read("state/preparation-receipts/startup.disposition.json");
        assert_eq!(transfer["request"]["policy"]["kind"], "transfer");
        let mut job = rig.read("state/jobs/startup.json");
        assert_eq!(job["state"], "pending");
        assert_eq!(transfer["request"]["policy"]["job"], job);
        match variant {
            "owner" => {
                job["owner"] = json!("replacement");
                rig.put("state/jobs/startup.json", &job);
            }
            "identity" => {
                job["startup"]["checkout"]["identity"]["generation"] = json!("0".repeat(32));
                rig.put("state/jobs/startup.json", &job);
            }
            "unrelated" => {
                job["id"] = json!("unrelated");
                rig.put("state/jobs/unrelated.json", &job);
            }
            "missing-transfer" => fs::remove_file(
                rig.dir
                    .join("state/preparation-receipts/startup.disposition.json"),
            )
            .unwrap(),
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
                let disposition = fs::read(
                    rig.dir
                        .join("state/preparation-receipts/startup.disposition.json"),
                )
                .unwrap();
                let receipt = fs::read(rig.dir.join("state/launches/startup.json")).unwrap();
                let (code, result) = rig.json(&args);
                assert_eq!(code, 0, "{result}");
                assert_eq!(
                    fs::read(
                        rig.dir
                            .join("state/preparation-receipts/startup.disposition.json")
                    )
                    .unwrap(),
                    disposition
                );
                assert_eq!(
                    fs::read(rig.dir.join("state/launches/startup.json")).unwrap(),
                    receipt
                );
                let mut duplicate = rig.spawn_recorded(
                    &rig.dir.join("startup-wrapper"),
                    "duplicate transferred wrapper",
                );
                assert!(!duplicate.wait().success());
                assert_eq!(rig.log("command-marker"), "once\n");
            } else {
                assert!(!status.success(), "{variant}");
                assert!(!rig.dir.join("command-marker").exists(), "{variant}");
                assert_eq!(rig.read("state/jobs/startup.json"), job);
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
    assert_eq!(rig.read("state/preparations.json"), json!([]));
    let completed = rig.read("state/preparation-receipts/startup.disposition.json");
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
    let job = rig.read("state/jobs/startup.json");
    assert_eq!(job["startup"]["checkout"], result["checkout"]);
    job
}

fn register_startup_tree(rig: &Rig, checkout: &Value) {
    rig.put("state/worktrees.json", &json!([checkout]));
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
    let job = rig.read("state/jobs/startup.json");
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
    assert!(!rig.dir.join("state/jobs/startup.json").exists());
    assert!(!rig.dir.join("state/launches/startup.json").exists());
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
        assert!(rig.read("state/jobs/startup.json")["ended_ms"].is_null());
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
    assert_eq!(rig.read("state/jobs/startup.json"), pending);
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
    let job = rig.read("state/jobs/runtime.json");
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
    assert_eq!(rig.read("state/jobs/runtime.json"), job);
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
        rig.put("state/jobs/startup.json", &job);
        rig.put("panes.json", &json!({"result":{"panes":[]}}));
        let request = startup_request(&rig, &tree);
        rig.put("state/queue.json", &json!([{"request":request,"id":"startup","state":"failed","created_ms":rig.now_ms(),"waiting":false,"waiting_until_ms":null,"notified":true,"result":null,"error":"dispatch failed"}]));
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
    let mut job = rig.read("state/jobs/legacy.json");
    job.as_object_mut().unwrap().remove("startup");
    rig.put("state/jobs/legacy.json", &job);
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
    assert_eq!(rig.read("state/jobs/legacy.json"), job);
}

#[test]
fn v1_only_host_refuses_jobs_before_dispatch() {
    let rig = Rig::new("startup-v1-only-peer");
    rig.peer();
    rig.snapshot("remote.json", rig.now_ms(), 0.0, 8);
    let mut peer = rig.read("remote.json");
    peer["job_validity"] = json!(1);
    rig.put("remote.json", &peer);
    let out = rig.run(&["run", "--host", "netcup", "--", "true"]);
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
    assert!(!rig.dir.join("state/jobs/startup.json").exists());
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
    rig.put(&format!("state/jobs/{id}.json"), job);
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
    let record = rig.dir.join("state/jobs/old.json");
    assert_eq!(record.exists(), kept, "{out:?}");
    assert_eq!(rig.dir.join("state/jobs/old.log").exists(), kept);
    if kept {
        assert_eq!(rig.read("state/jobs/old.json"), job);
    }
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
fn prune_old_job_with_terminal_evidence_removes_record() {
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
    let stopped = rig.read("state/jobs/pending.json");
    assert_eq!(stopped["state"], "blocked");
    assert_eq!(stopped["reason"], "stopped");
    assert!(stopped["ended_ms"].as_u64().is_some());
    assert!(stopped["started_ms"].is_null());
    assert!(stopped["exit"].is_null());
    assert!(stopped["signal"].is_null());
    assert_eq!(rig.read("state/jobs/pending.stop"), json!(true));

    let output = rig.run(&["exec", "pending", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("job already ended"),
        "{output:?}"
    );
    assert!(!marker.exists());
    assert_eq!(rig.read("state/jobs/pending.json"), stopped);

    // A started job must let its capture loop stop the process and record its end.
    rig.claim("started", &["sh", "-c", &command]);
    let mut started = rig.read("state/jobs/started.json");
    started["state"] = json!("working");
    started["started_ms"] = json!(100);
    rig.put("state/jobs/started.json", &started);
    let output = rig.run(&["stop", "started"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(rig.read("state/jobs/started.json"), started);
    assert!(started["ended_ms"].is_null());
    assert_eq!(rig.read("state/jobs/started.stop"), json!(true));
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

    // A corrupt record proves nothing, and never runs.
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    fs::write(rig.dir.join("state/jobs/corrupt.json"), "not json").unwrap();
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
    let mut started = rig.read("state/jobs/already-started.json");
    started["started_ms"] = json!(100);
    started["state"] = json!("working");
    rig.put("state/jobs/already-started.json", &started);
    let output = rig.run(&["exec", "already-started", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!marker.exists());

    // A claim that ended never runs again.
    rig.claim("already-ended", &["sh", "-c", &command]);
    let mut ended = rig.read("state/jobs/already-ended.json");
    ended["ended_ms"] = json!(101);
    ended["state"] = json!("idle");
    rig.put("state/jobs/already-ended.json", &ended);
    let output = rig.run(&["exec", "already-ended", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!marker.exists());

    // An uncertain claim authorizes nothing.
    rig.claim("uncertain-claim", &["sh", "-c", &command]);
    let mut uncertain = rig.read("state/jobs/uncertain-claim.json");
    uncertain["state"] = json!("uncertain");
    rig.put("state/jobs/uncertain-claim.json", &uncertain);
    let output = rig.run(&["exec", "uncertain-claim", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!marker.exists());

    // A pre-allocation intent is not executable either.
    rig.claim("intent-only", &["sh", "-c", &command]);
    let mut intent = rig.read("state/jobs/intent-only.json");
    intent["state"] = json!("claiming");
    rig.put("state/jobs/intent-only.json", &intent);
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
    // A.json holds B's identity. Its command matches the request, so only the identity
    // can refuse it.
    let a = json!({"id":"B","cmd":["sh","-c",&command],"cwd":home,"host":"local","owner":"w1:p1",
        "pane":"w2:p1","state":"pending","exit":Value::Null,"signal":Value::Null,"reason":Value::Null,
        "started_ms":Value::Null,"ended_ms":Value::Null,"timeout_ms":Value::Null,"idle_timeout_ms":Value::Null});
    rig.put("state/jobs/A.json", &a);
    // B.json is a valid, executable claim of its own, with its own cwd, options, and pane.
    let b = json!({"id":"B","cmd":["sh","-c",&command],"cwd":format!("{home}/other"),"host":"local",
        "owner":"w1:p1","pane":"w3:p1","state":"pending","exit":Value::Null,"signal":Value::Null,
        "reason":Value::Null,"started_ms":Value::Null,"ended_ms":Value::Null,"timeout_ms":json!(1000),
        "idle_timeout_ms":Value::Null});
    rig.put("state/jobs/B.json", &b);
    let output = rig.run(&["exec", "A", "--", "sh", "-c", &command]);
    assert_eq!(output.status.code(), Some(2));
    // Both records are unchanged: A is not B, and B was never started.
    assert_eq!(rig.read("state/jobs/A.json"), a);
    assert_eq!(rig.read("state/jobs/B.json"), b);
    // No process, log, or pane ran for either job.
    assert!(!marker.exists());
    assert!(!rig.dir.join("state/jobs/A.log").exists());
    assert!(!rig.dir.join("state/jobs/B.log").exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("belongs to job B"), "{stderr}");
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
    let mut carried = rig.read("state/jobs/carried-exit.json");
    carried["exit"] = json!(0);
    rig.put("state/jobs/carried-exit.json", &carried);
    rig.claim("carried-signal", &["sh", "-c", &command]);
    let mut carried = rig.read("state/jobs/carried-signal.json");
    carried["signal"] = json!(15);
    rig.put("state/jobs/carried-signal.json", &carried);
    rig.claim("carried-reason", &["sh", "-c", &command]);
    let mut carried = rig.read("state/jobs/carried-reason.json");
    carried["reason"] = json!("timeout");
    rig.put("state/jobs/carried-reason.json", &carried);
    // A pending claim without a directory, a host, or an owner is incomplete.
    for (id, field) in [
        ("no-cwd", "cwd"),
        ("no-host", "host"),
        ("no-owner", "owner"),
    ] {
        rig.claim(id, &["sh", "-c", &command]);
        let mut claim = rig.read(&format!("state/jobs/{id}.json"));
        claim[field] = json!("");
        rig.put(&format!("state/jobs/{id}.json"), &claim);
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
        let before = rig.read(&format!("state/jobs/{id}.json"));
        let output = rig.run(&["exec", id, "--", "sh", "-c", &command]);
        assert_eq!(output.status.code(), Some(2), "{id}");
        // The record is unchanged: nothing started, nothing ended, nothing ran.
        assert_eq!(rig.read(&format!("state/jobs/{id}.json")), before, "{id}");
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
    let job = rig.read("state/jobs/claimed.json");
    assert!(job["started_ms"].is_null());
    assert!(job["ended_ms"].is_null());
}

#[test]
fn run_wait_captures_exit_output_metadata_and_owner() {
    let rig = Rig::new("run-wait");
    rig.snapshot("state/host.json", now(), 0.0, 8);
    let output = rig.run(&[
        "run",
        "--wait",
        "--",
        "sh",
        "-c",
        "echo hi; echo err >&2; exit 3",
    ]);
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
        if let Ok(bytes) = fs::read(rig.dir.join(format!("at-close/{id}.json")))
            && let Ok(record) = serde_json::from_slice::<Value>(&bytes)
        {
            break record;
        }
        assert!(Instant::now() < deadline, "wrapper did not close its pane");
        std::thread::yield_now();
    };
    let job = rig.read(&format!("state/jobs/{id}.json"));
    assert_eq!(job["exit"], 3);
    assert_eq!(job["state"], "blocked");
    let log = rig.log(&format!("state/jobs/{id}.log"));
    assert!(log.contains("hi\n") && log.contains("err\n"));
    assert_eq!(rig.log("job-output"), log);
    assert_eq!(rig.read("state/items.json")[0]["owner_pane"], "w1:p1");
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
    assert!(rig.run(&["serve", "--once"]).status.success());
    assert_eq!(rig.read("state/host.json")["jobs"][0]["exit"], 3);
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_number());
}

#[test]
fn exec_signal_and_timeouts_persist_terminal_result_and_kill_descendants() {
    let rig = Rig::new("exec");
    rig.claim("signal", &["sh", "-c", "kill -TERM $$"]);
    let out = rig.run(&["exec", "signal", "--", "sh", "-c", "kill -TERM $$"]);
    assert_eq!(out.status.code(), Some(143));
    assert_eq!(rig.read("state/jobs/signal.json")["signal"], 15);
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
        let result = rig.read(&format!("state/jobs/{id}.json"));
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
    assert_eq!(rig.read("state/jobs/ok.json")["exit"], 0);
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
    assert!(rig.read("state/jobs/bad.json")["ended_ms"].is_number());
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
        assert_eq!(output.status.code(), Some(code));
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("dogsled pane close:"), "{stderr}");
        assert!(stderr.contains("close-refused"), "{stderr}");
        let job = rig.read(&format!("state/jobs/{id}.json"));
        assert_eq!(job["exit"], code);
        assert_eq!(job["reason"], Value::Null);
        assert!(job["ended_ms"].is_number());
        assert_eq!(rig.read(&format!("at-close/{id}.json")), job);
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
    let job = rig.read("state/jobs/spawn-error.json");
    assert_eq!(rig.read("at-close/spawn-error.json"), job);
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
    let job = rig.read("state/jobs/no-pane.json");
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
    assert!(rig.read("state/jobs/no-current-pane.json")["ended_ms"].is_number());
    assert!(rig.log("herdr.log").contains("pane close w2:p1"));
}

#[test]
fn wait_max_and_stop_use_durable_local_job_state() {
    let rig = Rig::new("stop-wait");
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    let mut job = complete("running");
    job["ended_ms"] = Value::Null;
    job["exit"] = Value::Null;
    rig.put("state/jobs/running.json", &job);
    let out = rig.run(&["wait", "running", "--max", "0ms"]);
    assert_eq!(out.status.code(), Some(124));
    assert_eq!(out.stdout, b"still running\n");
    assert!(rig.run(&["stop", "running"]).status.success());
    assert_eq!(rig.read("state/jobs/running.stop"), true);
    assert_eq!(rig.run(&["stop", "../escape"]).status.code(), Some(2));
}

#[test]
fn agent_places_starts_with_its_task_and_tracks_any_kind() {
    let rig = Rig::new("agent");
    let confirmed = now();
    fs::write(rig.dir.join("clock"), confirmed.to_string()).unwrap();
    rig.snapshot("state/host.json", confirmed, 0.0, 8);
    let (code, result) = rig.json(&[
        "agent",
        "--kind",
        "codex",
        "--name",
        "worker",
        "--message",
        "literal ' message",
    ]);
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
    assert_eq!(rig.read("state/items.json")[0]["kind"], "agent");
    assert_eq!(
        rig.read("state/items.json")[0]["launch_confirmed_ms"],
        confirmed
    );
    assert_eq!(
        rig.json(&["track", "local", "w3:p1", "--name", "other"]).0,
        0
    );
    assert_eq!(rig.read("state/items.json").as_array().unwrap().len(), 2);
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
    rig.snapshot("state/host.json", now(), 99.0, 2);
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
    assert_eq!(rig.read("state/items.json").as_array().unwrap().len(), 8);
    assert_eq!(rig.read("state/queue.json").as_array().unwrap().len(), 8);
}

#[test]
fn queue_admission_is_fifo_and_the_owner_hears_through_its_inbox() {
    let rig = Rig::new("queue-admit");
    rig.config("free_core_fraction_min = 2\n");
    rig.snapshot("state/host.json", now(), 0.0, 8);
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
    let tickets = rig.read("state/queue.json");
    assert_eq!(tickets.as_array().unwrap().len(), 3);
    assert_eq!(rig.json(&["items"]).1["queue"][2]["waiting"], false);
    assert_eq!(
        rig.run(&["cancel", second["ticket"].as_str().unwrap()])
            .status
            .code(),
        Some(0)
    );
    assert!(
        String::from_utf8(rig.run(&["status", "--brief"]).stdout)
            .unwrap()
            .contains("2 queued")
    );
    // Admission behavior is independent of the CI runner's real probe load/disk.
    // Exact capacity thresholds and FIFO blocking use the injected-time unit fixture.
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    let owner = json!({"result":{"agents":[{"pane_id":"w1:p1","agent":"pi",
        "agent_status":"working","terminal_id":"term_owner"}]}});
    rig.put("agents.json", &owner);
    assert!(rig.run(&["serve", "--once"]).status.success());
    let tickets = rig.read("state/queue.json");
    assert_eq!(tickets[0]["id"], first["ticket"]);
    assert_eq!(tickets[0]["state"], "admitted");
    assert_eq!(tickets[1]["state"], "cancelled");
    assert_eq!(tickets[2]["state"], "admitted");
    // The owner has no inbox reader yet, so nothing tells it, and nothing types into it.
    assert_eq!(tickets[0]["notified"], false);
    // Once it reads its inbox, the notice arrives there, while the owner still works.
    assert_eq!(rig.json(&["inbox", "claim", "--harness", "pi"]).0, 0);
    assert!(rig.run(&["serve", "--once"]).status.success());
    assert_eq!(rig.read("state/queue.json")[0]["notified"], true);
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
    fs::write(rig.dir.join("state/items.json"), serde_json::to_vec(&json!([{
        "id":"remote-job","kind":"job","host":"8103","pane":"w9:p1","owner_pane":"w1:p1","created_ms":0,"closed_ms":null,"name":null
    }])).unwrap()).unwrap();
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
    assert!(rig.dir.join("state/jobs/aged.json").exists());
    // One millisecond more: the log is gone and the record says so.
    set_clock(&rig, ended + LOG_WINDOW_MS + 1);
    serve(&rig);
    assert_eq!(log_of(&rig, "aged").as_deref(), Some("output expired\n"));
    let job = rig.read("state/jobs/aged.json");
    assert_eq!(job["output_expired"], true, "{job}");
    // The record itself is still there: only a day passed.
    assert!(rig.dir.join("state/jobs/aged.json").exists());
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
    assert!(rig.dir.join("state/jobs/aged.json").exists());
    // One millisecond more: the record and its log are gone.
    set_clock(&rig, ended + RECORD_WINDOW_MS + 1);
    serve(&rig);
    assert!(!rig.dir.join("state/jobs/aged.json").exists());
    assert!(!rig.dir.join("state/jobs/aged.log").exists());
}

/// Retention uses the durable high-water time after the wall clock moves back.
#[test]
fn retention_clock_rollback_preserves_exact_boundaries() {
    let rig = Rig::new("retention-clock-rollback");
    let ended = now();
    let high_water = ended + LOG_WINDOW_MS + 1;
    rig.put(
        "state/job-validity-clock.json",
        &json!({"version":1,"high_water_ms":high_water}),
    );
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
    assert!(rig.dir.join("state/jobs/month-boundary.json").exists());
    assert_eq!(
        rig.read("state/job-validity-clock.json")["high_water_ms"],
        high_water
    );
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
    assert!(rig.dir.join("state/jobs/running.json").exists());
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
    assert!(rig.dir.join("state/jobs/uncertain.json").exists());
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
    rig.put("state/jobs/aged.json.new", &expired);
    fs::rename(
        rig.dir.join("state/jobs/aged.json.new"),
        rig.dir.join("state/jobs/aged.json"),
    )
    .unwrap();
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
    rig.put("peer-state/jobs/aged.json", &running);
    fs::write(
        rig.dir.join("peer-state/jobs/aged.log"),
        "output already consumed before retention\n",
    )
    .unwrap();
    let mut reader = partially_read_logs(&rig);
    // The owner keeps its item after remote retention removes the record.
    fs::remove_file(rig.dir.join("peer-state/jobs/aged.json")).unwrap();
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
    assert!(!rig.dir.join("state/jobs/aged.json").exists());
    assert!(!rig.dir.join("state/jobs/aged.log").exists());
    rig.put(
        "state/items.json",
        &json!([{
            "id":"aged","kind":"job","host":"local","pane":"w2:p1",
            "owner_pane":"w1:p1","created_ms":ended,"closed_ms":ended,"name":null
        }]),
    );
    rig
}

#[test]
fn expired_record_read_reports_expiry_to_the_owner() {
    let rig = expired_item_rig("retention-gone-read");
    let (code, read) = rig.json(&["read", "aged"]);
    assert_eq!(code, 0, "{read}");
    assert_eq!(read["tail"], EXPIRED, "{read}");
    assert!(!rig.dir.join("state/jobs/aged.json").exists());
}

#[test]
fn expired_record_tail_reports_expiry_to_the_owner() {
    let rig = expired_item_rig("retention-gone-tail");
    let (code, tail) = rig.json(&["host", "tail", "job", "aged"]);
    assert_eq!(code, 0, "{tail}");
    assert_eq!(tail, EXPIRED, "{tail}");
    assert!(!rig.dir.join("state/jobs/aged.json").exists());
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
    assert!(!rig.dir.join("state/jobs/aged.json").exists());
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
    rig.put(
        "state/items.json",
        &json!([{
            "id":"aged","kind":"job","host":"8103","pane":"w9:p1",
            "owner_pane":"w1:p1","created_ms":0,"closed_ms":1,"name":null
        }]),
    );
    rig
}

#[test]
fn remote_expired_record_read_reports_expiry_to_the_owner() {
    let rig = remote_expired_item_rig("remote-expired-read");
    let (code, read) = rig.json(&["read", "aged"]);
    assert_eq!(code, 0, "{read}");
    assert_eq!(read["tail"], EXPIRED, "{read}");
    assert!(!rig.dir.join("peer-state/jobs/aged.json").exists());
    assert!(!rig.log("ssh.log").contains("host launch"));
}

#[test]
fn remote_expired_record_tail_reports_expiry_to_the_owner() {
    let rig = remote_expired_item_rig("remote-expired-tail");
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
    for args in [vec!["logs", "aged"], vec!["logs", "aged", "--follow"]] {
        let mut command = rig.start(&args);
        assert!(command.wait().success());
        let (stdout, stderr) = command.output();
        assert_eq!(stdout, format!("{EXPIRED}\n"), "{stderr}");
    }
    assert!(!rig.dir.join("peer-state/jobs/aged.json").exists());
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
    assert!(!rig.dir.join("state/jobs/aged.json").exists());
    // A second pass at the same time is a no-op.
    serve(&rig);
    assert!(!rig.dir.join("state/jobs/aged.json").exists());
    assert!(!rig.dir.join("state/jobs/aged.log").exists());
    // And a pass at a later time is a no-op too.
    set_clock(&rig, ended + 100 * RECORD_WINDOW_MS);
    serve(&rig);
    assert!(!rig.dir.join("state/jobs/aged.json").exists());
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
