use super::close::git;
use super::*;
use std::io::Write as _;

pub(super) fn prepared(name: &str, id: &str) -> (Rig, PathBuf) {
    let (rig, repo) = place::place_rig(name);
    let (code, result) = rig.json(&[
        "host",
        "prepare",
        "--preparation-protocol",
        "1",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "retire",
        "--base",
        "main",
        "--request-id",
        id,
    ]);
    assert_eq!(code, 0, "{result}");
    (rig, repo)
}

fn retire(rig: &Rig, id: &str) -> (i32, Value) {
    rig.json(&[
        "retire-preparation",
        id,
        "--cleanup",
        "--reason",
        "operator reconciles historical ownership",
    ])
}

fn remote(rig: &Rig) {
    rig.peer();
    let source = rig.dir.join("state/preparation-receipts");
    let target = rig.dir.join("peer-state/preparation-receipts");
    fs::create_dir_all(&target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.path().extension().is_some_and(|ext| ext == "json") {
            fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
    rig.put(
        "peer-state/preparations.json",
        &rig.read("state/preparations.json"),
    );
    rig.script(
        "ssh",
        &format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/ssh.log"
for remote do :; done
remote=${{remote#* host }}
case "$remote" in
  'preparation-lookup '*|'prepared '*)
    if test -f "$RIG/unreachable-release"; then exit 255; fi
    DOGSLED_STATE="$RIG/peer-state" sh -c '{} host '"$remote" > "$RIG/retirement-response"
    status=$?; test "$status" = 0 || exit "$status"
    case "$remote" in
      'prepared '*)
        if test -f "$RIG/lose-retirement-ack"; then rm "$RIG/lose-retirement-ack"; exit 255; fi ;;
    esac
    cat "$RIG/retirement-response" ;;
  *) exit 255 ;;
esac
"#,
            env!("CARGO_BIN_EXE_dogsled")
        ),
    );
}

fn remote_retire(rig: &Rig) -> (i32, Value) {
    rig.json(&[
        "retire-preparation",
        "a",
        "--host",
        "netcup",
        "--cleanup",
        "--reason",
        "audit",
    ])
}

fn service(rig: &Rig) {
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    rig.snapshot("state/host.json", now(), 0.0, 8);
    let output = rig.run(&["serve", "--once"]);
    assert!(output.status.success(), "{output:?}");
}

fn retired(result: &Value) {
    for (field, value) in [
        ("resource", "preparation"),
        ("status", "retired"),
        ("outcome", "unknown"),
        ("future_dispatch", "prohibited"),
        ("ownership", "released"),
        ("task", "unresolved"),
    ] {
        assert_eq!(result[field], value, "{result}");
    }
    assert!(result["receipt_id"].is_string(), "{result}");
}

#[test]
fn two_requests_on_one_path_retire_independently() {
    let (rig, repo) = prepared("retire-shared", "a");
    let (code, second) = rig.json(&[
        "host",
        "prepare",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "retire",
        "--request-id",
        "b",
    ]);
    assert_eq!(code, 0, "{second}");
    let claims = rig.read("state/preparations.json");
    let b = claims[1].clone();
    let b_receipt = fs::read(rig.dir.join("state/preparation-receipts/b.json")).unwrap();
    let path = PathBuf::from(claims[0]["path"].as_str().unwrap());
    let before = rig.log("herdr.log");
    let original = rig.read("state/preparation-receipts/a.json");
    let (code, result) = retire(&rig, "a");
    assert_eq!(code, 0, "{result}");
    retired(&result);
    assert_eq!(rig.read("state/preparations.json"), json!([b]));
    assert_eq!(
        fs::read(rig.dir.join("state/preparation-receipts/b.json")).unwrap(),
        b_receipt
    );
    assert!(path.exists());
    for effect in ["worktree open", "pane close", "agent start", "pane run"] {
        assert_eq!(
            rig.log("herdr.log").matches(effect).count(),
            before.matches(effect).count()
        );
    }
    let mut completed = rig.read("state/preparation-receipts/a.json");
    assert_eq!(
        completed["finished_ms"],
        rig.read("state/preparation-receipts/a.disposition.json")["finished_ms"]
    );
    assert!(
        completed
            .as_object_mut()
            .unwrap()
            .remove("finished_ms")
            .unwrap()
            .is_u64()
    );
    assert_eq!(completed, original);
    let (code, doctor) = rig.json(&["doctor"]);
    assert_eq!(code, 0, "{doctor}");
    assert!(
        doctor.to_string().contains("pending preparation"),
        "{doctor}"
    );
    assert_eq!(retire(&rig, "a"), (0, result));
}

#[test]
fn retirement_completes_raw_provenance_and_recovers_only_the_saved_time() {
    for explicit_null in [false, true] {
        let (rig, _) = prepared(&format!("retire-completion-{explicit_null}"), "a");
        let file = "state/preparation-receipts/a.json";
        let mut original = rig.read(file);
        original["historical_metadata"] = json!({"retained":true});
        if explicit_null {
            original["finished_ms"] = Value::Null;
        }
        rig.put(file, &original);
        let (code, result) = retire(&rig, "a");
        assert_eq!(code, 0, "{result}");
        retired(&result);
        let completed = rig.read(file);
        let fence = rig.read("state/preparation-receipts/a.disposition.json");
        assert_eq!(completed["finished_ms"], fence["finished_ms"]);
        let mut expected = original.clone();
        expected["finished_ms"] = fence["finished_ms"].clone();
        assert_eq!(completed, expected);
        // Simulate interruption after release but before the outcome timestamp was copied.
        rig.put(file, &original);
        fs::write(rig.dir.join("clock"), (now() + 1000).to_string()).unwrap();
        let (code, replay) =
            rig.json(&["host", "prepared", "--input", &fence["request"].to_string()]);
        assert_eq!(code, 0, "{replay}");
        assert_eq!(rig.read(file), completed);
        assert_eq!(
            rig.read("state/preparation-receipts/a.disposition.json"),
            fence
        );
    }
}

#[test]
fn retirement_replay_rejects_changed_completion_or_provenance() {
    for field in ["finished_ms", "prepared"] {
        let (rig, _) = prepared(&format!("retire-mutated-{field}"), "a");
        let (code, result) = retire(&rig, "a");
        assert_eq!(code, 0, "{result}");
        let fence = rig.read("state/preparation-receipts/a.disposition.json");
        let mut receipt = rig.read("state/preparation-receipts/a.json");
        receipt[field] = if field == "finished_ms" {
            json!(receipt[field].as_u64().unwrap() + 1)
        } else {
            Value::Null
        };
        rig.put("state/preparation-receipts/a.json", &receipt);
        let (code, result) =
            rig.json(&["host", "prepared", "--input", &fence["request"].to_string()]);
        assert_ne!(code, 0, "{result}");
        assert_eq!(rig.read("state/preparation-receipts/a.json"), receipt);
        assert_eq!(
            rig.read("state/preparation-receipts/a.disposition.json"),
            fence
        );
        assert_eq!(rig.read("state/preparations.json"), json!([]));
    }
}

#[test]
fn lost_ack_replays_the_same_retirement() {
    let (rig, _) = prepared("retire-lost-ack", "a");
    remote(&rig);
    fs::write(rig.dir.join("lose-retirement-ack"), "").unwrap();
    let (code, pending) = remote_retire(&rig);
    assert_eq!(code, 1, "{pending}");
    assert_eq!(pending["status"], "pending");
    assert_eq!(pending["ownership"], "pending");
    assert_eq!(rig.read("peer-state/preparations.json"), json!([]));
    let receipt = rig.read("peer-state/preparation-receipts/a.disposition.json");
    let (code, replay) = remote_retire(&rig);
    assert_eq!(code, 0, "{replay}");
    retired(&replay);
    assert_eq!(replay["audit"], pending["audit"]);
    assert_eq!(replay["receipt_id"], pending["receipt_id"]);
    assert_eq!(
        rig.read("peer-state/preparation-receipts/a.disposition.json"),
        receipt
    );
    fs::write(rig.dir.join("unreachable-release"), "").unwrap();
    assert_eq!(remote_retire(&rig), (0, replay));
}

#[test]
fn unreachable_peer_reports_pending_not_released() {
    let (rig, _) = prepared("retire-unreachable", "a");
    remote(&rig);
    // Capture and authorize once, then lose transport before the host transaction.
    let output = rig
        .cmd(&[
            "retire-preparation",
            "a",
            "--host",
            "netcup",
            "--cleanup",
            "--reason",
            "audit",
        ])
        .env("DOGSLED_TEST_UNIT_CRASH", "retirement-intent-saved")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(86), "{output:?}");
    let claims = rig.read("peer-state/preparations.json");
    fs::write(rig.dir.join("unreachable-release"), "").unwrap();
    let (code, pending) = remote_retire(&rig);
    assert_eq!(code, 1, "{pending}");
    assert_eq!(pending["ownership"], "pending");
    assert_eq!(rig.read("peer-state/preparations.json"), claims);
    service(&rig);
    assert_eq!(rig.read("peer-state/preparations.json"), claims);
    fs::remove_file(rig.dir.join("unreachable-release")).unwrap();
    service(&rig);
    let (code, result) = remote_retire(&rig);
    assert_eq!(code, 0, "{result}");
    retired(&result);
    assert_eq!(result["audit"], pending["audit"]);
    assert_eq!(rig.read("peer-state/preparations.json"), json!([]));
}

#[test]
fn replacement_checkout_is_never_touched() {
    let (rig, repo) = prepared("retire-replacement", "a");
    let claim = rig.read("state/preparations.json")[0].clone();
    let path = PathBuf::from(claim["path"].as_str().unwrap());
    let old = rig.dir.join("old-checkout");
    fs::rename(&path, &old).unwrap();
    fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-q", "-b", "main"]);
    fs::write(path.join("keep"), "replacement").unwrap();
    let replacement = git(&path, &["status", "--porcelain"]);
    let before = rig.log("setup.log");
    let (code, result) = retire(&rig, "a");
    assert_eq!(code, 0, "{result}");
    retired(&result);
    assert_eq!(
        fs::read_to_string(path.join("keep")).unwrap(),
        "replacement"
    );
    assert_eq!(git(&path, &["status", "--porcelain"]), replacement);
    assert!(!path.join(".git/dogsled-generation").exists());
    assert_eq!(rig.log("setup.log"), before);
    assert!(repo.exists());
    assert!(old.exists());
}

fn barrier(rig: &Rig, phase: &str, cleanup: bool) -> (Owned, Fifo) {
    let go = Fifo::new(&rig.dir, "retirement.go");
    let mut command = rig.cmd(&["retire-preparation", "a", "--reason", "audit"]);
    if cleanup {
        command.arg("--cleanup");
    }
    command
        .env(
            format!("DOGSLED_TEST_LAUNCH_{phase}"),
            rig.dir.join("retirement"),
        )
        .stdout(fs::File::create(rig.dir.join("retirement-out")).unwrap())
        .stderr(fs::File::create(rig.dir.join("retirement-err")).unwrap());
    let process = Owned::spawn(&mut command);
    read_command(&rig.dir.join("retirement.ready"), "retirement capture");
    (process, go)
}

#[test]
fn retirement_barrier_keeps_release_before_reader_opens() {
    let rig = Rig::new("retirement-early-release");
    let go = Fifo::new(&rig.dir, "retirement.go");
    // The ready file can reach the coordinator before the child opens its FIFO.
    // Closing the temporary writer must not discard the release in that interval.
    writeln!(go.hold(), "go").unwrap();
    assert_eq!(
        read_fifo_within(
            go.path(),
            "early retirement release",
            Duration::from_millis(50)
        ),
        Some("go\n".into())
    );
}

#[test]
fn changed_claim_or_custody_refuses_without_rebinding() {
    let (rig, _) = prepared("retire-changed-claim", "a");
    let (mut process, go) = barrier(&rig, "RETIREMENT_BEFORE_DISPOSE", true);
    let mut claims = rig.read("state/preparations.json");
    claims[0]["head"] = json!("replacement-provenance");
    rig.put("state/preparations.json", &claims);
    writeln!(go.hold(), "go").unwrap();
    assert!(!process.wait().success());
    assert_eq!(rig.read("state/preparations.json"), claims);
    assert!(
        !rig.dir
            .join("state/preparation-receipts/a.disposition.json")
            .exists()
    );
    let saved = rig.read("state/preparation-retirements/a.json");
    assert_ne!(
        saved["disposition"]["binding"]["claim"]["tree"]["head"],
        claims[0]["head"]
    );

    let (rig, _) = prepared("retire-changed-custody", "a");
    rig.put("agents.json", &json!({"result":{"agents":[{"pane_id":"w1:p1","agent":"pi","agent_session":{"kind":"id","value":"owner-session"}}]}}));
    let host = String::from_utf8(Command::new("hostname").output().unwrap().stdout)
        .unwrap()
        .trim()
        .to_owned();
    let custody = json!([{"item":"a","host":host,"owner_pane":"w1:p1","owner_session":"owner-session","custodian_pane":"w1:p1","custodian_session":"owner-session","revision":0,"claims":[]}]);
    rig.put("state/custody.json", &custody);
    let claims = rig.read("state/preparations.json");
    let (mut process, go) = barrier(&rig, "RETIREMENT_CAPTURED", false);
    let mut moved = custody;
    moved[0]["revision"] = json!(1);
    rig.put("state/custody.json", &moved);
    writeln!(go.hold(), "go").unwrap();
    assert!(!process.wait().success());
    assert_eq!(rig.read("state/preparations.json"), claims);
    assert!(
        !rig.dir
            .join("state/preparation-retirements/a.json")
            .exists()
    );
}

#[test]
fn job_ownership_survives_preparation_retirement() {
    for state in ["pending", "running"] {
        let (rig, _) = prepared(&format!("retire-job-{state}"), "a");
        rig.claim("a", &["true"]);
        let file = "state/jobs/a.json";
        let mut job = rig.read(file);
        job["state"] = json!(state);
        let mut tree = rig.read("state/preparations.json")[0].clone();
        tree.as_object_mut().unwrap().remove("request_id");
        job["cwd"] = tree["path"].clone();
        job["startup"] = json!({"version":1,"scope":"git","checkout":tree,"preparation":rig.read("state/preparation-receipts/a.json")["binding"]});
        if state == "running" {
            job["started_ms"] = json!(rig.origin_ms);
        }
        rig.put(file, &job);
        let before = fs::read(rig.dir.join(file)).unwrap();
        let (code, result) = retire(&rig, "a");
        assert_eq!(code, 0, "{result}");
        retired(&result);
        assert_eq!(fs::read(rig.dir.join(file)).unwrap(), before);
        let (code, doctor) = rig.json(&["doctor"]);
        assert_eq!(code, 0, "{doctor}");
        assert!(
            doctor.to_string().contains("job a owns this checkout"),
            "{doctor}"
        );
        let output = rig.run(&["exec", "a", "--job-validity", "2", "--", "true"]);
        assert!(!output.status.success(), "{output:?}");
        assert_eq!(fs::read(rig.dir.join(file)).unwrap(), before);
    }
}

#[test]
fn retired_resource_leaves_unit_unresolved() {
    let (rig, _) = prepared("retire-unit", "a");
    let request = json!({"id":"a","action":{"type":"agent","kind":"pi","name":"worker","message":"task"},"host":"local","branch":"retire","base":"main","cwd":rig.dir.join("home"),"repository":null,"workspace":"w1","workspace_name":"caller","owner":"w1:p1"});
    let execution = json!({"executor":"dogsled","ownerId":"w1:p1","requestId":"a"});
    let unit = json!({"origin":unit_requests::exported(0)["contract"]["ref"],"execution":execution,"placement":{},"tool_call_id":"call","abandoned":null,"committed":unit_requests::commit_input(&execution, 0)});
    fs::create_dir_all(rig.dir.join("state/admissions")).unwrap();
    rig.put("state/admissions/a.json", &json!({"request":request,"host":{"id":"local","label":"local"},"launch":null,"error":null,"unit":unit}));
    rig.put_tickets(&json!([{"id":"ticket","request":request,"created_ms":rig.origin_ms,"state":"failed","waiting":false,"notified":false}]));
    let before = rig.read("state/admissions/a.json");
    let (code, result) = retire(&rig, "a");
    assert_eq!(code, 0, "{result}");
    retired(&result);
    for host_only in [false, true] {
        if host_only {
            fs::remove_file(rig.dir.join("state/preparation-retirements/a.json")).unwrap();
        }
        service(&rig);
        assert_eq!(rig.read("state/admissions/a.json"), before);
        assert_eq!(
            rig.read("state/admissions/a.json")["unit"]["abandoned"],
            Value::Null
        );
        let (code, observed) =
            unit_requests::operation(&rig, "observe", &unit_requests::execution_input(&execution));
        assert_eq!(code, 0, "{observed}");
        assert!(observed["settlement"].is_null(), "{observed}");
        let (code, settlements) = rig.json(&["unit", "settlements"]);
        assert_eq!(code, 0, "{settlements}");
        assert_eq!(settlements, json!([]));
    }
}

#[test]
fn retirement_and_dispatch_share_the_request_gate() {
    for retire_first in [true, false] {
        let (rig, _) = prepared(&format!("retire-dispatch-{retire_first}"), "a");
        let prepared = rig.read("state/preparation-receipts/a.json")["prepared"].clone();
        let binding = rig.read("state/preparation-receipts/a.json")["binding"].clone();
        let launch = json!({"id":"a","validity":rig.job_validity(),"action":{"type":"agent","kind":"pi","name":"worker","message":"go"},"host":"local","cwd":prepared["path"],"pane":prepared["pane_id"],"terminal":prepared["terminal_id"],"workspace":"w1","workspace_name":"caller","owner":"parent","preparation":binding,"preparation_protocol":1});
        let (mut retirement, retire_go) = barrier(
            &rig,
            if retire_first {
                "RETIREMENT_ENTERED_GATE"
            } else {
                "RETIREMENT_BEFORE_DISPOSE"
            },
            true,
        );
        let ready = Fifo::new(&rig.dir, "dispatch.ready");
        let go = Fifo::new(&rig.dir, "dispatch.go");
        let _ready = ready.hold();
        let _go = go.hold();
        let mut command = rig.cmd(&["host", "launch", "--request", &launch.to_string()]);
        command
            .env(
                if retire_first {
                    "DOGSLED_TEST_LAUNCH_BEFORE_GATE"
                } else {
                    "DOGSLED_TEST_LAUNCH_ENTERED_GATE"
                },
                rig.dir.join("dispatch"),
            )
            .stdout(fs::File::create(rig.dir.join("dispatch-out")).unwrap())
            .stderr(fs::File::create(rig.dir.join("dispatch-err")).unwrap());
        let mut dispatch = Owned::spawn(&mut command);
        read_fifo(ready.path(), "dispatch gate");
        // The winning operation owns the same kernel preparation gate.
        let gate = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(rig.dir.join("state/preparation-receipts/a.lock"))
            .unwrap();
        assert!(matches!(gate.try_lock(), Err(fs::TryLockError::WouldBlock)));
        writeln!(retire_go.hold(), "go").unwrap();
        writeln!(go.hold(), "go").unwrap();
        let retired_status = retirement.wait();
        let dispatched_status = dispatch.wait();
        let fence = rig.read("state/preparation-receipts/a.disposition.json");
        if retire_first {
            assert!(retired_status.success(), "{}", rig.log("retirement-out"));
            assert!(!dispatched_status.success(), "{}", rig.log("dispatch-out"));
            assert_eq!(fence["request"]["outcome"], "unknown");
            assert!(!rig.log("herdr.log").contains("agent start"));
            assert!(!rig.dir.join("state/launches/a.json").exists());
        } else {
            assert!(
                dispatched_status.success(),
                "{} {}",
                rig.log("dispatch-out"),
                rig.log("dispatch-err")
            );
            assert!(!retired_status.success(), "{}", rig.log("retirement-out"));
            assert_eq!(fence["request"]["policy"]["kind"], "transfer");
            assert_eq!(fence["request"]["outcome"], "possibly_executed");
            let saved = rig.read("state/launches/a.json");
            assert_eq!(
                rig.json(&["host", "launch", "--request", &launch.to_string()]),
                (0, saved)
            );
            assert_eq!(
                rig.log("herdr.log").matches("agent start worker ").count(),
                1
            );
        }
        assert_eq!(rig.read("state/preparations.json"), json!([]));
        assert_ne!(fence["request"]["outcome"], "not_started");
    }
}

#[test]
fn identified_legacy_metadata_retires_without_discovering_identity() {
    let (rig, _) = prepared("retire-legacy", "a");
    let mut claims = rig.read("state/preparations.json");
    claims[0].as_object_mut().unwrap().remove("identity");
    rig.put("state/preparations.json", &claims);
    fs::remove_file(rig.dir.join("state/preparation-receipts/a.json")).unwrap();
    let (code, result) = retire(&rig, "a");
    assert_eq!(code, 0, "{result}");
    retired(&result);
    let fence = rig.read("state/preparation-receipts/a.disposition.json");
    assert!(fence["request"]["binding"]["claim"]["tree"]["identity"].is_null());
    assert_eq!(rig.read("state/preparations.json"), json!([]));
    assert_eq!(retire(&rig, "a"), (0, result));
    let repo = claims[0]["repo"].as_str().unwrap();
    let output = rig.run(&[
        "host",
        "prepare",
        "--repo",
        repo,
        "--branch",
        "retire",
        "--request-id",
        "a",
    ]);
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(rig.read("state/preparations.json"), json!([]));
    assert!(!rig.dir.join("state/preparation-receipts/a.json").exists());
}

#[test]
fn requestless_and_unknown_claims_refuse() {
    let (rig, _) = prepared("retire-refusals", "a");
    let original = rig.read("state/preparations.json");
    for id in ["", "unknown", "../a"] {
        assert!(
            !rig.run(&["retire-preparation", id, "--cleanup", "--reason", "audit"])
                .status
                .success()
        );
        assert_eq!(rig.read("state/preparations.json"), original);
    }
    let mut requestless = original.clone();
    requestless[0].as_object_mut().unwrap().remove("request_id");
    rig.put("state/preparations.json", &requestless);
    assert_ne!(retire(&rig, "a").0, 0);
    assert_eq!(rig.read("state/preparations.json"), requestless);
    let mut duplicate = original.clone();
    duplicate.as_array_mut().unwrap().push(original[0].clone());
    rig.put("state/preparations.json", &duplicate);
    assert_ne!(retire(&rig, "a").0, 0);
    assert_eq!(rig.read("state/preparations.json"), duplicate);
    assert!(
        !rig.dir
            .join("state/preparation-receipts/a.disposition.json")
            .exists()
    );
}

#[test]
fn cleanup_authority_is_explicit_and_audited() {
    let (rig, _) = prepared("retire-authority", "a");
    let (code, _) = rig.json(&["retire-preparation", "a", "--reason", "owner absent"]);
    assert_ne!(code, 0);
    let (code, result) = retire(&rig, "a");
    assert_eq!(code, 0, "{result}");
    retired(&result);
    assert_eq!(result["audit"]["authority"]["kind"], "cleanup");
    assert!(result["audit"]["actor"]["user"].is_string());
    assert!(result["audit"]["actor"]["host"].is_string());
    assert_eq!(retire(&rig, "a"), (0, result));
}
