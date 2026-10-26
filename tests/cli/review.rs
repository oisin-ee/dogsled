use super::*;
use std::io::{BufRead as _, Write as _};
use std::process::Stdio;
use std::time::{Duration, Instant};

#[test]
fn closed_wrapper_stdout_preserves_full_log_and_child_exit() {
    let rig = Rig::new("closed-output");
    let fifo = rig.dir.join("continue");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    rig.claim(
        "closed-output",
        &[
            "sh",
            "-c",
            "echo first; read -r ready < \"$RIG/continue\"; echo second; echo third >&2; exit 7",
        ],
    );
    let mut command = rig.cmd(&[
        "exec",
        "closed-output",
        "--",
        "sh",
        "-c",
        "echo first; read -r ready < \"$RIG/continue\"; echo second; echo third >&2; exit 7",
    ]);
    command.stdout(Stdio::piped());
    // The test owns the wrapper. A failed assertion kills it and every command it
    // started, so no wrapper outlives this test.
    let mut owned = Owned::spawn(&mut command);
    let mut output = std::io::BufReader::new(owned.child_mut().stdout.take().unwrap());
    let mut first = String::new();
    output.read_line(&mut first).unwrap();
    assert_eq!(first, "first\n");
    drop(output);
    // The FIFO is opened without blocking, so the write never waits for an open that
    // may never come.
    let mut writer = open_fifo(&fifo);
    writer.write_all(b"go\n").unwrap();
    writer.flush().unwrap();
    assert_eq!(owned.wait().code(), Some(7));
    let log = rig.log("state/jobs/closed-output.log");
    for line in ["first\n", "second\n", "third\n"] {
        assert!(log.contains(line), "{log}");
    }
    assert_eq!(rig.read("state/jobs/closed-output.json")["exit"], 7);
}

#[test]
fn deadline_includes_discovery_and_placement_ssh() {
    for (name, args) in [
        ("wait-discovery", vec!["wait", "unknown", "--max", "20ms"]),
        (
            "run-placement",
            vec!["run", "--wait", "--max", "20ms", "--", "true"],
        ),
        (
            "agent-placement",
            vec![
                "agent",
                "--kind",
                "pi",
                "--name",
                "worker",
                "--message",
                "hello",
                "--max",
                "20ms",
            ],
        ),
    ] {
        let rig = Rig::new(name);
        rig.peer();
        rig.script("ssh", "#!/bin/sh\nexec sleep 30\n");
        let start = Instant::now();
        let out = rig.run(&args);
        let diagnostic = format!(
            "{name}: status={:?}, stdout={:?}, stderr={:?}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(start.elapsed() < Duration::from_secs(2), "{diagnostic}");
        if name == "agent-placement" {
            assert_eq!(out.status.code(), Some(2), "{diagnostic}");
            assert!(
                String::from_utf8_lossy(&out.stderr).contains("deadline reached"),
                "{diagnostic}"
            );
        } else {
            assert_eq!(out.status.code(), Some(124), "{diagnostic}");
            assert_eq!(out.stdout, b"still running\n", "{diagnostic}");
        }
    }
}

#[test]
fn command_deadline_bounds_local_git_and_remote_report() {
    let rig = Rig::new("hung-git");
    rig.script("git", "#!/bin/sh\nexec sleep 30\n");
    let start = Instant::now();
    let out = rig.run(&["run", "--wait", "--max", "50ms", "--", "true"]);
    assert_eq!(out.status.code(), Some(124));
    assert!(start.elapsed() < Duration::from_secs(2));

    let rig = Rig::new("remote-deadline-report");
    rig.peer();
    rig.put("state/items.json", &json!([{"id":"remote","kind":"job","host":"8103","pane":"w9:p1","owner_pane":"w1:p1","name":null,"created_ms":0,"closed_ms":null}]));
    rig.script("ssh", "#!/bin/sh\nprintf '%s\\n' '{\"deadline\":true,\"error\":\"deadline reached: git clone\"}'; exit 2\n");
    let out = rig.run(&["wait", "remote", "--max", "1s"]);
    assert_eq!(out.status.code(), Some(124));
    assert_eq!(out.stdout, b"still running\n");
}

#[test]
fn command_deadline_bounds_host_lock_wait() {
    let rig = Rig::new("locked-placement");
    rig.snapshot("state/host.json", now(), 0.0, 8);
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(rig.dir.join("state/host.lock"))
        .unwrap();
    lock.lock().unwrap();
    let start = Instant::now();
    let out = rig.run(&["place", "--wait", "--max", "20ms"]);
    assert_eq!(out.status.code(), Some(124));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(out.stdout, b"still running\n");
}

#[test]
fn service_publishes_snapshots_while_remote_action_times_out() {
    let rig = Rig::new("service-hung-peer");
    rig.peer();
    rig.snapshot("state/host.json", now(), 9999.0, 1);
    rig.config("free_core_fraction_min = 2\n");
    // Queue without consulting a hung peer, then make that peer hang for admission.
    let (_, ticket) = rig.json(&["place", "--host", "netcup"]);
    rig.expire_cache();
    rig.script("ssh", "#!/bin/sh\nexec sleep 60\n");
    // Every agent-list call, which only a publication tick makes, reads the previous published
    // snapshot. This observes publication at each service tick without sleeps or polling.
    rig.script("herdr", r#"#!/bin/sh
case "$*" in
 'machine list --json') cat "$RIG/machines.json" ;;
 'agent list') cat "$RIG/state/host.json" >> "$RIG/publications"; printf '\n---\n' >> "$RIG/publications"; cat "$RIG/agents.json" ;;
 'pane list') cat "$RIG/panes.json" ;;
 *) exit 2 ;;
esac
"#);
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let records: Vec<Value> = rig
        .log("publications")
        .split("\n---\n")
        .filter(|s| !s.trim().is_empty())
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert!(records.len() >= 3);
    for pair in records.windows(2) {
        assert!(pair[1]["sampled_ms"].as_u64().unwrap() > pair[0]["sampled_ms"].as_u64().unwrap());
    }
    let queue = rig.read("state/queue.json");
    assert_eq!(queue[0]["id"], ticket["ticket"]);
    assert_eq!(queue[0]["state"], "failed");
    assert!(
        queue[0]["error"]
            .as_str()
            .unwrap()
            .contains("deadline reached")
    );
}
