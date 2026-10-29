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
    let (rig, _) = prepared("retire-shared", "a");
    let mut claims = rig.read("state/preparations.json");
    let mut b = claims[0].clone();
    b["request_id"] = json!("b");
    claims.as_array_mut().unwrap().push(b.clone());
    rig.put("state/preparations.json", &claims);
    let path = PathBuf::from(claims[0]["path"].as_str().unwrap());
    let before = rig.log("herdr.log");
    let original = rig.read("state/preparation-receipts/a.json");
    let (code, result) = retire(&rig, "a");
    assert_eq!(code, 0, "{result}");
    retired(&result);
    assert_eq!(rig.read("state/preparations.json"), json!([b]));
    assert!(path.exists());
    for effect in ["worktree open", "pane close", "agent start", "pane run"] {
        assert_eq!(
            rig.log("herdr.log").matches(effect).count(),
            before.matches(effect).count()
        );
    }
    assert_eq!(rig.read("state/preparation-receipts/a.json"), original);
    assert_eq!(retire(&rig, "a"), (0, result));
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
        job["cwd"] = rig.read("state/preparations.json")[0]["path"].clone();
        rig.put(file, &job);
        let before = fs::read(rig.dir.join(file)).unwrap();
        let (code, result) = retire(&rig, "a");
        assert_eq!(code, 0, "{result}");
        retired(&result);
        assert_eq!(fs::read(rig.dir.join(file)).unwrap(), before);
        let (code, doctor) = rig.json(&["doctor"]);
        assert_eq!(code, 0, "{doctor}");
        assert!(doctor.to_string().contains("job"), "{doctor}");
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
    rig.put("state/queue.json", &json!([{"id":"ticket","request":request,"created_ms":rig.origin_ms,"state":"failed","waiting":false,"notified":false}]));
    let before = rig.read("state/admissions/a.json");
    let (code, result) = retire(&rig, "a");
    assert_eq!(code, 0, "{result}");
    retired(&result);
    for _ in 0..2 {
        service(&rig);
        assert_eq!(rig.read("state/admissions/a.json"), before);
        assert_eq!(
            rig.read("state/admissions/a.json")["unit"]["abandoned"],
            Value::Null
        );
    }
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
