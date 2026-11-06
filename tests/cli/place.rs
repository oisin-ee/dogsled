use super::*;

fn saved_grant(attempt: &Value) -> Value {
    let progress = &attempt["placement"];
    match progress["phase"].as_str().unwrap() {
        "host_selected" | "preparation_pending" => progress["grant"].clone(),
        "prepared" => progress["prepared"]["grant"].clone(),
        _ => progress["saved"]["prepared"]["grant"].clone(),
    }
}

pub(super) fn interrupted_placement(boundary: &str) -> (Rig, PathBuf, String, Background) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let (rig, repo) = place_rig(&format!(
        "{boundary}-{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let observer = placement_observer(&rig);
    let out = rig
        .cmd(&[
            "place",
            "--host",
            "local",
            "--repo",
            repo.to_str().unwrap(),
            "--branch",
            "resume",
            "--base",
            "main",
        ])
        .env("DOGSLED_TEST_UNIT_CRASH", boundary)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(86), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    let id = stderr
        .lines()
        .find_map(|line| line.strip_prefix("placement request "))
        .unwrap()
        .to_owned();
    assert!(rig.has_admission(&id));
    (rig, repo, id, observer)
}

fn queue_interrupted(rig: &Rig, id: &str) {
    let request = rig.admission(id)["request"].clone();
    rig.put_tickets(&json!([{"id":"cancel-ticket","request":request,
        "created_ms":rig.origin_ms,"state":"pending","waiting":false,"notified":false,
        "retry":{"attempts":0,"next_ms":rig.origin_ms}}]));
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
}

#[test]
fn cancellation_rejects_another_requests_saved_disposition() {
    let (rig, _, id, observer) = interrupted_placement("placement-result-saved");
    let (other, _, other_id, other_observer) = interrupted_placement("placement-result-saved");
    drop(observer);
    drop(other_observer);
    queue_interrupted(&rig, &id);
    let mut admission = rig.admission(&id);
    admission["placement"]["saved"]["disposition"] =
        other.admission(&other_id)["placement"]["saved"]["disposition"].clone();
    rig.put_admission(&id, &admission);
    let claims = rig.preparations();
    let other_claims = other.preparation_record("state", "claims", "").read();
    assert_eq!(rig.json(&["cancel", "cancel-ticket"]).0, 2);
    assert!(rig.run(&["serve", "--once"]).status.success());
    assert_eq!(rig.preparations(), claims);
    assert_eq!(
        other.preparation_record("state", "claims", "").read(),
        other_claims
    );
    assert!(
        rig.tickets()[0]["error"]
            .as_str()
            .unwrap()
            .contains("disposition")
    );
    assert!(!rig.preparation_record("state", "disposition", &id).exists());
    assert!(!rig.log("ssh.log").contains("host prepared"));
}

#[test]
fn cancellation_after_resume_completion_is_too_late_with_pending_ticket() {
    let (rig, _, id, _observer) = interrupted_placement("placement-result-saved");
    queue_interrupted(&rig, &id);
    let (code, result) = rig.json(&["place", "--resume", &id]);
    assert_eq!(code, 0, "{result}");
    assert_eq!(rig.tickets()[0]["state"], "pending");
    let (code, cancelled) = rig.json(&["cancel", "cancel-ticket"]);
    assert_eq!(code, 2, "{cancelled}");
    assert_eq!(cancelled["status"], "too_late");
    assert_eq!(rig.json(&["place", "--resume", &id]), (0, result));
}

#[test]
fn cancellation_after_placement_result_saved_is_too_late() {
    let (rig, _, id, observer) = interrupted_placement("placement-result-saved");
    drop(observer);
    queue_interrupted(&rig, &id);
    let (code, cancelled) = rig.json(&["cancel", "cancel-ticket"]);
    assert_eq!(code, 2, "{cancelled}");
    assert_eq!(cancelled["status"], "too_late");
    assert_eq!(rig.tickets()[0]["state"], "pending");
    let result = rig.admission(&id)["placement"]["saved"]["result"].clone();
    assert_eq!(
        rig.preparation_record("state", "claims", "")
            .read()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(rig.run(&["serve", "--once"]).status.success());
    assert_eq!(rig.tickets()[0]["state"], "admitted");
    assert_eq!(rig.preparations(), json!([]));
    assert_eq!(rig.admission(&id)["placement"]["phase"], "completed");
    let receipt_file = rig.preparation_record("state", "disposition", &id);
    let receipt = receipt_file.read();
    assert_eq!(receipt["progress"], "released");
    assert!(rig.run(&["serve", "--once"]).status.success());
    assert_eq!(receipt_file.read(), receipt);
    assert_eq!(rig.json(&["place", "--resume", &id]), (0, result));
}

/// Every state file's bytes. The database counts by its rows: SQLite may checkpoint its
/// WAL into `state.db` on any close, which changes bytes but no record.
fn state_bytes(path: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    state_bytes_except(path, &[])
}
fn state_bytes_except(path: &Path, excluded_tables: &[&str]) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name == "state.db-wal" || name == "state.db-shm" {
            continue;
        }
        if name == "state.db" {
            let connection = rusqlite::Connection::open(&path).unwrap();
            let tables: Vec<String> = connection
                .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            let mut rows = Vec::new();
            for table in tables {
                if excluded_tables.contains(&table.as_str()) {
                    continue;
                }
                let mut statement = connection
                    .prepare(&format!("SELECT * FROM \"{table}\""))
                    .unwrap();
                let width = statement.column_count();
                let mut cursor = statement.query([]).unwrap();
                while let Some(row) = cursor.next().unwrap() {
                    let values: Vec<rusqlite::types::Value> =
                        (0..width).map(|i| row.get(i).unwrap()).collect();
                    rows.push(format!("{table}: {values:?}"));
                }
            }
            rows.sort();
            files.push((path, rows.join("\n").into_bytes()));
        } else if path.is_dir() {
            files.extend(state_bytes_except(&path, excluded_tables));
        } else if path.is_file() {
            files.push((path.clone(), fs::read(path).unwrap()));
        }
    }
    files.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    files
}

fn lookup_fixture(case: &str) -> (Rig, String, Value) {
    let (rig, _, id, observer) = interrupted_placement("placement-preparation-received");
    drop(observer);
    let admission = rig.admission(&id);
    let grant = saved_grant(&admission);
    let mut transport = grant["envelope"].clone();
    transport["expected_grant"] = grant;
    let mut query = json!({"version":1,"placement":transport,
        "preparation":admission["placement"]["preparation"],"binding":null});
    let file = rig.preparation_record("state", "receipt", &id);
    let mut receipt = file.read();
    match case {
        "absent" => {
            file.delete();
            rig.preparation_record("state", "claims", "")
                .put(&json!([]));
        }
        "incomplete" => {
            receipt["prepared"] = Value::Null;
            receipt["binding"] = Value::Null;
            file.put(&receipt);
        }
        "failed" => {
            receipt["prepared"] = Value::Null;
            receipt["binding"]["prepared"] = Value::Null;
            receipt["failure"] = json!({"operation":"prepare","kind":"other","request_id":id,"detail_withheld":true});
            file.put(&receipt);
        }
        "disposition" => {
            let input = rig.disposition_request(&id);
            assert_eq!(rig.json(&["host", "prepared", "--input", &input]).0, 0);
        }
        "unsupported" => {
            receipt["version"] = json!(99);
            file.put(&receipt);
        }
        "corrupt" => {
            let before = file.read();
            assert!(
                !rig.put_preparation_refused("state", "receipt", &id, &json!("{broken"))
                    .status
                    .success()
            );
            assert_eq!(file.read(), before);
            rig.state(&["fault", "preparations"]);
        }
        "mismatch" => {
            query["binding"] = receipt["binding"].clone();
            query["binding"]["claim"]["tree"]["head"] = json!("changed");
        }
        _ => {}
    }
    (rig, id, query)
}

fn assert_nonplacement_lookup(case: &str, state: &str, outcome: Option<&str>) {
    let (rig, id, placement) = lookup_fixture(case);
    let mut request = placement["placement"]["request"].clone();
    request["action"] = json!({"type":"agent","kind":"pi","name":"lookup","message":"read only"});
    let query = json!({"version":1,"request":request,
        "host":placement["placement"]["host"],"preparation":placement["preparation"],"binding":placement["binding"]});
    rig.preparation_record("state", "grant", &id).delete();
    let before = state_bytes(&rig.dir.join("state"));
    let (code, reply) = rig.json(&[
        "host",
        "preparation-lookup",
        "--preparation-protocol",
        "1",
        "--input",
        &query.to_string(),
    ]);
    assert_eq!(code, 0, "{reply}");
    assert_eq!(reply["query"], query);
    assert_eq!(reply["result"]["state"], state, "{reply}");
    if let Some(outcome) = outcome {
        assert_eq!(reply["result"]["outcome"]["outcome"], outcome, "{reply}");
    }
    assert_eq!(state_bytes(&rig.dir.join("state")), before);
}

#[test]
fn nonplacement_lookup_absent_is_read_only() {
    assert_nonplacement_lookup("absent", "absent", None);
}
#[test]
fn nonplacement_lookup_receipt_is_read_only() {
    assert_nonplacement_lookup("prepared", "receipt_only", Some("prepared"));
}
#[test]
fn nonplacement_lookup_incomplete_is_read_only() {
    assert_nonplacement_lookup("incomplete", "receipt_only", Some("incomplete"));
}
#[test]
fn nonplacement_lookup_failed_is_read_only() {
    assert_nonplacement_lookup("failed", "receipt_only", Some("failed"));
}
#[test]
fn nonplacement_lookup_disposition_is_read_only() {
    assert_nonplacement_lookup("disposition", "disposition", Some("prepared"));
}
#[test]
fn nonplacement_lookup_unsupported_is_read_only() {
    assert_nonplacement_lookup("unsupported", "unsupported_version", None);
}
#[test]
fn nonplacement_lookup_corrupt_is_unresolved() {
    assert_nonplacement_lookup("corrupt", "unresolved", None);
}
#[test]
fn nonplacement_lookup_mismatched_binding_is_refused() {
    assert_nonplacement_lookup("mismatch", "mismatch", None);
}
#[test]
fn nonplacement_lookup_mismatched_request_is_refused() {
    let (rig, _, placement) = lookup_fixture("prepared");
    let mut request = placement["placement"]["request"].clone();
    request["action"] = json!({"type":"agent","kind":"pi","name":"lookup","message":"read only"});
    request["id"] = json!("another-request");
    let query = json!({"version":1,"request":request,"host":placement["placement"]["host"],"preparation":placement["preparation"],"binding":null});
    let before = state_bytes(&rig.dir.join("state"));
    let (code, reply) = rig.json(&[
        "host",
        "preparation-lookup",
        "--preparation-protocol",
        "1",
        "--input",
        &query.to_string(),
    ]);
    assert_eq!(code, 0, "{reply}");
    assert_eq!(reply["result"]["state"], "mismatch", "{reply}");
    assert_eq!(state_bytes(&rig.dir.join("state")), before);
}

fn assert_lookup(case: &str, state: &str, outcome: Option<&str>) {
    let (rig, _, query) = lookup_fixture(case);
    let before = state_bytes(&rig.dir.join("state"));
    let (code, reply) = rig.json(&[
        "host",
        "preparation-lookup",
        "--preparation-protocol",
        "1",
        "--input",
        &query.to_string(),
    ]);
    assert_eq!(code, 0, "{reply}");
    assert_eq!(reply["query"], query);
    assert_eq!(reply["result"]["state"], state, "{reply}");
    if let Some(outcome) = outcome {
        assert_eq!(reply["result"]["outcome"]["outcome"], outcome, "{reply}");
    }
    assert_eq!(state_bytes(&rig.dir.join("state")), before);
}

#[test]
fn queued_placement_accepts_symlinked_repository_root() {
    let (rig, repo) = place_rig("queued-place-alias");
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    let temp_root = rig.dir.join("tmp-alias");
    std::os::unix::fs::symlink(rig.dir.join("home"), &temp_root).unwrap();
    let aliased_repo = temp_root.join(repo.file_name().unwrap());
    assert_ne!(aliased_repo, repo);
    rig.put_tickets(&json!([{
            "id":"alias-ticket","request":{"id":"alias-request","validity":request_authority(&rig,"alias-request"),"action":{"type":"place"},
                "host":"local","branch":"alias-branch","base":"main","cwd":aliased_repo,
                "repository":{"path":aliased_repo,"origin":"https://example.invalid/repo.git","relative":"repo space","working_root":aliased_repo},
                "workspace":null,"workspace_name":null,"owner":"w1:p1","owner_host":"local"},
            "created_ms":1,"state":"pending","waiting":false,"waiting_until_ms":null,
            "notified":false,"result":null,"error":null,"retry":{"attempts":0,"next_ms":1}
        }]),
    );
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(out.status.success(), "{out:?}");
    let ticket = &rig.tickets()[0];
    assert_eq!(ticket["state"], "admitted", "{ticket}");
    assert_eq!(ticket["retry"]["attempts"], 0, "{ticket}");
    let returned = ticket["result"].clone();
    assert_eq!(
        rig.json(&["place", "--resume", "alias-request"]),
        (0, returned)
    );
    assert_eq!(rig.log("herdr.log").matches("worktree open").count(), 1);
    assert_eq!(rig.log("setup.log").lines().count(), 1);
}

#[test]
fn cancellation_basis_refuses_invalid_operation_or_request_without_releasing_claim() {
    for case in ["operation", "request"] {
        let (rig, _, id, _observer) = interrupted_placement("placement-preparation-received");
        let mut intent: Value = serde_json::from_str(&rig.disposition_request(&id)).unwrap();
        let mut basis = json!({"operation_id":"cancel-operation","request_id":id});
        basis[if case == "operation" {
            "operation_id"
        } else {
            "request_id"
        }] = json!(if case == "operation" {
            "bad/id"
        } else {
            "another-request"
        });
        intent["policy"] = json!({"kind":"prohibit","basis":{"cancelled":basis}});
        let claims = rig.preparations();
        let (code, _) = rig.json(&["host", "prepared", "--input", &intent.to_string()]);
        assert_eq!(code, 2);
        assert_eq!(rig.preparations(), claims);
        assert!(!rig.preparation_record("state", "disposition", &id).exists());
    }
}

#[test]
fn preparation_lookup_absent_is_read_only() {
    assert_lookup("absent", "absent", None);
}
#[test]
fn preparation_lookup_receipt_is_read_only() {
    assert_lookup("prepared", "receipt_only", Some("prepared"));
}
#[test]
fn preparation_lookup_incomplete_stays_uncertain() {
    assert_lookup("incomplete", "receipt_only", Some("incomplete"));
}
#[test]
fn preparation_lookup_failed_is_read_only() {
    assert_lookup("failed", "receipt_only", Some("failed"));
}
#[test]
fn preparation_lookup_disposition_is_read_only() {
    assert_lookup("disposition", "disposition", Some("prepared"));
}
#[test]
fn preparation_lookup_unsupported_version_is_read_only() {
    assert_lookup("unsupported", "unsupported_version", None);
}
#[test]
fn preparation_lookup_corruption_is_unresolved() {
    assert_lookup("corrupt", "unresolved", None);
}
#[test]
fn preparation_lookup_rejects_mismatched_binding() {
    assert_lookup("mismatch", "mismatch", None);
    for field in ["request", "host", "claim"] {
        let (rig, id, mut query) = lookup_fixture("prepared");
        match field {
            "request" => query["preparation"]["request_id"] = json!("foreign"),
            "host" => query["placement"]["host"]["target"] = json!("changed-target"),
            _ => {
                query["binding"] =
                    rig.preparation_record("state", "receipt", &id).read()["binding"].clone();
                query["binding"]["claim"]["request_id"] = json!("foreign");
            }
        }
        let before = state_bytes(&rig.dir.join("state"));
        let (code, reply) = rig.json(&[
            "host",
            "preparation-lookup",
            "--preparation-protocol",
            "1",
            "--input",
            &query.to_string(),
        ]);
        assert_eq!(code, 0, "{reply}");
        assert_eq!(reply["result"]["state"], "mismatch", "{field}: {reply}");
        assert_eq!(state_bytes(&rig.dir.join("state")), before);
    }
}

#[test]
fn preparation_lookup_busy_gate_stays_unresolved_without_writes() {
    let (rig, id, query) = lookup_fixture("absent");
    let gate = fs::File::open(
        rig.dir
            .join(format!("state/preparation-receipts/{id}.lock")),
    )
    .unwrap();
    gate.lock().unwrap();
    let before = state_bytes(&rig.dir.join("state"));
    let (code, reply) = rig.json(&[
        "host",
        "preparation-lookup",
        "--preparation-protocol",
        "1",
        "--input",
        &query.to_string(),
    ]);
    assert_eq!(code, 0, "{reply}");
    assert_eq!(reply["result"]["state"], "unresolved");
    assert_eq!(state_bytes(&rig.dir.join("state")), before);
}

#[test]
fn preparation_lookup_rejects_unsupported_transport_without_writes() {
    let (rig, _, query) = lookup_fixture("prepared");
    let before = state_bytes(&rig.dir.join("state"));
    let (code, reply) = rig.json(&[
        "host",
        "preparation-lookup",
        "--preparation-protocol",
        "99",
        "--input",
        &query.to_string(),
    ]);
    assert_eq!(code, 2, "{reply}");
    assert!(
        reply["error"]
            .as_str()
            .unwrap()
            .contains("unsupported preparation")
    );
    assert_eq!(state_bytes(&rig.dir.join("state")), before);
}

pub(super) fn real_preparation_peer(rig: &Rig) {
    real_reservation_peer(rig, false);
    let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
    let handler = format!(
        r#"  *'host prepare '*|*'host prepared '*)
    for remote do :; done
    remote=${{remote#* host }}
    DOGSLED_STATE="$RIG/peer-state" sh -c '{} host '"$remote" > "$RIG/preparation-response"
    status=$?; test "$status" = 0 || exit "$status"
    case "$remote" in
      'prepare '*) marker=lose-preparation ;;
      *) marker=lose-release ;;
    esac
    if test -f "$RIG/$marker"; then rm "$RIG/$marker"; exit 255; fi
    cat "$RIG/preparation-response" ;;
"#,
        env!("CARGO_BIN_EXE_dogsled")
    );
    rig.script(
        "ssh",
        &script.replacen("case \"$*\" in", &format!("case \"$*\" in\n{handler}"), 1),
    );
}

fn lost_placement_reply(marker: &str) {
    let (rig, repo) = place_rig(marker);
    real_preparation_peer(&rig);
    fs::write(rig.dir.join(marker), "").unwrap();
    let _observer = peer_observer(&rig);
    let (code, first) = rig.json(&[
        "place",
        "--host",
        "netcup",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "lost-reply",
        "--base",
        "main",
    ]);
    assert_eq!(code, 1, "{first}");
    let id = first["request_id"].as_str().unwrap();
    let original = rig.preparation_record("peer-state", "receipt", id).read();
    let grant = rig.preparation_record("peer-state", "grant", id).read();
    let saved = rig.admission(id);
    let result = if marker == "lose-release" {
        assert_eq!(first["status"], "pending_release");
        assert_eq!(saved["placement"]["phase"], "result_saved");
        assert_eq!(saved["placement"]["saved"]["result"], first["result"]);
        assert_eq!(
            rig.preparation_record("peer-state", "disposition", id)
                .read()["progress"],
            "released"
        );
        Some(first["result"].clone())
    } else {
        assert_eq!(first["queued"], true);
        assert_eq!(saved["placement"]["phase"], "preparation_pending");
        None
    };
    let (code, resumed) = rig.json(&["place", "--resume", id]);
    assert_eq!(code, 0, "{resumed}");
    assert_eq!(resumed["request_id"], id);
    assert_eq!(resumed["reservation"], id);
    assert_eq!(resumed["path"], original["prepared"]["path"]);
    assert_eq!(resumed["pane_id"], original["prepared"]["pane_id"]);
    if let Some(result) = result {
        assert_eq!(resumed, result);
    }
    assert_eq!(
        outcome_identity(rig.preparation_record("peer-state", "receipt", id).read()),
        outcome_identity(original)
    );
    assert_eq!(
        rig.preparation_record("peer-state", "grant", id).read(),
        grant
    );
    let log = rig.log("ssh.log");
    assert_eq!(log.matches("host reserve ").count(), 1, "{log}");
    assert_eq!(log.matches("host prepare ").count(), 1, "{log}");
    assert_eq!(
        log.matches("host prepared ").count(),
        if marker == "lose-release" { 2 } else { 1 },
        "{log}"
    );
    assert_eq!(rig.log("setup.log").lines().count(), 1);
    assert_eq!(rig.log("herdr.log").matches("worktree open").count(), 1);
    let before = rig.log("ssh.log");
    assert_eq!(rig.json(&["place", "--resume", id]), (0, resumed));
    assert_eq!(rig.log("ssh.log"), before);
    assert_eq!(
        rig.preparation_record("peer-state", "claims", "").read(),
        json!([])
    );
}

#[test]
fn review_verified_absent_preparation_resends_original_delivery() {
    let (rig, repo) = place_rig("before-preparation");
    real_preparation_peer(&rig);
    let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
    rig.script("ssh", &script.replacen("  *'host prepare '*|*'host prepared '*)", "  *'host prepare '*|*'host prepared '*)\n    case \"$*\" in *'host prepare '*) if test ! -f \"$RIG/failed-before-receipt\"; then touch \"$RIG/failed-before-receipt\"; exit 255; fi ;; esac", 1));
    let _observer = peer_observer(&rig);
    let (code, first) = rig.json(&[
        "place",
        "--host",
        "netcup",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "before-receipt",
        "--base",
        "main",
    ]);
    assert_eq!(code, 1, "{first}");
    let id = first["request_id"].as_str().unwrap();
    assert!(!rig.preparation_record("peer-state", "receipt", id).exists());
    let original = rig.admission(id);
    let grant = rig.preparation_record("peer-state", "grant", id).read();
    let (code, resumed) = rig.json(&["place", "--resume", id]);
    assert_eq!(code, 0, "{resumed}");
    assert_eq!(resumed["request_id"], id);
    assert_eq!(
        rig.preparation_record("peer-state", "grant", id).read(),
        grant
    );
    assert_eq!(
        rig.admission(id)["placement"]["saved"]["prepared"]["preparation"],
        original["placement"]["preparation"]
    );
    assert_eq!(rig.log("setup.log").lines().count(), 1);
    assert_eq!(rig.log("herdr.log").matches("worktree open").count(), 1);
    let log = rig.log("ssh.log");
    assert_eq!(log.matches("host reserve ").count(), 1, "{log}");
    assert_eq!(log.matches("host prepare ").count(), 2, "{log}");
    let deliveries: Vec<_> = log
        .lines()
        .filter(|line| line.contains("host prepare "))
        .map(|line| line.split(" --budget-ms ").next().unwrap())
        .collect();
    assert_eq!(deliveries[0], deliveries[1]);
    assert!(deliveries[0].contains("\"expected_grant\":{"));
}

#[test]
fn review_failed_preparation_finishes_matching_failure_disposition() {
    let (rig, repo) = place_rig("failure-recorded-resume");
    fs::write(rig.dir.join("setup-fails"), "").unwrap();
    let _observer = placement_observer(&rig);
    let out = rig
        .cmd(&[
            "place",
            "--host",
            "local",
            "--repo",
            repo.to_str().unwrap(),
            "--branch",
            "failed-resume",
            "--base",
            "main",
        ])
        .env("DOGSLED_TEST_UNIT_CRASH", "preparation-failure-recorded")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(86), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    let id = stderr
        .lines()
        .find_map(|line| line.strip_prefix("placement request "))
        .unwrap();
    let original = rig.preparation_record("state", "receipt", id).read();
    assert!(original["failure"].is_object());
    assert_eq!(rig.preparations()[0]["request_id"], id);
    let other = json!({"request_id":"other","path":"/other","repo":"/other","branch":"other","head":"other"});
    let mut held = rig.preparations();
    held.as_array_mut().unwrap().push(other.clone());
    rig.preparation_record("state", "claims", "").put(&held);
    let before = rig.log("setup.log");
    let (code, failure) = rig.json(&["place", "--resume", id]);
    assert_eq!(code, 2, "{failure}");
    assert_eq!(rig.preparations(), json!([other.clone()]));
    assert_eq!(failure["request_id"], id);
    assert_eq!(
        failure["diagnostic"]["cause"],
        original["failure"]["diagnostic"]["cause"]
    );
    let disposition = rig.preparation_record("state", "disposition", id).read();
    assert_eq!(disposition["request"]["binding"], original["binding"]);
    assert_eq!(
        disposition["request"]["policy"]["basis"],
        "preparation_failed"
    );
    assert_eq!(disposition["progress"], "released");
    assert_eq!(
        outcome_identity(rig.preparation_record("state", "receipt", id).read()),
        outcome_identity(original)
    );
    assert_eq!(rig.json(&["place", "--resume", id]), (2, failure));
    assert_eq!(
        rig.preparation_record("state", "disposition", id).read(),
        disposition
    );
    assert_eq!(rig.log("setup.log"), before);
    assert_eq!(rig.preparations(), json!([other]));
}

#[test]
fn review_symlinked_host_home_recovers_lost_preparation_reply() {
    use std::os::unix::fs::symlink;
    let (rig, repo) = place_rig("host-home-alias");
    symlink(rig.dir.join("home"), rig.dir.join("alias-home")).unwrap();
    real_preparation_peer(&rig);
    let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
    rig.script(
        "ssh",
        &script.replace("DOGSLED_STATE=", "HOME=\"$RIG/alias-home\" DOGSLED_STATE="),
    );
    fs::write(rig.dir.join("lose-preparation"), "").unwrap();
    let _observer = peer_observer(&rig);
    let (code, first) = rig.json(&[
        "place",
        "--host",
        "netcup",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "symlink-reply",
        "--base",
        "main",
    ]);
    assert_eq!(code, 1, "{first}");
    let id = first["request_id"].as_str().unwrap();
    let original = rig.preparation_record("peer-state", "receipt", id).read();
    let grant = rig.preparation_record("peer-state", "grant", id).read();
    let (code, resumed) = rig.json(&["place", "--resume", id]);
    assert_eq!(code, 0, "{resumed}");
    assert_eq!(resumed["path"], original["prepared"]["path"]);
    assert_eq!(resumed["pane_id"], original["prepared"]["pane_id"]);
    assert_eq!(rig.log("setup.log").lines().count(), 1);
    assert_eq!(rig.log("herdr.log").matches("worktree open").count(), 1);
    assert_eq!(rig.log("ssh.log").matches("host reserve ").count(), 1);
    assert_eq!(rig.log("ssh.log").matches("host prepare ").count(), 1);
    assert_eq!(
        outcome_identity(rig.preparation_record("peer-state", "receipt", id).read()),
        outcome_identity(original)
    );
    assert_eq!(
        rig.preparation_record("peer-state", "grant", id).read(),
        grant
    );
}

#[test]
fn lost_preparation_reply_resumes_saved_host_outcome() {
    lost_placement_reply("lose-preparation");
}
#[test]
fn lost_release_ack_resumes_without_preparing_again() {
    lost_placement_reply("lose-release");
}

#[test]
fn concurrent_cli_resume_and_service_replay_once() {
    use std::io::Write as _;
    let (rig, _, id, observer) = interrupted_placement("placement-preparation-received");
    drop(observer);
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    let original = rig.admission(&id);
    let grant = rig.preparation_record("state", "grant", &id).read();
    rig.put_tickets(&json!([{"id":"resume-ticket","request":original["request"],
        "created_ms":rig.origin_ms,"state":"pending","waiting":false,"notified":false,
        "retry":{"attempts":0,"next_ms":rig.origin_ms}}]));
    let admission = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(rig.dir.join(format!("state/admissions/{id}.lock")))
        .unwrap();
    admission.lock().unwrap();
    let ready: Vec<_> = ["cli", "service"]
        .map(|name| Fifo::new(&rig.dir, &format!("{name}.ready")))
        .into();
    let go: Vec<_> = ["cli", "service"]
        .map(|name| Fifo::new(&rig.dir, &format!("{name}.go")))
        .into();
    let _ready_holds: Vec<_> = ready.iter().map(Fifo::hold).collect();
    let mut go_holds: Vec<_> = go.iter().map(Fifo::hold).collect();
    let mut contenders = Vec::new();
    for (name, args) in [
        ("cli", vec!["--json", "place", "--resume", id.as_str()]),
        ("service", vec!["serve", "--once"]),
    ] {
        let mut cmd = rig.cmd(&args);
        cmd.env(
            if name == "cli" {
                "DOGSLED_TEST_LAUNCH_PLACEMENT_BEFORE_ADMISSION"
            } else {
                "DOGSLED_TEST_LAUNCH_PLACEMENT_SERVICE_BEFORE_ADMISSION"
            },
            rig.dir.join(name),
        );
        cmd.stdout(fs::File::create(rig.dir.join(format!("{name}-output"))).unwrap());
        cmd.stderr(fs::File::create(rig.dir.join(format!("{name}-error"))).unwrap());
        contenders.push(Owned::spawn(&mut cmd));
    }
    for fifo in &ready {
        read_fifo(fifo.path(), "placement contender before admission");
    }
    assert_eq!(rig.admission(&id), original);
    for fifo in &mut go_holds {
        fifo.write_all(b"go\n").unwrap();
    }
    drop(admission);
    for child in &mut contenders {
        assert!(child.wait().success());
    }
    let returned: Value = serde_json::from_str(&rig.log("cli-output")).unwrap();
    let queue = rig.tickets();
    assert_eq!(queue.as_array().unwrap().len(), 1);
    assert_eq!(queue[0]["state"], "admitted", "{queue}");
    assert_eq!(queue[0]["result"], returned);
    assert_eq!(rig.admission(&id)["placement"]["saved"]["result"], returned);
    assert_eq!(rig.preparation_record("state", "grant", &id).read(), grant);
    assert_eq!(rig.reservations().as_array().unwrap().len(), 1);
    assert_eq!(rig.preparations(), json!([]));
    assert_eq!(rig.log("setup.log").lines().count(), 1);
    assert_eq!(rig.log("herdr.log").matches("worktree open").count(), 1);
}

#[test]
fn branchless_completed_resume_skips_repository_capacity_and_preparation() {
    let rig = Rig::short("branchless-resume");
    rig.snapshot("state", rig.origin_ms, 0.0, 8);
    let observer = placement_observer(&rig);
    let (code, placed) = rig.json(&["place", "--host", "local"]);
    assert_eq!(code, 0, "{placed}");
    assert!(placed["path"].is_null());
    assert!(placed["pane_id"].is_null());
    let id = placed["request_id"].as_str().unwrap();
    drop(observer);
    let excluded_tables = [
        "host_snapshot",
        "host_snapshot_probe_agents_by_status",
        "host_snapshot_probe_agent_panes",
        "host_snapshot_probe_reservations",
        "host_snapshot_probe_panes",
        "host_snapshot_jobs",
    ];
    let before = state_bytes_except(&rig.dir.join("state"), &excluded_tables);
    rig.script("git", "#!/bin/sh\necho must-not-discover >&2\nexit 3\n");
    rig.snapshot("state", rig.origin_ms, 1000.0, 1);
    let out = rig.json(&["place", "--resume", id]);
    assert_eq!(out, (0, placed.clone()));
    let after = state_bytes_except(&rig.dir.join("state"), &excluded_tables);
    assert_eq!(before, after);
    assert_eq!(rig.state(&["preparation", "receipt"]), json!([]));
    assert!(!rig.log("herdr.log").contains("worktree open"));
}

#[test]
fn resume_refuses_unknown_or_changed_requests_without_effects() {
    for case in [
        "unknown",
        "action",
        "host",
        "branch",
        "base",
        "repo",
        "claim",
        "incomplete",
    ] {
        let (rig, _, id, observer) = interrupted_placement("placement-preparation-received");
        drop(observer);
        let mut resume_id = id.clone();
        let mut args = vec!["place", "--resume"];
        match case {
            "unknown" => resume_id = "unknown-placement".into(),
            "action" => {
                let mut saved = rig.admission(&id);
                saved["request"]["action"] =
                    json!({"type":"job","cmd":["true"],"timeout_ms":null,"idle_timeout_ms":null});
                rig.put_admission(&id, &saved);
            }
            "claim" => {
                let file = rig.preparation_record("state", "receipt", &id);
                let mut saved = file.read();
                saved["binding"]["claim"]["request_id"] = json!("foreign");
                file.put(&saved);
            }
            "incomplete" => {
                let file = rig.preparation_record("state", "receipt", &id);
                let mut saved = file.read();
                saved["prepared"] = Value::Null;
                saved["binding"] = Value::Null;
                file.put(&saved);
            }
            _ => {}
        }
        args.push(&resume_id);
        let flag = format!("--{case}");
        if matches!(case, "host" | "branch" | "base" | "repo") {
            args.extend([flag.as_str(), "changed"]);
        }
        let before = state_bytes(&rig.dir.join("state"));
        let setup = rig.log("setup.log");
        let herdr = rig.log("herdr.log");
        let out = rig.run(&args);
        assert!(!out.status.success(), "{case}: {out:?}");
        assert_eq!(state_bytes(&rig.dir.join("state")), before, "{case}");
        assert_eq!(rig.log("setup.log"), setup);
        assert_eq!(
            rig.log("herdr.log")
                .lines()
                .filter(|line| *line != "machine list --json")
                .collect::<Vec<_>>(),
            herdr
                .lines()
                .filter(|line| *line != "machine list --json")
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn placement_host_report_deadline_fails_without_granting_capacity() {
    let rig = Rig::short("placement-report-deadline");
    rig.peer();
    rig.script("ssh", r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/ssh.log"
case "$*" in
 *'host snapshot'*) printf '%s\n' '{"deadline":true,"error":"host report deadline reached"}'; exit 2 ;;
 *) echo 'must not reserve after report deadline' >&2; exit 2 ;;
esac
"#);
    let _observer = rig.serve();
    wait_publication(&rig, |frame| {
        frame["views"]["hosts"]["data"]
            .as_array()
            .is_some_and(|hosts| {
                hosts
                    .iter()
                    .any(|host| host["id"] == "8103" && host["failure"] == "deadline")
            })
    });
    assert!(!rig.log("ssh.log").contains("host reserve"));
    let (code, error) = rig.json(&["place", "--host", "netcup"]);
    assert_eq!(code, 2, "{error}");
    assert_eq!(error["deadline"], true, "{error}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("deadline reached"),
        "{error}"
    );
    assert_eq!(rig.state(&["preparation", "grant"]), json!([]));
    assert_eq!(rig.reservations(), json!([]));
    let log = rig.log("ssh.log");
    assert!(!log.contains("host reserve"), "{log}");
    assert!(!log.contains("host prepare"), "{log}");
    assert!(!log.contains("host launch"), "{log}");
}

fn request_authority(rig: &Rig, id: &str) -> Value {
    let stamp = json!({"version":1,"created_ms":rig.now_ms()});
    rig.put_validity(id, &stamp);
    stamp
}

fn placement_envelope(rig: &Rig, id: &str) -> Value {
    json!({"version":3,"expected_grant":null,"reservation":id,"host":{"id":"local","label":"local"},
        "request":{"id":id,"validity":request_authority(rig,id),"action":{"type":"place"},"host":null,"branch":null,"base":null,
            "cwd":rig.dir.join("home"),"repository":null,"workspace":null,"workspace_name":null,
            "owner":"w1:p1","owner_host":null}})
}

fn reserve_delivery(rig: &Rig, envelope: &Value) -> (i32, Value) {
    rig.json(&[
        "host",
        "reserve",
        "--id",
        envelope["reservation"].as_str().unwrap(),
        "--placement-replay",
        &envelope.to_string(),
    ])
}

pub(super) fn real_reservation_peer(rig: &Rig, pause: bool) {
    rig.peer();
    fs::create_dir_all(rig.dir.join("peer-state")).unwrap();
    rig.snapshot("peer-state", rig.origin_ms, 0.0, 16);
    let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
    let barrier = if pause {
        "printf 'reserve\\n' > \"$RIG/reserve.ready\"; read go < \"$RIG/reserve.go\""
    } else {
        ":"
    };
    let handler = format!(
        r#"  *'host preparation-lookup'*)
    for remote do :; done
    remote=${{remote#* host }}
    DOGSLED_STATE="$RIG/peer-state" sh -c '{} host '"$remote" ;;
  *'host reserve'*)
    {barrier}
    for remote do :; done
    remote=${{remote#* host }}
    DOGSLED_STATE="$RIG/peer-state" sh -c '{} host '"$remote" > "$RIG/reserve-response"
    status=$?
    test "$status" = 0 || exit "$status"
    if test -f "$RIG/lose-reply"; then rm "$RIG/lose-reply"; exit 255; fi
    cat "$RIG/reserve-response" ;;"#,
        env!("CARGO_BIN_EXE_dogsled"),
        env!("CARGO_BIN_EXE_dogsled")
    );
    rig.script("ssh", &script.replace("  *'host reserve'*)\n    for remote do :; done\n    sh -c \"$RIG/bin/peer-fixture $remote\" ;;", &handler));
}

#[test]
fn placement_request_is_recorded_before_reservation() {
    use std::io::Write as _;
    let rig = Rig::short("placement-capture");
    real_reservation_peer(&rig, true);
    let ready = Fifo::new(&rig.dir, "reserve.ready");
    let go = Fifo::new(&rig.dir, "reserve.go");
    let _ready = ready.hold();
    let mut release = go.hold();
    let _observer = peer_observer(&rig);
    let mut child = rig.start(&["place", "--host", "netcup"]);
    assert!(read_fifo_within(ready.path(), "reserve ingress", PROCESS_DEADLINE).is_some());
    let files = rig.admissions().as_array().unwrap().clone();
    assert_eq!(
        files.len(),
        1,
        "complete admission must precede reservation"
    );
    let attempt = files[0].clone();
    assert_eq!(attempt["request"]["action"]["type"], "place");
    assert_eq!(attempt["placement"]["phase"], "selection_pending");
    assert_eq!(
        attempt["placement"]["envelope"]["request"],
        attempt["request"]
    );
    assert_eq!(
        attempt["placement"]["envelope"]["host"]["target"],
        "netcup-dev"
    );
    writeln!(release, "go").unwrap();
    assert!(child.wait().success(), "{:?}", child.output());
}

fn lost_reply_replay(after_ttl: bool) {
    let rig = Rig::short(if after_ttl {
        "placement-ttl"
    } else {
        "placement-lost"
    });
    real_reservation_peer(&rig, false);
    fs::write(rig.dir.join("lose-reply"), "").unwrap();
    let observer = peer_observer(&rig);
    let (code, queued) = rig.json(&["place", "--host", "netcup"]);
    assert_eq!(code, 1, "{queued}");
    let ticket = rig.tickets()[0].clone();
    let id = ticket["request"]["id"].as_str().unwrap();
    let grant_path = rig.preparation_record("peer-state", "grant", id);
    let original = grant_path.read();
    let hold = rig.peer_state(&["reservations"]);
    assert_eq!(original["reservation"], hold[0]);
    let retry = ticket["retry"]["next_ms"].as_u64().unwrap();
    let later = if after_ttl { retry + 100_000 } else { retry };
    drop(observer);
    fs::write(rig.dir.join("clock"), later.to_string()).unwrap();
    rig.snapshot("peer-state", later, 1000.0, 16);
    let output = rig.run(&["serve", "--once"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ticket = rig.tickets()[0].clone();
    assert_eq!(ticket["state"], "admitted", "{ticket}");
    assert_eq!(grant_path.read(), original);
    let holds = rig.peer_state(&["reservations"]);
    if after_ttl {
        assert_eq!(holds, json!([]));
    } else {
        assert_eq!(holds, hold);
    }
    assert_eq!(rig.log("ssh.log").matches("host reserve").count(), 2);
    assert!(!rig.log("ssh.log").contains("host launch"));
}

#[test]
fn lost_reservation_reply_replays_one_capacity_grant() {
    lost_reply_replay(false);
}

#[test]
fn reservation_reply_replay_after_ttl_does_not_readmit() {
    lost_reply_replay(true);
}

fn acknowledged_remote_placement(name: &str) -> (Rig, String) {
    let rig = Rig::short(name);
    real_reservation_peer(&rig, false);
    let repo = rig.dir.join("home/repo");
    repository(&repo);
    let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
    rig.script("ssh", &script.replace("  *'host checkout'*)", "  *'host checkout'*) if ! test -f \"$RIG/failed-once\"; then touch \"$RIG/failed-once\"; echo reset >&2; exit 255; fi;"));
    let _observer = peer_observer(&rig);
    let (code, queued) = rig.json(&[
        "place",
        "--host",
        "netcup",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "remote-complete",
        "--base",
        "main",
    ]);
    assert_eq!(code, 1, "{queued}");
    let id = rig.tickets()[0]["request"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let attempt = rig.admission(&id);
    assert_eq!(attempt["placement"]["phase"], "preparation_pending");
    assert_eq!(attempt["placement"]["preparation"]["request_id"], id);
    assert_eq!(
        attempt["placement"]["grant"],
        rig.preparation_record("peer-state", "grant", &id).read()
    );
    (rig, id)
}

#[test]
fn acknowledged_placement_grant_replays_after_real_ttl_without_readmission() {
    let (rig, id) = acknowledged_remote_placement("placement-ack-ttl");
    let grant_path = rig.preparation_record("peer-state", "grant", &id);
    let grant = grant_path.read();
    let original_hold = rig.peer_state(&["reservations"]);
    let later = original_hold[0]["created_ms"].as_u64().unwrap() + 100_000;
    fs::write(rig.dir.join("clock"), later.to_string()).unwrap();
    rig.snapshot("peer-state", later, 1000.0, 16);
    assert!(rig.run(&["serve", "--once"]).status.success());
    let ticket = rig.tickets()[0].clone();
    assert_eq!(ticket["state"], "admitted", "{ticket}");
    assert_eq!(grant_path.read(), grant);
    assert_eq!(saved_grant(&rig.admission(&id)), grant);
    assert_eq!(rig.peer_state(&["reservations"]), original_hold);
    let log = rig.log("ssh.log");
    assert_eq!(log.matches("host reserve").count(), 1, "{log}");
    assert_eq!(log.matches("host prepare ").count(), 1, "{log}");
    assert!(!log.contains("host launch"), "{log}");
}

#[test]
fn acknowledged_placement_missing_grant_never_allocates_after_ttl() {
    let (rig, id) = acknowledged_remote_placement("placement-missing-grant");
    let grant_path = rig.preparation_record("peer-state", "grant", &id);
    let grant = grant_path.read();
    let later = grant["reservation"]["created_ms"].as_u64().unwrap() + 100_000;
    fs::write(rig.dir.join("clock"), later.to_string()).unwrap();
    rig.snapshot("peer-state", later, 0.0, 16);
    rig.put_peer_state(&["reservations", "--put"], &json!([]));
    grant_path.delete();
    let before = rig.peer_state(&["reservations"]);
    assert!(rig.run(&["serve", "--once"]).status.success());
    let ticket = rig.tickets()[0].clone();
    assert_eq!(ticket["state"], "failed", "{ticket}");
    assert!(
        ticket["error"].as_str().unwrap().contains("changed"),
        "{ticket}"
    );
    assert_eq!(rig.log("ssh.log").matches("host reserve").count(), 1);
    assert!(!grant_path.exists());
    assert_eq!(rig.peer_state(&["reservations"]), before);
    assert_eq!(saved_grant(&rig.admission(&id)), grant);
    let log = rig.log("ssh.log");
    assert_eq!(log.matches("host checkout").count(), 1, "{log}");
    assert!(!log.contains("host prepare "), "{log}");
}

#[test]
fn queued_preparation_pending_recovers_the_acknowledged_grant() {
    let (rig, id) = acknowledged_remote_placement("placement-selected-recovery");
    let grant = rig.preparation_record("peer-state", "grant", &id).read();
    let mut queue = rig.tickets();
    queue[0]["state"] = json!("admitting");
    let next = queue[0]["retry"]["next_ms"].as_u64().unwrap();
    rig.put_tickets(&queue);
    fs::write(rig.dir.join("clock"), (next - 1).to_string()).unwrap();
    assert!(rig.run(&["serve", "--once"]).status.success());
    assert_eq!(rig.tickets()[0]["state"], "pending");
    fs::write(rig.dir.join("clock"), next.to_string()).unwrap();
    assert!(rig.run(&["serve", "--once"]).status.success());
    let ticket = rig.tickets()[0].clone();
    assert_eq!(ticket["state"], "admitted", "{ticket}");
    assert_eq!(saved_grant(&rig.admission(&id)), grant);
    assert_eq!(rig.log("ssh.log").matches("host prepare ").count(), 1);
}

#[test]
fn cancellation_of_saved_pending_release_is_too_late_and_preserves_result() {
    let (rig, repo) = place_rig("cancel-pending-release");
    real_preparation_peer(&rig);
    fs::write(rig.dir.join("lose-release"), "").unwrap();
    let observer = peer_observer(&rig);
    let (code, result) = rig.json(&[
        "place",
        "--host",
        "netcup",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "cancel-release",
        "--base",
        "main",
    ]);
    assert_eq!(code, 1, "{result}");
    assert_eq!(result["status"], "pending_release");
    let id = result["request_id"].as_str().unwrap();
    let ticket = rig.tickets()[0]["id"].as_str().unwrap().to_owned();
    let (code, cancellation) = rig.json(&["cancel", &ticket]);
    assert_eq!(code, 2, "{cancellation}");
    assert_eq!(cancellation["status"], "too_late");
    drop(observer);
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    assert!(rig.run(&["serve", "--once"]).status.success());
    assert_eq!(rig.tickets()[0]["state"], "admitted");
    assert!(rig.cancellation(id).is_null());
    assert_eq!(
        rig.admission(id)["placement"]["saved"]["result"],
        result["result"]
    );
    assert_eq!(
        rig.preparation_record("peer-state", "claims", "").read(),
        json!([])
    );
    assert_eq!(rig.admission(id)["placement"]["phase"], "completed");
    assert_eq!(rig.log("ssh.log").matches("host prepared ").count(), 2);
    assert_eq!(rig.log("ssh.log").matches("host prepare ").count(), 1);
}

#[test]
fn result_is_durable_before_host_release() {
    let (rig, repo) = place_rig("result-before-release");
    let request = json!({"id":"durable-placement","validity":request_authority(&rig,"durable-placement"),"action":{"type":"place"},"host":"local",
        "branch":"durable","base":"main","cwd":repo,"owner":"w1:p1","owner_host":"local",
        "repository":{"path":repo,"working_root":repo,"relative":"repo space",
            "origin":"https://example.invalid/repo.git"}});
    rig.put_tickets(&json!([{"id":"durable-ticket","request":request,
        "created_ms":rig.origin_ms,"state":"pending","waiting":false,"notified":false,
        "retry":{"attempts":0,"next_ms":rig.origin_ms}}]));
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    let out = rig
        .cmd(&["serve", "--once"])
        .env("DOGSLED_TEST_UNIT_CRASH", "preparation-fenced")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(86), "{out:?}");
    let saved = rig.admission("durable-placement");
    let result = &saved["placement"]["saved"]["result"];
    assert!(
        result.is_object(),
        "placement result must be durable before host release: {saved}"
    );
    assert_eq!(result["request_id"], "durable-placement");
    assert_eq!(result["reservation"], "durable-placement");
    let receipt = rig
        .preparation_record("state", "receipt", "durable-placement")
        .read();
    assert_eq!(result["path"], receipt["prepared"]["path"]);
    assert_eq!(result["pane_id"], receipt["prepared"]["pane_id"]);
    assert_eq!(
        rig.preparation_record("state", "disposition", "durable-placement")
            .read()["progress"],
        "fenced"
    );
    assert_eq!(rig.preparations()[0]["request_id"], "durable-placement");
}

#[test]
fn placement_crash_boundaries_replay_one_result() {
    for boundary in [
        "placement-host-selected",
        "placement-preparation-received",
        "placement-preparation-recovered",
        "placement-prepared",
        "placement-result-saved",
        "placement-acknowledgement-saved",
        "placement-completed",
        "preparation-fenced",
        "preparation-claim-released",
    ] {
        let (rig, repo) = place_rig(boundary);
        let request = json!({"id":"queued-placement", "validity":request_authority(&rig,"queued-placement"), "action":{"type":"place"}, "host":"local",
        "branch":"recover-placement", "base":"main", "cwd":repo, "owner":"w1:p1","owner_host":"local",
        "repository":{"path":repo,"working_root":repo,"relative":"repo space",
            "origin":"https://example.invalid/repo.git"}});
        rig.put_tickets(&json!([{"id":"placement-ticket", "request":request,
        "created_ms":rig.origin_ms, "state":"pending", "waiting":false, "notified":false,
        "retry":{"attempts":0,"next_ms":rig.origin_ms}}]));
        rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
        let out = rig
            .cmd(&["serve", "--once"])
            .env(
                "DOGSLED_TEST_UNIT_CRASH",
                if boundary == "placement-preparation-recovered" {
                    "placement-preparation-received"
                } else {
                    boundary
                },
            )
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(86), "{out:?}");
        if boundary == "placement-preparation-recovered" {
            let out = rig
                .cmd(&["serve", "--once"])
                .env("DOGSLED_TEST_UNIT_CRASH", boundary)
                .output()
                .unwrap();
            assert_eq!(out.status.code(), Some(86), "{out:?}");
        }
        assert_eq!(rig.tickets()[0]["state"], "admitting");
        let receipt = if boundary == "placement-host-selected" {
            assert!(
                !rig.preparation_record("state", "receipt", "queued-placement")
                    .exists()
            );
            assert!(!rig.dir.join("setup.log").exists());
            None
        } else {
            let receipt = rig
                .preparation_record("state", "receipt", "queued-placement")
                .read();
            assert!(!receipt["prepared"].is_null());
            assert_eq!(rig.log("setup.log").lines().count(), 1);
            Some(receipt)
        };
        let grant = rig
            .preparation_record("state", "grant", "queued-placement")
            .read();
        assert!(rig.run(&["serve", "--once"]).status.success());
        let ticket = rig.tickets()[0].clone();
        assert_eq!(ticket["state"], "admitted", "{ticket}");
        let final_attempt = rig.admission("queued-placement");
        assert_eq!(final_attempt["placement"]["phase"], "completed");
        assert_eq!(
            ticket["result"],
            final_attempt["placement"]["saved"]["result"]
        );
        assert_eq!(
            rig.json(&["place", "--resume", "queued-placement"]),
            (0, ticket["result"].clone())
        );
        if let Some(mut receipt) = receipt {
            let mut completed = rig
                .preparation_record("state", "receipt", "queued-placement")
                .read();
            assert!(completed["finished_ms"].is_u64());
            completed.as_object_mut().unwrap().remove("finished_ms");
            receipt.as_object_mut().unwrap().remove("finished_ms");
            assert_eq!(completed, receipt);
        }
        assert_eq!(
            rig.preparation_record("state", "grant", "queued-placement")
                .read(),
            grant
        );
        assert_eq!(
            rig.preparation_record("state", "disposition", "queued-placement")
                .read()["progress"],
            "released"
        );
        assert_eq!(rig.log("setup.log").lines().count(), 1);
        assert_eq!(rig.log("herdr.log").matches("worktree open").count(), 1);
    }
}

#[test]
fn acknowledged_placement_rejects_mutated_grant_or_hold_identity() {
    for changed in ["grant", "hold", "both"] {
        let (rig, id) = acknowledged_remote_placement(changed);
        let grant_path = rig.preparation_record("peer-state", "grant", &id);
        let mut grant = grant_path.read();
        let mut holds = rig.peer_state(&["reservations"]);
        let admission = rig.admission(&id);
        if changed != "hold" {
            grant["reservation"]["created_ms"] = json!(1);
            grant_path.put(&grant);
        }
        if changed != "grant" {
            holds[0]["created_ms"] = json!(1);
            rig.put_peer_state(&["reservations", "--put"], &holds);
        }
        let next = rig.tickets()[0]["retry"]["next_ms"].as_u64().unwrap();
        fs::write(rig.dir.join("clock"), next.to_string()).unwrap();
        assert!(rig.run(&["serve", "--once"]).status.success());
        let ticket = rig.tickets()[0].clone();
        assert_eq!(ticket["state"], "failed", "{changed}: {ticket}");
        assert_eq!(rig.admission(&id), admission);
        assert_eq!(grant_path.read(), grant);
        assert_eq!(rig.peer_state(&["reservations"]), holds);
        let log = rig.log("ssh.log");
        assert_eq!(log.matches("host checkout").count(), 1, "{log}");
        assert!(!log.contains("host prepare "), "{log}");
        assert!(!log.contains("host launch"), "{log}");
    }
}

#[test]
fn concurrent_reservation_replays_publish_one_hold() {
    use std::io::Write as _;
    let rig = Rig::short("placement-concurrent");
    rig.snapshot("state", rig.origin_ms, 0.0, 8);
    let envelope = placement_envelope(&rig, "same-grant").to_string();
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(rig.dir.join("state/host.lock"))
        .unwrap();
    lock.lock().unwrap();
    let ready: Vec<_> = (0..2)
        .map(|i| Fifo::new(&rig.dir, &format!("delivery-{i}.ready")))
        .collect();
    let go: Vec<_> = (0..2)
        .map(|i| Fifo::new(&rig.dir, &format!("delivery-{i}.go")))
        .collect();
    let _ready_holds: Vec<_> = ready.iter().map(Fifo::hold).collect();
    let mut go_holds: Vec<_> = go.iter().map(Fifo::hold).collect();
    let mut children: Vec<_> = (0..2)
        .map(|i| {
            let mut cmd = rig.cmd(&[
                "host",
                "reserve",
                "--id",
                "same-grant",
                "--placement-replay",
                &envelope,
            ]);
            let phase = if i == 0 {
                "PLACEMENT_ENTERED_GATE"
            } else {
                "PLACEMENT_BEFORE_GATE"
            };
            cmd.env(
                format!("DOGSLED_TEST_LAUNCH_{phase}"),
                rig.dir.join(format!("delivery-{i}")),
            )
            .stdout(fs::File::create(rig.dir.join(format!("delivery-{i}.json"))).unwrap());
            Owned::spawn(&mut cmd)
        })
        .collect();
    read_fifo(ready[0].path(), "first reservation owns preparation gate");
    read_fifo(ready[1].path(), "second reservation reached host ingress");
    let prep = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(rig.dir.join("state/preparation-receipts/same-grant.lock"))
        .unwrap();
    assert!(matches!(prep.try_lock(), Err(fs::TryLockError::WouldBlock)));
    assert!(
        !rig.preparation_record("state", "grant", "same-grant")
            .exists()
    );
    for release in &mut go_holds {
        writeln!(release, "go").unwrap();
    }
    lock.unlock().unwrap();
    let replies: Vec<Value> = children
        .iter_mut()
        .enumerate()
        .map(|(i, child)| {
            assert!(child.wait().success());
            rig.read(&format!("delivery-{i}.json"))
        })
        .collect();
    assert_eq!(replies[0], replies[1]);
    assert_eq!(replies[0]["placement"]["grant"]["materialized"], true);
    assert_eq!(rig.reservations().as_array().unwrap().len(), 1);
}

#[test]
fn reservation_replay_rejects_changed_identity_and_old_peer() {
    let rig = Rig::short("placement-conflicts");
    rig.snapshot("state", rig.origin_ms, 0.0, 8);
    let original = placement_envelope(&rig, "same-grant");
    assert_eq!(reserve_delivery(&rig, &original).0, 0);
    let hold = rig.reservations();
    let grant = rig
        .preparation_record("state", "grant", "same-grant")
        .read();
    for change in ["request", "target", "version", "missing"] {
        let mut changed = original.clone();
        match change {
            "request" => changed["request"]["cwd"] = json!("/different"),
            "target" => changed["host"]["target"] = json!("other-target"),
            "version" => changed["version"] = json!(4),
            "missing" => {
                changed.as_object_mut().unwrap().remove("request");
            }
            _ => unreachable!(),
        }
        assert_eq!(reserve_delivery(&rig, &changed).0, 2, "{change}");
        assert_eq!(rig.reservations(), hold);
        assert_eq!(
            rig.preparation_record("state", "grant", "same-grant")
                .read(),
            grant
        );
    }
    let peer = Rig::short("placement-old-peer");
    peer.peer();
    let mut snapshot = peer.read("remote.json");
    snapshot.as_object_mut().unwrap().remove("placement_replay");
    peer.put("remote.json", &snapshot);
    let _peer_observer = peer_observer(&peer);
    let (code, error) = peer.json(&["place", "--host", "netcup"]);
    assert_eq!(code, 2, "{error}");
    assert!(!peer.log("ssh.log").contains("host reserve"));
    assert!(!peer.log("ssh.log").contains("host prepare"));
    // A peer advertising support still needs to return the original correlation.
    let unsupported = Rig::short("placement-missing-reply");
    unsupported.peer();
    let script = fs::read_to_string(unsupported.dir.join("bin/ssh")).unwrap();
    unsupported.script("ssh", &script.replace("  *'host reserve'*)\n    for remote do :; done\n    sh -c \"$RIG/bin/peer-fixture $remote\" ;;", "  *'host reserve'*) cat \"$RIG/reserve.json\" ;;"));
    let _unsupported_observer = peer_observer(&unsupported);
    let (code, error) = unsupported.json(&["place", "--host", "netcup"]);
    assert_eq!(code, 2, "{error}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("placement replay evidence")
    );
    assert!(!unsupported.log("ssh.log").contains("host checkout"));
    assert!(!unsupported.log("ssh.log").contains("host prepare"));
    assert!(!rig.log("herdr.log").contains("worktree open"));
}

#[test]
fn retired_placement_cannot_repair_capacity_through_delayed_delivery() {
    let (rig, repo) = place_rig("placement-retired");
    let observer = placement_observer(&rig);
    let (code, placed) = place(&rig, &repo, "retired", None);
    assert_eq!(code, 0, "{placed}");
    drop(observer);
    let id = placed["reservation"].as_str().unwrap();
    let grant_path = rig.preparation_record("state", "grant", id);
    let grant = grant_path.read();
    let envelope = grant["envelope"].clone();
    let before = rig.reservations();
    let log = rig.log("herdr.log");
    let (code, error) = reserve_delivery(&rig, &envelope);
    assert_eq!(code, 2, "{error}");
    assert_eq!(grant_path.read(), grant);
    assert_eq!(rig.reservations(), before);
    assert_eq!(rig.log("herdr.log"), log);
    // Neither omitting the placement envelope nor losing publication progress can
    // downgrade the request into the old TTL admission path.
    assert_eq!(rig.json(&["host", "reserve", "--id", id]).0, 2);
    let mut interrupted = grant.clone();
    interrupted["materialized"] = json!(false);
    grant_path.put(&interrupted);
    rig.put_reservations(&json!([]));
    assert_eq!(reserve_delivery(&rig, &envelope).0, 2);
    assert_eq!(grant_path.read(), interrupted);
    assert_eq!(rig.reservations(), json!([]));
}

#[test]
fn reservation_publication_crashes_recover_the_original_timestamp() {
    for boundary in ["placement-grant-intent", "placement-hold-published"] {
        let rig = Rig::short(boundary);
        fs::write(rig.dir.join("clock"), "1000").unwrap();
        rig.snapshot("state", 1000, 0.0, 8);
        let envelope = placement_envelope(&rig, "crashed-grant");
        let output = rig
            .cmd(&[
                "host",
                "reserve",
                "--id",
                "crashed-grant",
                "--placement-replay",
                &envelope.to_string(),
            ])
            .env("DOGSLED_TEST_UNIT_CRASH", boundary)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(86),
            "{boundary}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let intent = rig
            .preparation_record("state", "grant", "crashed-grant")
            .read();
        assert_eq!(intent["reservation"]["created_ms"], 1000);
        assert_eq!(intent["materialized"], false);
        fs::write(rig.dir.join("clock"), "2000").unwrap();
        rig.snapshot("state", 2000, 1000.0, 8);
        assert_eq!(reserve_delivery(&rig, &envelope).0, 0);
        assert_eq!(
            rig.reservations(),
            json!([{"id":"crashed-grant","created_ms":1000}])
        );
        assert_eq!(
            rig.preparation_record("state", "grant", "crashed-grant")
                .read()["materialized"],
            true
        );
    }
}

/// The repository's setup: it logs where it ran and the branch, and its second command fails
/// while `$RIG/setup-fails` exists.
const SETUP: &str = r#"[worktree]
setup = [
  "printf '%s %s\\n' \"$(pwd -P)\" {branch} >> \"$RIG/setup.log\"",
  "if test -f \"$RIG/setup-fails\"; then printf 'setup failed: no network\\n' >&2; exit 1; fi",
]
"#;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = isolated("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

pub(super) fn peer_observer(rig: &Rig) -> Background {
    let sampled = rig.read("remote.json")["sampled_ms"].clone();
    let observer = rig.serve();
    wait_publication(rig, |frame| {
        frame["views"]["hosts"]["data"]
            .as_array()
            .is_some_and(|hosts| {
                hosts.iter().any(|host| {
                    host["id"] == "8103"
                        && host["target"] == "netcup-dev"
                        && host["sampled_ms"] == sampled
                })
            })
    });
    observer
}

/// Stop an observer between admission attempts. The service marks a ticket `admitting` only
/// while it holds that request's admission lock, so a kill while the test holds every queued
/// request's lock cannot leave a ticket admitting. A loaded machine otherwise lands the kill
/// inside an attempt, and the next pass fails the ticket as interrupted.
pub(super) fn stop_observer(rig: &Rig, observer: Background) {
    let dir = rig.dir.join("state/admissions");
    fs::create_dir_all(&dir).unwrap();
    let locks: Vec<fs::File> = rig
        .tickets()
        .as_array()
        .unwrap()
        .iter()
        .map(|ticket| {
            let id = ticket["request"]["id"].as_str().unwrap();
            let lock = fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(dir.join(format!("{id}.lock")))
                .unwrap();
            lock.lock().unwrap();
            lock
        })
        .collect();
    drop(observer);
    drop(locks);
}

/// Crash/replay fixtures neutralize only the host's real CPU load, not other capacity checks.
pub(super) fn placement_observer(rig: &Rig) -> Background {
    placement_observer_every(rig, "1")
}

/// An observer whose service lanes run at startup and then once per `interval` seconds.
pub(super) fn placement_observer_every(rig: &Rig, interval: &str) -> Background {
    let config = rig.dir.join("config.toml");
    let mut policy: toml::Table = if config.exists() {
        toml::from_str(&fs::read_to_string(config).unwrap()).unwrap()
    } else {
        toml::Table::new()
    };
    policy.insert("free_core_fraction_min".into(), toml::Value::Integer(0));
    rig.config(&toml::to_string(&policy).unwrap());
    let disk = rig.host()["probe"]["disk_free_percent"].as_f64().unwrap();
    rig.script("df", &format!("#!/bin/sh\nprintf 'Filesystem 1024-blocks Used Available Capacity Mounted on\nfixture 10000 0 {} 0%% /\n'\n", disk * 100.0));
    let observer = rig.serve_at(&rig.dir.join("state"), interval);
    wait_publication(rig, |frame| {
        let host = &frame["views"]["hosts"]["data"][0];
        host["id"] == "local"
            && host["service"] == "up"
            && host["job_validity"] == 2
            && frame["local"]["host"]["request_validity"] == 1
            && frame["local"]["host"]["placement_replay"] == 3
            && host["headroom"]["eligible"] == true
            && host["sampled_ms"]
                .as_u64()
                .is_some_and(|sampled| sampled >= rig.origin_ms)
    });
    observer
}

/// A real repository with a `dogsled.toml` setup, and a fake Herdr whose `worktree open`
/// opens the main checkout's workspace w7 as the anchor on first use.
pub(super) fn place_rig(name: &str) -> (Rig, PathBuf) {
    place_rig_with(name, SETUP)
}

pub(super) fn place_rig_with(name: &str, config: &str) -> (Rig, PathBuf) {
    let rig = Rig::short(name);
    let repo = rig.dir.join("home/repo space");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/repo.git",
        ],
    );
    fs::write(repo.join("dogsled.toml"), config).unwrap();
    git(&repo, &["add", "dogsled.toml"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    let at = now();
    fs::write(rig.dir.join("clock"), at.to_string()).unwrap();
    rig.snapshot("state", at, 0.0, 8);
    rig.script(
        "herdr",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/herdr.log"
case "$*" in
  'machine list --json') cat "$RIG/machines.json" ;;
  'workspace list') printf '%s\n' '{"result":{"workspaces":[{"workspace_id":"w1","label":"caller"}]}}' ;;
  'agent list') cat "${RIG_AGENTS:-$RIG/agents.json}" ;;
  'pane list') printf '%s\n' '{"result":{"panes":[{"pane_id":"w8:p1","terminal_id":"term_tree"}]}}' ;;
  'pane process-info'*) printf '%s\n' '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":10,"name":"zsh","argv":["zsh"]}]}}}' ;;
  'agent start'*)
    if test -f "$RIG/start-fails"; then echo 'original startup failed' >&2; exit 1; fi
    printf '{}\n' ;;
  'worktree list --cwd'*)
    if test -f "$RIG/anchor"; then main='"w7"'; else main=null; fi
    printf '{"result":{"worktrees":[{"path":"%s","open_workspace_id":%s,"is_linked_worktree":false}]}}\n' "$4" "$main" ;;
  'worktree open'*)
    while test "$#" -gt 0; do if test "$1" = --path; then path="$2"; fi; shift; done
    touch "$RIG/anchor"
    if test -f "$RIG/already-open"; then open=true; else open=false; fi
    printf '{"result":{"already_open":%s,"workspace":{"workspace_id":"w8"},"root_pane":{"pane_id":"w8:p1","terminal_id":"term_tree"},"worktree":{"path":"%s"}}}\n' "$open" "$path" ;;
  *) echo unexpected >&2; exit 2 ;;
esac
"#,
    );
    (rig, repo.canonicalize().unwrap())
}

/// Each call starts with no reservations, so admission never decides a test's outcome.
fn place(rig: &Rig, repo: &Path, branch: &str, base: Option<&str>) -> (i32, Value) {
    rig.put_reservations(&json!([]));
    wait_publication(rig, |frame| {
        let host = &frame["views"]["hosts"]["data"][0];
        host["probe"]["reservations"] == json!([]) && host["headroom"]["eligible"] == true
    });
    let mut args = vec![
        "place",
        "--branch",
        branch,
        "--repo",
        repo.to_str().unwrap(),
    ];
    if let Some(base) = base {
        args.extend(["--base", base]);
    }
    rig.json(&args)
}

fn locks(repo: &Path) -> String {
    git(repo, &["worktree", "list", "--porcelain"])
}

#[test]
fn place_creates_a_locked_worktree_runs_setup_opens_it_and_records_the_anchor() {
    let (rig, repo) = place_rig("place-create");
    let _observer = placement_observer(&rig);
    // The caller works in another linked worktree's subdirectory; placement uses the main checkout.
    git(
        &repo,
        &["worktree", "add", "-q", "-b", "caller", "../caller"],
    );
    let caller = rig.dir.join("home/caller/subdir");
    fs::create_dir_all(&caller).unwrap();
    let (code, placed) = place(&rig, &caller, "feat/a;literal", None);
    assert_eq!(code, 0, "{placed}");
    let tree = repo.join(".worktrees/feat-a;literal");
    assert_eq!(placed["path"], json!(tree));
    assert_eq!(placed["pane_id"], "w8:p1");
    assert_eq!(
        rig.log("setup.log"),
        format!("{} feat/a;literal\n", tree.display())
    );
    assert!(
        locks(&repo).contains(&format!(
            "worktree {}\nHEAD {}branch refs/heads/feat/a;literal\nlocked agent:worktree\n",
            tree.display(),
            git(&repo, &["rev-parse", "main"])
        )),
        "{}",
        locks(&repo)
    );
    assert!(rig.log("herdr.log").contains(&format!(
        "worktree open --cwd {} --path {} --no-focus",
        repo.display(),
        tree.display()
    )));
    assert_eq!(
        fs::read_to_string(repo.join(".git/agent-worktree-anchor")).unwrap(),
        "w7\n"
    );
    // The worktree is registered for doctor, and a reservation holds the capacity.
    assert_eq!(rig.state(&["worktrees"])[0]["path"], json!(tree));
    assert_eq!(rig.reservations().as_array().unwrap().len(), 1);
    // A second child does not replace the anchor: the workspace was open before it.
    fs::write(repo.join(".git/agent-worktree-anchor"), "kept\n").unwrap();
    assert_eq!(place(&rig, &repo, "feat/b", None).0, 0);
    assert_eq!(
        fs::read_to_string(repo.join(".git/agent-worktree-anchor")).unwrap(),
        "kept\n"
    );
}

#[test]
fn place_runs_setup_from_the_base_revision_instead_of_the_main_checkout() {
    let (rig, repo) = place_rig_with("place-base-setup", "[worktree]\nsetup = [\"false\"]\n");
    git(&repo, &["checkout", "-q", "-b", "setup-base"]);
    fs::write(
        repo.join("dogsled.toml"),
        "[worktree]\nsetup = [\"printf ready > setup-marker\"]\n",
    )
    .unwrap();
    git(&repo, &["commit", "-q", "-am", "new setup"]);
    let base = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "-q", "main"]);
    let _observer = placement_observer(&rig);

    let (code, placed) = place(&rig, &repo, "from-new-setup", Some(base.trim()));
    assert_eq!(code, 0, "{placed}");
    let tree = repo.join(".worktrees/from-new-setup");
    assert_eq!(placed["path"], json!(tree));
    assert_eq!(
        fs::read_to_string(tree.join("setup-marker")).unwrap(),
        "ready"
    );
}

#[test]
fn place_skips_setup_when_the_base_revision_has_no_repository_config() {
    let (rig, repo) = place_rig_with("place-base-no-config", "[worktree]\nsetup = [\"false\"]\n");
    git(&repo, &["checkout", "-q", "-b", "no-config-base"]);
    git(&repo, &["rm", "-q", "dogsled.toml"]);
    git(&repo, &["commit", "-q", "-m", "remove repository config"]);
    let base = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "-q", "main"]);
    let _observer = placement_observer(&rig);

    let (code, placed) = place(&rig, &repo, "from-no-config", Some(base.trim()));
    assert_eq!(code, 0, "{placed}");
    let tree = repo.join(".worktrees/from-no-config");
    assert_eq!(placed["path"], json!(tree));
    assert!(!tree.join("dogsled.toml").exists());
    assert_eq!(rig.log("setup.log"), "");
}

#[test]
fn place_bases_new_branches_reuses_existing_ones_and_refuses_foreign_work() {
    let (rig, repo) = place_rig("place-branches");
    let observer = placement_observer(&rig);
    drop(observer);
    rig.placement_capacity();
    // Branch checks reuse explicit capacity; no live lane may republish old reservations.
    let _observer = super::observation::seeded(&rig);
    let first = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "second"]);
    assert_eq!(place(&rig, &repo, "from-base", Some("HEAD~1")).0, 0);
    assert_eq!(git(&repo, &["rev-parse", "from-base"]), first);
    assert_eq!(place(&rig, &repo, "from-main", None).0, 0);
    assert_eq!(
        git(&repo, &["rev-parse", "from-main"]),
        git(&repo, &["rev-parse", "main"])
    );
    // An existing branch takes no base.
    let (code, error) = place(&rig, &repo, "from-base", Some("main"));
    assert_eq!(code, 2);
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("branch from-base exists; omit --base")
    );
    // An existing worktree is reused without a second setup.
    fs::write(rig.dir.join("setup.log"), "").unwrap();
    let (code, placed) = place(&rig, &repo, "from-base", None);
    assert_eq!(code, 0, "{placed}");
    assert_eq!(placed["path"], json!(repo.join(".worktrees/from-base")));
    assert_eq!(rig.log("setup.log"), "");
    // An existing branch without a worktree gets one, with setup.
    git(&repo, &["branch", "kept"]);
    assert_eq!(place(&rig, &repo, "kept", None).0, 0);
    assert!(rig.log("setup.log").contains("/.worktrees/kept kept"));
    // Someone else's lock, an open workspace, the main checkout, or a bad name is refused.
    let tree = repo.join(".worktrees/kept");
    git(&repo, &["worktree", "unlock", tree.to_str().unwrap()]);
    git(
        &repo,
        &[
            "worktree",
            "lock",
            "--reason",
            "mine",
            tree.to_str().unwrap(),
        ],
    );
    let (_, error) = place(&rig, &repo, "kept", None);
    assert_eq!(error["diagnostic"]["cause"], "workspace_occupied");
    assert_eq!(error["diagnostic"]["context"]["occupancy"], "locked");
    assert_eq!(error["diagnostic"]["context"]["workspace"], json!(tree));
    assert!(!error["error"].as_str().unwrap().contains("mine"));
    fs::write(rig.dir.join("already-open"), "").unwrap();
    let (code, error) = place(&rig, &repo, "from-main", None);
    assert_eq!(code, 2);
    assert_eq!(error["diagnostic"]["cause"], "workspace_occupied");
    assert_eq!(
        error["diagnostic"]["context"]["occupancy"],
        "open_workspace"
    );
    assert_eq!(
        error["diagnostic"]["context"]["workspace"],
        json!(repo.join(".worktrees/from-main"))
    );
    let (_, error) = place(&rig, &repo, "main", None);
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("checked out in the main checkout")
    );
    let (_, error) = place(&rig, &repo, "bad..name", None);
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("not a valid branch name")
    );
}

#[test]
fn setup_failure_removes_the_new_worktree_and_branch_and_keeps_the_reservation() {
    let (rig, repo) = place_rig("place-setup-fails");
    let _observer = placement_observer(&rig);
    fs::write(rig.dir.join("setup-fails"), "").unwrap();
    let (code, error) = place(&rig, &repo, "feat/x", None);
    assert_eq!(code, 2, "{error}");
    let message = error["error"].as_str().unwrap();
    assert!(message.contains("setup command"), "{message}");
    assert_eq!(error["diagnostic"]["cause"], "setup_failed");
    assert_eq!(error["diagnostic"]["context"]["new_worktree_removed"], true);
    assert!(!message.contains("setup failed: no network"), "{message}");
    assert!(message.contains("untrusted detail withheld"), "{message}");
    assert!(
        message.contains("the new worktree was removed"),
        "{message}"
    );
    assert!(!repo.join(".worktrees/feat-x").exists());
    assert!(!locks(&repo).contains("feat-x"));
    assert_eq!(git(&repo, &["branch", "--list", "feat/x"]), "");
    assert!(!rig.log("herdr.log").contains("worktree open"));
    assert_eq!(rig.reservations().as_array().unwrap().len(), 1);
    // A branch that existed before placement stays.
    git(&repo, &["branch", "kept"]);
    assert_eq!(place(&rig, &repo, "kept", None).0, 2);
    assert!(!repo.join(".worktrees/kept").exists());
    assert_ne!(git(&repo, &["branch", "--list", "kept"]), "");
    // An invalid config in the new worktree follows the same cleanup path.
    git(&repo, &["checkout", "-q", "-b", "invalid-config-base"]);
    fs::write(repo.join("dogsled.toml"), "[worktree]\nhooks = []\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "invalid repository config"]);
    let base = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "-q", "main"]);
    let setup = rig.log("setup.log");
    let (code, error) = place(&rig, &repo, "invalid-config", Some(base.trim()));
    assert_eq!(code, 2, "{error}");
    assert_eq!(error["diagnostic"]["cause"], "setup_failed");
    assert_eq!(error["diagnostic"]["context"]["new_worktree_removed"], true);
    assert!(!repo.join(".worktrees/invalid-config").exists());
    assert!(!locks(&repo).contains("branch refs/heads/invalid-config\n"));
    assert_eq!(git(&repo, &["branch", "--list", "invalid-config"]), "");
    assert_eq!(rig.log("setup.log"), setup);
    assert!(!rig.log("herdr.log").contains("worktree open"));
    assert_eq!(rig.reservations().as_array().unwrap().len(), 1);
}

#[test]
fn repository_and_host_config_errors_are_explicit() {
    let (rig, repo) = place_rig("place-config");
    let observer = placement_observer(&rig);
    fs::write(
        repo.join("dogsled.toml"),
        "[worktree]\nsetup = [\"echo {base}\"]\n",
    )
    .unwrap();
    let (code, error) = place(&rig, &repo, "feat/x", None);
    assert_eq!(code, 2);
    let message = error["error"].as_str().unwrap();
    assert!(message.contains("dogsled.toml"), "{message}");
    assert!(message.contains("unknown placeholder {base}"), "{message}");
    fs::write(repo.join("dogsled.toml"), "[worktree]\nhooks = []\n").unwrap();
    let (_, error) = place(&rig, &repo, "feat/x", None);
    assert!(error["error"].as_str().unwrap().contains("unknown field"));
    assert!(!repo.join(".worktrees/feat-x").exists());
    // The host hooks moved to the repository.
    drop(observer);
    rig.config("[hooks]\nprepare = ['true']\n");
    rig.put_reservations(&json!([]));
    let (code, error) = rig.json(&[
        "place",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "feat/x",
    ]);
    assert_eq!(code, 2);
    assert!(error["error"].as_str().unwrap().contains("unknown field"));
}
