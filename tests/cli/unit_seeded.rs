use super::unit_requests::*;
use super::*;

fn launched(name: &str) -> (Rig, Value, Value) {
    let rig = rig(name, false);
    let execution = prepare(&rig);
    let input = commit_input(&execution, 0);
    assert_eq!(operation(&rig, "commit", &input).0, 0);
    (rig, execution, input)
}

fn transcript(rig: &Rig, execution: &Value, entries: &[Value]) -> PathBuf {
    let id = execution["requestId"].as_str().unwrap();
    let path = rig
        .dir
        .join(format!("state/launches/{id}.sessions/test_{id}.jsonl"));
    let progress = rig.read(&format!("state/launches/{id}.progress.json"));
    let mut lines = vec![
        json!({"type":"session","version":3,"id":id,"cwd":progress["request"]["cwd"]}).to_string(),
    ];
    lines.extend(entries.iter().map(Value::to_string));
    fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{
            "pane_id":"w2:p1","terminal_id":"term_child","agent":"pi","name":expected_name(execution, &exported(0)["contract"]["ref"]),
            "agent_status":"idle","completion_seq":7,
            "agent_session":{"agent":"pi","kind":"path","value":path}
        }]}}),
    );
    path
}

fn entries() -> Vec<Value> {
    vec![
        json!({"type":"message","id":"old","parentId":null,"message":{"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"OLD"}]}}),
        json!({"type":"custom","id":"seal","parentId":"old","customType":"opaque-provider-state"}),
        json!({"type":"message","id":"reply","parentId":"seal","message":{"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"EXACT FINAL"}]}}),
        json!({"type":"custom","id":"leaf","parentId":"reply","customType":"boundary-metadata"}),
        // Chronologically newest is not the acknowledged branch.
        json!({"type":"message","id":"other","parentId":"old","message":{"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"OTHER BRANCH"}]}}),
    ]
}

fn acknowledgement(execution: &Value, input: &Value) -> Value {
    json!({"execution":execution,"unitRef":input["binding"]["contract"],"digest":input["exported"]["digest"],"sealEntryId":"seal","sessionLeafId":"leaf"})
}

fn ack(rig: &Rig, input: &Value) -> (i32, Value) {
    let out = rig
        .cmd(&["unit", "terminal-ack", "--input", &input.to_string()])
        .env("HERDR_PANE_ID", "w2:p1")
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        serde_json::from_slice(&out.stdout).unwrap(),
    )
}

fn refused_unit(name: &str) -> (Rig, Value, Value) {
    let rig = rig(name, false);
    rig.script("herdr", &fs::read_to_string(rig.dir.join("bin/herdr")).unwrap().replace(
        "  'agent start'*|",
        "  'agent start'*) echo '{\"error\":{\"code\":\"invalid_agent_name\",\"message\":\"native refusal\"}}' >&2; exit 1 ;;\n  ",
    ));
    let execution = prepare(&rig);
    let input = commit_input(&execution, 0);
    assert_eq!(operation(&rig, "commit", &input).0, 2);
    (rig, execution, input)
}

#[test]
fn done_handoff_waits_for_host_terminal_evidence_not_owner_acknowledgement() {
    let (rig, execution, input) = launched("seeded-done-handoff");
    transcript(&rig, &execution, &entries());
    rig.done("term_child", "w2:p1", 7);
    assert!(rig.run(&["serve", "--once"]).status.success());
    assert!(!rig.log("herdr.log").contains("pane close"));
    assert_eq!(ack(&rig, &acknowledgement(&execution, &input)).0, 0);
    let out = rig.run(&["serve", "--once"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(rig.log("herdr.log").contains("pane close w2:p1"));
    let marks = rig.read("state/done.json");
    let mark = marks.as_object().unwrap().values().next().unwrap();
    assert!(mark["closed_ms"].is_u64());
    assert_eq!(mark["reply"]["text"], "EXACT FINAL");
}

#[test]
fn committed_native_refusal_settles_once_without_start_or_acceptance() {
    let (rig, execution, _) = refused_unit("refused-unit-settlement");
    let id = execution["requestId"].as_str().unwrap();
    let path = format!("state/launches/{id}.progress.json");
    let mut progress = rig.read(&path);
    // Existing old-style committed names are evidence, never recomputed or migrated.
    progress["request"]["action"]["name"] =
        json!("unit-u-6ce8048d-2317-4b7e-988e-cf6591d1b7a0-1-r1");
    rig.put(&path, &progress);
    let mut attempt = admission(&rig, &execution);
    attempt["launch"]["action"]["name"] = progress["request"]["action"]["name"].clone();
    rig.put(&format!("state/admissions/{id}.json"), &attempt);
    let (code, events) = rig.json(&["unit", "settlements"]);
    assert_eq!(code, 0, "{events}");
    assert_eq!(events.as_array().unwrap().len(), 1, "{events}");
    assert_eq!(events[0]["result"]["kind"], "not_started");
    assert_eq!(events[0]["execution"], execution);
    assert_eq!(events[0]["origin"], attempt["unit"]["origin"]);
    assert_eq!(rig.json(&["unit", "settlements"]).1, events);
    let ack = json!({"execution":execution,"eventId":events[0]["eventId"]});
    assert_eq!(operation(&rig, "ack-settlement", &ack).0, 0);
    assert_eq!(operation(&rig, "ack-settlement", &ack).0, 0);
    assert_eq!(rig.json(&["unit", "settlements"]).1, json!([]));
    let (_, observed) = operation(&rig, "observe", &execution_input(&execution));
    assert_eq!(observed["phase"], "not_started");
    assert_eq!(rig.log("herdr.log").matches("agent start ").count(), 1);
    assert!(!rig.dir.join("state/items.json").exists());
}

#[test]
fn refusal_requires_final_consistent_host_provenance() {
    for case in [
        "lost",
        "unattempted",
        "confirmed",
        "terminal",
        "seed",
        "execution",
        "error",
        "host",
        "commit",
        "returned",
    ] {
        let (rig, execution, _) = refused_unit(&format!("refused-invalid-{case}"));
        let id = execution["requestId"].as_str().unwrap();
        let file = format!("state/launches/{id}.progress.json");
        let mut progress = rig.read(&file);
        let mut attempt = admission(&rig, &execution);
        match case {
            "lost" => fs::remove_file(rig.dir.join(format!("state/launches/{id}.json"))).unwrap(),
            "unattempted" => progress["attempted"] = json!(false),
            "confirmed" => progress["confirmed"] = json!(true),
            "terminal" => {
                progress["terminal_receipt"] = json!({"ack":acknowledgement(&execution, &commit_input(&execution, 0)),"sessionId":id,"replyEntryId":"reply","text":"contradiction"});
            }
            "seed" => {
                let mut seed = rig.read(&format!("state/launches/{id}.unit.json"));
                seed["exported"]["brief"] = json!("changed");
                rig.put(&format!("state/launches/{id}.unit.json"), &seed);
            }
            "execution" => progress["request"]["unit"]["execution"]["ownerId"] = json!("another"),
            "error" => progress["error"] = json!("transport failed"),
            "host" => attempt["host"]["id"] = json!("other-host"),
            "commit" => attempt["unit"]["committed"]["exported"]["brief"] = json!("changed"),
            "returned" => {
                attempt["unit"]["returned"] = json!({"version":1,"eventId":"returned","binding":commit_input(&execution,0)["binding"],"result":{"kind":"returned","evidence":{"execution":execution,"resultId":"reply","summary":"done"}}});
            }
            _ => unreachable!(),
        }
        rig.put(&file, &progress);
        rig.put(&format!("state/admissions/{id}.json"), &attempt);
        let _ = rig.json(&["unit", "settlements"]);
        assert!(
            admission(&rig, &execution)["unit"]["abandoned"].is_null(),
            "{case}"
        );
        assert_eq!(rig.log("herdr.log").matches("agent start ").count(), 1);
    }
}

#[test]
fn refusal_release_and_settlement_recover_after_each_durable_boundary() {
    for point in ["refusal-hold-released", "refusal-settlement-persisted"] {
        let (rig, execution, _) = refused_unit(point);
        let id = execution["requestId"].as_str().unwrap();
        let cwd = admission(&rig, &execution)["launch"]["cwd"].clone();
        let own = json!({"repo":rig.dir.join("home"),"path":cwd,"branch":"proof","head":"","request_id":id});
        let legacy = json!({"repo":rig.dir.join("home"),"path":cwd,"branch":"proof","head":""});
        let mut other = own.clone();
        other["request_id"] = json!("later-request");
        rig.put("state/preparations.json", &json!([own, legacy, other]));
        let result = rig
            .cmd(&["unit", "settlements"])
            .env("DOGSLED_TEST_UNIT_CRASH", point)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(86));
        assert_eq!(rig.read("state/preparations.json"), json!([legacy, other]));
        let (code, events) = rig.json(&["unit", "settlements"]);
        assert_eq!(code, 0, "{events}");
        assert_eq!(events.as_array().unwrap().len(), 1);
        assert_eq!(events[0]["result"]["kind"], "not_started");
        assert_eq!(rig.read("state/preparations.json"), json!([legacy, other]));
        assert_eq!(rig.log("herdr.log").matches("agent start ").count(), 1);
    }
}

#[test]
fn remote_refusal_correlates_full_launch_and_rejects_contradictions() {
    let (rig, execution, input) = refused_unit("refused-remote-correlation");
    let id = execution["requestId"].as_str().unwrap();
    let mut attempt = admission(&rig, &execution);
    attempt["host"] = json!({"id":"8103","label":"remote","target":"remote-host"});
    attempt["launch"]["host"] = json!("8103");
    rig.put(&format!("state/admissions/{id}.json"), &attempt);
    let receipt = json!({"kind":"refused","launch":attempt["launch"],"failed":"native refusal"});
    rig.script(
        "ssh",
        &fs::read_to_string(rig.dir.join("bin/ssh"))
            .unwrap()
            .replace(
                "case \"$*\" in",
                "case \"$*\" in\n  *'host unit-receipt'*) cat \"$RIG/remote-receipt.json\" ;;",
            ),
    );
    for changed in ["host", "seed", "execution"] {
        let mut wrong = receipt.clone();
        match changed {
            "host" => wrong["launch"]["host"] = json!("other"),
            "seed" => wrong["launch"]["unit"]["exported"]["brief"] = json!("different"),
            _ => wrong["launch"]["unit"]["execution"]["requestId"] = json!("different"),
        }
        rig.put("remote-receipt.json", &wrong);
        let before = admission(&rig, &execution);
        let events = rejected_refresh(
            &rig,
            &["unit", "settlements"],
            id,
            "host refusal does not match selected launch",
        );
        assert_eq!(events, json!([]), "{changed}");
        assert_eq!(admission(&rig, &execution), before);
    }
    rig.put("remote-receipt.json", &receipt);
    let (code, events) = rig.json(&["unit", "settlements"]);
    assert_eq!(code, 0, "{events}");
    assert_eq!(events[0]["result"]["kind"], "not_started");
    let mut changed = receipt;
    changed["failed"] = json!("contradictory refusal");
    rig.put("remote-receipt.json", &changed);
    let saved = admission(&rig, &execution);
    assert_eq!(
        rejected_refresh(
            &rig,
            &["unit", "settlements"],
            id,
            "contradictory refusal evidence"
        ),
        events
    );
    assert_eq!(admission(&rig, &execution), saved);
    rig.put("remote-receipt.json", &json!({"ack":acknowledgement(&execution,&input),"sessionId":id,"replyEntryId":"reply","text":"impossible return"}));
    assert_eq!(
        rejected_refresh(
            &rig,
            &["unit", "settlements"],
            id,
            "host terminal evidence does not match commitment"
        ),
        events
    );
    assert_eq!(admission(&rig, &execution), saved);
}

fn rejected_refresh(rig: &Rig, args: &[&str], id: &str, reason: &str) -> Value {
    let out = rig.run(args);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "{} {stderr}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        stderr.contains(&format!("dogsled unit evidence {id}:")) && stderr.contains(reason),
        "{stderr}"
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn merge_records(target: &Rig, source: &Rig, directory: &str) {
    for entry in fs::read_dir(source.dir.join(directory)).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            fs::copy(
                entry.path(),
                target.dir.join(directory).join(entry.file_name()),
            )
            .unwrap();
        }
    }
}

#[test]
fn invalid_evidence_is_request_scoped_and_other_owners_can_receive_and_ack() {
    let (rig, local, _) = refused_unit("isolation-local");
    let (remote_rig, remote, _) = refused_unit("isolation-remote");
    let (healthy_rig, healthy, _) = refused_unit("isolation-healthy");
    let local_id = local["requestId"].as_str().unwrap();
    let remote_id = remote["requestId"].as_str().unwrap();
    let healthy_id = healthy["requestId"].as_str().unwrap();
    assert!(local_id < healthy_id && remote_id < healthy_id);
    merge_records(&rig, &remote_rig, "state/admissions");
    merge_records(&rig, &healthy_rig, "state/admissions");
    merge_records(&rig, &healthy_rig, "state/launches");
    let progress_file = format!("state/launches/{local_id}.progress.json");
    let mut progress = rig.read(&progress_file);
    progress["confirmed"] = json!(true);
    rig.put(&progress_file, &progress);
    let mut attempt = admission(&rig, &remote);
    attempt["host"] = json!({"id":"8103","label":"remote","target":"remote-host"});
    attempt["launch"]["host"] = json!("8103");
    rig.put(&format!("state/admissions/{remote_id}.json"), &attempt);
    let mut wrong = json!({"kind":"refused","launch":attempt["launch"],"failed":"native refusal"});
    wrong["launch"]["host"] = json!("wrong-host");
    rig.put("remote-receipt.json", &wrong);
    rig.script("ssh", "#!/bin/sh\ncat \"$RIG/remote-receipt.json\"\n");
    let prepared = rig
        .cmd(&[
            "unit",
            "prepare",
            "--input",
            &json!({"origin":exported(0)["contract"]["ref"],"placement":{}}).to_string(),
            "--tool-call-id",
            "other-call",
        ])
        .env("HERDR_PANE_ID", "other-owner")
        .output()
        .unwrap();
    assert!(prepared.status.success());
    let other: Value = serde_json::from_slice(&prepared.stdout).unwrap();
    let abandoned = rig
        .cmd(&[
            "unit",
            "abandon-prepared",
            "--input",
            &execution_input(&other).to_string(),
        ])
        .env("HERDR_PANE_ID", "other-owner")
        .output()
        .unwrap();
    assert!(abandoned.status.success());
    let other_event: Value = serde_json::from_slice(&abandoned.stdout).unwrap();
    let before_local = admission(&rig, &local);
    let before_remote = admission(&rig, &remote);
    for (owner, expected) in [("w1:p1", healthy.clone()), ("other-owner", other.clone())] {
        let run = |args: &[&str]| rig.cmd(args).env("HERDR_PANE_ID", owner).output().unwrap();
        let out = run(&["unit", "settlements"]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{stderr}");
        assert!(
            stderr.contains(local_id)
                && stderr.contains("host refusal lacks consistent launch provenance"),
            "{stderr}"
        );
        assert!(
            stderr.contains(remote_id)
                && stderr.contains("host refusal does not match selected launch"),
            "{stderr}"
        );
        let events: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(events.as_array().unwrap().len(), 1, "{events}");
        assert_eq!(events[0]["execution"], expected);
        assert_eq!(events[0]["result"]["kind"], "not_started");
        if owner == "other-owner" {
            assert_eq!(events[0], other_event);
        }
        let ack = json!({"execution":expected,"eventId":events[0]["eventId"]});
        let out = run(&["unit", "ack-settlement", "--input", &ack.to_string()]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let out = run(&["unit", "settlements"]);
        assert!(out.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&out.stdout).unwrap(),
            json!([])
        );
    }
    assert_eq!(admission(&rig, &local), before_local);
    assert_eq!(admission(&rig, &remote), before_remote);
    for execution in [&local, &remote] {
        let (code, observed) = operation(&rig, "observe", &execution_input(execution));
        assert_eq!(code, 0, "{observed}");
        assert_eq!(observed["phase"], "unresolved");
        assert!(observed["settlement"].is_null());
    }
}

#[test]
fn invalid_host_evidence_never_falls_back_to_cached_return_projection() {
    let (rig, execution, input) = launched("isolation-no-fallback");
    transcript(&rig, &execution, &entries());
    assert_eq!(ack(&rig, &acknowledgement(&execution, &input)).0, 0);
    assert_eq!(rig.json(&["unit", "settlements"]).0, 0);
    let id = execution["requestId"].as_str().unwrap();
    let saved = admission(&rig, &execution);
    let mut items = rig.read("state/items.json");
    items[0]["reply"] = Value::Null;
    rig.put("state/items.json", &items);
    let progress_file = format!("state/launches/{id}.progress.json");
    let mut progress = rig.read(&progress_file);
    progress["request"]["unit"]["execution"]["ownerId"] = json!("wrong-owner");
    rig.put(&progress_file, &progress);
    let events = rejected_refresh(&rig, &["unit", "settlements"], id, "REQUEST_CONFLICT");
    assert_eq!(events, json!([saved["unit"]["returned"]]));
    assert_eq!(rig.read("state/items.json"), items);
    assert_eq!(admission(&rig, &execution), saved);
    let mut remote = saved;
    remote["host"] = json!({"id":"8103","label":"remote","target":"remote-host"});
    rig.put(&format!("state/admissions/{id}.json"), &remote);
    rig.script("ssh", "#!/bin/sh\nprintf 'malformed evidence'\n");
    rejected_refresh(&rig, &["unit", "settlements"], id, "unexpected output");
    assert_eq!(rig.read("state/items.json"), items);
    assert_eq!(admission(&rig, &execution), remote);
}

#[test]
fn refresh_storage_read_and_write_failures_are_not_request_rejections() {
    for failure in ["host-read", "preparation-read", "admission-write"] {
        let (rig, execution, _) = refused_unit(&format!("isolation-storage-{failure}"));
        let id = execution["requestId"].as_str().unwrap();
        let saved = admission(&rig, &execution);
        match failure {
            "host-read" => {
                let path = rig.dir.join(format!("state/launches/{id}.progress.json"));
                fs::remove_file(&path).unwrap();
                fs::create_dir(path).unwrap();
            }
            "preparation-read" => fs::create_dir(rig.dir.join("state/preparations.json")).unwrap(),
            "admission-write" => {
                let remote =
                    json!({"kind":"refused","launch":saved["launch"],"failed":"native refusal"});
                let mut attempt = saved.clone();
                attempt["host"]["target"] = json!("remote-host");
                // Host identity stays correlated; the non-local target selects SSH.
                attempt["host"]["id"] = json!("8103");
                attempt["launch"]["host"] = json!("8103");
                let mut remote = remote;
                remote["launch"] = attempt["launch"].clone();
                rig.put(&format!("state/admissions/{id}.json"), &attempt);
                rig.put("remote-receipt.json", &remote);
                rig.script("ssh", &format!("#!/bin/sh\ncat \"$RIG/remote-receipt.json\"\nmkdir -p \"$RIG/state/admissions/.{id}.json.r-1-$PPID-0.tmp\"\n"));
                fs::write(rig.dir.join("clock"), "1").unwrap();
            }
            _ => unreachable!(),
        }
        let out = rig.run(&["unit", "settlements"]);
        assert!(
            !out.status.success(),
            "{failure}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(!String::from_utf8_lossy(&out.stderr).contains("dogsled unit evidence"));
        assert!(
            admission(&rig, &execution)["unit"]["abandoned"].is_null(),
            "{failure}"
        );
    }
}

#[test]
fn refused_host_cannot_acknowledge_return_and_cleanup_query_never_closes_panes() {
    let (rig, execution, input) = refused_unit("refused-host-ack");
    let before = rig.log("herdr.log").matches("pane close").count();
    let (code, receipt) = rig.json(&[
        "host",
        "unit-receipt",
        "--input",
        &execution_input(&execution).to_string(),
    ]);
    assert_eq!(code, 0, "{receipt}");
    assert_eq!(receipt["kind"], "refused");
    assert_eq!(rig.log("herdr.log").matches("pane close").count(), before);
    assert_eq!(ack(&rig, &acknowledgement(&execution, &input)).0, 2);
}

#[test]
fn preparation_owners_coexist_and_release_only_their_records() {
    let (rig, repo) = place::place_rig("preparation-owners");
    for owner in ["first", "first", "later"] {
        let (code, result) = rig.json(&[
            "host",
            "prepare",
            "--repo",
            repo.to_str().unwrap(),
            "--branch",
            "owned",
            "--request-id",
            owner,
        ]);
        assert_eq!(code, 0, "{result}");
    }
    let path = repo.join(".worktrees/owned");
    let mut holds = rig.read("state/preparations.json");
    assert_eq!(holds.as_array().unwrap().len(), 2);
    let mut legacy = holds[0].clone();
    legacy.as_object_mut().unwrap().remove("request_id");
    holds.as_array_mut().unwrap().push(legacy.clone());
    rig.put("state/preparations.json", &holds);
    assert_eq!(
        rig.json(&[
            "host",
            "prepared",
            "--path",
            path.to_str().unwrap(),
            "--request-id",
            "first"
        ])
        .0,
        0
    );
    let remaining = rig.read("state/preparations.json");
    assert_eq!(remaining.as_array().unwrap().len(), 2);
    assert_eq!(remaining[0]["request_id"], "later");
    assert_eq!(remaining[1], legacy);
    assert_eq!(
        rig.json(&["host", "prepared", "--path", path.to_str().unwrap()])
            .0,
        0
    );
    assert_eq!(rig.read("state/preparations.json"), json!([remaining[0]]));
    let before = rig.log("herdr.log");
    assert_eq!(
        rig.json(&[
            "host",
            "prepare",
            "--repo",
            repo.to_str().unwrap(),
            "--branch",
            "invalid",
            "--request-id",
            "../escape"
        ])
        .0,
        2
    );
    assert_eq!(rig.log("herdr.log"), before);
    assert_eq!(rig.read("state/preparations.json"), json!([remaining[0]]));
}

#[test]
fn successful_agent_place_and_failed_preparation_release_only_owned_preparation() {
    for action in ["agent", "place", "setup-failure", "already-open"] {
        let (rig, repo) = place::place_rig(&format!("preparation-release-{action}"));
        let path = repo.join(".worktrees/owned");
        let legacy = json!({"repo":repo,"path":path,"branch":"owned","head":""});
        let mut other = legacy.clone();
        other["request_id"] = json!("other-request");
        rig.put("state/preparations.json", &json!([legacy, other]));
        if action == "setup-failure" {
            fs::write(rig.dir.join("setup-fails"), "").unwrap();
        }
        // Herdr refusing the workspace ends this preparation: no launch can follow it.
        if action == "already-open" {
            fs::write(rig.dir.join("already-open"), "").unwrap();
        }
        let args = if action == "agent" {
            vec![
                "agent",
                "--cwd",
                repo.to_str().unwrap(),
                "--branch",
                "owned",
                "--base",
                "main",
                "--name",
                "worker",
                "--message",
                "go",
            ]
        } else {
            vec![
                "place",
                "--repo",
                repo.to_str().unwrap(),
                "--branch",
                "owned",
                "--base",
                "main",
            ]
        };
        let (code, result) = rig.json(&args);
        assert_eq!(
            code,
            if matches!(action, "setup-failure" | "already-open") {
                2
            } else {
                0
            },
            "{action}: {result}"
        );
        assert_eq!(
            rig.read("state/preparations.json"),
            json!([legacy, other]),
            "{action}"
        );
    }
}

#[test]
fn remote_preparation_and_placement_release_route_the_same_request_id() {
    let rig = rig("remote-preparation-owner", false);
    let repo = rig.dir.join("home/repo");
    repository(&repo);
    rig.peer();
    rig.snapshot("remote.json", now(), 0.0, 8);
    rig.script(
        "ssh",
        &fs::read_to_string(rig.dir.join("bin/ssh"))
            .unwrap()
            .replace(
                "case \"$*\" in",
                "case \"$*\" in\n  *'host prepared'*) printf '{}\\n' ;;",
            ),
    );
    let (code, result) = rig.json(&[
        "place",
        "--host",
        "netcup",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "owned",
        "--base",
        "main",
    ]);
    assert_eq!(code, 0, "{result}");
    let log = rig.log("ssh.log");
    let prepare = log
        .lines()
        .find(|line| line.contains("host prepare "))
        .unwrap();
    let released = log
        .lines()
        .find(|line| line.contains("host prepared "))
        .unwrap();
    let request_arg = prepare
        .split("--request-id")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap();
    assert!(
        released.contains(&format!("--request-id {request_arg}")),
        "{log}"
    );
}

#[test]
fn seed_artifact_and_fresh_session_flags_are_exact_and_idempotent() {
    let (rig, execution, input) = launched("seed-artifact");
    let id = execution["requestId"].as_str().unwrap();
    let mut seed = input.clone();
    seed["version"] = json!(1);
    let path = format!("state/launches/{id}.unit.json");
    assert_eq!(rig.read(&path), seed);
    assert_eq!(admission(&rig, &execution)["launch"]["unit"], seed);
    let log = rig.log("herdr.log");
    for flag in [
        "--todo-unit-file",
        "--session-dir",
        "--session-id",
        "--model alias/sol --thinking medium",
        "--append-system-prompt",
    ] {
        assert!(log.contains(flag), "{log}");
    }
    let before = fs::read(rig.dir.join(&path)).unwrap();
    assert_eq!(operation(&rig, "commit", &input).0, 0);
    assert_eq!(fs::read(rig.dir.join(path)).unwrap(), before);
    assert_eq!(rig.log("herdr.log").matches("agent start ").count(), 1);
}

#[test]
fn profile_session_flags_are_refused_without_mutation_or_worker_start() {
    for (i, flag) in [
        "--continue",
        "-c",
        "--resume",
        "-r",
        "--session=old",
        "--session-id",
        "--session-dir",
        "--fork",
        "--no-session",
        "--todo-unit-file",
        "--",
    ]
    .iter()
    .enumerate()
    {
        let rig = rig(&format!("seed-profile-{i}"), false);
        let execution = prepare(&rig);
        let file = rig.dir.join("profiles/worker/profile.json");
        let profile = json!({"pi_args":[flag],"model":"alias/sol"}).to_string();
        fs::write(&file, &profile).unwrap();
        let (code, error) = operation(&rig, "commit", &commit_input(&execution, 0));
        assert_eq!(code, 2, "{flag}: {error}");
        assert!(!rig.log("herdr.log").contains("agent start"));
        assert_eq!(fs::read_to_string(file).unwrap(), profile);
        assert!(operation(&rig, "observe", &execution_input(&execution)).1["settlement"].is_null());
    }
}

#[test]
fn terminal_receipt_pairs_seal_and_exact_reply_and_delivery_replays_until_ack() {
    let (rig, execution, input) = launched("seed-terminal");
    transcript(&rig, &execution, &entries());
    let acknowledgement = acknowledgement(&execution, &input);
    assert_eq!(operation(&rig, "terminal-ack", &acknowledgement).0, 2); // owner is not worker
    let (code, receipt) = ack(&rig, &acknowledgement);
    assert_eq!(code, 0, "{receipt}");
    assert_eq!(receipt["text"], "EXACT FINAL");
    assert_eq!(receipt["replyEntryId"], "reply");
    assert_eq!(ack(&rig, &acknowledgement), (code, receipt.clone()));
    // Lost response replay needs no transcript reread once the exact receipt is durable.
    let id = execution["requestId"].as_str().unwrap();
    fs::remove_file(
        rig.dir
            .join(format!("state/launches/{id}.sessions/test_{id}.jsonl")),
    )
    .unwrap();
    assert_eq!(ack(&rig, &acknowledgement), (code, receipt.clone()));
    let mut changed = acknowledgement.clone();
    changed["sessionLeafId"] = json!("other");
    assert_eq!(ack(&rig, &changed).0, 2);
    let (_, observed) = operation(&rig, "observe", &execution_input(&execution));
    assert_eq!(observed["phase"], "returned");
    let event = &observed["settlement"];
    assert_eq!(event["binding"], input["binding"]);
    assert_eq!(event["result"]["evidence"]["summary"], "EXACT FINAL");
    assert_eq!(
        rig.read("state/items.json")[0]["reply"]["text"],
        "EXACT FINAL"
    );
    assert_eq!(rig.json(&["unit", "settlements"]).1, json!([event]));
    // Transcript delivery and elapsed time cannot consume task delivery.
    let mut items = rig.read("state/items.json");
    items[0]["reply"]["reported_ms"] = json!(now());
    items[0]["closed_ms"] = json!(now());
    rig.put("state/items.json", &items);
    fs::write(rig.dir.join("clock"), (now() + 90 * 86_400_000).to_string()).unwrap();
    rig.put("agents.json", &json!({"result":{"agents":[]}}));
    assert_eq!(rig.json(&["unit", "settlements"]).1, json!([event]));
    let ack_input = json!({"execution":execution,"eventId":event["eventId"]});
    let mut bad = ack_input.clone();
    bad["eventId"] = json!("stale");
    assert_eq!(operation(&rig, "ack-settlement", &bad).0, 2);
    assert_eq!(
        operation(&rig, "ack-settlement", &ack_input).1,
        json!({"applied":true})
    );
    assert_eq!(
        operation(&rig, "ack-settlement", &ack_input).1,
        json!({"applied":false})
    );
    assert_eq!(rig.json(&["unit", "settlements"]).1, json!([]));
    assert_eq!(
        operation(&rig, "observe", &execution_input(&execution)).1["settlement"],
        *event
    );
}

fn tell_once(rig: &Rig) -> Value {
    let mut agents = rig.read("agents.json");
    agents["result"]["agents"].as_array_mut().unwrap().push(json!({
        "pane_id":"w1:p1","terminal_id":"term_owner","agent":"pi","agent_status":"idle","name":"lead"
    }));
    rig.put("agents.json", &agents);
    assert_eq!(rig.json(&["inbox", "claim", "--harness", "pi"]).0, 0);
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    let mut items = rig.read("state/items.json");
    items[0]["closed_ms"] = json!(now());
    rig.put("state/items.json", &items);
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let records = rig.read("state/inbox/term_owner.json");
    let replies: Vec<_> = records
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["kind"] == "reply")
        .collect();
    assert_eq!(replies.len(), 1, "{records}");
    assert!(replies[0]["text"].as_str().unwrap().contains("EXACT FINAL"));
    let saved = rig.read("state/items.json")[0]["reply"].clone();
    assert!(saved["reported_ms"].is_number());
    saved
}

fn replay_keeps_notification(rig: &Rig, execution: &Value, event: &Value, reply: &Value) {
    for _ in 0..2 {
        let (code, state) = operation(rig, "observe", &execution_input(execution));
        assert_eq!(code, 0, "{state}");
        assert_eq!(state["settlement"], *event);
        let out = rig.run(&["serve", "--once", "--interval", "1"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(rig.read("state/items.json")[0]["reply"], *reply);
        assert_eq!(
            rig.read("state/inbox/term_owner.json")
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["kind"] == "reply")
                .count(),
            1
        );
    }
}

#[test]
fn receipt_before_item_registration_restores_exact_reply_after_finish_replays() {
    let (rig, execution, input) = launched("seed-before-item");
    transcript(&rig, &execution, &entries());
    assert_eq!(ack(&rig, &acknowledgement(&execution, &input)).0, 0);
    let id = execution["requestId"].as_str().unwrap();
    // Reproduce host completion before owner finish() registers its item/result.
    rig.put("state/items.json", &json!([]));
    let mut attempt = admission(&rig, &execution);
    attempt["result"] = Value::Null;
    rig.put(&format!("state/admissions/{id}.json"), &attempt);
    let (code, state) = operation(&rig, "observe", &execution_input(&execution));
    assert_eq!(code, 0, "{state}");
    let event = state["settlement"].clone();
    assert_eq!(event["result"]["kind"], "returned");
    assert_eq!(rig.read("state/items.json"), json!([]));
    assert_eq!(operation(&rig, "commit", &input).0, 0);
    // Projection repair must not depend on the host still serving its receipt.
    let path = format!("state/launches/{id}.progress.json");
    let mut progress = rig.read(&path);
    progress["terminal_receipt"] = Value::Null;
    rig.put(&path, &progress);
    assert_eq!(
        operation(&rig, "observe", &execution_input(&execution)).0,
        0
    );
    let reply = rig.read("state/items.json")[0]["reply"].clone();
    assert_eq!(reply["text"], "EXACT FINAL");
    assert_eq!(reply["result_id"], event["result"]["evidence"]["resultId"]);
    let notified = tell_once(&rig);
    replay_keeps_notification(&rig, &execution, &event, &notified);
    assert_eq!(rig.log("herdr.log").matches("agent start ").count(), 1);
}

#[test]
fn terminal_projection_recovers_each_persistence_boundary_and_legacy_write_order() {
    for point in ["settlement-persisted", "reply-persisted"] {
        let (rig, execution, input) = launched(&format!("seed-projection-{point}"));
        transcript(&rig, &execution, &entries());
        assert_eq!(ack(&rig, &acknowledgement(&execution, &input)).0, 0);
        let out = rig
            .cmd(&[
                "unit",
                "observe",
                "--input",
                &execution_input(&execution).to_string(),
            ])
            .env("DOGSLED_TEST_UNIT_CRASH", point)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(86));
        let event = admission(&rig, &execution)["unit"]["returned"].clone();
        let has_reply = !rig.read("state/items.json")[0]["reply"].is_null();
        assert_eq!(has_reply, point == "reply-persisted");
        assert_eq!(
            operation(&rig, "observe", &execution_input(&execution)).0,
            0
        );
        let notified = tell_once(&rig);
        // The prior implementation wrote the item first; replay that interrupted order.
        let mut attempt = admission(&rig, &execution);
        attempt["unit"]["returned"] = Value::Null;
        attempt["unit"]["terminal_receipt"] = Value::Null;
        rig.put(
            &format!(
                "state/admissions/{}.json",
                execution["requestId"].as_str().unwrap()
            ),
            &attempt,
        );
        replay_keeps_notification(&rig, &execution, &event, &notified);
    }
}

#[test]
fn terminal_replay_rejects_changed_full_receipt_and_changed_correlated_item() {
    let (rig, execution, input) = launched("seed-replay-conflict");
    let mut entries = entries();
    entries[2]["message"]["content"][0]["text"] =
        json!(format!("{}original tail", "x".repeat(2048)));
    transcript(&rig, &execution, &entries);
    assert_eq!(ack(&rig, &acknowledgement(&execution, &input)).0, 0);
    assert_eq!(
        operation(&rig, "observe", &execution_input(&execution)).0,
        0
    );
    let saved = admission(&rig, &execution);
    let id = execution["requestId"].as_str().unwrap();
    let path = format!("state/launches/{id}.progress.json");
    let original = rig.read(&path);
    let mut changed = original.clone();
    changed["terminal_receipt"]["text"] = json!(format!("{}changed tail", "x".repeat(2048)));
    rig.put(&path, &changed);
    let args = [
        "unit",
        "observe",
        "--input",
        &execution_input(&execution).to_string(),
    ];
    let observed = rejected_refresh(&rig, &args, id, "contradictory terminal evidence");
    assert_eq!(observed["settlement"], saved["unit"]["returned"]);
    assert_eq!(admission(&rig, &execution), saved);
    rig.put(&path, &original);
    let mut items = rig.read("state/items.json");
    items[0]["reply"]["text"] = json!("contradictory item");
    rig.put("state/items.json", &items);
    let observed = rejected_refresh(&rig, &args, id, "REQUEST_CONFLICT");
    assert_eq!(observed["settlement"], saved["unit"]["returned"]);
    assert_eq!(rig.read("state/items.json"), items);
    assert_eq!(admission(&rig, &execution), saved);
}

#[test]
fn old_reply_wrong_branch_malformed_and_partial_transcripts_never_release() {
    let (rig, execution, input) = launched("seed-bad-evidence");
    let acknowledgement = acknowledgement(&execution, &input);
    let base = entries();
    for variant in 0..10 {
        let mut entries = base.clone();
        match variant {
            0 => entries[3]["parentId"] = json!("seal"), // only old prose precedes seal
            1 => entries[2]["parentId"] = json!("old"),  // seal on another branch
            2 => entries[2]["parentId"] = json!("reply"), // cycle
            3 => entries[2]["parentId"] = json!("missing"),
            4 => entries.push(entries[2].clone()),
            5 => entries[2]["message"]["stopReason"] = json!("toolUse"),
            9 => entries[3]["type"] = json!("custom_message"),
            _ => (),
        }
        let path = transcript(&rig, &execution, &entries);
        if variant == 6 {
            fs::write(&path, "{broken}\n").unwrap();
        }
        if variant == 7 {
            let bytes = fs::read(&path).unwrap();
            fs::write(&path, &bytes[..bytes.len() - 1]).unwrap();
        }
        if variant == 8 {
            let text = fs::read_to_string(&path)
                .unwrap()
                .replace(execution["requestId"].as_str().unwrap(), "other-session");
            fs::write(&path, text).unwrap();
        }
        assert_eq!(ack(&rig, &acknowledgement).0, 2, "variant {variant}");
        assert!(operation(&rig, "observe", &execution_input(&execution)).1["settlement"].is_null());
    }
    transcript(&rig, &execution, &base);
    for field in ["digest", "sealEntryId", "sessionLeafId"] {
        let mut changed = acknowledgement.clone();
        changed[field] = json!("wrong");
        assert_eq!(ack(&rig, &changed).0, 2);
    }
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[{"pane_id":"w2:p1","terminal_id":"replacement"}]}}),
    );
    assert_eq!(ack(&rig, &acknowledgement).0, 2);
}

#[test]
fn transcript_path_and_session_header_cannot_escape_launch_ownership() {
    let (rig, execution, input) = launched("seed-path-ownership");
    let acknowledgement = acknowledgement(&execution, &input);
    let path = transcript(&rig, &execution, &entries());
    let outside = rig.dir.join("outside.jsonl");
    fs::copy(&path, &outside).unwrap();
    let mut agents = rig.read("agents.json");
    agents["result"]["agents"][0]["agent_session"]["value"] = json!(outside);
    rig.put("agents.json", &agents);
    assert_eq!(ack(&rig, &acknowledgement).0, 2);
    transcript(&rig, &execution, &entries());
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&outside, &path).unwrap();
    assert_eq!(ack(&rig, &acknowledgement).0, 2);
    fs::remove_file(&path).unwrap();
    transcript(&rig, &execution, &entries());
    let text = fs::read_to_string(&path).unwrap();
    let mut lines: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    lines[0]["cwd"] = json!("/");
    fs::write(
        &path,
        format!(
            "{}\n",
            lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        ),
    )
    .unwrap();
    assert_eq!(ack(&rig, &acknowledgement).0, 2);
    assert!(operation(&rig, "observe", &execution_input(&execution)).1["settlement"].is_null());
}

#[test]
fn selected_remote_host_receives_exact_seed_and_durable_receipt_refresh() {
    let rig = rig("seed-remote", false);
    rig.put(
        "machines.json",
        &json!([{"id":"8103","label":"remote","target":"remote-host","enabled":true}]),
    );
    rig.snapshot("remote.json", now(), 0.0, 2);
    rig.put("reserve.json", &json!({"reserved":true,"headroom":null}));
    let (_, execution) = rig.json(&[
        "unit",
        "prepare",
        "--input",
        &json!({"origin":exported(0)["contract"]["ref"],"placement":{"host":"remote"}}).to_string(),
        "--tool-call-id",
        "remote-call",
    ]);
    assert!(execution["requestId"].is_string(), "{execution}");
    let input = commit_input(&execution, 0);
    let (code, result) = operation(&rig, "commit", &input);
    assert_eq!(code, 0, "{result}");
    assert_eq!(result["disposition"], "launched");
    let mut seed = input.clone();
    seed["version"] = json!(1);
    assert_eq!(admission(&rig, &execution)["launch"]["unit"], seed);
    let log = rig.log("ssh.log");
    assert!(
        log.contains("--request") && log.contains("exportDigest") && log.contains("prerequisites"),
        "{log}"
    );
    // The host endpoint is the production caller: execute the same transported launch on
    // a separate private host fixture, then let owner refresh fetch its immutable receipt.
    let host = super::unit_requests::rig("seed-remote-host", false);
    let mut launch = admission(&rig, &execution)["launch"].clone();
    launch["cwd"] = json!(host.dir.join("home"));
    let (code, response) = host.json(&["host", "launch", "--request", &launch.to_string()]);
    assert_eq!(code, 0, "{response}");
    let id = execution["requestId"].as_str().unwrap();
    assert_eq!(host.read(&format!("state/launches/{id}.unit.json")), seed);
    transcript(&host, &execution, &entries());
    assert_eq!(ack(&host, &acknowledgement(&execution, &input)).0, 0);
    let (_, receipt) = host.json(&[
        "host",
        "unit-receipt",
        "--input",
        &execution_input(&execution).to_string(),
    ]);
    rig.put("remote-receipt.json", &receipt);
    rig.script(
        "ssh",
        &fs::read_to_string(rig.dir.join("bin/ssh"))
            .unwrap()
            .replace(
                "case \"$*\" in",
                "case \"$*\" in\n  *'host unit-receipt'*) cat \"$RIG/remote-receipt.json\" ;;",
            ),
    );
    assert_eq!(
        operation(&rig, "observe", &execution_input(&execution)).1["phase"],
        "returned"
    );
    assert_eq!(
        rig.read("state/items.json")[0]["reply"]["text"],
        "EXACT FINAL"
    );
}

#[test]
fn existing_service_refresh_imports_receipt_and_does_not_close_unsealed_unit() {
    let (rig, execution, input) = launched("seed-service");
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    transcript(&rig, &execution, &entries());
    let run = || {
        let out = rig.run(&["serve", "--once", "--interval", "1"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run();
    assert!(!rig.log("herdr.log").contains("pane close"));
    assert!(admission(&rig, &execution)["unit"]["returned"].is_null());
    let (code, receipt) = ack(&rig, &acknowledgement(&execution, &input));
    assert_eq!(code, 0, "{receipt}");
    run();
    assert_eq!(
        admission(&rig, &execution)["unit"]["returned"]["result"]["kind"],
        "returned"
    );
}

#[test]
fn fresh_launch_refuses_existing_session_and_changed_artifact() {
    for changed_artifact in [false, true] {
        let rig = rig(
            if changed_artifact {
                "seed-changed"
            } else {
                "seed-existing"
            },
            false,
        );
        let execution = prepare(&rig);
        let input = commit_input(&execution, 0);
        let id = execution["requestId"].as_str().unwrap();
        if changed_artifact {
            fs::create_dir_all(rig.dir.join("state/launches")).unwrap();
            rig.put(&format!("state/launches/{id}.unit.json"), &json!({"version":2,"execution":execution,"binding":input["binding"],"exported":input["exported"]}));
        } else {
            fs::create_dir_all(rig.dir.join(format!("state/launches/{id}.sessions"))).unwrap();
        }
        assert_eq!(operation(&rig, "commit", &input).0, 2);
        assert!(!rig.log("herdr.log").contains("agent start"));
        assert!(operation(&rig, "observe", &execution_input(&execution)).1["settlement"].is_null());
    }
}

#[test]
fn not_started_settlement_delivery_has_origin_without_digest() {
    let rig = rig("seed-abandon-delivery", true);
    let execution = prepare(&rig);
    let (_, event) = operation(&rig, "abandon-prepared", &execution_input(&execution));
    assert!(event.get("binding").is_none());
    assert!(event.get("digest").is_none());
    assert_eq!(event["execution"], execution);
    assert_eq!(rig.json(&["unit", "settlements"]).1, json!([event]));
    assert_eq!(
        operation(
            &rig,
            "ack-settlement",
            &json!({"execution":execution,"eventId":event["eventId"]})
        )
        .0,
        0
    );
    assert_eq!(rig.json(&["unit", "settlements"]).1, json!([]));
}

/// The seeded worker in `state`, published so `send` can resolve it, with a Pi inbox reader.
fn worker_state(rig: &Rig, state: &str) {
    let mut agents = rig.read("agents.json");
    agents["result"]["agents"][0]["agent_status"] = json!(state);
    rig.put("agents.json", &agents);
    let agent = &agents["result"]["agents"][0];
    rig.snapshot("state/host.json", now(), 0.0, 2);
    let mut host = rig.read("state/host.json");
    host["probe"]["agent_panes"] = json!([{"pane":"w2:p1","agent":"pi","state":state,"name":agent["name"],"terminal_id":"term_child"}]);
    rig.put("state/host.json", &host);
    fs::create_dir_all(rig.dir.join("state/inbox")).unwrap();
    rig.put(
        "state/inbox/term_child.reader.json",
        &json!({"pane":"w2:p1","harness":"pi","registered_ms":1}),
    );
}

/// A seeded worker that called `done` during its turn and then went idle, with no terminal
/// acknowledgement: its unit has not returned.
fn done_unreturned(name: &str) -> (Rig, Value, Value) {
    let (rig, execution, input) = launched(name);
    transcript(&rig, &execution, &entries());
    worker_state(&rig, "working");
    let out = rig
        .cmd(&["done"])
        .env("HERDR_PANE_ID", "w2:p1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    worker_state(&rig, "idle");
    (rig, execution, input)
}

fn send_from(rig: &Rig, pane: &str, text: &str) -> (i32, Value) {
    let out = rig
        .cmd(&["send", "w2:p1", "--message", text, "--json"])
        .env("HERDR_PANE_ID", pane)
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null),
    )
}

fn worker_claim(rig: &Rig) -> Output {
    rig.cmd(&["inbox", "claim", "--harness", "pi", "--json"])
        .env("HERDR_PANE_ID", "w2:p1")
        .output()
        .unwrap()
}

fn inbox(rig: &Rig) -> Value {
    serde_json::from_slice(
        &fs::read(rig.dir.join("state/inbox/term_child.json")).unwrap_or_default(),
    )
    .unwrap_or_else(|_| json!([]))
}

fn serve_once(rig: &Rig) {
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn published_done(rig: &Rig) -> bool {
    rig.read("state/host.json")["probe"]["agent_panes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["pane"] == "w2:p1" && a["done"] == true)
}

fn assert_refused(rig: &Rig, pane: &str, expected: &str) {
    let marks = rig.read("state/done.json");
    let messages = inbox(rig);
    let (code, refused) = send_from(rig, pane, "continue");
    assert_eq!(code, 2, "{refused}");
    let error = refused["error"].as_str().unwrap_or_default();
    assert!(error.contains(expected), "{error}");
    assert_eq!(rig.read("state/done.json"), marks);
    assert_eq!(inbox(rig), messages);
}

#[test]
fn done_without_terminal_evidence_stays_open_until_its_owner_continues_it() {
    let (rig, _, _) = done_unreturned("seed-continue");
    serve_once(&rig);
    assert!(!rig.log("herdr.log").contains("pane close"));
    assert!(!published_done(&rig));
    let marks = rig.read("state/done.json");
    let mark = marks.as_object().unwrap().values().next().unwrap();
    assert!(mark["sealed"].is_null(), "{mark}");
    worker_state(&rig, "idle");
    // Negative controls: neither another agent nor the worker itself can reopen it.
    assert_refused(&rig, "w3:p1", "only its owner w1:p1 can continue it");
    let before = rig.read("state/done.json");
    assert!(!worker_claim(&rig).status.success());
    assert_eq!(rig.read("state/done.json"), before);
    // An owner record that is not stored cannot cancel the mark.
    let reader = rig.dir.join("state/inbox/term_child.reader.json");
    fs::remove_file(&reader).unwrap();
    assert_ne!(send_from(&rig, "w1:p1", "lost").0, 0);
    assert_eq!(rig.read("state/done.json"), before);
    worker_state(&rig, "idle");
    // The owner continues the same execution: the same pane, session, and checkout.
    let (code, sent) = send_from(&rig, "w1:p1", "fix the build configuration");
    assert_eq!(code, 0, "{sent}");
    assert_eq!(rig.read("state/done.json"), json!({}));
    let out = worker_claim(&rig);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let claimed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(claimed[0]["text"], "fix the build configuration");
    // Continuation is not a retry: the execution started once and its pane never closed.
    assert_eq!(rig.log("herdr.log").matches("agent start").count(), 1);
    assert!(!rig.log("herdr.log").contains("pane close"));
}

/// A record sent from `pane` on `host`, put through the host command a peer runs over SSH.
fn put_from(rig: &Rig, pane: &str, host: &str, text: &str) -> Output {
    let record = json!({
        "id":"rec-1","terminal":"term_child","pane":"w2:p1",
        "from":{"pane":pane,"host":host},"kind":"message","text":text,
        "created_ms":1,"state":"pending"
    });
    rig.cmd(&["host", "inbox-put", "--record", &record.to_string()])
        .output()
        .unwrap()
}

fn assert_put_refused(rig: &Rig, pane: &str, host: &str) {
    let marks = rig.read("state/done.json");
    let messages = inbox(rig);
    let out = put_from(rig, pane, host, "continue");
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("only its owner"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(rig.read("state/done.json"), marks);
    assert_eq!(inbox(rig), messages);
}

fn owner_progress(rig: &Rig, execution: &Value) -> (String, Value) {
    let id = execution["requestId"].as_str().unwrap();
    let file = format!("state/launches/{id}.progress.json");
    let progress = rig.read(&file);
    (file, progress)
}

#[test]
fn the_owner_pane_id_on_another_host_cannot_continue_the_worker() {
    let (rig, execution, _) = done_unreturned("seed-foreign-host");
    let (_, progress) = owner_progress(&rig, &execution);
    let host = progress["request"]["owner_host"]
        .as_str()
        .unwrap()
        .to_owned();
    worker_state(&rig, "idle");
    // Same pane ID as the owner, but another machine: a different agent.
    assert_put_refused(&rig, "w1:p1", "another-machine");
    // The real owner, on the recorded host, still continues it.
    let out = put_from(&rig, "w1:p1", &host, "continue");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(rig.read("state/done.json"), json!({}));
}

#[test]
fn a_launch_record_without_an_owner_host_refuses_every_sender() {
    let (rig, execution, _) = done_unreturned("seed-legacy-owner");
    let (file, mut progress) = owner_progress(&rig, &execution);
    let host = progress["request"]["owner_host"]
        .as_str()
        .unwrap()
        .to_owned();
    progress["request"]
        .as_object_mut()
        .unwrap()
        .remove("owner_host");
    rig.put(&file, &progress);
    worker_state(&rig, "idle");
    assert_put_refused(&rig, "w1:p1", &host);
    assert_refused(&rig, "w1:p1", "only its owner w1:p1 can continue it");
}

#[test]
fn an_owner_on_a_remote_host_continues_the_worker_and_a_local_namesake_cannot() {
    let (rig, execution, _) = done_unreturned("seed-remote-owner");
    let (file, mut progress) = owner_progress(&rig, &execution);
    progress["request"]["owner_host"] = json!("owner-machine");
    rig.put(&file, &progress);
    worker_state(&rig, "idle");
    // A local sender with the owner's pane ID is not the owner.
    assert_refused(&rig, "w1:p1", "only its owner w1:p1 can continue it");
    let out = put_from(&rig, "w1:p1", "owner-machine", "remote continue");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(rig.read("state/done.json"), json!({}));
    let claimed: Value = serde_json::from_slice(&worker_claim(&rig).stdout).unwrap();
    assert_eq!(claimed[0]["text"], "remote continue");
}

#[test]
fn legacy_sealed_done_without_terminal_evidence_is_continued_only_by_its_owner() {
    let (rig, execution, _) = launched("seed-legacy");
    transcript(&rig, &execution, &entries());
    // The shape an earlier release wrote: sealed at the first idle, with no receipt.
    rig.done("term_child", "w2:p1", 7);
    worker_state(&rig, "idle");
    serve_once(&rig);
    assert!(!rig.log("herdr.log").contains("pane close"));
    assert!(!published_done(&rig));
    let marks = rig.read("state/done.json");
    assert_eq!(
        marks.as_object().unwrap().values().next().unwrap()["sealed"],
        7
    );
    worker_state(&rig, "idle");
    assert_refused(&rig, "w3:p1", "only its owner w1:p1 can continue it");
    let (code, sent) = send_from(&rig, "w1:p1", "continue");
    assert_eq!(code, 0, "{sent}");
    assert_eq!(rig.read("state/done.json"), json!({}));
    assert!(worker_claim(&rig).status.success());
}

#[test]
fn returned_unit_refuses_its_owner_and_closes_with_the_exact_reply() {
    let (rig, execution, input) = done_unreturned("seed-returned");
    worker_state(&rig, "working");
    assert_eq!(ack(&rig, &acknowledgement(&execution, &input)).0, 0);
    assert_refused(&rig, "w1:p1", "returned its unit");
    worker_state(&rig, "idle");
    // The first idle input observation seals a returned unit; the message is still refused.
    let messages = inbox(&rig);
    let (code, refused) = send_from(&rig, "w1:p1", "continue");
    assert_eq!(code, 2, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("finished; start a new child")
    );
    assert_eq!(inbox(&rig), messages);
    let marks = rig.read("state/done.json");
    assert_eq!(
        marks.as_object().unwrap().values().next().unwrap()["sealed"],
        7
    );
    assert_refused(&rig, "w1:p1", "finished; start a new child for more work");
    serve_once(&rig);
    assert!(rig.log("herdr.log").contains("pane close w2:p1"));
    let marks = rig.read("state/done.json");
    assert_eq!(
        marks.as_object().unwrap().values().next().unwrap()["reply"]["text"],
        "EXACT FINAL"
    );
}

#[test]
fn unavailable_or_mismatched_unit_evidence_refuses_continuation_and_close() {
    for case in ["progress", "seed", "terminal", "malformed"] {
        let (rig, execution, _) = done_unreturned(&format!("seed-evidence-{case}"));
        let id = execution["requestId"].as_str().unwrap();
        let launches = rig.dir.join("state/launches");
        match case {
            "progress" => fs::remove_file(launches.join(format!("{id}.progress.json"))).unwrap(),
            "seed" => fs::remove_file(launches.join(format!("{id}.unit.json"))).unwrap(),
            "malformed" => fs::write(launches.join(format!("{id}.progress.json")), "{").unwrap(),
            _ => {
                let mut agents = rig.read("agents.json");
                agents["result"]["agents"][0]["terminal_id"] = json!("term_other");
                rig.put("agents.json", &agents);
            }
        }
        assert_refused(&rig, "w1:p1", "unit evidence");
        assert!(!worker_claim(&rig).status.success(), "{case}");
        // The service may report the bad evidence; it must never close the worker.
        rig.run(&["serve", "--once", "--interval", "1"]);
        assert!(!rig.log("herdr.log").contains("pane close"), "{case}");
    }
}
