use super::*;

fn saved_grant(attempt: &Value) -> Value {
    let progress = &attempt["placement"];
    match progress["phase"].as_str().unwrap() {
        "host_selected" | "preparation_pending" => progress["grant"].clone(),
        "prepared" => progress["prepared"]["grant"].clone(),
        _ => progress["saved"]["prepared"]["grant"].clone(),
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
    assert!(!rig.dir.join("state/placement-grants").exists());
    assert!(!rig.dir.join("state/placements.json").exists());
    let log = rig.log("ssh.log");
    assert!(!log.contains("host reserve"), "{log}");
    assert!(!log.contains("host prepare"), "{log}");
    assert!(!log.contains("host launch"), "{log}");
}

fn placement_envelope(rig: &Rig, id: &str) -> Value {
    json!({"version":2,"expected_grant":null,"reservation":id,"host":{"id":"local","label":"local"},
        "request":{"id":id,"action":{"type":"place"},"host":null,"branch":null,"base":null,
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

fn real_reservation_peer(rig: &Rig, pause: bool) {
    rig.peer();
    fs::create_dir_all(rig.dir.join("peer-state")).unwrap();
    rig.snapshot("peer-state/host.json", rig.origin_ms, 0.0, 16);
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
    let mut child = rig.start(&["place", "--host", "netcup"]);
    assert!(read_fifo_within(ready.path(), "reserve ingress", PROCESS_DEADLINE).is_some());
    let files: Vec<_> = fs::read_dir(rig.dir.join("state/admissions"))
        .unwrap()
        .map(|f| f.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    assert_eq!(
        files.len(),
        1,
        "complete admission must precede reservation"
    );
    let attempt: Value = serde_json::from_slice(&fs::read(&files[0]).unwrap()).unwrap();
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
    let (code, queued) = rig.json(&["place", "--host", "netcup"]);
    assert_eq!(code, 1, "{queued}");
    let ticket = rig.read("state/queue.json")[0].clone();
    let id = ticket["request"]["id"].as_str().unwrap();
    let grant_path = format!("peer-state/placement-grants/{id}.json");
    let original = rig.read(&grant_path);
    let hold = rig.read("peer-state/placements.json");
    assert_eq!(original["reservation"], hold[0]);
    let retry = ticket["retry"]["next_ms"].as_u64().unwrap();
    let later = if after_ttl { retry + 100_000 } else { retry };
    fs::write(rig.dir.join("clock"), later.to_string()).unwrap();
    rig.snapshot("peer-state/host.json", later, 1000.0, 16);
    let output = rig.run(&["serve", "--once"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ticket = rig.read("state/queue.json")[0].clone();
    assert_eq!(ticket["state"], "admitted", "{ticket}");
    assert_eq!(rig.read(&grant_path), original);
    let holds = rig.read("peer-state/placements.json");
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
    let id = rig.read("state/queue.json")[0]["request"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let attempt = rig.read(&format!("state/admissions/{id}.json"));
    assert_eq!(attempt["placement"]["phase"], "host_selected");
    assert_eq!(
        attempt["placement"]["grant"],
        rig.read(&format!("peer-state/placement-grants/{id}.json"))
    );
    (rig, id)
}

#[test]
fn acknowledged_placement_grant_replays_after_real_ttl_without_readmission() {
    let (rig, id) = acknowledged_remote_placement("placement-ack-ttl");
    let grant_path = format!("peer-state/placement-grants/{id}.json");
    let grant = rig.read(&grant_path);
    let original_hold = rig.read("peer-state/placements.json");
    let later = original_hold[0]["created_ms"].as_u64().unwrap() + 100_000;
    fs::write(rig.dir.join("clock"), later.to_string()).unwrap();
    rig.snapshot("peer-state/host.json", later, 1000.0, 16);
    assert!(rig.run(&["serve", "--once"]).status.success());
    let ticket = rig.read("state/queue.json")[0].clone();
    assert_eq!(ticket["state"], "admitted", "{ticket}");
    assert_eq!(rig.read(&grant_path), grant);
    assert_eq!(
        saved_grant(&rig.read(&format!("state/admissions/{id}.json"))),
        grant
    );
    assert_eq!(rig.read("peer-state/placements.json"), original_hold);
    let log = rig.log("ssh.log");
    assert_eq!(log.matches("host reserve").count(), 1, "{log}");
    assert_eq!(log.matches("host prepare ").count(), 1, "{log}");
    assert!(!log.contains("host launch"), "{log}");
}

#[test]
fn acknowledged_placement_missing_grant_never_allocates_after_ttl() {
    let (rig, id) = acknowledged_remote_placement("placement-missing-grant");
    let grant_path = format!("peer-state/placement-grants/{id}.json");
    let grant = rig.read(&grant_path);
    let later = grant["reservation"]["created_ms"].as_u64().unwrap() + 100_000;
    fs::write(rig.dir.join("clock"), later.to_string()).unwrap();
    rig.snapshot("peer-state/host.json", later, 0.0, 16);
    rig.put("peer-state/placements.json", &json!([]));
    fs::remove_file(rig.dir.join(&grant_path)).unwrap();
    let before = rig.read("peer-state/placements.json");
    assert!(rig.run(&["serve", "--once"]).status.success());
    let ticket = rig.read("state/queue.json")[0].clone();
    assert_eq!(ticket["state"], "failed", "{ticket}");
    assert!(
        ticket["error"].as_str().unwrap().contains("changed"),
        "{ticket}"
    );
    assert_eq!(rig.log("ssh.log").matches("host reserve").count(), 1);
    assert!(!rig.dir.join(&grant_path).exists());
    assert_eq!(rig.read("peer-state/placements.json"), before);
    assert_eq!(
        saved_grant(&rig.read(&format!("state/admissions/{id}.json"))),
        grant
    );
    let log = rig.log("ssh.log");
    assert_eq!(log.matches("host checkout").count(), 1, "{log}");
    assert!(!log.contains("host prepare "), "{log}");
}

#[test]
fn queued_host_selected_recovers_the_acknowledged_grant() {
    let (rig, id) = acknowledged_remote_placement("placement-selected-recovery");
    let grant = rig.read(&format!("peer-state/placement-grants/{id}.json"));
    let mut queue = rig.read("state/queue.json");
    queue[0]["state"] = json!("admitting");
    let next = queue[0]["retry"]["next_ms"].as_u64().unwrap();
    rig.put("state/queue.json", &queue);
    fs::write(rig.dir.join("clock"), (next - 1).to_string()).unwrap();
    assert!(rig.run(&["serve", "--once"]).status.success());
    assert_eq!(rig.read("state/queue.json")[0]["state"], "pending");
    fs::write(rig.dir.join("clock"), next.to_string()).unwrap();
    assert!(rig.run(&["serve", "--once"]).status.success());
    let ticket = rig.read("state/queue.json")[0].clone();
    assert_eq!(ticket["state"], "admitted", "{ticket}");
    assert_eq!(
        saved_grant(&rig.read(&format!("state/admissions/{id}.json"))),
        grant
    );
    assert_eq!(rig.log("ssh.log").matches("host prepare ").count(), 1);
}

#[test]
fn placement_crash_boundaries_replay_one_result() {
    for boundary in [
        "placement-host-selected",
        "placement-preparation-received",
        "placement-result-saved",
        "placement-acknowledgement-saved",
        "placement-completed",
        "preparation-fenced",
        "preparation-claim-released",
    ] {
        let (rig, repo) = place_rig(boundary);
        let request = json!({"id":"queued-placement", "action":{"type":"place"}, "host":"local",
        "branch":"recover-placement", "base":"main", "cwd":repo, "owner":"w1:p1",
        "repository":{"path":repo,"working_root":repo,"relative":"repo space",
            "origin":"https://example.invalid/repo.git"}});
        rig.put(
            "state/queue.json",
            &json!([{"id":"placement-ticket", "request":request,
        "created_ms":rig.origin_ms, "state":"pending", "waiting":false, "notified":false,
        "retry":{"attempts":0,"next_ms":rig.origin_ms}}]),
        );
        rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
        let out = rig
            .cmd(&["serve", "--once"])
            .env("DOGSLED_TEST_UNIT_CRASH", boundary)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(86), "{out:?}");
        assert_eq!(rig.read("state/queue.json")[0]["state"], "admitting");
        let receipt = if boundary == "placement-host-selected" {
            assert!(
                !rig.dir
                    .join("state/preparation-receipts/queued-placement.json")
                    .exists()
            );
            assert!(!rig.dir.join("setup.log").exists());
            None
        } else {
            let receipt = rig.read("state/preparation-receipts/queued-placement.json");
            assert!(!receipt["prepared"].is_null());
            assert_eq!(rig.log("setup.log").lines().count(), 1);
            Some(receipt)
        };
        let grant = rig.read("state/placement-grants/queued-placement.json");
        assert!(rig.run(&["serve", "--once"]).status.success());
        let ticket = rig.read("state/queue.json")[0].clone();
        assert_eq!(ticket["state"], "admitted", "{ticket}");
        if let Some(receipt) = receipt {
            assert_eq!(
                rig.read("state/preparation-receipts/queued-placement.json"),
                receipt
            );
        }
        assert_eq!(
            rig.read("state/placement-grants/queued-placement.json"),
            grant
        );
        assert_eq!(
            rig.read("state/preparation-receipts/queued-placement.disposition.json")["progress"],
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
        let grant_path = format!("peer-state/placement-grants/{id}.json");
        let mut grant = rig.read(&grant_path);
        let mut holds = rig.read("peer-state/placements.json");
        let admission_path = format!("state/admissions/{id}.json");
        let admission = rig.read(&admission_path);
        if changed != "hold" {
            grant["reservation"]["created_ms"] = json!(1);
            rig.put(&grant_path, &grant);
        }
        if changed != "grant" {
            holds[0]["created_ms"] = json!(1);
            rig.put("peer-state/placements.json", &holds);
        }
        let next = rig.read("state/queue.json")[0]["retry"]["next_ms"]
            .as_u64()
            .unwrap();
        fs::write(rig.dir.join("clock"), next.to_string()).unwrap();
        assert!(rig.run(&["serve", "--once"]).status.success());
        let ticket = rig.read("state/queue.json")[0].clone();
        assert_eq!(ticket["state"], "failed", "{changed}: {ticket}");
        assert_eq!(rig.read(&admission_path), admission);
        assert_eq!(rig.read(&grant_path), grant);
        assert_eq!(rig.read("peer-state/placements.json"), holds);
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
    rig.snapshot("state/host.json", rig.origin_ms, 0.0, 8);
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
        !rig.dir
            .join("state/placement-grants/same-grant.json")
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
    assert_eq!(
        rig.read("state/placements.json").as_array().unwrap().len(),
        1
    );
}

#[test]
fn reservation_replay_rejects_changed_identity_and_old_peer() {
    let rig = Rig::short("placement-conflicts");
    rig.snapshot("state/host.json", rig.origin_ms, 0.0, 8);
    let original = placement_envelope(&rig, "same-grant");
    assert_eq!(reserve_delivery(&rig, &original).0, 0);
    let hold = rig.read("state/placements.json");
    let grant = rig.read("state/placement-grants/same-grant.json");
    for change in ["request", "target", "version", "missing"] {
        let mut changed = original.clone();
        match change {
            "request" => changed["request"]["cwd"] = json!("/different"),
            "target" => changed["host"]["target"] = json!("other-target"),
            "version" => changed["version"] = json!(3),
            "missing" => {
                changed.as_object_mut().unwrap().remove("request");
            }
            _ => unreachable!(),
        }
        assert_eq!(reserve_delivery(&rig, &changed).0, 2, "{change}");
        assert_eq!(rig.read("state/placements.json"), hold);
        assert_eq!(rig.read("state/placement-grants/same-grant.json"), grant);
    }
    let peer = Rig::short("placement-old-peer");
    peer.peer();
    let mut snapshot = peer.read("remote.json");
    snapshot.as_object_mut().unwrap().remove("placement_replay");
    peer.put("remote.json", &snapshot);
    let (code, error) = peer.json(&["place", "--host", "netcup"]);
    assert_eq!(code, 2, "{error}");
    assert!(!peer.log("ssh.log").contains("host reserve"));
    assert!(!peer.log("ssh.log").contains("host prepare"));
    // A peer advertising support still needs to return the original correlation.
    let unsupported = Rig::short("placement-missing-reply");
    unsupported.peer();
    let script = fs::read_to_string(unsupported.dir.join("bin/ssh")).unwrap();
    unsupported.script("ssh", &script.replace("  *'host reserve'*)\n    for remote do :; done\n    sh -c \"$RIG/bin/peer-fixture $remote\" ;;", "  *'host reserve'*) cat \"$RIG/reserve.json\" ;;"));
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
    let (code, placed) = place(&rig, &repo, "retired", None);
    assert_eq!(code, 0, "{placed}");
    let id = placed["reservation"].as_str().unwrap();
    let grant_path = format!("state/placement-grants/{id}.json");
    let grant = rig.read(&grant_path);
    let envelope = grant["envelope"].clone();
    let before = rig.read("state/placements.json");
    let log = rig.log("herdr.log");
    let (code, error) = reserve_delivery(&rig, &envelope);
    assert_eq!(code, 2, "{error}");
    assert_eq!(rig.read(&grant_path), grant);
    assert_eq!(rig.read("state/placements.json"), before);
    assert_eq!(rig.log("herdr.log"), log);
    // Neither omitting the placement envelope nor losing publication progress can
    // downgrade the request into the old TTL admission path.
    assert_eq!(rig.json(&["host", "reserve", "--id", id]).0, 2);
    let mut interrupted = grant.clone();
    interrupted["materialized"] = json!(false);
    rig.put(&grant_path, &interrupted);
    rig.put("state/placements.json", &json!([]));
    assert_eq!(reserve_delivery(&rig, &envelope).0, 2);
    assert_eq!(rig.read(&grant_path), interrupted);
    assert_eq!(rig.read("state/placements.json"), json!([]));
}

#[test]
fn reservation_publication_crashes_recover_the_original_timestamp() {
    for boundary in ["placement-grant-intent", "placement-hold-published"] {
        let rig = Rig::short(boundary);
        fs::write(rig.dir.join("clock"), "1000").unwrap();
        rig.snapshot("state/host.json", 1000, 0.0, 8);
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
        let intent = rig.read("state/placement-grants/crashed-grant.json");
        assert_eq!(intent["reservation"]["created_ms"], 1000);
        assert_eq!(intent["materialized"], false);
        fs::write(rig.dir.join("clock"), "2000").unwrap();
        rig.snapshot("state/host.json", 2000, 1000.0, 8);
        assert_eq!(reserve_delivery(&rig, &envelope).0, 0);
        assert_eq!(
            rig.read("state/placements.json"),
            json!([{"id":"crashed-grant","created_ms":1000}])
        );
        assert_eq!(
            rig.read("state/placement-grants/crashed-grant.json")["materialized"],
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

/// A real repository with a `dogsled.toml` setup, and a fake Herdr whose `worktree open`
/// opens the main checkout's workspace w7 as the anchor on first use.
pub(super) fn place_rig(name: &str) -> (Rig, PathBuf) {
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
    fs::write(repo.join("dogsled.toml"), SETUP).unwrap();
    git(&repo, &["add", "dogsled.toml"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    rig.snapshot("state/host.json", now(), 0.0, 8);
    rig.script(
        "herdr",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/herdr.log"
case "$*" in
  'machine list --json') cat "$RIG/machines.json" ;;
  'workspace list') printf '%s\n' '{"result":{"workspaces":[{"workspace_id":"w1","label":"caller"}]}}' ;;
  'agent list') cat "$RIG/agents.json" ;;
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
    rig.put("state/placements.json", &json!([]));
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
    assert_eq!(rig.read("state/worktrees.json")[0]["path"], json!(tree));
    assert_eq!(
        rig.read("state/placements.json").as_array().unwrap().len(),
        1
    );
    // A second child does not replace the anchor: the workspace was open before it.
    fs::write(repo.join(".git/agent-worktree-anchor"), "kept\n").unwrap();
    assert_eq!(place(&rig, &repo, "feat/b", None).0, 0);
    assert_eq!(
        fs::read_to_string(repo.join(".git/agent-worktree-anchor")).unwrap(),
        "kept\n"
    );
}

#[test]
fn place_bases_new_branches_reuses_existing_ones_and_refuses_foreign_work() {
    let (rig, repo) = place_rig("place-branches");
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
    assert_eq!(
        rig.read("state/placements.json").as_array().unwrap().len(),
        1
    );
    // A branch that existed before placement stays.
    git(&repo, &["branch", "kept"]);
    assert_eq!(place(&rig, &repo, "kept", None).0, 2);
    assert!(!repo.join(".worktrees/kept").exists());
    assert_ne!(git(&repo, &["branch", "--list", "kept"]), "");
}

#[test]
fn repository_and_host_config_errors_are_explicit() {
    let (rig, repo) = place_rig("place-config");
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
    rig.config("[hooks]\nprepare = ['true']\n");
    let (code, error) = place(&rig, &repo, "feat/x", None);
    assert_eq!(code, 2);
    assert!(error["error"].as_str().unwrap().contains("unknown field"));
}
