//! `dogsled legacy-continue`: the origin-host owner continues a seeded worker whose launch
//! record predates the owner host. Ordinary `send` still refuses every sender for it.

use super::unit_requests::*;
use super::unit_seeded::*;
use super::*;

/// The immutable records of request `id` that continuation must never rewrite.
const IMMUTABLE: [&str; 6] = [
    "admissions/{id}.json",
    "launches/{id}.progress.json",
    "launches/{id}.unit.json",
    "launches/{id}.json",
    "queue.json",
    "items.json",
];

fn request_id(execution: &Value) -> String {
    execution["requestId"].as_str().unwrap().to_owned()
}

fn immutable(rig: &Rig, id: &str) -> Vec<Vec<u8>> {
    IMMUTABLE
        .iter()
        .map(|path| {
            fs::read(rig.dir.join("state").join(path.replace("{id}", id))).unwrap_or_default()
        })
        .collect()
}

/// Every record a refusal must leave unchanged.
fn state(rig: &Rig, id: &str) -> (Vec<Vec<u8>>, Value, Value, Vec<u8>) {
    (
        immutable(rig, id),
        rig.read("state/done.json"),
        inbox(rig),
        fs::read(rig.dir.join("state/outbox.json")).unwrap_or_default(),
    )
}

/// Write the owner's Pi transcript with session `session`, and list `pane` as a Pi agent
/// running it, beside the worker.
fn owner_agent(rig: &Rig, pane: &str, terminal: &str, session: &str) {
    let path = rig.dir.join(format!("home/owner-{pane}.jsonl"));
    fs::write(
        &path,
        format!(
            "{}\n",
            json!({"type":"session","version":3,"id":session,"cwd":rig.dir})
        ),
    )
    .unwrap();
    let mut agents = rig.read("agents.json");
    let list = agents["result"]["agents"].as_array_mut().unwrap();
    list.retain(|a| a["pane_id"] != pane);
    list.push(json!({
        "pane_id":pane,"terminal_id":terminal,"agent":"pi","name":"owner","agent_status":"working",
        "agent_session":{"agent":"pi","kind":"path","value":path}
    }));
    rig.put("agents.json", &agents);
}

fn origin_session() -> String {
    exported(0)["contract"]["ref"]["sessionId"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// A worker that called `done` before its unit returned, recorded as an earlier release wrote
/// it: no owner host in the admitted request, the saved launch, or the host launch record.
fn legacy_unreturned(name: &str) -> (Rig, Value, Value) {
    let (rig, execution, input) = done_unreturned(name);
    let id = request_id(&execution);
    let (file, mut progress) = owner_progress(&rig, &execution);
    progress["request"]
        .as_object_mut()
        .unwrap()
        .remove("owner_host")
        .unwrap();
    rig.put(&file, &progress);
    let file = format!("state/admissions/{id}.json");
    let mut admission = rig.read(&file);
    for key in ["request", "launch"] {
        admission[key]
            .as_object_mut()
            .unwrap()
            .remove("owner_host")
            .unwrap();
    }
    assert_eq!(admission["launch"], progress["request"]);
    rig.put(&file, &admission);
    owner_agent(&rig, "w1:p1", "term_owner", &origin_session());
    worker_state(&rig, "idle");
    (rig, execution, input)
}

fn continue_from(rig: &Rig, pane: &str, text: &str) -> (i32, Value) {
    let out = rig
        .cmd(&["legacy-continue", "w2:p1", "--message", text, "--json"])
        .env("HERDR_PANE_ID", pane)
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null),
    )
}

fn assert_continue_refused(rig: &Rig, id: &str, pane: &str, expected: &str) {
    let before = state(rig, id);
    let (code, refused) = continue_from(rig, pane, "continue");
    assert_eq!(code, 2, "{refused}");
    let error = refused["error"].as_str().unwrap_or_default();
    assert!(error.contains(expected), "{expected}: {error}");
    assert_eq!(state(rig, id), before, "{expected}");
}

#[test]
fn the_origin_host_owner_continues_a_legacy_worker_without_any_relaunch() {
    let (rig, execution, _) = legacy_unreturned("legacy-continue");
    let id = request_id(&execution);
    // #133 stays: ordinary input from the owner is still refused for a legacy record.
    assert_refused(&rig, "w1:p1", "only its owner w1:p1 can continue it");
    let records = immutable(&rig, &id);
    let herdr = rig.log("herdr.log");
    let (code, sent) = continue_from(&rig, "w1:p1", "fix the build configuration");
    assert_eq!(code, 0, "{sent}");
    assert_eq!(sent["from"]["pane"], "w1:p1");
    assert_eq!(sent["from"]["terminal"], "term_owner");
    // Only the continued mark is removed. No immutable record is rewritten or backfilled.
    assert_eq!(rig.read("state/done.json"), json!({}));
    assert_eq!(immutable(&rig, &id), records);
    let progress = rig.read(&format!("state/launches/{id}.progress.json"));
    assert!(progress["request"].get("owner_host").is_none());
    let admission = rig.read(&format!("state/admissions/{id}.json"));
    assert!(admission["request"].get("owner_host").is_none());
    assert!(admission["launch"].get("owner_host").is_none());
    // The command only read Herdr: no start, close, pane, or worktree operation, and no peer.
    let commands = rig.log("herdr.log");
    let added = commands.strip_prefix(herdr.as_str()).unwrap();
    assert!(
        added
            .lines()
            .all(|line| line == "agent list" || line == "pane list"),
        "{added}"
    );
    assert!(!rig.dir.join("ssh.log").exists());
    assert_eq!(rig.log("herdr.log").matches("agent start").count(), 1);
    // The same session, worktree, and branch take the input.
    let out = worker_claim(&rig);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let claimed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(claimed[0]["text"], "fix the build configuration");
    assert_eq!(claimed[0]["id"], sent["id"]);
    let outbox = rig.read("state/outbox.json");
    assert_eq!(outbox.as_array().unwrap().len(), 1);
}

#[test]
fn another_pane_or_a_replaced_owner_session_cannot_continue() {
    let (rig, execution, _) = legacy_unreturned("legacy-identity");
    let id = request_id(&execution);
    // Another local Pi agent in the same origin session is not the owner pane.
    owner_agent(&rig, "w3:p1", "term_other", &origin_session());
    assert_continue_refused(&rig, &id, "w3:p1", "only its owner w1:p1 can continue it");
    // The worker cannot continue itself.
    assert_continue_refused(&rig, &id, "w2:p1", "cannot continue its own pane");
    // The owner pane now runs another session: a replaced or reused pane.
    owner_agent(
        &rig,
        "w1:p1",
        "term_owner",
        "01a00000-0000-7000-8000-000000000000",
    );
    assert_continue_refused(&rig, &id, "w1:p1", "no longer runs the session");
    // A transcript without a session header proves nothing.
    owner_agent(&rig, "w1:p1", "term_owner", &origin_session());
    let path = rig.dir.join("home/owner-w1:p1.jsonl");
    fs::write(&path, "{\"type\":\"message\"}\n").unwrap();
    assert_continue_refused(&rig, &id, "w1:p1", "not a session header");
    fs::write(
        &path,
        format!("{}", json!({"type":"session","id":origin_session()})),
    )
    .unwrap();
    assert_continue_refused(&rig, &id, "w1:p1", "partial");
    // An owner pane that is not a Pi agent, or is gone, proves nothing.
    let mut agents = rig.read("agents.json");
    let list = agents["result"]["agents"].as_array_mut().unwrap();
    for agent in list.iter_mut().filter(|a| a["pane_id"] == "w1:p1") {
        agent["agent"] = json!("claude");
    }
    rig.put("agents.json", &agents);
    assert_continue_refused(&rig, &id, "w1:p1", "is not a Pi agent");
    agents["result"]["agents"]
        .as_array_mut()
        .unwrap()
        .retain(|a| a["pane_id"] != "w1:p1");
    rig.put("agents.json", &agents);
    assert_continue_refused(&rig, &id, "w1:p1", "is not an agent pane on this host");
    // The real owner still continues it afterwards.
    owner_agent(&rig, "w1:p1", "term_owner", &origin_session());
    assert_eq!(continue_from(&rig, "w1:p1", "continue").0, 0);
}

#[test]
fn missing_corrupt_or_contradictory_admission_refuses_with_no_change() {
    let cases: [(&str, &str); 10] = [
        ("missing", "no original admission"),
        ("corrupt", ""),
        ("launch", "saved launch does not equal"),
        ("digest", "export digest does not match"),
        (
            "binding",
            "export, binding, and prepared request do not match",
        ),
        ("remote", "did not select this host"),
        ("settled", "already settled"),
        ("owner", "not the execution owner"),
        ("request", "admitted request does not match"),
        ("pane", "did not launch this pane"),
    ];
    for (case, expected) in cases {
        let (rig, execution, _) = legacy_unreturned(&format!("legacy-admission-{case}"));
        let id = request_id(&execution);
        let file = format!("state/admissions/{id}.json");
        let mut admission = rig.read(&file);
        match case {
            // A copied seed and launch record without the original admission.
            "missing" => fs::remove_file(rig.dir.join(&file)).unwrap(),
            "corrupt" => fs::write(rig.dir.join(&file), "{").unwrap(),
            "launch" => admission["launch"]["cwd"] = json!("/elsewhere"),
            "digest" => {
                admission["unit"]["committed"]["exported"]["digest"] = json!("0".repeat(64));
            }
            "binding" => {
                admission["unit"]["committed"]["binding"]["contract"]["revision"] = json!(9);
            }
            "remote" => admission["host"] = json!({"id":"8103","label":"netcup","target":"netcup"}),
            "settled" => admission["unit"]["settlement_ack"] = json!("event"),
            "owner" => admission["unit"]["execution"]["ownerId"] = json!("w3:p1"),
            "request" => admission["request"]["owner_host"] = json!("Oisin's MacBook Pro"),
            _ => admission["result"]["pane"] = json!("w9:p9"),
        }
        if !matches!(case, "missing" | "corrupt") {
            rig.put(&file, &admission);
        }
        assert_continue_refused(&rig, &id, "w1:p1", expected);
        assert!(!rig.log("herdr.log").contains("pane close"), "{case}");
    }
}

#[test]
fn mismatched_launch_evidence_refuses_with_no_change() {
    for case in [
        "seed",
        "progress",
        "unconfirmed",
        "competing",
        "item",
        "terminal",
        "owner_host",
        "reader",
    ] {
        let (rig, execution, _) = legacy_unreturned(&format!("legacy-launch-{case}"));
        let id = request_id(&execution);
        let launches = rig.dir.join("state/launches");
        let (file, mut progress) = owner_progress(&rig, &execution);
        let expected = match case {
            "seed" => {
                let mut seed = rig.read(&format!("state/launches/{id}.unit.json"));
                seed["exported"]["brief"] = json!("changed");
                rig.put(&format!("state/launches/{id}.unit.json"), &seed);
                "unit evidence"
            }
            "progress" => {
                progress["request"]["cwd"] = json!("/elsewhere");
                rig.put(&file, &progress);
                "saved launch does not equal"
            }
            "unconfirmed" => {
                progress["confirmed"] = json!(false);
                rig.put(&file, &progress);
                "not a started, confirmed, unreturned legacy launch"
            }
            "competing" => {
                fs::copy(
                    launches.join(format!("{id}.progress.json")),
                    launches.join("r-competing.progress.json"),
                )
                .unwrap();
                "competing"
            }
            "item" => {
                let mut items = rig.read("state/items.json");
                items[0]["closed_ms"] = json!(1);
                rig.put("state/items.json", &items);
                "closed, missing, or competing"
            }
            "terminal" => {
                let mut agents = rig.read("agents.json");
                agents["result"]["agents"][0]["terminal_id"] = json!("term_other");
                rig.put("agents.json", &agents);
                "unit evidence"
            }
            // A record that names its owner host continues through ordinary send.
            "owner_host" => {
                progress["request"]["owner_host"] = json!("Oisin's MacBook Pro");
                rig.put(&file, &progress);
                "continue it with dogsled send"
            }
            _ => {
                fs::remove_file(rig.dir.join("state/inbox/term_child.reader.json")).unwrap();
                "no dogsled inbox reader"
            }
        };
        assert_continue_refused(&rig, &id, "w1:p1", expected);
    }
}

#[test]
fn returned_or_remote_workers_cannot_be_continued() {
    let (rig, execution, input) = legacy_unreturned("legacy-returned");
    let id = request_id(&execution);
    worker_state(&rig, "working");
    assert_eq!(ack(&rig, &acknowledgement(&execution, &input)).0, 0);
    assert_continue_refused(&rig, &id, "w1:p1", "returned its unit");
    // A worker on another host is never continued from here: only local panes resolve.
    let (rig, execution, _) = legacy_unreturned("legacy-remote");
    let id = request_id(&execution);
    let before = state(&rig, &id);
    let out = rig
        .cmd(&[
            "legacy-continue",
            "w9:p1",
            "--message",
            "continue",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stdout).contains("not an agent pane on this host"));
    assert_eq!(state(&rig, &id), before);
    assert!(!rig.dir.join("ssh.log").exists());
    // Without a done mark there is nothing to continue: ordinary send serves that worker.
    let (rig, execution, _) = legacy_unreturned("legacy-unmarked");
    let id = request_id(&execution);
    rig.put("state/done.json", &json!({}));
    assert_continue_refused(&rig, &id, "w1:p1", "no done mark to continue");
}

#[test]
fn a_crash_after_the_input_persists_recovers_once_and_a_stale_replay_changes_nothing() {
    let (rig, execution, _) = legacy_unreturned("legacy-crash");
    let id = request_id(&execution);
    let marks = rig.read("state/done.json");
    let out = rig
        .cmd(&[
            "legacy-continue",
            "w2:p1",
            "--message",
            "continue",
            "--json",
        ])
        .env("DOGSLED_TEST_UNIT_CRASH", "legacy-continue-stored")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(86));
    // The input is durable and the mark is still in place.
    assert_eq!(rig.read("state/done.json"), marks);
    let stored = inbox(&rig);
    assert_eq!(stored.as_array().unwrap().len(), 1);
    // The retry finds the same record and removes the mark it continued.
    let (code, sent) = continue_from(&rig, "w1:p1", "continue");
    assert_eq!(code, 0, "{sent}");
    assert_eq!(sent["id"], stored[0]["id"]);
    assert_eq!(inbox(&rig), stored);
    assert_eq!(rig.read("state/done.json"), json!({}));
    // The worker takes the input, works, and calls done again.
    assert!(worker_claim(&rig).status.success());
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
    // A stale replay of the first continuation never clears the later mark.
    assert_continue_refused(&rig, &id, "w1:p1", "for an earlier done mark");
    assert_ne!(rig.read("state/done.json"), json!({}));
    // A new continuation is accepted once.
    assert_eq!(continue_from(&rig, "w1:p1", "next step").0, 0);
    assert_eq!(rig.read("state/done.json"), json!({}));
    assert_eq!(inbox(&rig).as_array().unwrap().len(), 2);
    assert_eq!(rig.log("herdr.log").matches("agent start").count(), 1);
}

#[test]
fn continuation_waits_for_the_launch_lock_that_terminal_acknowledgement_holds() {
    let (rig, execution, input) = legacy_unreturned("legacy-lock");
    let id = request_id(&execution);
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(rig.dir.join(format!("state/launches/{id}.lock")))
        .unwrap();
    lock.try_lock().unwrap();
    let before = state(&rig, &id);
    let out = rig
        .cmd(&[
            "legacy-continue",
            "w2:p1",
            "--message",
            "continue",
            "--json",
            "--budget-ms",
            "300",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let refused: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(refused["deadline"], true, "{refused}");
    assert_eq!(state(&rig, &id), before);
    lock.unlock().unwrap();
    // The acknowledgement wins the lock first: the continuation then sees the return.
    worker_state(&rig, "working");
    assert_eq!(ack(&rig, &acknowledgement(&execution, &input)).0, 0);
    assert_continue_refused(&rig, &id, "w1:p1", "returned its unit");
}
