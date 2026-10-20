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
            "pane_id":"w2:p1","terminal_id":"term_child","agent":"pi","name":"unit-u-17-r1",
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
