//! Strict owner-proven retention through the real service and prune commands.
use super::*;

const DAY: u64 = 86_400_000;
const WINDOW: u64 = 30 * DAY;

fn clock(rig: &Rig, now: u64) {
    fs::write(rig.dir.join("clock"), now.to_string()).unwrap();
}

fn serve(rig: &Rig) {
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    rig.snapshot("state/host.json", rig.now_ms(), 0.0, 8);
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(out.status.success(), "{out:?}");
    fs::write(rig.dir.join("retention-output"), &out.stderr).unwrap();
}

fn job(rig: &Rig, id: &str, ended: u64) {
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    rig.put(&format!("state/jobs/{id}.json"), &json!({
        "id":id,"cmd":["true"],"cwd":rig.dir.join("home"),"host":"local",
        "owner":"w1:p1","pane":"w2:p1","state":"idle","exit":0,"signal":null,
        "reason":null,"started_ms":ended,"ended_ms":ended,"timeout_ms":null,"idle_timeout_ms":null
    }));
    fs::write(rig.dir.join(format!("state/jobs/{id}.log")), "old output").unwrap();
}

#[test]
fn r1b_explicit_prune_keeps_two_day_terminal_record_and_reports_expired() {
    let rig = Rig::new("r1-prune");
    let ended = rig.now_ms();
    job(&rig, "old", ended);
    clock(&rig, ended + 2 * DAY);
    let out = rig.run(&["prune", "--yes", "jobs"]);
    assert!(out.status.success(), "{out:?}");
    assert!(rig.dir.join("state/jobs/old.json").exists());
    assert_eq!(rig.read("state/jobs/old.json")["output_expired"], true);
    let out = rig.run(&["logs", "old"]);
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("output expired"));
}

fn closed_item(id: &str, ended: u64) -> Value {
    json!({"id":id,"kind":"agent","host":"local","pane":id,"owner_pane":"w1:p1",
        "name":id,"created_ms":ended-1,"closed_ms":ended,"native_done":true,
        "reply":{"seq":1,"text":"final","captured_ms":ended,"reported_ms":ended,"done":true}})
}

#[test]
fn r1b_closed_items_and_marks_expire_only_after_the_strict_window() {
    let rig = Rig::new("r1-items");
    let ended = rig.now_ms();
    let item = closed_item("finished", ended);
    let mut held = closed_item("unlanded", ended);
    held["branch"] = json!({"repo":"/tmp/repo","name":"feature","ahead":1,"landed":false});
    let mut open = closed_item("open", ended);
    open["closed_ms"] = Value::Null;
    rig.put("state/items.json", &json!([item, held, open]));
    let mark = json!({"pane":"finished","item":"finished","session":"old",
        "marked_ms":ended-1,"sealed":1,"closed_ms":ended,"native_done":true,
        "reply":{"seq":1,"text":"final","captured_ms":ended,"done":true}});
    rig.put(
        "state/done.json",
        &json!({"[\"finished\",\"finished\",\"old\"]":mark}),
    );
    clock(&rig, ended + WINDOW);
    serve(&rig);
    assert_eq!(rig.read("state/items.json").as_array().unwrap().len(), 3);
    assert_eq!(rig.read("state/done.json").as_object().unwrap().len(), 1);
    clock(&rig, ended + WINDOW + 1);
    serve(&rig);
    let items = rig.read("state/items.json");
    assert_eq!(items.as_array().unwrap().len(), 2, "{items}");
    assert!(
        !items
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == "finished")
    );
    assert_eq!(rig.read("state/done.json"), json!({}));
}

#[test]
fn r1b_last_job_identity_expiry_never_resolves_a_reused_agent() {
    let rig = Rig::new("r1-no-tombstone");
    let ended = rig.now_ms();
    job(&rig, "r-old-job", ended);
    let mut item = closed_item("r-old-job", ended);
    item["kind"] = json!("job");
    item["name"] = json!("reused");
    rig.put("state/items.json", &json!([item]));
    clock(&rig, ended + WINDOW + 1);
    serve(&rig);
    assert!(!rig.dir.join("state/jobs/r-old-job.json").exists());
    assert_eq!(rig.read("state/items.json"), json!([]));
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{
            "pane_id":"replacement","agent":"pi","name":"reused","agent_status":"idle"
        }]}}),
    );
    for target in ["r-old-job", "never-known"] {
        let (code, value) = rig.json(&["read", target]);
        assert_eq!(code, 2, "{value}");
        assert_eq!(
            value["error"], "result unavailable: expired or unknown",
            "{value}"
        );
    }
    assert!(
        !rig.log("herdr.log")
            .contains("pane read --pane replacement")
    );
}

#[test]
fn r1a_settled_placement_family_expires_without_reallocation() {
    let (rig, repo) = place::place_rig("r1-placement");
    let ended = rig.now_ms();
    clock(&rig, ended);
    let (code, result) = rig.json(&[
        "place",
        "--host",
        "local",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "settled",
        "--base",
        "main",
    ]);
    assert_eq!(code, 0, "{result}");
    let id = result["request_id"].as_str().unwrap();
    clock(&rig, ended + WINDOW + 1);
    serve(&rig);
    for path in [
        format!("admissions/{id}.json"),
        format!("placement-grants/{id}.json"),
        format!("placement-grants/{id}.lifecycle.json"),
        format!("preparation-receipts/{id}.json"),
        format!("preparation-receipts/{id}.disposition.json"),
    ] {
        assert!(!rig.dir.join("state").join(&path).exists(), "{path}");
    }
    let before = rig.log("herdr.log");
    assert_eq!(rig.json(&["place", "--resume", id]).0, 2);
    assert_eq!(
        rig.log("herdr.log").matches("workspace create").count(),
        before.matches("workspace create").count()
    );
    assert!(rig.dir.join(format!("state/admissions/{id}.lock")).exists());
}

#[test]
fn r1b_live_nonchild_agent_reads_still_work_by_name_and_pane() {
    let rig = watchdog::watchdog_rig("r1-live");
    rig.put("agents.json", &json!({"result":{"agents":[{
        "pane_id":"w2:p1","terminal_id":"term_child","agent":"pi","name":"live-agent","agent_status":"idle"
    }]}}));
    rig.snapshot("state/host.json", rig.now_ms(), 0.0, 8);
    let mut snapshot = rig.read("state/host.json");
    snapshot["probe"]["agent_panes"] = json!([{
        "pane":"w2:p1","terminal_id":"term_child","agent":"pi","state":"idle","name":"live-agent"
    }]);
    rig.put("state/host.json", &snapshot);
    for target in ["live-agent", "w2:p1"] {
        let (code, value) = rig.json(&["read", target]);
        assert_eq!(code, 0, "{value}");
        assert_eq!(value["pane"], "w2:p1");
    }
}

#[test]
fn r1c_remote_ordinary_job_requires_a_versioned_release_certificate() {
    let rig = Rig::new("r1-ordinary");
    let ended = rig.now_ms();
    clock(&rig, ended);
    job(&rig, "ordinary", ended);
    let mut terminal = rig.read("state/jobs/ordinary.json");
    terminal["host"] = json!("remote-host");
    terminal["validity"] = json!({"version":1,"created_ms":ended});
    rig.put("state/jobs/ordinary.json", &terminal);
    let launch = json!({"id":"ordinary","action":{"type":"job","cmd":["true"],
        "timeout_ms":null,"idle_timeout_ms":null,"validity":{"version":1,"created_ms":ended}},
        "host":"remote-host","cwd":rig.dir.join("home"),"pane":null,"terminal":null,
        "workspace":"w1","workspace_name":"caller","owner":"w1:p1","owner_host":"origin-host"});
    fs::create_dir_all(rig.dir.join("state/launches/ordinary.sessions")).unwrap();
    fs::write(
        rig.dir.join("state/launches/ordinary.sessions/owned.jsonl"),
        "owned transcript",
    )
    .unwrap();
    rig.put(
        "state/launches/ordinary.json",
        &json!({"id":"ordinary","host":"remote-host",
        "pane":"w2:p1","launch":launch,"terminal_job":terminal}),
    );
    let certificate = json!({"version":2,"origin_host":"origin-host",
        "execution":{"executor":"dogsled","ownerId":"w1:p1","requestId":"ordinary"},
        "launch":launch,"binding":{"kind":"none"},"custody_revision":0,
        "terminal_evidence":{"kind":"job","job":terminal},
        "released_references":["execution","recovery","custody"]});
    clock(&rig, ended + WINDOW + 1);
    serve(&rig);
    assert!(rig.dir.join("state/launches/ordinary.json").exists());
    assert!(
        rig.dir
            .join("state/launches/ordinary.sessions/owned.jsonl")
            .exists()
    );
    let mut changed = certificate.clone();
    changed["launch"]["owner"] = json!("other");
    assert_eq!(
        rig.json(&[
            "host",
            "release-references",
            "--input",
            &changed.to_string()
        ])
        .0,
        2
    );
    let (code, accepted) = rig.json(&[
        "host",
        "release-references",
        "--input",
        &certificate.to_string(),
    ]);
    assert_eq!(code, 0, "{accepted}");
    assert_eq!(
        rig.json(&[
            "host",
            "release-references",
            "--input",
            &certificate.to_string()
        ])
        .1,
        accepted
    );
    serve(&rig);
    assert!(!rig.dir.join("state/launches/ordinary.json").exists());
    assert!(!rig.dir.join("state/launches/ordinary.sessions").exists());
    assert_eq!(
        rig.json(&[
            "host",
            "release-references",
            "--input",
            &certificate.to_string()
        ])
        .1,
        accepted
    );
    clock(&rig, ended + 2 * WINDOW + 2);
    serve(&rig);
    assert!(
        !rig.dir
            .join("state/launches/ordinary.release.json")
            .exists()
    );
    assert!(rig.dir.join("state/launches/ordinary.lock").exists());
}

#[test]
fn r1c_released_seeded_family_expires_but_live_or_symlinked_evidence_stays() {
    for held in ["none", "live", "symlink", "unreleased"] {
        let (rig, execution, certificate) =
            unit_seeded::release_fixture(&format!("r1-seeded-{held}"));
        let id = execution["requestId"].as_str().unwrap();
        let ended = rig.now_ms();
        clock(&rig, ended);
        let event =
            unit_requests::admission(&rig, &execution)["unit"]["returned"]["eventId"].clone();
        assert_eq!(
            unit_requests::operation(
                &rig,
                "ack-settlement",
                &json!({"execution":execution,"eventId":event})
            )
            .0,
            0
        );
        if held != "unreleased" {
            let (code, value) = unit_requests::operation(&rig, "release-references", &certificate);
            assert_eq!(code, 0, "{value}");
            assert!(value["acknowledgement"].is_object(), "{value}");
        }
        let mut items = rig.read("state/items.json");
        items[0]["closed_ms"] = json!(ended);
        items[0]["native_done"] = json!(true);
        items[0]["reply"]["done"] = json!(true);
        items[0]["reply"]["reported_ms"] = json!(ended);
        rig.put("state/items.json", &items);
        if held != "live" {
            rig.put("agents.json", &json!({"result":{"agents":[]}}));
            rig.put("panes.json", &json!({"result":{"panes":[]}}));
        }
        let sessions = rig.dir.join(format!("state/launches/{id}.sessions"));
        let unrelated = rig.dir.join("unrelated.jsonl");
        fs::write(&unrelated, "untouched").unwrap();
        if held == "symlink" {
            std::os::unix::fs::symlink(&unrelated, sessions.join("linked.jsonl")).unwrap();
        }
        clock(&rig, ended + WINDOW + 1);
        serve(&rig);
        let retained = held != "none";
        assert_eq!(
            sessions.exists(),
            retained,
            "{held}: {}\nadmission: {}\nprogress: {}\nitems:{}",
            rig.log("retention-output"),
            rig.log(&format!("state/admissions/{id}.json")),
            rig.log(&format!("state/launches/{id}.progress.json")),
            rig.log("state/items.json")
        );
        assert_eq!(
            rig.dir
                .join(format!("state/launches/{id}.progress.json"))
                .exists(),
            retained,
            "{held}"
        );
        assert_eq!(fs::read_to_string(unrelated).unwrap(), "untouched");
        assert!(rig.dir.join(format!("state/launches/{id}.lock")).exists());
        if !retained {
            assert!(!rig.dir.join(format!("state/admissions/{id}.json")).exists());
            assert_eq!(
                unit_requests::operation(&rig, "observe", &json!({"execution":execution})).0,
                2
            );
            let calls = rig.log("herdr.log").matches("agent start").count();
            let mut stale = certificate["launch"].clone();
            stale["validity"] = rig.job_validity();
            assert_eq!(
                rig.json(&[
                    "host",
                    "launch",
                    "--request-validity",
                    "1",
                    "--request",
                    &stale.to_string()
                ])
                .0,
                2
            );
            assert_eq!(rig.log("herdr.log").matches("agent start").count(), calls);
        }
    }
}

#[test]
fn r1a_grant_expiry_requires_origin_terminal_proof() {
    let (rig, repo) = place::place_rig("r1-grant-proof");
    let ended = rig.now_ms();
    clock(&rig, ended);
    let (code, result) = rig.json(&[
        "place",
        "--host",
        "local",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "proof",
        "--base",
        "main",
    ]);
    assert_eq!(code, 0, "{result}");
    let id = result["request_id"].as_str().unwrap();
    let lifecycle = format!("state/placement-grants/{id}.lifecycle.json");
    let proof = rig.read(&lifecycle);
    fs::remove_file(rig.dir.join(&lifecycle)).unwrap();
    clock(&rig, ended + WINDOW + 1);
    serve(&rig);
    assert!(
        rig.dir
            .join(format!("state/placement-grants/{id}.json"))
            .exists()
    );
    rig.put(&lifecycle, &proof);
    serve(&rig);
    assert!(
        !rig.dir
            .join(format!("state/placement-grants/{id}.json"))
            .exists()
    );
}

#[test]
fn r1a_deleted_grant_replay_without_expectation_never_allocates() {
    let (rig, repo) = place::place_rig("r1-grant-replay");
    let ended = rig.now_ms();
    clock(&rig, ended);
    let (code, result) = rig.json(&[
        "place",
        "--host",
        "local",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "replay",
        "--base",
        "main",
    ]);
    assert_eq!(code, 0, "{result}");
    let id = result["request_id"].as_str().unwrap();
    let grant = rig.read(&format!("state/placement-grants/{id}.json"));
    clock(&rig, ended + WINDOW + 1);
    serve(&rig);
    let before = rig.log("herdr.log");
    let holds = rig.read("state/placements.json");
    for expected in [Value::Null, grant.clone()] {
        let replay = json!({"envelope":grant["envelope"],"expected_grant":expected});
        assert_eq!(
            rig.json(&[
                "host",
                "reserve",
                "--id",
                id,
                "--placement-replay",
                &replay.to_string(),
                "--request-validity",
                "1",
                "--preparation-protocol",
                "1"
            ])
            .0,
            2
        );
        assert!(
            !rig.dir
                .join(format!("state/placement-grants/{id}.json"))
                .exists()
        );
        assert_eq!(rig.read("state/placements.json"), holds);
    }
    assert_eq!(rig.json(&["place", "--resume", id]).0, 2);
    assert_eq!(
        rig.log("herdr.log").matches("workspace create").count(),
        before.matches("workspace create").count()
    );
}

#[test]
fn r1a_unknown_cancelled_ticket_remains_unsettled_despite_age() {
    let rig = Rig::short("r1-ticket");
    let issued = rig.now_ms();
    clock(&rig, issued);
    rig.config("free_core_fraction_min = 2\n");
    rig.snapshot("state/host.json", issued, 0.0, 8);
    let (code, queued) = rig.json(&["agent", "--name", "cancelled", "--message", "test"]);
    assert_eq!(code, 1, "{queued}");
    let ticket = queued["ticket"].as_str().unwrap();
    clock(&rig, issued + WINDOW - 1);
    let (code, result) = rig.json(&["cancel", ticket]);
    assert_eq!(code, 0, "{result}");
    let finished = rig.now_ms();
    clock(&rig, finished + WINDOW);
    serve(&rig);
    assert_eq!(rig.read("state/queue.json").as_array().unwrap().len(), 1);
    clock(&rig, finished + WINDOW + 1);
    serve(&rig);
    assert_eq!(rig.read("state/queue.json").as_array().unwrap().len(), 1);
    assert!(rig.read("state/queue.json")[0]["finished_ms"].is_number());
}

#[test]
fn r1c_same_host_ordinary_job_captures_release_and_expires_without_owner_read() {
    let rig = Rig::short("r1-local-job");
    rig.snapshot("state/host.json", rig.now_ms(), 0.0, 8);
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    let (code, result) = rig.json(&["run", "--host", "local", "--", "true"]);
    assert_eq!(code, 0, "{result}");
    let id = result["id"].as_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if rig.read(&format!("state/jobs/{id}.json"))["ended_ms"].is_number() {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    serve(&rig);
    let attempt = rig.read(&format!("state/admissions/{id}.json"));
    assert_eq!(
        attempt["ordinary_release"]["certificate"]["binding"],
        json!({"kind":"none"}),
        "{attempt}"
    );
    assert!(
        attempt["ordinary_release"]["acknowledgement"].is_object(),
        "{attempt}"
    );
    let finished = attempt["finished_ms"].as_u64().unwrap();
    clock(&rig, finished + WINDOW + 1);
    serve(&rig);
    assert!(!rig.dir.join(format!("state/launches/{id}.json")).exists());
    assert!(!rig.dir.join(format!("state/admissions/{id}.json")).exists());
    assert_eq!(rig.read("state/items.json"), json!([]));
}

#[test]
fn r1a_failed_placement_expires_after_exact_origin_and_host_completion() {
    let (rig, repo) = place::place_rig("r1-failed-preparation");
    let ended = rig.now_ms();
    clock(&rig, ended);
    fs::write(rig.dir.join("setup-fails"), "").unwrap();
    let (code, error) = rig.json(&[
        "place",
        "--host",
        "local",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "failure",
        "--base",
        "main",
    ]);
    assert_eq!(code, 2, "{error}");
    let id = error["request_id"].as_str().unwrap();
    assert_eq!(rig.json(&["place", "--resume", id]).0, 2);
    clock(&rig, ended + WINDOW + 1);
    serve(&rig);
    assert!(
        !rig.dir
            .join(format!("state/preparation-receipts/{id}.json"))
            .exists(),
        "{}\nadmission:{}\npreparation:{}\nlifecycle:{}",
        rig.log("retention-output"),
        rig.log(&format!("state/admissions/{id}.json")),
        rig.log(&format!("state/preparation-receipts/{id}.json")),
        rig.log(&format!("state/placement-grants/{id}.lifecycle.json"))
    );
    assert!(!rig.dir.join(format!("state/admissions/{id}.json")).exists());
    assert!(
        !rig.dir
            .join(format!("state/placement-grants/{id}.json"))
            .exists()
    );
}

#[test]
fn r1a_abandoned_acknowledged_unit_expires_without_recovery_reenqueue() {
    let rig = unit_requests::rig("r1-abandoned", false);
    let execution = unit_requests::prepare(&rig);
    let id = execution["requestId"].as_str().unwrap();
    let (_, abandoned) =
        unit_requests::operation(&rig, "abandon-prepared", &json!({"execution":execution}));
    assert_eq!(
        unit_requests::operation(
            &rig,
            "ack-settlement",
            &json!({"execution":execution,"eventId":abandoned["eventId"]})
        )
        .0,
        0
    );
    let finished = unit_requests::admission(&rig, &execution)["finished_ms"]
        .as_u64()
        .unwrap();
    clock(&rig, finished + WINDOW);
    serve(&rig);
    assert!(rig.dir.join(format!("state/admissions/{id}.json")).exists());
    clock(&rig, finished + WINDOW + 1);
    serve(&rig);
    assert!(!rig.dir.join(format!("state/admissions/{id}.json")).exists());
    assert_eq!(
        rig.json(&[
            "unit",
            "observe",
            "--input",
            &json!({"execution":execution}).to_string()
        ])
        .0,
        2
    );
    serve(&rig);
    if rig.dir.join("state/queue.json").exists() {
        assert_eq!(rig.read("state/queue.json"), json!([]));
    }
}
