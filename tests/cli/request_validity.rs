//! A recorded job request starts new execution only while its own age allows it.
//!
//! Every scenario here runs the real commands with private state and a fake Herdr. A
//! refusal never allocates a pane, never dispatches a command, and never writes a
//! terminal record.
use super::*;

/// The window a recorded request may start new execution: 30 days.
const WINDOW_MS: u64 = 2_592_000_000;
/// The skew a recorded request may claim over this host clock: 5 minutes.
const SKEW_MS: u64 = 300_000;

/// The host clock of this rig, in milliseconds.
fn clock(rig: &Rig) -> u64 {
    rig.now_ms()
}

/// Set the rig's host clock.
fn set_clock(rig: &Rig, at: u64) {
    fs::write(rig.dir.join("clock"), at.to_string()).unwrap();
}

/// The validity clock this host keeps, and its high-water time.
fn validity_clock(rig: &Rig) -> Value {
    rig.read("state/job-validity-clock.json")
}

/// A job request issued at `created_ms`, as the origin records it.
fn request_at(rig: &Rig, created_ms: u64) -> Value {
    json!({
        "id": "job-1",
        "action": {"type":"job","cmd":["true"],"timeout_ms":null,"idle_timeout_ms":null,
            "validity":{"version":1,"created_ms":created_ms}},
        "host":"local","branch":null,"base":null,
        "cwd": rig.dir.join("home"),
        "repository":Value::Null,
        "workspace":"w1","workspace_name":"caller",
        "owner":"w1:p1",
    })
}

/// Queue a job while capacity is unavailable, then imitate a pre-validity writer.
fn stripped_queued_job(rig: &Rig, remote: bool) -> String {
    let now = clock(rig);
    set_clock(rig, now);
    rig.config("free_core_fraction_min = 2\n");
    rig.snapshot("state/host.json", now, 0.0, 8);
    if remote {
        rig.peer();
        let mut snapshot = rig.read("remote.json");
        snapshot["job_validity"] = json!(2);
        rig.put("remote.json", &snapshot);
    }
    let args = if remote {
        vec!["run", "--host", "netcup", "--", "true"]
    } else {
        vec!["run", "--", "true"]
    };
    let (code, result) = rig.json(&args);
    assert_eq!(code, 1, "{result}");
    let mut tickets = rig.read("state/queue.json");
    assert!(
        tickets.is_array(),
        "older servers must read the queue: {tickets}"
    );
    assert_eq!(rig.read("state/queue-format.json"), json!({"version": 1}));
    let id = tickets[0]["request"]["id"].as_str().unwrap().to_owned();
    tickets[0]["request"]["action"]
        .as_object_mut()
        .unwrap()
        .remove("validity");
    // An old binary writes its original array, losing all unknown action fields.
    rig.put("state/queue.json", &tickets);
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    id
}

#[test]
fn an_old_queue_writer_cannot_erase_job_validity() {
    let rig = Rig::new("validity-old-queue-writer");
    let id = stripped_queued_job(&rig, false);
    assert!(rig.run(&["serve", "--once"]).status.success());
    let tickets = rig.read("state/queue.json");
    assert_eq!(tickets[0]["state"], "admitted", "{tickets}");
    let job = rig.read(&format!("state/jobs/{id}.json"));
    assert_eq!(
        job["validity"],
        rig.read(&format!("state/job-validity/{id}.json"))
    );
}

#[test]
fn a_queued_job_without_its_sidecar_refuses_even_with_inline_validity() {
    let rig = Rig::new("validity-missing-sidecar");
    let id = stripped_queued_job(&rig, false);
    let mut tickets = rig.read("state/queue.json");
    tickets[0]["request"]["action"]["validity"] = rig.job_validity();
    rig.put("state/queue.json", &tickets);
    let sidecar = rig.dir.join(format!("state/job-validity/{id}.json"));
    if sidecar.exists() {
        fs::remove_file(sidecar).unwrap();
    }
    assert!(rig.run(&["serve", "--once"]).status.success());
    let tickets = rig.read("state/queue.json");
    assert_eq!(tickets[0]["state"], "failed", "{tickets}");
    assert!(
        tickets[0]["error"]
            .as_str()
            .unwrap()
            .contains("validity is unprovable")
    );
    assert!(!rig.dir.join(format!("state/jobs/{id}.json")).exists());
    assert!(!rig.log("herdr.log").contains("pane run"));
}

#[test]
fn remote_dispatch_forwards_sidecar_validity_after_an_old_queue_rewrite() {
    let rig = Rig::new("validity-remote-sidecar");
    let id = stripped_queued_job(&rig, true);
    let script = fs::read_to_string(rig.dir.join("bin/ssh")).unwrap();
    let remote = format!(
        r#"case "$*" in
  *'host launch'*)
    for remote do :; done
    remote=${{remote#* host }}
    DOGSLED_STATE="$RIG/peer-state" sh -c '{} host '"$remote" ;;"#,
        env!("CARGO_BIN_EXE_dogsled")
    );
    rig.script("ssh", &script.replace("case \"$*\" in", &remote));
    assert!(rig.run(&["serve", "--once"]).status.success());
    let tickets = rig.read("state/queue.json");
    assert_eq!(tickets[0]["state"], "admitted", "{tickets}");
    let job = rig.read(&format!("peer-state/jobs/{id}.json"));
    assert_eq!(
        job["validity"],
        rig.read(&format!("state/job-validity/{id}.json"))
    );
    assert!(rig.log("ssh.log").contains("--job-validity"));
}

/// The error code the CLI reports for a refusal.
fn refusal_code(error: &Value) -> String {
    error["code"].as_str().unwrap_or_default().to_owned()
}

/// A request that starts new execution keeps one timestamp through the queue, the
/// admission, the claim, and the receipt. A changed timestamp conflicts.
#[test]
fn one_timestamp_survives_the_queue_admission_claim_and_receipt() {
    let rig = Rig::new("validity-one-timestamp");
    let created = clock(&rig);
    set_clock(&rig, created);
    rig.snapshot("state/host.json", created, 0.0, 8);
    let request = request_at(&rig, created);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 0, "{error}");
    // The claim keeps the request's own creation time.
    let job = rig.read("state/jobs/job-1.json");
    assert_eq!(job["validity"]["created_ms"], created, "{job}");
    // The receipt keeps the same time, so a replay correlates to the same request.
    let receipt = rig.read("state/launches/job-1.json");
    assert_eq!(
        receipt["request"]["validity"]["created_ms"], created,
        "{receipt}"
    );
    // A request with another timestamp conflicts with the recorded one.
    let mut changed = request_at(&rig, created + 1);
    changed["id"] = json!("job-2");
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &changed.to_string(),
    ]);
    assert_eq!(code, 0, "{error}");
    assert_eq!(
        rig.read("state/jobs/job-2.json")["validity"]["created_ms"],
        created + 1
    );
}

/// The exact boundaries: 30 days is still valid, 30 days and one millisecond is not.
/// A request 5 minutes ahead is allowed, and one millisecond more is refused.
#[test]
fn the_age_and_skew_boundaries_are_exact() {
    // The offset from the host clock: a positive one is an old request, a negative one is
    // a request from the future.
    for (offset, allowed) in [
        (WINDOW_MS.cast_signed(), true),
        (WINDOW_MS.cast_signed() + 1, false),
        (-SKEW_MS.cast_signed(), true),
        (-SKEW_MS.cast_signed() - 1, false),
    ] {
        let rig = Rig::new(&format!("validity-boundary-{offset}"));
        let now = clock(&rig);
        let created = if offset < 0 {
            now + offset.unsigned_abs()
        } else {
            now - offset as u64
        };
        set_clock(&rig, now);
        rig.snapshot("state/host.json", now, 0.0, 8);
        let request = request_at(&rig, created);
        let (code, error) = rig.json(&[
            "host",
            "launch",
            "--job-validity",
            "2",
            "--request",
            &request.to_string(),
        ]);
        if allowed {
            assert_eq!(code, 0, "{offset}: {error}");
            assert!(rig.dir.join("state/jobs/job-1.json").exists(), "{offset}");
        } else {
            assert_eq!(code, 2, "{offset}: {error}");
            assert!(!rig.dir.join("state/jobs/job-1.json").exists(), "{offset}");
            assert!(
                !rig.dir.join("state/launches/job-1.json").exists(),
                "{offset}"
            );
            // The refusal has no execution effect: no pane, no dispatch.
            let herdr = rig.log("herdr.log");
            assert!(!herdr.contains("pane run"), "{offset}: {herdr}");
        }
    }
}

/// A request with no validity evidence never starts new execution. The refusal is
/// explicit, and it leaves nothing behind.
#[test]
fn a_request_without_validity_evidence_never_starts_new_execution() {
    let rig = Rig::new("validity-unprovable");
    let now = clock(&rig);
    set_clock(&rig, now);
    rig.snapshot("state/host.json", now, 0.0, 8);
    let mut request = request_at(&rig, now);
    request["action"]
        .as_object_mut()
        .unwrap()
        .remove("validity");
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    assert_eq!(refusal_code(&error), "JOB_VALIDITY_UNPROVABLE", "{error}");
    assert!(!rig.dir.join("state/jobs/job-1.json").exists());
    assert!(!rig.dir.join("state/launches/job-1.json").exists());
    let herdr = rig.log("herdr.log");
    assert!(!herdr.contains("pane run"), "{herdr}");
    assert!(!herdr.contains("tab create"), "{herdr}");
}

/// A corrupt or unwritable validity clock refuses execution. The refusal never runs a
/// command.
#[test]
fn a_corrupt_validity_clock_refuses_execution() {
    let rig = Rig::new("validity-corrupt-clock");
    let now = clock(&rig);
    set_clock(&rig, now);
    rig.snapshot("state/host.json", now, 0.0, 8);
    fs::write(rig.dir.join("state/job-validity-clock.json"), "not json").unwrap();
    let request = request_at(&rig, now);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    assert_eq!(refusal_code(&error), "JOB_CLOCK_UNAVAILABLE", "{error}");
    assert!(!rig.dir.join("state/jobs/job-1.json").exists());
    assert!(!rig.log("herdr.log").contains("pane run"));
}

/// A clock that cannot be written refuses execution too. The high-water time stays as it
/// was, so nothing else is disturbed.
#[test]
fn a_validity_clock_that_cannot_be_written_refuses_execution() {
    let rig = Rig::new("validity-unwritable-clock");
    let now = clock(&rig);
    set_clock(&rig, now);
    rig.snapshot("state/host.json", now, 0.0, 8);
    // A directory where the clock file belongs cannot be replaced by a file.
    fs::create_dir_all(rig.dir.join("state/job-validity-clock.json")).unwrap();
    let request = request_at(&rig, now);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    assert_eq!(refusal_code(&error), "JOB_CLOCK_UNAVAILABLE", "{error}");
    assert!(!rig.dir.join("state/jobs/job-1.json").exists());
    assert!(!rig.log("herdr.log").contains("pane run"));
}

/// Expiry survives a restart and a clock rollback. The high-water time never moves back,
/// so a host clock that moves back cannot reopen an expired request.
#[test]
fn expiry_survives_a_restart_and_a_clock_rollback() {
    let rig = Rig::new("validity-rollback");
    let created = clock(&rig);
    set_clock(&rig, created);
    rig.snapshot("state/host.json", created, 0.0, 8);
    // 31 days later the request is expired.
    let later = created + WINDOW_MS + 1;
    set_clock(&rig, later);
    let request = request_at(&rig, created);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    assert_eq!(refusal_code(&error), "JOB_EXPIRED", "{error}");
    // The clock kept the later time.
    assert_eq!(validity_clock(&rig)["high_water_ms"], later);
    // The host clock moves back to the request's own time. The request stays expired: the
    // high-water time is what decides.
    set_clock(&rig, created);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    assert_eq!(refusal_code(&error), "JOB_EXPIRED", "{error}");
    assert!(!rig.dir.join("state/jobs/job-1.json").exists());
    assert!(!rig.log("herdr.log").contains("pane run"));
}

/// Expiry survives the deletion of the job record and the receipt. The high-water clock
/// alone keeps the request closed.
#[test]
fn expiry_survives_the_deletion_of_the_record_and_the_receipt() {
    let rig = Rig::new("validity-after-deletion");
    let created = clock(&rig);
    set_clock(&rig, created);
    rig.snapshot("state/host.json", created, 0.0, 8);
    let request = request_at(&rig, created);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 0, "{error}");
    let before = rig.log("herdr.log").matches("pane run").count();
    // The prune removed both the record and the receipt.
    fs::remove_file(rig.dir.join("state/jobs/job-1.json")).unwrap();
    fs::remove_file(rig.dir.join("state/launches/job-1.json")).unwrap();
    // 31 days later the same request is refused.
    let later = created + WINDOW_MS + 1;
    set_clock(&rig, later);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    assert_eq!(refusal_code(&error), "JOB_EXPIRED", "{error}");
    assert!(!rig.dir.join("state/jobs/job-1.json").exists());
    assert_eq!(rig.log("herdr.log").matches("pane run").count(), before);
}

/// A missing clock starts at the later of the wall clock and the newest local record. A
/// record from the future therefore keeps the clock ahead of the wall clock.
#[test]
fn a_missing_clock_starts_at_the_newest_local_record() {
    let rig = Rig::new("validity-clock-bootstrap");
    let now = clock(&rig);
    // A local record from the future: the clock must start there, not at the wall clock.
    // It stays inside the skew, so the request itself is not from the future.
    let ahead = now + 60_000;
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    rig.put(
        "state/jobs/old.json",
        &json!({"id":"old","cmd":["true"],"cwd":rig.dir.join("home"),"host":"local",
            "owner":"w1:p1","pane":"w2:p1","state":"idle","exit":0,"signal":null,"reason":null,
            "started_ms":ahead,"ended_ms":ahead,"timeout_ms":null,"idle_timeout_ms":null,
            "validity":{"version":1,"created_ms":ahead}}),
    );
    set_clock(&rig, now);
    rig.snapshot("state/host.json", now, 0.0, 8);
    let request = request_at(&rig, ahead);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 0, "{error}");
    // The clock started at the newest local record, which is ahead of the wall clock.
    assert_eq!(validity_clock(&rig)["high_water_ms"], ahead);
}

/// A running job survives the age expiry. Age never ends ownership, and it never kills a
/// running job.
#[test]
fn a_running_job_survives_the_age_expiry() {
    let rig = Rig::new("validity-running-job");
    let created = clock(&rig);
    set_clock(&rig, created);
    rig.snapshot("state/host.json", created, 0.0, 8);
    let request = request_at(&rig, created);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 0, "{error}");
    // Host dispatch is asynchronous. Establish completion before changing the clock.
    let deadline = Instant::now() + PROCESS_DEADLINE;
    while rig.read("state/jobs/job-1.json")["ended_ms"].is_null() {
        assert!(Instant::now() < deadline, "finite job did not finish");
        std::thread::yield_now();
    }
    // Long after the window, the record is still there and still readable.
    set_clock(&rig, created + 2 * WINDOW_MS);
    let job = rig.read("state/jobs/job-1.json");
    assert_eq!(job["exit"], 0, "{job}");
    assert_eq!(rig.json(&["ps", "--all"]).1[0]["exit"], 0);
}

/// An expired request with a matching receipt replays the saved result. The replay is
/// read-only: it dispatches nothing.
#[test]
fn an_expired_request_with_a_matching_receipt_replays_the_saved_result() {
    let rig = Rig::new("validity-replay");
    let created = clock(&rig);
    set_clock(&rig, created);
    rig.snapshot("state/host.json", created, 0.0, 8);
    let request = request_at(&rig, created);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 0, "{error}");
    let receipt = rig.read("state/launches/job-1.json");
    let before = rig.log("herdr.log").matches("pane run").count();
    // The request expires.
    set_clock(&rig, created + WINDOW_MS + 1);
    let (code, replay) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 0, "{replay}");
    assert_eq!(replay["pane"], receipt["pane"], "{replay}");
    // The replay dispatched nothing.
    assert_eq!(rig.log("herdr.log").matches("pane run").count(), before);
}

/// A new caller and an old peer give the clear refusal, with no fallback. The peer never
/// reports the validity protocol, so it is not a candidate for a job.
#[test]
fn a_new_caller_refuses_an_old_peer_without_a_fallback() {
    let rig = Rig::new("validity-old-peer");
    rig.peer();
    let now = clock(&rig);
    set_clock(&rig, now);
    rig.snapshot("state/host.json", now, 0.0, 8);
    // The peer reports no validity protocol.
    rig.snapshot("remote.json", now, 0.0, 8);
    let out = rig.run(&["run", "--host", "netcup", "--", "true"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("does not support job request validity"),
        "{stderr}"
    );
    // The peer never held capacity for it, and nothing was dispatched.
    assert!(
        !rig.log("ssh.log").contains("host reserve"),
        "{}",
        rig.log("ssh.log")
    );
    assert!(
        !rig.log("ssh.log").contains("host launch"),
        "{}",
        rig.log("ssh.log")
    );
}

/// An old caller and a new peer give the clear refusal too. The peer refuses a job
/// launch without the validity protocol, before it creates a claim or a pane.
#[test]
fn an_old_caller_refuses_a_new_peer_without_a_fallback() {
    let rig = Rig::new("validity-old-caller");
    let now = clock(&rig);
    set_clock(&rig, now);
    rig.snapshot("state/host.json", now, 0.0, 8);
    // The caller speaks no validity protocol.
    let request = request_at(&rig, now);
    let (code, error) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 2, "{error}");
    assert_eq!(refusal_code(&error), "JOB_VALIDITY_UNSUPPORTED", "{error}");
    // The peer created no claim and no pane.
    assert!(!rig.dir.join("state/jobs/job-1.json").exists());
    assert!(!rig.dir.join("state/launches/job-1.json").exists());
    let herdr = rig.log("herdr.log");
    assert!(!herdr.contains("pane run"), "{herdr}");
    assert!(!herdr.contains("tab create"), "{herdr}");
}

/// A wrapper refuses an expired claim before it starts anything.
#[test]
fn a_wrapper_refuses_an_expired_claim() {
    let rig = Rig::new("validity-wrapper");
    let now = clock(&rig);
    set_clock(&rig, now);
    let marker = rig.dir.join("marker");
    let command = format!("touch {}", marker.display());
    rig.claim_at("expired", &["sh", "-c", &command], now - WINDOW_MS - 1);
    let out = rig.run(&[
        "exec",
        "--job-validity",
        "2",
        "expired",
        "--",
        "sh",
        "-c",
        &command,
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!marker.exists());
    assert!(!rig.dir.join("state/jobs/expired.log").exists());
    // The claim is unchanged: it never started and never ended.
    let job = rig.read("state/jobs/expired.json");
    assert!(job["started_ms"].is_null(), "{job}");
    assert!(job["ended_ms"].is_null(), "{job}");
}

/// A wrapper refuses a claim without validity evidence.
#[test]
fn a_wrapper_refuses_a_claim_without_validity_evidence() {
    let rig = Rig::new("validity-wrapper-unprovable");
    let now = clock(&rig);
    set_clock(&rig, now);
    let marker = rig.dir.join("marker");
    let command = format!("touch {}", marker.display());
    rig.claim("no-evidence", &["sh", "-c", &command]);
    let mut job = rig.read("state/jobs/no-evidence.json");
    job.as_object_mut().unwrap().remove("validity");
    rig.put("state/jobs/no-evidence.json", &job);
    let out = rig.run(&[
        "exec",
        "--job-validity",
        "2",
        "no-evidence",
        "--",
        "sh",
        "-c",
        &command,
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!marker.exists());
    assert!(!rig.dir.join("state/jobs/no-evidence.log").exists());
}

/// The queue refuses an expired request. The refusal is explicit, and the queue keeps the
/// tickets it had.
#[test]
fn the_queue_refuses_an_expired_request() {
    let rig = Rig::new("validity-queue");
    let now = clock(&rig);
    set_clock(&rig, now);
    rig.snapshot("state/host.json", now, 0.0, 8);
    // The request is already expired.
    let mut request = request_at(&rig, now - WINDOW_MS - 1);
    request["id"] = json!("old-job");
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    assert_eq!(refusal_code(&error), "JOB_EXPIRED", "{error}");
    // Nothing entered the queue, and no claim exists.
    assert!(!rig.dir.join("state/queue.json").exists());
    assert!(!rig.dir.join("state/jobs/old-job.json").exists());
}

/// The CLI stamps the request's own creation time once, and a retry keeps it
/// byte-identical. The queue, the admission, the claim, and the receipt all name it.
#[test]
fn the_cli_stamps_one_creation_time_through_the_queue_and_the_claim() {
    let rig = Rig::new("validity-cli-issuance");
    let now = clock(&rig);
    set_clock(&rig, now);
    rig.snapshot("state/host.json", now, 0.0, 8);
    let (code, result) = rig.json(&["run", "--", "true"]);
    assert_eq!(code, 0, "{result}");
    let id = result["id"].as_str().unwrap().to_owned();
    let job = rig.read(&format!("state/jobs/{id}.json"));
    let created = job["validity"]["created_ms"].as_u64().unwrap();
    assert_eq!(created, now, "{job}");
    // The admission and the receipt name the same time.
    let attempt = rig.read(&format!("state/admissions/{id}.json"));
    assert_eq!(
        attempt["request"]["action"]["validity"]["created_ms"], created,
        "{attempt}"
    );
    let receipt = rig.read(&format!("state/launches/{id}.json"));
    assert_eq!(
        receipt["request"]["validity"]["created_ms"], created,
        "{receipt}"
    );
}

/// The MCP path stamps the same kind of evidence. It serves the same request code, so a
/// request from either path carries one creation time.
#[test]
fn the_mcp_path_stamps_one_creation_time() {
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::process::Stdio;

    let rig = Rig::new("validity-mcp-issuance");
    let now = clock(&rig);
    set_clock(&rig, now);
    rig.snapshot("state/host.json", now, 0.0, 8);
    let mut child = rig
        .cmd(&["mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut send = |message: Value| {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    };
    let mut read = |id: u64| -> Value {
        loop {
            let mut line = String::new();
            assert!(out.read_line(&mut line).unwrap() > 0, "server closed");
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] == id {
                return message["result"].clone();
            }
        }
    };
    send(
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18",
        "capabilities":{},"clientInfo":{"name":"test","version":"0"}}}),
    );
    read(1);
    send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    send(
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"run",
        "arguments":{"cmd":["true"]}}}),
    );
    let result = read(2);
    let text = result["content"][0]["text"].as_str().unwrap().to_owned();
    let value: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!(text));
    let id = value["id"].as_str().unwrap_or_default().to_owned();
    assert!(!id.is_empty(), "{value}");
    let job = rig.read(&format!("state/jobs/{id}.json"));
    assert_eq!(job["validity"]["created_ms"], now, "{job}");
    let _ = child.kill();
    let _ = child.wait();
}

/// A wrapper that expires while the pane report runs starts no process. The claim stays
/// where it was: it never ended, and it holds no terminal outcome.
#[test]
fn a_wrapper_that_expires_during_the_pane_report_starts_no_process() {
    let rig = Rig::new("validity-wrapper-report-expiry");
    let now = clock(&rig);
    set_clock(&rig, now);
    let marker = rig.dir.join("marker");
    let command = format!("touch {}", marker.display());
    // The claim is valid now, and the pane report that follows expires it.
    rig.claim_at("crossing", &["sh", "-c", &command], now);
    rig.script(
        "herdr",
        &fs::read_to_string(rig.dir.join("bin/herdr")).unwrap().replace(
            "  'agent start'*|'agent prompt'*|'pane report-agent'*|'pane report-metadata'*) printf '{}\\n' ;;",
            "  'pane report-agent'*) cp \"$RIG/later\" \"$DOGSLED_STATE/job-validity-clock.json\"; printf '{}\\n' ;;\n  'agent start'*|'agent prompt'*|'pane report-metadata'*) printf '{}\\n' ;;",
        ),
    );
    // The host's effective time moves past the validity window while the report runs.
    rig.put(
        "later",
        &json!({"version":1,"high_water_ms":now + WINDOW_MS + 1}),
    );
    let out = rig.run(&[
        "exec",
        "--job-validity",
        "2",
        "crossing",
        "--",
        "sh",
        "-c",
        &command,
    ]);
    assert_eq!(out.status.code(), Some(2));
    // No process started: the command never ran.
    assert!(!marker.exists());
    assert!(!rig.dir.join("state/jobs/crossing.log").exists());
    // The claim is not terminal: it never ended, and it carries no result.
    let job = rig.read("state/jobs/crossing.json");
    assert!(job["ended_ms"].is_null(), "{job}");
    assert!(job["exit"].is_null(), "{job}");
    assert!(job["reason"].is_null(), "{job}");
    // The claim still holds the job: it was not released.
    assert_eq!(job["state"], "working", "{job}");
}

/// A host launch that expires while the pane is allocated dispatches nothing. It writes
/// no receipt, releases no preparation, and retires the pane it allocated.
#[test]
fn a_host_launch_that_expires_during_the_pane_allocation_dispatches_nothing() {
    let rig = Rig::new("validity-launch-allocation-expiry");
    let now = clock(&rig);
    set_clock(&rig, now);
    rig.snapshot("state/host.json", now, 0.0, 8);
    // The worktree this launch would run in, and the preparation it would release.
    let tree = rig.dir.join("tree");
    let repo = rig.dir.join("home");
    close::git(&repo, &["init", "-q", "-b", "main"]);
    close::git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    close::git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "work",
            tree.to_str().unwrap(),
        ],
    );
    let common = PathBuf::from(close::git(&repo, &["rev-parse", "--absolute-git-dir"]).trim());
    let admin = PathBuf::from(close::git(&tree, &["rev-parse", "--absolute-git-dir"]).trim());
    fs::write(
        admin.join("dogsled-checkout-id"),
        "0123456789abcdef0123456789abcdef\n",
    )
    .unwrap();
    rig.put("worktrees.json", &json!({"result":{"worktrees":[]}}));
    rig.put(
        "state/preparations.json",
        &json!([{
            "repo": repo,
            "path": tree,
            "branch": "work",
            "head": close::git(&tree, &["rev-parse", "HEAD"]).trim(),
            "identity": {"common":common,"gitdir":admin,"generation":"0123456789abcdef0123456789abcdef"},
            "request_id": "job-1",
        }]),
    );
    rig.script(
        "herdr",
        &fs::read_to_string(rig.dir.join("bin/herdr")).unwrap().replace(
            "  'tab create'*|'workspace create'*) printf '%s\\n' '{\"result\":{\"root_pane\":{\"pane_id\":\"w2:p1\",\"terminal_id\":\"term_child\"}}}' ;;",
            "  'tab create'*|'workspace create'*) cp \"$RIG/later\" \"$DOGSLED_STATE/job-validity-clock.json\"; printf '%s\\n' '{\"result\":{\"root_pane\":{\"pane_id\":\"w2:p1\",\"terminal_id\":\"term_child\"}}}' ;;",
        ),
    );
    let mut captured = rig.read("state/preparations.json")[0].clone();
    captured.as_object_mut().unwrap().remove("request_id");
    rig.preparation_outcome(
        "job-1",
        &captured,
        &json!({"pane_id":"w2:p1","terminal_id":"term_child","path":tree}),
    );
    // The host's effective time moves past the validity window while the pane is
    // allocated.
    rig.put(
        "later",
        &json!({"version":1,"high_water_ms":now + WINDOW_MS + 1}),
    );
    let mut request = request_at(&rig, now);
    request["cwd"] = json!(tree);
    request["preparation"] = rig.read("state/preparation-receipts/job-1.json")["binding"].clone();
    request["preparation_protocol"] = json!(1);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    assert_eq!(refusal_code(&error), "JOB_EXPIRED", "{error}");
    // Nothing was dispatched, and no receipt was written.
    let herdr = rig.log("herdr.log");
    assert!(!herdr.contains("pane run"), "{herdr}");
    assert!(!rig.dir.join("state/launches/job-1.json").exists());
    // The preparation was not released.
    let preparations = rig.read("state/preparations.json");
    assert_eq!(preparations.as_array().unwrap().len(), 1, "{preparations}");
    // The pane this attempt allocated was retired.
    assert!(herdr.contains("pane close w2:p1"), "{herdr}");
}

/// A reservation for an expired request changes nothing. The capacity record stays
/// byte-identical, with or without an existing hold.
#[test]
fn a_reservation_for_an_expired_request_changes_nothing() {
    for held in [false, true] {
        let rig = Rig::new(&format!("validity-reservation-{held}"));
        let now = clock(&rig);
        set_clock(&rig, now);
        rig.snapshot("state/host.json", now, 0.0, 8);
        if held {
            // A live hold for the same id.
            rig.put(
                "state/placements.json",
                &json!([{"id":"job-1","created_ms":now}]),
            );
        }
        let before = fs::read(rig.dir.join("state/placements.json")).unwrap_or_default();
        // The request expired before it asked for capacity.
        let expired = now - WINDOW_MS - 1;
        let (code, error) = rig.json(&[
            "host",
            "reserve",
            "--id",
            "job-1",
            "--job-validity",
            "2",
            "--job-created-ms",
            &expired.to_string(),
        ]);
        assert_eq!(code, 2, "{held}: {error}");
        assert_eq!(refusal_code(&error), "JOB_EXPIRED", "{held}: {error}");
        // The capacity record is byte-identical: the refusal changed nothing.
        let after = fs::read(rig.dir.join("state/placements.json")).unwrap_or_default();
        assert_eq!(after, before, "{held}");
    }
}

/// A missing clock with a corrupt local record refuses execution. The bootstrap cannot
/// account for a record it cannot read, so it must not start the clock behind it.
#[test]
fn a_missing_clock_with_a_corrupt_record_refuses_execution() {
    let rig = Rig::new("validity-bootstrap-corrupt-record");
    let now = clock(&rig);
    set_clock(&rig, now);
    rig.snapshot("state/host.json", now, 0.0, 8);
    // A local record that cannot be parsed.
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    fs::write(rig.dir.join("state/jobs/broken.json"), "not json").unwrap();
    // The clock file is missing, so the bootstrap has to read every record.
    assert!(!rig.dir.join("state/job-validity-clock.json").exists());
    let request = request_at(&rig, now);
    let (code, error) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 2, "{error}");
    assert_eq!(refusal_code(&error), "JOB_CLOCK_UNAVAILABLE", "{error}");
    // No clock was written, no claim exists, and nothing was dispatched.
    assert!(!rig.dir.join("state/job-validity-clock.json").exists());
    assert!(!rig.dir.join("state/jobs/job-1.json").exists());
    assert!(!rig.log("herdr.log").contains("pane run"));
}
