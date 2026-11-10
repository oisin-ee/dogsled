//! Strict owner-proven retention through the real service and prune commands.
use super::*;

const DAY: u64 = 86_400_000;
const WINDOW: u64 = 30 * DAY;

fn clock(rig: &Rig, now: u64) {
    fs::write(rig.dir.join("clock"), now.to_string()).unwrap();
}

fn serve(rig: &Rig) {
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    rig.snapshot("state", rig.now_ms(), 0.0, 8);
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(out.status.success(), "{out:?}");
    fs::write(rig.dir.join("retention-output"), &out.stderr).unwrap();
}

fn job(rig: &Rig, id: &str, ended: u64) {
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    rig.put_job(id, &json!({
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
    assert!(!rig.job("old").is_null());
    assert_eq!(rig.job("old")["output_expired"], true);
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
    rig.put_items(&json!([item, held, open]));
    let mark = json!({"pane":"finished","item":"finished","session":"old",
        "marked_ms":ended-1,"sealed":1,"closed_ms":ended,"native_done":true,
        "reply":{"seq":1,"text":"final","captured_ms":ended,"done":true}});
    rig.put_done(&json!({"[\"finished\",\"finished\",\"old\"]":mark}));
    clock(&rig, ended + WINDOW);
    serve(&rig);
    assert_eq!(rig.items().as_array().unwrap().len(), 3);
    assert_eq!(rig.state(&["done"]).as_object().unwrap().len(), 1);
    clock(&rig, ended + WINDOW + 1);
    serve(&rig);
    let items = rig.items();
    assert_eq!(items.as_array().unwrap().len(), 2, "{items}");
    assert!(
        !items
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == "finished")
    );
    assert_eq!(rig.state(&["done"]), json!({}));
}

#[test]
fn r1b_last_job_identity_expiry_never_resolves_a_reused_agent() {
    let rig = Rig::new("r1-no-tombstone");
    let ended = rig.now_ms();
    job(&rig, "r-old-job", ended);
    let mut item = closed_item("r-old-job", ended);
    item["kind"] = json!("job");
    item["name"] = json!("reused");
    rig.put_items(&json!([item]));
    clock(&rig, ended + WINDOW + 1);
    serve(&rig);
    assert!(rig.job("r-old-job").is_null());
    assert_eq!(rig.items(), json!([]));
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{
            "pane_id":"replacement","agent":"pi","name":"reused","agent_status":"idle"
        }]}}),
    );
    let _publisher = super::observation::resolved(&rig);
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
    let publisher = super::place::placement_observer(&rig);
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
    drop(publisher);
    let id = result["request_id"].as_str().unwrap();
    clock(&rig, ended + WINDOW + 1);
    serve(&rig);
    assert!(!rig.has_admission(id));
    for kind in ["grant", "lifecycle", "receipt", "disposition"] {
        assert!(
            !rig.preparation_record("state", kind, id).exists(),
            "{kind}"
        );
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
    rig.snapshot("state", rig.now_ms(), 0.0, 8);
    let mut snapshot = rig.host();
    snapshot["probe"]["agent_panes"] = json!([{
        "pane":"w2:p1","terminal_id":"term_child","agent":"pi","state":"idle","name":"live-agent"
    }]);
    rig.put_host(&snapshot);
    let _publisher = super::observation::resolved(&rig);
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
    let mut terminal = rig.job("ordinary");
    terminal["host"] = json!("remote-host");
    terminal["validity"] = json!({"version":1,"created_ms":ended});
    rig.put_job("ordinary", &terminal);
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
    rig.put_launch(
        "ordinary",
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
    assert!(!rig.launch_record("ordinary", "receipt").is_null());
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
    assert!(rig.launch_record("ordinary", "receipt").is_null());
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
    assert!(rig.launch_record("ordinary", "release").is_null());
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
        let mut items = rig.items();
        items[0]["closed_ms"] = json!(ended);
        items[0]["native_done"] = json!(true);
        items[0]["reply"]["done"] = json!(true);
        items[0]["reply"]["reported_ms"] = json!(ended);
        rig.put_items(&items);
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
            rig.admission(id),
            rig.launch_progress(id),
            rig.items()
        );
        assert_eq!(
            !rig.launch_record(id, "progress").is_null(),
            retained,
            "{held}"
        );
        assert_eq!(fs::read_to_string(unrelated).unwrap(), "untouched");
        assert!(rig.dir.join(format!("state/launches/{id}.lock")).exists());
        if !retained {
            assert!(!rig.has_admission(id));
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
    let publisher = super::place::placement_observer(&rig);
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
    drop(publisher);
    let id = result["request_id"].as_str().unwrap();
    let lifecycle = rig.preparation_record("state", "lifecycle", id);
    let proof = lifecycle.read();
    lifecycle.delete();
    clock(&rig, ended + WINDOW + 1);
    serve(&rig);
    assert!(rig.preparation_record("state", "grant", id).exists());
    lifecycle.put(&proof);
    serve(&rig);
    assert!(!rig.preparation_record("state", "grant", id).exists());
}

#[test]
fn r1a_deleted_grant_replay_without_expectation_never_allocates() {
    let (rig, repo) = place::place_rig("r1-grant-replay");
    let ended = rig.now_ms();
    clock(&rig, ended);
    let publisher = super::place::placement_observer(&rig);
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
    drop(publisher);
    let id = result["request_id"].as_str().unwrap();
    let grant = rig.preparation_record("state", "grant", id).read();
    clock(&rig, ended + WINDOW + 1);
    serve(&rig);
    let before = rig.log("herdr.log");
    let holds = rig.reservations();
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
        assert!(!rig.preparation_record("state", "grant", id).exists());
        assert_eq!(rig.reservations(), holds);
    }
    assert_eq!(rig.json(&["place", "--resume", id]).0, 2);
    assert_eq!(
        rig.log("herdr.log").matches("workspace create").count(),
        before.matches("workspace create").count()
    );
}

#[test]
fn unstarted_release_fence_expires_only_after_its_completion_window() {
    let rig = Rig::new("unstarted-retention");
    let issued = rig.now_ms();
    clock(&rig, issued);
    rig.snapshot("state", issued, 0.0, 8);
    let created = issued.to_string();
    let (code, result) = rig.json(&[
        "host",
        "reserve",
        "--id",
        "unstarted",
        "--job-validity",
        "2",
        "--job-created-ms",
        &created,
    ]);
    assert!([0, 3].contains(&code), "{result}");
    clock(&rig, issued + DAY);
    let proof = json!({"id":"unstarted","validity":{"version":1,"created_ms":issued},"owner":"w1:p1","owner_host":"origin"});
    let args = ["host", "release-unstarted", "--input", &proof.to_string()];
    let (code, acknowledgement) = rig.json(&args);
    assert_eq!(code, 0, "{acknowledgement}");
    assert_eq!(rig.json(&args).1, acknowledgement);
    let mut changed = proof.clone();
    changed["owner"] = json!("w9:p9");
    assert_eq!(
        rig.json(&["host", "release-unstarted", "--input", &changed.to_string()])
            .0,
        2
    );
    assert_eq!(rig.reservations(), json!([]));
    let ended = acknowledgement["finished_ms"].as_u64().unwrap();
    clock(&rig, ended + WINDOW);
    serve(&rig);
    assert!(!rig.validity("unstarted").is_null());
    let observer = rig.serve();
    super::wait_publication(&rig, |frame| frame["local"]["host"]["holds"] == json!([]));
    assert_eq!(rig.json(&["host", "snapshot"]).1["holds"], json!([]));
    drop(observer);
    assert_eq!(
        rig.json(&[
            "host",
            "reserve",
            "--id",
            "unstarted",
            "--job-validity",
            "2",
            "--job-created-ms",
            &created
        ])
        .0,
        2
    );
    clock(&rig, ended + WINDOW + 1);
    serve(&rig);
    assert!(rig.validity("unstarted").is_null());
    assert_eq!(rig.json(&args).0, 2);
    assert!(rig.validity("unstarted").is_null());
}

#[test]
fn r1a_unknown_cancelled_ticket_remains_unsettled_despite_age() {
    let rig = Rig::short("r1-ticket");
    let issued = rig.now_ms();
    clock(&rig, issued);
    rig.config("free_core_fraction_min = 2\n");
    rig.snapshot("state", issued, 0.0, 8);
    let publisher = super::observation::seeded(&rig);
    let (code, queued) = rig.json(&["agent", "--name", "cancelled", "--message", "test"]);
    assert_eq!(code, 1, "{queued}");
    drop(publisher);
    let ticket = queued["ticket"].as_str().unwrap();
    clock(&rig, issued + WINDOW - 1);
    let (code, result) = rig.json(&["cancel", ticket]);
    assert_eq!(code, 0, "{result}");
    let finished = rig.now_ms();
    clock(&rig, finished + WINDOW);
    serve(&rig);
    assert_eq!(rig.tickets().as_array().unwrap().len(), 1);
    clock(&rig, finished + WINDOW + 1);
    serve(&rig);
    assert_eq!(rig.tickets().as_array().unwrap().len(), 1);
    assert!(rig.tickets()[0]["finished_ms"].is_number());
}

#[test]
fn r1c_same_host_ordinary_job_captures_release_and_expires_without_owner_read() {
    let rig = Rig::short("r1-local-job");
    rig.snapshot("state", rig.now_ms(), 0.0, 8);
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    let publisher = rig.serve();
    wait_publication(&rig, |frame| {
        frame["views"]["hosts"]["data"][0]["headroom"]["eligible"] == true
    });
    let (code, result) = rig.json(&["run", "--host", "local", "--", "true"]);
    assert_eq!(code, 0, "{result}");
    let id = result["id"].as_str().unwrap();
    let deadline = Instant::now() + PROCESS_DEADLINE;
    loop {
        if rig.job(id)["ended_ms"].is_number() {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(publisher);
    serve(&rig);
    let attempt = rig.admission(id);
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
    assert!(rig.launch_record(id, "receipt").is_null());
    assert!(!rig.has_admission(id));
    assert_eq!(rig.items(), json!([]));
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
        !rig.preparation_record("state", "receipt", id).exists(),
        "{}\nadmission:{}\npreparation:{}\nlifecycle:{}",
        rig.log("retention-output"),
        rig.admission(id),
        rig.preparation_record("state", "receipt", id).read(),
        rig.preparation_record("state", "lifecycle", id).read()
    );
    assert!(!rig.has_admission(id));
    assert!(!rig.preparation_record("state", "grant", id).exists());
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
    assert!(rig.has_admission(id));
    clock(&rig, finished + WINDOW + 1);
    serve(&rig);
    assert!(!rig.has_admission(id));
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
    assert_eq!(rig.tickets(), json!([]));
}
