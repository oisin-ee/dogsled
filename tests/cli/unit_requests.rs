use super::*;
use std::io::Write as _;

pub(super) fn exported(index: usize) -> Value {
    let fixtures: Value = serde_json::from_str(include_str!(
        "../../crates/todo-core/tests/fixtures/unit-exports.json"
    ))
    .unwrap();
    let mut value: Value =
        serde_json::from_str(fixtures[index]["canonical"].as_str().unwrap()).unwrap();
    value["digest"] = fixtures[index]["digest"].clone();
    value
}

pub(super) fn rig(name: &str, full: bool) -> Rig {
    let rig = Rig::short(name);
    rig.snapshot("state/host.json", now(), if full { 3.0 } else { 0.0 }, 2);
    worker_profile(&rig);
    rig
}

fn worker_profile(rig: &Rig) {
    let dir = rig.dir.join("profiles/worker");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("profile.json"), json!({"description":"worker","model":"alias/sol","thinking":"medium","prompt":"prompt.md","pi_args":[],"role":"reader"}).to_string()).unwrap();
    fs::write(dir.join("prompt.md"), "Implement the unit.").unwrap();
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
    assert_eq!(rig.tickets(), json!([]));
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
    assert_eq!(rig.tickets().as_array().unwrap().len(), 1);
    assert_eq!(
        operation(&rig, "commit", &commit_input(&executions[0], 1)).1["error"]["code"],
        "REQUEST_CONFLICT"
    );
    assert!(
        rig.run(&["cancel", receipts[0].1["ticket"].as_str().unwrap()])
            .status
            .success()
    );
    // A cancelled ticket never launched: the unit settles as not started, permanently.
    let (_, state) = operation(&rig, "observe", &execution_input(&executions[0]));
    assert_eq!(state["phase"], "not_started");
    assert_eq!(state["settlement"]["result"]["kind"], "not_started");
    assert_eq!(
        operation(&rig, "commit", &input).1["error"]["code"],
        "REQUEST_CONFLICT"
    );
}

#[test]
fn cancel_before_admission_fences_every_replay() {
    let (rig, execution, ticket) = committed_queued("cancel-replays");
    assert!(rig.run(&["cancel", &ticket]).status.success());
    let id = execution["requestId"].as_str().unwrap();
    let saved = rig.read(&format!("state/cancellations/{id}.json"));
    assert_eq!(saved["version"], 1);
    assert_eq!(saved["outcome"], "not_started");
    assert!(rig.run(&["cancel", &ticket]).status.success());
    assert_eq!(saved, rig.read(&format!("state/cancellations/{id}.json")));
    let first = operation(&rig, "observe", &execution_input(&execution)).1["settlement"].clone();
    assert_eq!(first["result"]["kind"], "not_started");
    assert_eq!(
        operation(&rig, "commit", &commit_input(&execution, 0)).1["error"]["code"],
        "REQUEST_CONFLICT"
    );
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    assert!(rig.run(&["serve", "--once"]).status.success());
    assert_eq!(rig.tickets()[0]["state"], "cancelled");
    assert_eq!(
        operation(&rig, "observe", &execution_input(&execution)).1["settlement"],
        first
    );
    assert!(!rig.log("herdr.log").contains("agent start"));
}

#[test]
fn terminal_ticket_without_no_launch_proof_stays_unresolved() {
    for case in ["legacy", "missing", "corrupt", "launch", "mismatch", "ack"] {
        let (rig, execution, _) = committed_queued(&format!("cancel-no-proof-{case}"));
        set_ticket(&rig, "cancelled", Value::Null);
        let file = format!(
            "state/admissions/{}.json",
            execution["requestId"].as_str().unwrap()
        );
        if case == "missing" {
            fs::remove_file(rig.dir.join(&file)).unwrap();
        }
        if case == "corrupt" {
            fs::write(rig.dir.join(&file), "{corrupt").unwrap();
        }
        if case == "launch" {
            let mut attempt = rig.read(&file);
            attempt["launch"] = json!({"id":execution["requestId"],"action":attempt["request"]["action"],"host":"local","cwd":"/tmp","pane":null,"workspace":null,"workspace_name":null,"owner":"w1:p1"});
            rig.put(&file, &attempt);
        }
        if case == "mismatch" {
            let mut attempt = rig.read(&file);
            attempt["request"]["cwd"] = json!("/changed");
            rig.put(&file, &attempt);
        }
        if case == "ack" {
            fs::create_dir_all(rig.dir.join("state/cancellations")).unwrap();
            let ticket = rig.tickets()[0].clone();
            let binding = json!({"request_id":execution["requestId"],"request":["/repo","worker",null],
                "claim":{"request_id":execution["requestId"],"tree":{"repo":"/repo","path":"/repo/tree","branch":"worker","head":"head"}},"prepared":null});
            let policy = json!({"kind":"prohibit","basis":{"cancelled":{"operation_id":"cancel-ack","request_id":execution["requestId"]}}});
            let intent = json!({"version":1,"binding":binding,"policy":policy,"outcome":"unknown"});
            let ack = json!({"version":1,"binding":binding,"policy":policy,"outcome":"unknown","released":true});
            rig.put(&format!("state/cancellations/{}.json", execution["requestId"].as_str().unwrap()),
                &json!({"version":1,"operation_id":"cancel-ack","ticket":ticket,"admission":rig.read(&file),
                    "preparation":null,"placement":null,"outcome":"unknown","release":"acknowledged",
                    "intent":intent,"acknowledgement":ack,"error":null}));
        }
        let (_, state) = operation(&rig, "observe", &execution_input(&execution));
        assert!(state["settlement"].is_null(), "{case}: {state}");
        let (code, settlements) = rig.json(&["unit", "settlements"]);
        assert!(
            code == 2 || settlements == json!([]),
            "{case}: {settlements}"
        );
    }
}

fn admission_service(rig: &Rig) {
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    rig.snapshot("state/host.json", rig.origin_ms, 0.0, 16);
}

#[test]
fn cancellation_intent_crash_repairs_queue_without_readmission() {
    let (rig, execution, ticket) = committed_queued("cancel-intent-crash");
    let out = rig
        .cmd(&["cancel", &ticket])
        .env("DOGSLED_TEST_UNIT_CRASH", "cancellation-intent-saved")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(86), "{out:?}");
    assert_eq!(rig.tickets()[0]["state"], "pending");
    let id = execution["requestId"].as_str().unwrap();
    let intent = rig.read(&format!("state/cancellations/{id}.json"));
    admission_service(&rig);
    assert!(rig.run(&["serve", "--once"]).status.success());
    assert_eq!(rig.tickets()[0]["state"], "cancelled");
    assert_eq!(rig.read(&format!("state/cancellations/{id}.json")), intent);
    assert_eq!(
        operation(&rig, "observe", &execution_input(&execution)).1["settlement"]["result"]["kind"],
        "not_started"
    );
    assert!(!rig.log("herdr.log").contains("agent start"));
}

#[test]
fn cancellation_after_saved_result_repairs_projection_as_too_late() {
    let (rig, execution, ticket) = committed_queued("cancel-saved-result");
    admission_service(&rig);
    let out = rig
        .cmd(&["serve", "--once"])
        .env("DOGSLED_TEST_UNIT_CRASH", "admission-result-saved")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(86), "{out:?}");
    assert_eq!(rig.tickets()[0]["state"], "admitting");
    let result = admission(&rig, &execution)["result"].clone();
    let (code, reply) = rig.json(&["cancel", &ticket]);
    assert_eq!(code, 2, "{reply}");
    assert_eq!(reply["status"], "too_late");
    assert_eq!(reply["result"], result);
    assert_eq!(rig.tickets()[0]["state"], "admitted");
    assert_eq!(rig.tickets()[0]["result"], result);
    assert!(operation(&rig, "observe", &execution_input(&execution)).1["settlement"].is_null());
    assert!(!rig.dir.join("state/cancellations").exists());
}

#[test]
fn cancel_during_admitting_orders_before_finalization() {
    let (rig, execution, ticket) = committed_queued("cancel-finalization");
    admission_service(&rig);
    let ready = Fifo::new(&rig.dir, "final.ready");
    let go = Fifo::new(&rig.dir, "final.go");
    let _ready = ready.hold();
    let mut go = go.hold();
    let mut service = Owned::spawn(rig.cmd(&["serve", "--once"]).env(
        "DOGSLED_TEST_LAUNCH_BEFORE_FINALIZATION",
        rig.dir.join("final"),
    ));
    read_fifo(ready.path(), "admission before finalization");
    let result = admission(&rig, &execution)["result"].clone();
    assert!(result.is_object(), "{result}");
    let cancel_ready = Fifo::new(&rig.dir, "cancel.ready");
    let cancel_go = Fifo::new(&rig.dir, "cancel.go");
    let _cancel_ready = cancel_ready.hold();
    let mut cancel_go = cancel_go.hold();
    let mut cancel = Owned::spawn(rig.cmd(&["cancel", &ticket]).env(
        "DOGSLED_TEST_LAUNCH_CANCEL_BEFORE_ADMISSION",
        rig.dir.join("cancel"),
    ));
    read_fifo(cancel_ready.path(), "cancel before admission gate");
    cancel_go.write_all(b"go\n").unwrap();
    assert_eq!(rig.tickets()[0]["state"], "admitting");
    go.write_all(b"go\n").unwrap();
    assert!(service.wait().success());
    assert_eq!(cancel.wait().code(), Some(2));
    assert_eq!(rig.tickets()[0]["state"], "admitted");
    assert_eq!(rig.tickets()[0]["result"], result);
    assert_eq!(admission(&rig, &execution)["result"], result);
    assert!(operation(&rig, "observe", &execution_input(&execution)).1["settlement"].is_null());
}

#[test]
fn dispatch_before_cancel_preserves_possible_execution() {
    let rig = rig("cancel-lost-launch-reply", true);
    rig.peer();
    rig.snapshot("remote.json", rig.origin_ms, 1000.0, 1);
    let input = json!({"origin":exported(0)["contract"]["ref"],"placement":{"host":"netcup"}});
    let (code, execution) = rig.json(&[
        "unit",
        "prepare",
        "--input",
        &input.to_string(),
        "--tool-call-id",
        "lost-launch",
    ]);
    assert_eq!(code, 0, "{execution}");
    let (code, queued) = operation(&rig, "commit", &commit_input(&execution, 0));
    assert_eq!(code, 0, "{queued}");
    let ticket = queued["ticket"].as_str().unwrap();
    admission_service(&rig);
    let ready = Fifo::new(&rig.dir, "launch.ready");
    let go = Fifo::new(&rig.dir, "launch.go");
    let _ready = ready.hold();
    let mut go = go.hold();
    let original = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
    let handler = format!(
        r#"case "$*" in
  *'host launch '*)
    printf 'launch\n' > "$RIG/launch.ready"
    read go < "$RIG/launch.go"
    for remote do :; done
    remote=${{remote#* host }}
    DOGSLED_STATE="$RIG/peer-state" sh -c '{} host '"$remote" > "$RIG/launch-response"
    code=$?; test "$code" = 0 || exit "$code"
    exit 255 ;;
"#,
        env!("CARGO_BIN_EXE_dogsled")
    );
    rig.script("ssh", &original.replacen("case \"$*\" in", &handler, 1));
    let mut service = Owned::spawn(&mut rig.cmd(&["serve", "--once"]));
    read_fifo(ready.path(), "origin saved launch before host dispatch");
    let saved_launch = admission(&rig, &execution)["launch"].clone();
    assert!(saved_launch.is_object());
    let cancel_ready = Fifo::new(&rig.dir, "cancel.ready");
    let cancel_go = Fifo::new(&rig.dir, "cancel.go");
    let _cancel_ready = cancel_ready.hold();
    let mut cancel_go = cancel_go.hold();
    let mut cancel = Owned::spawn(rig.cmd(&["cancel", ticket]).env(
        "DOGSLED_TEST_LAUNCH_CANCEL_BEFORE_ADMISSION",
        rig.dir.join("cancel"),
    ));
    read_fifo(cancel_ready.path(), "cancel waits behind dispatch");
    cancel_go.write_all(b"go\n").unwrap();
    go.write_all(b"go\n").unwrap();
    assert!(service.wait().success());
    assert!(cancel.wait().success());
    let id = execution["requestId"].as_str().unwrap();
    let cancellation = rig.read(&format!("state/cancellations/{id}.json"));
    assert_eq!(cancellation["outcome"], "possibly_executed");
    assert_eq!(admission(&rig, &execution)["launch"], saved_launch);
    let host = rig.read(&format!("peer-state/launches/{id}.json"));
    assert!(host["pane"].is_string(), "{host}");
    assert!(operation(&rig, "observe", &execution_input(&execution)).1["settlement"].is_null());
    assert_eq!(rig.json(&["unit", "settlements"]).1, json!([]));
    assert!(rig.run(&["serve", "--once"]).status.success());
    assert_eq!(rig.read(&format!("peer-state/launches/{id}.json")), host);
    assert_eq!(rig.log("ssh.log").matches("host launch ").count(), 1);
    assert_eq!(rig.log("herdr.log").matches("agent start ").count(), 1);
    assert!(!rig.log("herdr.log").contains("pane close"));
}

#[test]
fn cancellation_wins_admission_before_service_and_commit_replay() {
    let (rig, execution, ticket) = committed_queued("cancel-wins-admission");
    let id = execution["requestId"].as_str().unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(rig.dir.join(format!("state/admissions/{id}.lock")))
        .unwrap();
    lock.lock().unwrap();
    let ready = Fifo::new(&rig.dir, "cancel.ready");
    let go = Fifo::new(&rig.dir, "cancel.go");
    let entered = Fifo::new(&rig.dir, "entered.ready");
    let entered_go = Fifo::new(&rig.dir, "entered.go");
    let _ready = ready.hold();
    let _entered = entered.hold();
    let mut go = go.hold();
    let mut entered_go = entered_go.hold();
    let mut cancel = Owned::spawn(
        rig.cmd(&["cancel", &ticket])
            .env(
                "DOGSLED_TEST_LAUNCH_CANCEL_BEFORE_ADMISSION",
                rig.dir.join("cancel"),
            )
            .env(
                "DOGSLED_TEST_LAUNCH_CANCEL_ENTERED_ADMISSION",
                rig.dir.join("entered"),
            ),
    );
    read_fifo(ready.path(), "cancel before held admission");
    go.write_all(b"go\n").unwrap();
    drop(lock);
    read_fifo(entered.path(), "cancellation owns admission");
    admission_service(&rig);
    let mut service = Owned::spawn(&mut rig.cmd(&["serve", "--once"]));
    let input = commit_input(&execution, 0).to_string();
    let mut replay = Owned::spawn(&mut rig.cmd(&["unit", "commit", "--input", &input]));
    entered_go.write_all(b"go\n").unwrap();
    assert!(cancel.wait().success());
    assert!(service.wait().success());
    assert_eq!(replay.wait().code(), Some(2));
    assert_eq!(rig.tickets()[0]["state"], "cancelled");
    assert_eq!(
        operation(&rig, "observe", &execution_input(&execution)).1["phase"],
        "not_started"
    );
    assert!(!rig.log("herdr.log").contains("agent start"));
}

fn remotely_prepared_queued(name: &str) -> (Rig, Value, String) {
    let (rig, repo) = super::place::place_rig(name);
    worker_profile(&rig);
    rig.peer();
    rig.snapshot("remote.json", rig.origin_ms, 1000.0, 1);
    let input = json!({"origin":exported(0)["contract"]["ref"],"placement":{"host":"netcup","branch":"cancel-worker","base":"main"}});
    let out = rig
        .cmd(&[
            "unit",
            "prepare",
            "--input",
            &input.to_string(),
            "--tool-call-id",
            "remote-cancel",
            "--json",
        ])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let execution: Value = serde_json::from_slice(&out.stdout).unwrap();
    let (code, receipt) = operation(&rig, "commit", &commit_input(&execution, 0));
    assert_eq!(code, 0, "{receipt}");
    let ticket = receipt["ticket"].as_str().unwrap().to_owned();
    admission_service(&rig);
    super::place::real_preparation_peer(&rig);
    let out = rig
        .cmd(&["serve", "--once"])
        .env(
            "DOGSLED_TEST_UNIT_CRASH",
            "preparation-returned-before-launch",
        )
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(86),
        "{out:?} queue={} ssh={}",
        rig.tickets(),
        rig.log("ssh.log")
    );
    assert!(admission(&rig, &execution)["launch"].is_null());
    (rig, execution, ticket)
}

fn cancellation_launch_fixture(name: &str) -> (Rig, Value, String, Value) {
    let (rig, execution, ticket) = remotely_prepared_queued(name);
    set_ticket(&rig, "pending", Value::Null);
    let id = execution["requestId"].as_str().unwrap();
    let file = format!("state/admissions/{id}.json");
    let mut attempt = admission(&rig, &execution);
    let receipt = rig.read(&format!("peer-state/preparation-receipts/{id}.json"));
    let commit = &attempt["unit"]["committed"];
    let request = &attempt["request"];
    let launch = json!({"id":id,"validity":request["validity"],"action":request["action"],"host":attempt["host"]["id"],
        "cwd":receipt["prepared"]["path"],"pane":receipt["prepared"]["pane_id"],
        "terminal":receipt["prepared"]["terminal_id"],"workspace":null,"workspace_name":request["workspace_name"],
        "owner":request["owner"],"owner_host":request["owner_host"],"preparation_protocol":1,"preparation":receipt["binding"],
        "unit":{"version":1,"execution":commit["execution"],"binding":commit["binding"],"exported":commit["exported"]}});
    attempt["launch"] = launch.clone();
    rig.put(&file, &attempt);
    (rig, execution, ticket, launch)
}

#[test]
fn cancellation_adopts_transfer_after_lost_launch_reply() {
    let (rig, execution, ticket, launch) = cancellation_launch_fixture("cancel-transfer");
    let id = execution["requestId"].as_str().unwrap();
    let out = rig
        .cmd(&[
            "host",
            "launch",
            "--preparation-protocol",
            "1",
            "--request",
            &launch.to_string(),
        ])
        .env("DOGSLED_STATE", rig.dir.join("peer-state"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(
        rig.dir
            .join(format!(
                "peer-state/preparation-receipts/{id}.disposition.json"
            ))
            .exists(),
        "queue={} admission={} ssh={} herdr={}",
        rig.tickets(),
        admission(&rig, &execution),
        rig.log("ssh.log"),
        rig.log("herdr.log")
    );
    let disposition = rig.read(&format!(
        "peer-state/preparation-receipts/{id}.disposition.json"
    ));
    assert_eq!(disposition["request"]["policy"]["kind"], "transfer");
    assert!(rig.run(&["cancel", &ticket]).status.success());
    assert!(rig.run(&["serve", "--once"]).status.success());
    let cancellation = rig.read(&format!("state/cancellations/{id}.json"));
    assert_eq!(cancellation["release"], "acknowledged", "{cancellation}");
    assert_eq!(cancellation["intent"], disposition["request"]);
    assert_eq!(rig.read("peer-state/preparations.json"), json!([]));
    assert!(operation(&rig, "observe", &execution_input(&execution)).1["settlement"].is_null());
}

#[test]
fn native_refusal_and_cancellation_settle_in_both_orders() {
    for cancel_first in [true, false] {
        let (rig, execution, ticket, launch) = cancellation_launch_fixture("cancel-refusal");
        let id = execution["requestId"].as_str().unwrap();
        let ssh = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
        let handler = format!(
            r#"case "$*" in
  *'host unit-receipts '*)
    for remote do :; done
    remote=${{remote#* host }}
    DOGSLED_STATE="$RIG/peer-state" sh -c '{} host '"$remote" ;;
"#,
            env!("CARGO_BIN_EXE_dogsled")
        );
        rig.script("ssh", &ssh.replacen("case \"$*\" in", &handler, 1));
        let script = fs::read_to_string(rig.dir.join("bin/herdr")).unwrap();
        rig.script("herdr", &script.replace("  'agent start'*)", "  'agent start'*) echo '{\"error\":{\"code\":\"agent_name_taken\"}}' >&2; exit 1 ;;\n  'unused start'*)"));
        let out = rig
            .cmd(&[
                "host",
                "launch",
                "--preparation-protocol",
                "1",
                "--request",
                &launch.to_string(),
            ])
            .env("DOGSLED_STATE", rig.dir.join("peer-state"))
            .output()
            .unwrap();
        assert!(!out.status.success());
        let input = json!({"execution":execution});
        if cancel_first {
            assert!(rig.run(&["cancel", &ticket]).status.success());
            let intent = rig.read(&format!("state/cancellations/{id}.json"))["intent"].clone();
            let out = rig
                .cmd(&[
                    "host",
                    "prepared",
                    "--preparation-protocol",
                    "1",
                    "--input",
                    &intent.to_string(),
                ])
                .env("DOGSLED_STATE", rig.dir.join("peer-state"))
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
            assert!(rig.run(&["serve", "--once"]).status.success());
        } else {
            let out = rig
                .cmd(&["host", "unit-receipt", "--input", &input.to_string()])
                .env("DOGSLED_STATE", rig.dir.join("peer-state"))
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
            assert!(rig.run(&["cancel", &ticket]).status.success());
        }
        assert!(rig.run(&["serve", "--once"]).status.success());
        let state = operation(&rig, "observe", &input).1;
        assert_eq!(state["phase"], "not_started", "{state}");
        let event = state["settlement"].clone();
        assert_eq!(operation(&rig, "observe", &input).1["settlement"], event);
        assert_eq!(rig.json(&["unit", "settlements"]).1, json!([event]));
        assert_eq!(rig.read("peer-state/preparations.json"), json!([]));
        let disposition = rig.read(&format!(
            "peer-state/preparation-receipts/{id}.disposition.json"
        ));
        assert_eq!(disposition["progress"], "released");
        assert_eq!(rig.log("herdr.log").matches("agent start ").count(), 1);
        assert!(rig.run(&["serve", "--once"]).status.success());
        assert_eq!(
            rig.read(&format!(
                "peer-state/preparation-receipts/{id}.disposition.json"
            )),
            disposition
        );
    }
}

#[test]
fn unreachable_host_keeps_cancel_release_pending() {
    let (rig, execution, ticket) = remotely_prepared_queued("cancel-remote-release");
    let id = execution["requestId"].as_str().unwrap();
    let receipt = rig.read(&format!("peer-state/preparation-receipts/{id}.json"));
    let mut claims = rig.read("peer-state/preparations.json");
    let mut unrelated = claims[0].clone();
    unrelated["request_id"] = json!("other-request");
    claims.as_array_mut().unwrap().push(unrelated.clone());
    rig.put("peer-state/preparations.json", &claims);
    let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
    // Keep the real transport handler; this guard runs before its case dispatch.
    rig.script("ssh", &script.replacen("case \"$*\" in", "if test -f \"$RIG/disposition-offline\"; then case \"$*\" in *'host prepared '*) exit 255 ;; esac; fi\ncase \"$*\" in", 1));
    fs::write(rig.dir.join("disposition-offline"), "").unwrap();
    assert!(rig.run(&["cancel", &ticket]).status.success());
    for _ in 0..2 {
        assert!(rig.run(&["serve", "--once"]).status.success());
        let saved = rig.read(&format!("state/cancellations/{id}.json"));
        assert_eq!(saved["release"], "pending", "{saved}");
        assert_eq!(saved["outcome"], "not_started");
        assert_eq!(rig.tickets()[0]["state"], "cancelled");
    }
    assert_eq!(rig.read("peer-state/preparations.json"), claims);
    fs::remove_file(rig.dir.join("disposition-offline")).unwrap();
    let out = rig
        .cmd(&["serve", "--once"])
        .env(
            "DOGSLED_TEST_UNIT_CRASH",
            "cancellation-acknowledgement-saved",
        )
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(86), "{out:?}");
    let acknowledged = rig.read(&format!("state/cancellations/{id}.json"));
    assert_eq!(
        acknowledged["acknowledgement"]["released"], true,
        "{acknowledged}"
    );
    // After saving proof, every remote operation fails, including read-only lookup.
    rig.script(
        "ssh",
        &script.replacen("case \"$*\" in", "exit 255\ncase \"$*\" in", 1),
    );
    let transport_before = rig.log("ssh.log");
    assert!(rig.run(&["serve", "--once"]).status.success());
    let saved = rig.read(&format!("state/cancellations/{id}.json"));
    assert_eq!(saved["release"], "acknowledged", "{saved}");
    for command in ["host preparation-lookup ", "host prepared "] {
        assert_eq!(
            rig.log("ssh.log").matches(command).count(),
            transport_before.matches(command).count()
        );
    }
    assert_eq!(saved["operation_id"], acknowledged["operation_id"]);
    assert_eq!(saved["acknowledgement"], acknowledged["acknowledgement"]);
    assert_eq!(rig.read("peer-state/preparations.json"), json!([unrelated]));
    assert_eq!(
        outcome_identity(rig.read(&format!("peer-state/preparation-receipts/{id}.json"))),
        outcome_identity(receipt)
    );
    assert_eq!(
        operation(&rig, "observe", &execution_input(&execution)).1["phase"],
        "not_started"
    );
    assert!(!rig.log("ssh.log").contains("host launch"));
}

fn committed_queued(name: &str) -> (Rig, Value, String) {
    let rig = rig(name, true);
    let execution = prepare(&rig);
    let (code, receipt) = operation(&rig, "commit", &commit_input(&execution, 0));
    assert_eq!(code, 0, "{receipt}");
    assert_eq!(receipt["disposition"], "queued");
    let ticket = receipt["ticket"].as_str().unwrap().to_owned();
    (rig, execution, ticket)
}

#[test]
fn corrupt_cancellation_holds_only_its_committed_unit() {
    for unsupported in [true, false] {
        let (rig, execution, ticket) = committed_queued("cancel-corrupt-isolation");
        let id = execution["requestId"].as_str().unwrap();
        let mut queue = rig.tickets();
        assert!(rig.run(&["cancel", &ticket]).status.success());
        let file = format!("state/cancellations/{id}.json");
        let corrupt = if unsupported {
            let mut saved = rig.read(&file);
            saved["version"] = json!(99);
            saved
        } else {
            json!("invalid cancellation")
        };
        rig.put(&file, &corrupt);
        let mut unrelated = queue[0].clone();
        unrelated["id"] = json!("unrelated-ticket");
        fs::create_dir_all(rig.dir.join("state/job-validity")).unwrap();
        rig.put(
            "state/job-validity/unrelated-place.json",
            &rig.job_validity(),
        );
        unrelated["request"] = json!({"id":"unrelated-place","validity":rig.job_validity(),"action":{"type":"place"},
            "host":"local","cwd":rig.dir.join("home"),"owner":"w1:p1"});
        queue.as_array_mut().unwrap().push(unrelated);
        rig.put_tickets(&queue);
        rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
        rig.snapshot("state/host.json", now(), 0.0, 2);
        let out = rig.run(&["serve", "--once"]);
        assert!(out.status.success(), "{out:?}");
        let queue = rig.tickets();
        assert_eq!(queue[0]["id"], ticket);
        assert_eq!(queue[0]["state"], "pending", "{queue}");
        let reason = queue[0]["error"].as_str().unwrap();
        assert!(
            reason.contains(if unsupported {
                "unsupported"
            } else {
                "invalid type"
            }),
            "{queue}"
        );
        assert_eq!(queue[1]["state"], "admitted", "{queue}");
        assert_eq!(rig.read(&format!("state/cancellations/{id}.json")), corrupt);
        assert!(admission(&rig, &execution)["launch"].is_null());
        assert!(!rig.log("herdr.log").contains("agent start"));
    }
}

fn set_ticket(rig: &Rig, state: &str, error: Value) {
    let mut queue = rig.tickets();
    queue[0]["state"] = json!(state);
    queue[0]["error"] = error;
    rig.put_tickets(&queue);
}

#[test]
fn failed_launch_ticket_settles_unit_as_not_started_and_delivers_once() {
    let (rig, execution, ticket) = committed_queued("unit-launch-failed");
    let reason = "ssh netcup: exit 255: Connection closed by 100.119.94.92 port 2222";
    set_ticket(&rig, "failed", json!(reason));
    let (_, state) = operation(&rig, "observe", &execution_input(&execution));
    assert_eq!(state["phase"], "not_started", "{state}");
    let event = state["settlement"].clone();
    assert_eq!(event["result"]["kind"], "not_started");
    assert_eq!(event["execution"], execution);
    assert_eq!(event["origin"], exported(0)["contract"]["ref"]);
    let text = event["result"]["reason"].as_str().unwrap();
    assert!(text.contains(reason) && text.contains(&ticket), "{text}");
    // Replay is stable, and the owner receives it through the normal delivery lane.
    assert_eq!(
        operation(&rig, "observe", &execution_input(&execution)).1["settlement"],
        event
    );
    assert_eq!(rig.json(&["unit", "settlements"]).1, json!([event]));
    let ack = json!({"execution":execution,"eventId":event["eventId"]});
    assert_eq!(operation(&rig, "ack-settlement", &ack).1["applied"], true);
    assert_eq!(rig.json(&["unit", "settlements"]).1, json!([]));
    assert!(!rig.log("herdr.log").contains("agent start"));
}

#[test]
fn unknown_release_does_not_settle_possibly_launched_unit() {
    for response in ["lost", "mismatch"] {
        let rig = rig(&format!("unknown-release-{response}"), false);
        let repo = rig.dir.join("home");
        repository(&repo);
        rig.peer();
        let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
        let handler = if response == "lost" {
            "  *'host prepared'*) echo response-lost >&2; exit 255;"
        } else {
            "  *'host prepared'*) printf '{\"prepared\":true}\n'; exit 0;"
        };
        rig.script("ssh", &script.replace("  *'host prepared'*)", handler));
        let input = json!({"origin":exported(0)["contract"]["ref"],"placement":{"host":"netcup","branch":"ambiguous","base":"main"}});
        let (code, execution) = rig.json(&[
            "unit",
            "prepare",
            "--input",
            &input.to_string(),
            "--tool-call-id",
            "call-ambiguous",
        ]);
        assert_eq!(code, 0, "{execution}");
        let (code, reply) = operation(&rig, "commit", &commit_input(&execution, 0));
        assert_eq!(code, 2, "{reply}");
        let attempt = admission(&rig, &execution);
        assert!(attempt["launch"]["preparation"].is_object(), "{attempt}");
        let (_, state) = operation(&rig, "observe", &execution_input(&execution));
        assert_eq!(state["phase"], "unresolved", "{state}");
        assert!(state["settlement"].is_null(), "{state}");
        assert_eq!(rig.json(&["unit", "settlements"]).1, json!([]));
        assert_eq!(rig.log("ssh.log").matches("host launch ").count(), 1);
        assert_eq!(rig.log("ssh.log").matches("host prepared ").count(), 1);
        assert!(!rig.log("herdr.log").contains("agent start"));
    }
}

#[test]
fn unfinished_or_possibly_launched_tickets_never_settle() {
    // Negative control: a ticket still waiting for the service proves nothing.
    let (rig, execution, _) = committed_queued("unit-pending-no-settle");
    let (_, state) = operation(&rig, "observe", &execution_input(&execution));
    assert_eq!(state["phase"], "queued");
    assert!(state["settlement"].is_null());
    // A failure after a launch request was saved may have reached the host.
    let path = format!(
        "state/admissions/{}.json",
        execution["requestId"].as_str().unwrap()
    );
    let mut attempt = rig.read(&path);
    attempt["launch"] = json!({"id":execution["requestId"],"action":{"type":"agent","kind":"pi","name":"n","message":"m"},"host":"local","cwd":"/tmp","pane":null,"workspace":null,"workspace_name":null,"owner":"w1:p1"});
    rig.put(&path, &attempt);
    set_ticket(&rig, "failed", json!("startup response lost"));
    let (_, state) = operation(&rig, "observe", &execution_input(&execution));
    assert_eq!(state["phase"], "unresolved");
    assert!(state["settlement"].is_null());
    assert_eq!(rig.json(&["unit", "settlements"]).1, json!([]));
}

#[test]
fn settling_does_not_touch_other_owners_or_prepared_units() {
    let (rig, execution, ticket) = committed_queued("unit-settle-scope");
    assert!(rig.run(&["cancel", &ticket]).status.success());
    let other = rig
        .cmd(&["unit", "settlements"])
        .env("HERDR_PANE_ID", "other")
        .output()
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&other.stdout).unwrap(),
        json!([])
    );
    // The owner still sees exactly its own event.
    assert_eq!(
        rig.json(&["unit", "settlements"])
            .1
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let _ = execution;
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
        assert_eq!(rig.tickets().as_array().unwrap().len(), 1);
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
fn unprovable_committed_unit_holds_only_itself_and_peers_are_admitted() {
    let rig = rig("unit-recover-unprovable", true);
    let execution = prepare(&rig);
    crash(&rig, &commit_input(&execution, 0), "committed");
    // An older writer saved this admission without request validity.
    let id = execution["requestId"].as_str().unwrap();
    let file = format!("state/admissions/{id}.json");
    let mut attempt = rig.read(&file);
    attempt["request"]
        .as_object_mut()
        .unwrap()
        .remove("validity");
    rig.put(&file, &attempt);
    fs::remove_file(rig.dir.join(format!("state/job-validity/{id}.json"))).ok();
    fs::create_dir_all(rig.dir.join("state/job-validity")).unwrap();
    rig.put("state/job-validity/peer-place.json", &rig.job_validity());
    rig.put_tickets(&json!([{"id":"peer-ticket","request":{"id":"peer-place",
        "validity":rig.job_validity(),"action":{"type":"place"},"host":"local",
        "cwd":rig.dir.join("home"),"owner":"w1:p1"},"created_ms":now(),"state":"pending",
        "waiting":false,"waiting_until_ms":null,"notified":false,"result":null,"error":null}]));
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    rig.snapshot("state/host.json", now(), 0.0, 2);
    let out = rig.run(&["serve", "--once"]);
    assert!(out.status.success(), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains(&format!("dogsled unit recovery {id}")),
        "{out:?}"
    );
    let tickets = rig.tickets();
    assert_eq!(tickets.as_array().unwrap().len(), 1, "{tickets}");
    assert_eq!(tickets[0]["id"], "peer-ticket");
    assert_eq!(tickets[0]["state"], "admitted", "{tickets}");
    assert!(admission(&rig, &execution)["launch"].is_null());
}

#[test]
fn service_recovers_commit_before_enqueue_once_without_abandoning_prepared() {
    let rig = rig("unit-commit-crash", true);
    let execution = prepare(&rig);
    let input = commit_input(&execution, 0);
    crash(&rig, &input, "committed");
    assert!(admission(&rig, &execution)["unit"]["committed"].is_object());
    assert_eq!(rig.tickets(), json!([]));
    assert_eq!(
        operation(&rig, "abandon-prepared", &execution_input(&execution)).0,
        2
    );
    // Keep host full during service refresh. This is capacity policy, not a timeout.
    rig.config("memory_reserve_bytes = 18446744073709551615\n");
    serve(&rig);
    serve(&rig);
    let tickets = rig.tickets();
    assert_eq!(tickets.as_array().unwrap().len(), 1);
    assert_eq!(tickets[0]["request"]["id"], execution["requestId"]);
    assert_eq!(
        operation(&rig, "commit", &input).1["ticket"],
        tickets[0]["id"]
    );
    // Crash after the queue claim but before perform can select a host.
    let mut interrupted = tickets;
    interrupted[0]["state"] = json!("admitting");
    rig.put_tickets(&interrupted);
    assert!(!rig.log("herdr.log").contains("agent start"));
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    serve(&rig);
    assert_eq!(rig.tickets()[0]["state"], "admitted");
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
    assert_eq!(rig.tickets(), json!([]));
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

    let mut items = rig.items();
    assert_eq!(items[0]["id"], execution["requestId"]);
    items[0]["reply"] = json!({"seq":1,"text":"Work complete.","captured_ms":now()});
    for closed in [Value::Null, json!(now())] {
        items[0]["closed_ms"] = closed;
        rig.put_items(&items);
        let (code, state) = operation(&rig, "observe", &input);
        assert_eq!(code, 0, "{state}");
        assert_eq!(state["phase"], "unresolved");
        assert!(state["settlement"].is_null());
        assert_eq!(state["receipt"], receipt);
    }
    rig.put_items(&json!([]));
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
        let fixtures: Value = serde_json::from_str(include_str!(
            "../../crates/todo-core/tests/fixtures/unit-exports.json"
        ))
        .unwrap();
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
            assert_eq!(rig.tickets(), json!([]));
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
    let fixtures: Value = serde_json::from_str(include_str!(
        "../../crates/todo-core/tests/fixtures/unit-exports.json"
    ))
    .unwrap();
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
    assert_eq!(rig.tickets(), json!([]));
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

// ---- Custody: a claimant settles the inherited unit. ----

const ID: &str = "r-1793174082524-94752-0";

fn local_host() -> String {
    let out = Command::new("hostname").output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn returned_event(execution: &Value, with: &str) -> Value {
    let export = exported(0);
    let binding = json!({"contract":export["contract"]["ref"],"execution":execution,"exportDigest":export["digest"]});
    match with {
        "returned" => json!({"version":1,"eventId":format!("{ID}-returned"),"binding":binding,
            "result":{"kind":"returned","evidence":{"execution":execution,"resultId":"res-1","summary":"done"}}}),
        _ => json!({"version":1,"eventId":format!("{ID}-stopped"),"binding":binding,
            "result":{"kind":"stopped","reason":"worker pane closed without a final reply"}}),
    }
}

/// A committed unit owned by `w1:p1` with a saved settlement of the given kind.
fn settled(name: &str, kind: &str) -> (Rig, Value) {
    let rig = Rig::new(name);
    let export = exported(0);
    let execution = json!({"executor":"dogsled","ownerId":"w1:p1","requestId":ID});
    let binding = json!({"contract":export["contract"]["ref"],"execution":execution,"exportDigest":export["digest"]});
    let mut unit = json!({
        "origin":export["contract"]["ref"],"execution":execution,"placement":{},
        "tool_call_id":"call-1","abandoned":null,
        "committed":{"execution":execution,"binding":binding,"exported":export},
    });
    let key = if kind == "returned" {
        "returned"
    } else {
        "stopped"
    };
    unit[key] = returned_event(&execution, kind);
    fs::create_dir_all(rig.dir.join("state/admissions")).unwrap();
    rig.put(
        &format!("state/admissions/{ID}.json"),
        &json!({"request":{},"launch":null,"error":null,"unit":unit}),
    );
    (rig, execution)
}

/// Move the custody of the unit to `pane` with `session`, as a claim would.
fn claimed(rig: &Rig, host: &str, pane: &str, session: Option<&str>, revision: u64) {
    let mut record = json!({"item":ID,"host":host,"owner_pane":"w1:p1",
        "custodian_pane":pane,"revision":revision,
        "claims":[{"id":format!("claim-{revision}"),"claimant_pane":pane,"claimed_ms":1,
            "previous_custodian":"w1:p1","revision":revision}]});
    if let Some(session) = session {
        record["custodian_session"] = json!(session);
        record["claims"][0]["claimant_session"] = json!(session);
    }
    rig.put("state/custody.json", &json!([record]));
}

fn live_session(rig: &Rig, pane: &str, session: &str) {
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{"pane_id":pane,"agent":"pi","agent_status":"working",
            "agent_session":{"kind":"id","value":session}}]}}),
    );
}

fn as_pane(rig: &Rig, pane: &str, args: &[&str]) -> (i32, Value) {
    let out = rig.cmd(args).env("HERDR_PANE_ID", pane).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    (
        out.status.code().unwrap_or(-1),
        serde_json::from_str(&text).unwrap_or(Value::Null),
    )
}

fn reference(revision: u64, pane: &str) -> Value {
    json!({"itemId":ID,"claimId":format!("claim-{revision}"),"revision":revision,"recipient":pane})
}

fn ack(execution: &Value, kind: &str, custody: &Value) -> String {
    json!({"execution":execution,"eventId":format!("{ID}-{kind}"),"custody":custody}).to_string()
}

#[test]
fn a_claimant_receives_the_unchanged_event_in_a_custody_envelope() {
    let (rig, execution) = settled("unit-custody-delivery", "returned");
    claimed(&rig, &local_host(), "w2:p1", Some("s2"), 1);
    live_session(&rig, "w2:p1", "s2");
    let path = format!("state/admissions/{ID}.json");
    let before = fs::read(rig.dir.join(&path)).unwrap();
    let (code, pending) = as_pane(&rig, "w2:p1", &["unit", "settlements"]);
    assert_eq!(code, 0, "{pending}");
    assert_eq!(
        pending,
        json!([{"version":1,"custody":reference(1, "w2:p1"),
            "settlement":returned_event(&execution, "returned")}])
    );
    assert_eq!(fs::read(rig.dir.join(&path)).unwrap(), before);
    // The previous custodian no longer receives it.
    assert_eq!(rig.json(&["unit", "settlements"]).1, json!([]));
}

#[test]
fn delivery_and_ack_refuse_a_caller_that_is_not_the_live_custodian() {
    let (rig, execution) = settled("unit-custody-refused", "returned");
    let host = local_host();
    let custody = reference(1, "w2:p1");
    let input = ack(&execution, "returned", &custody);
    let attempt = |pane: &str| {
        let (_, pending) = as_pane(&rig, pane, &["unit", "settlements"]);
        let (code, _) = as_pane(&rig, pane, &["unit", "ack-settlement", "--input", &input]);
        (pending, code)
    };
    // A foreign host record is refused.
    claimed(&rig, "elsewhere", "w2:p1", Some("s2"), 2);
    live_session(&rig, "w2:p1", "s2");
    assert_eq!(attempt("w2:p1"), (json!([]), 2));
    // The session behind a reused pane differs.
    claimed(&rig, &host, "w2:p1", Some("s2"), 2);
    live_session(&rig, "w2:p1", "other");
    assert_eq!(attempt("w2:p1"), (json!([]), 2));
    // An unreadable session is refused.
    rig.put("agents.json", &json!({"result":{"agents":[]}}));
    assert_eq!(attempt("w2:p1"), (json!([]), 2));
    // A previous custodian is refused.
    live_session(&rig, "w2:p1", "s2");
    assert_eq!(attempt("w1:p1").1, 2);
    // A stale revision is refused.
    claimed(&rig, &host, "w2:p1", Some("s2"), 2);
    assert_eq!(attempt("w2:p1").1, 2);
    let saved = rig.read(&format!("state/admissions/{ID}.json"));
    assert!(saved["unit"]["custody_acks"].is_null(), "{saved}");
}

#[test]
fn custody_does_not_widen_commit_or_abandonment() {
    let (rig, execution) = settled("unit-custody-narrow", "returned");
    claimed(&rig, &local_host(), "w2:p1", Some("s2"), 1);
    live_session(&rig, "w2:p1", "s2");
    let input = execution_input(&execution).to_string();
    assert_eq!(
        as_pane(
            &rig,
            "w2:p1",
            &["unit", "abandon-prepared", "--input", &input]
        )
        .0,
        2
    );
    let commit = commit_input(&execution, 0).to_string();
    assert_eq!(
        as_pane(&rig, "w2:p1", &["unit", "commit", "--input", &commit]).0,
        2
    );
}

#[test]
fn a_new_custodian_gets_acknowledged_evidence_once_and_duplicate_acks_are_harmless() {
    let (rig, execution) = settled("unit-custody-replay", "returned");
    // The origin owner acknowledges first.
    let owner_ack = json!({"execution":execution,"eventId":format!("{ID}-returned")});
    assert_eq!(
        operation(&rig, "ack-settlement", &owner_ack).1["applied"],
        true
    );
    assert_eq!(rig.json(&["unit", "settlements"]).1, json!([]));
    claimed(&rig, &local_host(), "w2:p1", Some("s2"), 1);
    live_session(&rig, "w2:p1", "s2");
    let (_, pending) = as_pane(&rig, "w2:p1", &["unit", "settlements"]);
    assert_eq!(pending.as_array().unwrap().len(), 1, "{pending}");
    let input = ack(&execution, "returned", &reference(1, "w2:p1"));
    let args = ["unit", "ack-settlement", "--input", &input];
    assert_eq!(as_pane(&rig, "w2:p1", &args).1["applied"], true);
    assert_eq!(as_pane(&rig, "w2:p1", &args).1["applied"], false);
    assert_eq!(
        as_pane(&rig, "w2:p1", &["unit", "settlements"]).1,
        json!([])
    );
    // The historical owner acknowledgement is kept.
    let saved = rig.read(&format!("state/admissions/{ID}.json"));
    assert_eq!(saved["unit"]["settlement_ack"], format!("{ID}-returned"));
}

#[test]
fn a_custody_less_ack_is_refused_once_custody_is_claimed() {
    let (rig, execution) = settled("unit-custody-legacy-ack", "returned");
    let path = format!("state/admissions/{ID}.json");
    let legacy = json!({"execution":execution,"eventId":format!("{ID}-returned")});
    // Before any claim the legacy acknowledgement still works.
    assert_eq!(
        operation(&rig, "ack-settlement", &legacy).1["applied"],
        true
    );
    let (rig, execution) = settled("unit-custody-legacy-claimed", "returned");
    claimed(&rig, &local_host(), "w2:p1", Some("s2"), 1);
    live_session(&rig, "w2:p1", "s2");
    let before = fs::read(rig.dir.join(&path)).unwrap();
    // The origin owner passes the owner check, but custody has moved.
    let legacy = json!({"execution":execution,"eventId":format!("{ID}-returned")}).to_string();
    let (code, _) = rig.json(&["unit", "ack-settlement", "--input", &legacy]);
    assert_eq!(code, 2);
    assert_eq!(fs::read(rig.dir.join(&path)).unwrap(), before);
    let saved = rig.read(&path);
    assert!(saved["unit"]["settlement_ack"].is_null(), "{saved}");
    // The live custodian still acknowledges with its custody reference.
    let input = ack(&execution, "returned", &reference(1, "w2:p1"));
    let args = ["unit", "ack-settlement", "--input", &input];
    assert_eq!(as_pane(&rig, "w2:p1", &args).1["applied"], true);
}
