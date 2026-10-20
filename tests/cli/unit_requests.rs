use super::*;

pub(super) fn exported(index: usize) -> Value {
    let fixtures: Value =
        serde_json::from_str(include_str!("../fixtures/unit-exports.json")).unwrap();
    let mut value: Value =
        serde_json::from_str(fixtures[index]["canonical"].as_str().unwrap()).unwrap();
    value["digest"] = fixtures[index]["digest"].clone();
    value
}

pub(super) fn rig(name: &str, full: bool) -> Rig {
    let rig = Rig::new(name);
    rig.snapshot("state/host.json", now(), if full { 3.0 } else { 0.0 }, 2);
    let dir = rig.dir.join("profiles/worker");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("profile.json"), json!({"description":"worker","model":"alias/sol","thinking":"medium","prompt":"prompt.md","pi_args":[],"role":"reader"}).to_string()).unwrap();
    fs::write(dir.join("prompt.md"), "Implement the unit.").unwrap();
    rig
}

pub(super) fn prepare(rig: &Rig) -> Value {
    let input = json!({"origin":exported(0)["contract"]["ref"],"placement":{}});
    let (code, execution) = rig.json(&[
        "unit",
        "prepare",
        "--input",
        &input.to_string(),
        "--tool-call-id",
        "call-1",
    ]);
    assert_eq!(code, 0, "{execution}");
    execution
}

pub(super) fn commit_input(execution: &Value, index: usize) -> Value {
    let export = exported(index);
    json!({"execution":execution,"binding":{"contract":export["contract"]["ref"],"execution":execution,"exportDigest":export["digest"]},"exported":export})
}

// Independent ordered projection; do not call the production naming helper.
pub(super) fn expected_name(execution: &Value, origin: &Value) -> String {
    use sha2::{Digest as _, Sha256};
    use std::fmt::Write as _;
    let field = |value: &Value| serde_json::to_string(value).unwrap();
    let bytes = format!(
        "[{{\"sessionId\":{},\"unitId\":{},\"revision\":{},\"contractEntryId\":{}}},{{\"executor\":{},\"ownerId\":{},\"requestId\":{}}}]",
        field(&origin["sessionId"]),
        field(&origin["unitId"]),
        field(&origin["revision"]),
        field(&origin["contractEntryId"]),
        field(&execution["executor"]),
        field(&execution["ownerId"]),
        field(&execution["requestId"])
    );
    let hex =
        Sha256::digest(bytes.as_bytes())
            .iter()
            .take(15)
            .fold(String::new(), |mut hex, byte| {
                write!(hex, "{byte:02x}").unwrap();
                hex
            });
    format!("u-{hex}")
}

pub(super) fn operation(rig: &Rig, operation: &str, input: &Value) -> (i32, Value) {
    rig.json(&["unit", operation, "--input", &input.to_string()])
}

pub(super) fn execution_input(execution: &Value) -> Value {
    json!({"execution":execution})
}

pub(super) fn admission(rig: &Rig, execution: &Value) -> Value {
    rig.read(&format!(
        "state/admissions/{}.json",
        execution["requestId"].as_str().unwrap()
    ))
}

fn no_launch(rig: &Rig) {
    assert!(!rig.dir.join("state/placements.json").exists());
    assert!(!rig.dir.join("state/queue.json").exists());
    let log = rig.log("herdr.log");
    for forbidden in ["tab create", "worktree open", "agent start", "pane close"] {
        assert!(!log.contains(forbidden), "{log}");
    }
}

#[test]
fn prepared_listing_is_owner_scoped_and_never_abandons_or_launches() {
    let rig = rig("unit-prepared-list", true);
    let execution = prepare(&rig);
    let (code, pending) = rig.json(&["unit", "prepared"]);
    assert_eq!(code, 0);
    assert_eq!(
        pending,
        json!([{ "execution": execution, "origin": exported(0)["contract"]["ref"], "toolCallId": "call-1" }])
    );
    let other = rig
        .cmd(&["unit", "prepared"])
        .env("HERDR_PANE_ID", "other")
        .output()
        .unwrap();
    assert!(other.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&other.stdout).unwrap(),
        json!([])
    );
    no_launch(&rig);
    assert_eq!(operation(&rig, "commit", &commit_input(&execution, 0)).0, 0);
    assert_eq!(rig.json(&["unit", "prepared"]).1, json!([]));
}

#[test]
fn abandoned_preparation_disappears_from_recovery_listing() {
    let rig = rig("unit-prepared-abandoned", false);
    let execution = prepare(&rig);
    assert_eq!(
        operation(&rig, "abandon-prepared", &execution_input(&execution)).0,
        0
    );
    assert_eq!(rig.json(&["unit", "prepared"]).1, json!([]));
    no_launch(&rig);
}

#[test]
fn prepare_is_durable_owner_scoped_and_side_effect_free() {
    let rig = rig("unit-prepare", false);
    let execution = prepare(&rig);
    assert_eq!(prepare(&rig), execution);
    assert_eq!(
        admission(&rig, &execution)["unit"]["tool_call_id"],
        "call-1"
    );
    no_launch(&rig);
    let (_, state) = operation(&rig, "observe", &execution_input(&execution));
    assert_eq!(state["phase"], "prepared");
    assert!(state["receipt"].is_null());
    let changed = json!({"origin":exported(0)["contract"]["ref"],"placement":{"host":"local"}});
    let (code, error) = rig.json(&[
        "unit",
        "prepare",
        "--input",
        &changed.to_string(),
        "--tool-call-id",
        "call-2",
    ]);
    assert_eq!(code, 2);
    assert_eq!(error["error"]["code"], "REQUEST_CONFLICT");
    let other = rig
        .cmd(&[
            "unit",
            "observe",
            "--input",
            &execution_input(&execution).to_string(),
        ])
        .env("HERDR_PANE_ID", "other")
        .output()
        .unwrap();
    assert_eq!(other.status.code(), Some(2));
    let same_origin = json!({"origin":exported(0)["contract"]["ref"],"placement":{}});
    let out = rig
        .cmd(&[
            "unit",
            "prepare",
            "--input",
            &same_origin.to_string(),
            "--tool-call-id",
            "other-call",
        ])
        .env("HERDR_PANE_ID", "other")
        .output()
        .unwrap();
    assert!(out.status.success());
    let other: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_ne!(other["requestId"], execution["requestId"]);
}

#[test]
fn concurrent_prepare_and_commit_return_one_request_and_one_ticket() {
    let rig = rig("unit-concurrent", true);
    let executions = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..6).map(|_| scope.spawn(|| prepare(&rig))).collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(executions.iter().all(|e| e == &executions[0]));
    let input = commit_input(&executions[0], 0);
    let receipts = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..6)
            .map(|_| scope.spawn(|| operation(&rig, "commit", &input)))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(receipts.iter().all(|r| r == &receipts[0]), "{receipts:?}");
    assert_eq!(receipts[0].0, 0);
    assert_eq!(receipts[0].1["disposition"], "queued");
    assert_eq!(rig.read("state/queue.json").as_array().unwrap().len(), 1);
    assert_eq!(
        operation(&rig, "commit", &commit_input(&executions[0], 1)).1["error"]["code"],
        "REQUEST_CONFLICT"
    );
    assert!(
        rig.run(&["cancel", receipts[0].1["ticket"].as_str().unwrap()])
            .status
            .success()
    );
    let (_, state) = operation(&rig, "observe", &execution_input(&executions[0]));
    assert_eq!(state["phase"], "unresolved");
    assert!(state["settlement"].is_null());
    assert_eq!(
        operation(&rig, "commit", &input).1["disposition"],
        "unresolved"
    );
}

#[test]
fn abandoned_after_claim_crash_needs_no_export_and_is_permanent() {
    let rig = rig("unit-abandon", false);
    let execution = prepare(&rig);
    // A provider can have claimed this ID; Dogsled never received its binding/export.
    let input = execution_input(&execution);
    let (code, event) = operation(&rig, "abandon-prepared", &input);
    assert_eq!(code, 0, "{event}");
    assert_eq!(event["result"]["kind"], "not_started");
    assert_eq!(event["origin"], exported(0)["contract"]["ref"]);
    assert_eq!(event["execution"], execution);
    assert!(event.get("binding").is_none());
    assert_eq!(operation(&rig, "abandon-prepared", &input).1, event);
    assert_eq!(operation(&rig, "observe", &input).1["settlement"], event);
    assert_eq!(prepare(&rig), execution);
    assert_eq!(
        operation(&rig, "commit", &commit_input(&execution, 0)).1["error"]["code"],
        "REQUEST_CONFLICT"
    );
    no_launch(&rig);
}

#[test]
fn abandon_and_commit_are_serialized_before_any_launch() {
    let rig = rig("unit-abandon-race", true);
    let execution = prepare(&rig);
    let commit = commit_input(&execution, 0);
    let abandon = execution_input(&execution);
    let (committed, abandoned) = std::thread::scope(|scope| {
        let a = scope.spawn(|| operation(&rig, "commit", &commit));
        let b = scope.spawn(|| operation(&rig, "abandon-prepared", &abandon));
        (a.join().unwrap(), b.join().unwrap())
    });
    assert_ne!(committed.0, abandoned.0, "{committed:?} {abandoned:?}");
    if abandoned.0 == 0 {
        no_launch(&rig);
    } else {
        assert_eq!(committed.0, 0);
        assert_eq!(abandoned.1["error"]["code"], "REQUEST_CONFLICT");
        assert_eq!(rig.read("state/queue.json").as_array().unwrap().len(), 1);
    }
}

fn crash(rig: &Rig, input: &Value, point: &str) {
    let out = rig
        .cmd(&["unit", "commit", "--input", &input.to_string()])
        .env("DOGSLED_TEST_UNIT_CRASH", point)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(86),
        "{} {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn serve(rig: &Rig) {
    let out = rig.run(&["serve", "--once"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn service_recovers_commit_before_enqueue_once_without_abandoning_prepared() {
    let rig = rig("unit-commit-crash", true);
    let execution = prepare(&rig);
    let input = commit_input(&execution, 0);
    crash(&rig, &input, "committed");
    assert!(admission(&rig, &execution)["unit"]["committed"].is_object());
    assert!(!rig.dir.join("state/queue.json").exists());
    assert_eq!(
        operation(&rig, "abandon-prepared", &execution_input(&execution)).0,
        2
    );
    // Keep host full during service refresh. This is capacity policy, not a timeout.
    rig.config("memory_reserve_bytes = 18446744073709551615\n");
    serve(&rig);
    serve(&rig);
    let tickets = rig.read("state/queue.json");
    assert_eq!(tickets.as_array().unwrap().len(), 1);
    assert_eq!(tickets[0]["request"]["id"], execution["requestId"]);
    assert_eq!(
        operation(&rig, "commit", &input).1["ticket"],
        tickets[0]["id"]
    );
    // Crash after the queue claim but before perform can select a host.
    let mut interrupted = tickets;
    interrupted[0]["state"] = json!("admitting");
    rig.put("state/queue.json", &interrupted);
    assert!(!rig.log("herdr.log").contains("agent start"));
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    serve(&rig);
    assert_eq!(rig.read("state/queue.json")[0]["state"], "admitted");
    assert_eq!(
        operation(&rig, "commit", &input).1["disposition"],
        "launched"
    );
    assert_eq!(
        rig.log("herdr.log")
            .matches(&format!(
                "agent start {} ",
                expected_name(&execution, &exported(0)["contract"]["ref"])
            ))
            .count(),
        1
    );
}

#[test]
fn lost_commit_response_reuses_launched_receipt_and_exact_profile() {
    let rig = rig("unit-response-lost", false);
    let execution = prepare(&rig);
    let input = commit_input(&execution, 1);
    crash(&rig, &input, "started");
    let (code, receipt) = operation(&rig, "commit", &input);
    assert_eq!(code, 0, "{receipt}");
    assert_eq!(receipt["disposition"], "launched");
    assert_eq!(operation(&rig, "commit", &input).1, receipt);
    serve(&rig);
    assert_eq!(
        rig.log("herdr.log")
            .matches(&format!(
                "agent start {} ",
                expected_name(&execution, &exported(0)["contract"]["ref"])
            ))
            .count(),
        1
    );
    assert_eq!(rig.log("herdr.log").matches("tab create").count(), 1);
    assert!(
        rig.log("herdr.log")
            .contains("--model alias/sol --thinking medium")
    );
    assert!(!rig.dir.join("state/queue.json").exists());
    assert_eq!(
        operation(&rig, "observe", &execution_input(&execution)).1["phase"],
        "running"
    );
}

#[test]
fn captured_reply_without_terminal_evidence_never_projects_returned() {
    let rig = rig("unit-prose-not-terminal", false);
    let execution = prepare(&rig);
    let (code, receipt) = operation(&rig, "commit", &commit_input(&execution, 0));
    assert_eq!(code, 0, "{receipt}");
    assert_eq!(receipt["disposition"], "launched");
    let input = execution_input(&execution);
    assert_eq!(operation(&rig, "observe", &input).1["phase"], "running");

    let mut items = rig.read("state/items.json");
    assert_eq!(items[0]["id"], execution["requestId"]);
    items[0]["reply"] = json!({"seq":1,"text":"Work complete.","captured_ms":now()});
    for closed in [Value::Null, json!(now())] {
        items[0]["closed_ms"] = closed;
        rig.put("state/items.json", &items);
        let (code, state) = operation(&rig, "observe", &input);
        assert_eq!(code, 0, "{state}");
        assert_eq!(state["phase"], "unresolved");
        assert!(state["settlement"].is_null());
        assert_eq!(state["receipt"], receipt);
    }
    rig.put("state/items.json", &json!([]));
    let (_, state) = operation(&rig, "observe", &input);
    assert_eq!(state["phase"], "unresolved");
    assert!(state["settlement"].is_null());
    assert_eq!(
        rig.log("herdr.log")
            .matches(&format!(
                "agent start {} ",
                expected_name(&execution, &exported(0)["contract"]["ref"])
            ))
            .count(),
        1
    );
}

#[test]
fn rejects_unknown_fields_versions_correlation_and_digest_before_commit() {
    let rig = rig("unit-invalid", false);
    let execution = prepare(&rig);
    let original = commit_input(&execution, 0);
    for pointer in [
        "/exported/extra",
        "/exported/contract/scope/extra",
        "/exported/contract/acceptance/extra",
        "/binding/extra",
    ] {
        let mut input = original.clone();
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        input.pointer_mut(parent).unwrap()[key] = json!(true);
        assert_eq!(operation(&rig, "commit", &input).0, 2);
    }
    for (pointer, value) in [
        ("/exported/version", json!(2)),
        ("/binding/exportDigest", json!("bad")),
        ("/binding/contract/revision", json!(2)),
        ("/exported/brief", json!("changed")),
    ] {
        let mut input = original.clone();
        *input.pointer_mut(pointer).unwrap() = value;
        let (code, error) = operation(&rig, "commit", &input);
        assert_eq!(code, 2, "{error}");
        assert_eq!(error["error"]["code"], "REQUEST_CONFLICT");
    }
    assert!(admission(&rig, &execution)["unit"]["committed"].is_null());
    no_launch(&rig);
}

#[test]
fn uuid_unit_names_fit_native_limits_and_native_collisions_stay_final() {
    use sha2::{Digest as _, Sha256};
    use std::fmt::Write as _;
    for collision in [false, true] {
        let rig = rig(&format!("unit-native-name-{collision}"), false);
        let fixtures: Value =
            serde_json::from_str(include_str!("../fixtures/unit-exports.json")).unwrap();
        let canonical = fixtures[0]["canonical"]
            .as_str()
            .unwrap()
            .replace("u-17", "u-6ce8048d-2317-4b7e-988e-cf6591d1b7a0-1");
        let mut export: Value = serde_json::from_str(&canonical).unwrap();
        let digest =
            Sha256::digest(canonical.as_bytes())
                .iter()
                .fold(String::new(), |mut hex, byte| {
                    write!(hex, "{byte:02x}").unwrap();
                    hex
                });
        export["digest"] = json!(digest);
        let (code, execution) = rig.json(&[
            "unit",
            "prepare",
            "--input",
            &json!({"origin":export["contract"]["ref"],"placement":{}}).to_string(),
            "--tool-call-id",
            "uuid-call",
        ]);
        assert_eq!(code, 0, "{execution}");
        let input = json!({"execution":execution,"binding":{"contract":export["contract"]["ref"],"execution":execution,"exportDigest":digest},"exported":export});
        let result = if collision {
            "echo '{\"error\":{\"code\":\"agent_name_taken\",\"message\":\"name is occupied\"}}' >&2; exit 1"
        } else {
            "printf '{}\\n'"
        };
        rig.script("herdr", &fs::read_to_string(rig.dir.join("bin/herdr")).unwrap().replace(
            "  'agent start'*|", &format!("  'agent start'*) name=\"$3\"; case \"$name\" in [a-z]*) ;; *) exit 2 ;; esac; case \"$name\" in *[!a-z0-9_-]*) exit 2 ;; esac; if test \"${{#name}}\" -gt 32; then echo '{{\"error\":{{\"code\":\"invalid_agent_name\",\"message\":\"name exceeds 32 bytes\"}}}}' >&2; exit 1; fi; {result} ;;\n  "),
        ));
        let (code, response) = operation(&rig, "commit", &input);
        if collision {
            assert_eq!(code, 2, "{response}");
            assert!(
                response.to_string().contains("agent_name_taken"),
                "{response}"
            );
            assert_eq!(operation(&rig, "commit", &input).0, 2);
            let receipt = rig.read(&format!(
                "state/launches/{}.json",
                execution["requestId"].as_str().unwrap()
            ));
            assert!(
                receipt["failed"]
                    .as_str()
                    .unwrap()
                    .contains("agent_name_taken")
            );
            assert!(!rig.dir.join("state/queue.json").exists());
        } else {
            assert_eq!(code, 0, "{response}");
            assert_eq!(operation(&rig, "commit", &input).0, 0);
        }
        let expected = expected_name(&execution, &input["binding"]["contract"]);
        assert_eq!(
            rig.log("herdr.log")
                .matches(&format!("agent start {expected} "))
                .count(),
            1
        );
        assert_eq!(
            admission(&rig, &execution)["unit"]["committed"]["exported"]["digest"],
            digest
        );
    }
}

#[test]
fn task_domain_policy_is_not_validated_by_dogsled() {
    use sha2::{Digest as _, Sha256};
    use std::fmt::Write as _;

    let rig = rig("unit-policy-boundary", true);
    let execution = prepare(&rig);
    let fixtures: Value =
        serde_json::from_str(include_str!("../fixtures/unit-exports.json")).unwrap();
    let canonical = fixtures[0]["canonical"]
        .as_str()
        .unwrap()
        .replace(
            "\"owns\":[\"src/\"]",
            "\"owns\":[\"/outside\",\"../parent\"]",
        )
        .replace("\"dependsOn\":[]", "\"dependsOn\":[\"uncompleted\"]");
    let digest =
        Sha256::digest(canonical.as_bytes())
            .iter()
            .fold(String::new(), |mut hex, byte| {
                write!(hex, "{byte:02x}").unwrap();
                hex
            });
    let mut input = commit_input(&execution, 0);
    input["exported"] = serde_json::from_str(&canonical).unwrap();
    input["exported"]["digest"] = json!(digest);
    input["binding"]["exportDigest"] = json!(digest);
    let (code, receipt) = operation(&rig, "commit", &input);
    assert_eq!(code, 0, "{receipt}");
    assert_eq!(receipt["disposition"], "queued");
}

#[test]
fn missing_profile_never_releases_binding() {
    let rig = rig("unit-profile-failure", false);
    let execution = prepare(&rig);
    let input = commit_input(&execution, 0);
    fs::remove_file(rig.dir.join("profiles/worker/profile.json")).unwrap();
    let (code, error) = operation(&rig, "commit", &input);
    assert_eq!(code, 2);
    assert_eq!(error["error"]["code"], "PROFILE_INVALID");
    no_launch(&rig);
    assert!(operation(&rig, "observe", &execution_input(&execution)).1["settlement"].is_null());
}

#[test]
fn service_never_expires_prepared_invocations() {
    let rig = rig("unit-no-expiry", true);
    let execution = prepare(&rig);
    fs::write(rig.dir.join("clock"), (now() + 86_400_000).to_string()).unwrap();
    serve(&rig);
    let (_, state) = operation(&rig, "observe", &execution_input(&execution));
    assert_eq!(state["phase"], "prepared");
    assert!(state["settlement"].is_null());
    // Node::refresh writes the pruned reservation list even when it is empty.
    assert_eq!(rig.read("state/placements.json"), json!([]));
    assert!(!rig.dir.join("state/queue.json").exists());
    assert!(!rig.log("herdr.log").contains("agent start"));
    assert!(!rig.log("herdr.log").contains("tab create"));
}

#[test]
fn unknown_host_launch_outcome_reuses_attempt_without_safe_release() {
    let rig = rig("unit-unknown-launch", false);
    rig.script(
        "herdr",
        &fs::read_to_string(rig.dir.join("bin/herdr"))
            .unwrap()
            .replace(
                "  'agent start'*|",
                "  'agent start'*) echo 'startup response lost' >&2; exit 1 ;;\n  ",
            ),
    );
    let execution = prepare(&rig);
    let input = commit_input(&execution, 0);
    let (code, receipt) = operation(&rig, "commit", &input);
    assert_eq!(code, 0, "{receipt}");
    assert_eq!(receipt["disposition"], "queued");
    assert_eq!(operation(&rig, "commit", &input).1, receipt);
    assert_eq!(
        operation(&rig, "abandon-prepared", &execution_input(&execution)).0,
        2
    );
    assert!(operation(&rig, "observe", &execution_input(&execution)).1["settlement"].is_null());
    assert_eq!(
        rig.log("herdr.log")
            .matches(&format!(
                "agent start {} ",
                expected_name(&execution, &exported(0)["contract"]["ref"])
            ))
            .count(),
        1
    );
}
