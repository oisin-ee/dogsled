use super::*;

/// Read the first line of a child's output, or fail the test on the bound. The read runs
/// on its own thread, so a child that never writes cannot hang the test. The reader comes
/// back with the line, so the pipe stays open for the rest of the child's output.
fn first_line(
    stdout: std::process::ChildStdout,
) -> (String, std::io::BufReader<std::process::ChildStdout>) {
    use std::io::BufRead as _;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stdout);
        let mut line = String::new();
        let read = reader.read_line(&mut line).map(|count| (count, line));
        let _ = sender.send((read, reader));
    });
    match receiver.recv_timeout(PROCESS_DEADLINE) {
        Ok((Ok((0, _line)), _reader)) => panic!("the child closed its output without a line"),
        Ok((Ok((_count, line)), reader)) => (line, reader),
        Ok((Err(error), _reader)) => panic!("the child's output could not be read: {error}"),
        Err(error) => panic!("the child wrote no line within {PROCESS_DEADLINE:?}: {error}"),
    }
}

#[test]
fn wrapper_termination_kills_command_group_and_records_result() {
    let rig = Rig::new("wrapper-signal");
    rig.claim("wrapper", &["sh", "-c", "echo ready; sleep 30"]);
    let mut command = rig.cmd(&["exec", "wrapper", "--", "sh", "-c", "echo ready; sleep 30"]);
    command.stdout(std::process::Stdio::piped());
    // The test owns the wrapper as its own child, so the test reaps it and reads its
    // exit status. A failed assertion kills the wrapper and its own group.
    let mut child = Owned::spawn(&mut command);
    let (line, _output) = first_line(child.child_mut().stdout.take().unwrap());
    assert_eq!(line, "ready\n");
    assert!(
        Command::new("kill")
            .args(["-TERM", &child.child_mut().id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(child.wait().code(), Some(137));
    assert_eq!(rig.read("state/jobs/wrapper.json")["reason"], "signal 15");
}

#[test]
fn remote_wait_deadline_bounds_a_silent_ssh_process() {
    let rig = Rig::new("bounded-ssh");
    rig.peer();
    rig.put("state/items.json", &json!([{"id":"remote","kind":"job","host":"8103","pane":"w9:p1","owner_pane":"w1:p1","name":null,"created_ms":0,"closed_ms":null}]));
    rig.script("ssh", "#!/bin/sh\nsleep 30\n");
    let output = rig.run(&["wait", "remote", "--max", "20ms"]);
    assert_eq!(output.status.code(), Some(124));
    assert_eq!(output.stdout, b"still running\n");
}

#[test]
fn stop_kills_running_process_after_child_output_without_test_sleep() {
    let rig = Rig::new("stop-real");
    rig.claim("stoppable", &["sh", "-c", "echo ready; sleep 30"]);
    let mut command = rig.cmd(&[
        "exec",
        "stoppable",
        "--",
        "sh",
        "-c",
        "echo ready; sleep 30",
    ]);
    command.stdout(std::process::Stdio::piped());
    // The test owns the wrapper as its own child, so the test reaps it and reads its
    // exit status. A failed assertion kills the wrapper and its own group.
    let mut child = Owned::spawn(&mut command);
    let (line, _output) = first_line(child.child_mut().stdout.take().unwrap());
    assert_eq!(line, "ready\n");
    assert!(rig.run(&["stop", "stoppable"]).status.success());
    assert_eq!(child.wait().code(), Some(137));
    assert_eq!(rig.read("state/jobs/stoppable.json")["reason"], "stopped");
}

#[test]
fn waiting_queue_caller_observes_service_admission_without_sleep() {
    let rig = Rig::new("queue-wait");
    rig.snapshot("state/host.json", now(), 99.0, 1);
    // Queueing, the service pass, and observing admission each need scheduling
    // headroom. This is only a failure bound: the output handshake drives progress.
    let admission_deadline = format!("{}s", (PROCESS_DEADLINE * 3).as_secs());
    let mut command = rig.cmd(&[
        "run",
        "--wait",
        "--max",
        &admission_deadline,
        "--",
        "sh",
        "-c",
        "exit 3",
    ]);
    command.stdout(std::process::Stdio::piped());
    // The test owns the command as its own child, so the test reaps it and reads its
    // exit status. A failed assertion kills the command and its own group.
    let mut child = Owned::spawn(&mut command);
    let (line, _output) = first_line(child.child_mut().stdout.take().unwrap());
    let queued: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(queued["queued"], true);
    // The fixture supplies admission capacity, not the CI runner's current load.
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    assert!(rig.run(&["serve", "--once"]).status.success());
    assert_eq!(child.wait().code(), Some(3));
    assert_eq!(rig.read("state/queue.json")[0]["state"], "admitted");
    assert_eq!(rig.read("state/queue.json")[0]["waiting"], true);
    assert!(!rig.log("herdr.log").contains("dogsled: ticket"));
}

#[test]
fn remote_repository_clone_precedes_prepare_and_failures_do_not_fall_back() {
    let rig = Rig::new("remote-checkout");
    let repo = rig.dir.join("home/repo");
    repository(&repo);
    rig.peer();
    rig.snapshot("state/host.json", now(), 0.0, 8);
    let args = [
        "place",
        "--host",
        "netcup",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "feature",
    ];
    assert_eq!(rig.json(&args).0, 0);
    let log = rig.log("ssh.log");
    assert!(log.find("host checkout").unwrap() < log.find("host prepare").unwrap());
    assert!(log.contains("--origin https://example.invalid/repo.git"));
    rig.script("ssh", r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/ssh.log"
case "$*" in
 *'host snapshot'*) cat "$RIG/remote.json" ;;
 *'host reserve'*)
   for remote do :; done
   sh -c "$RIG/bin/peer-fixture $remote" ;;
 *'host checkout'*) printf '%s\n' '{"error":"netcup: clone https://example.invalid/repo.git failed: credentials","code":"CLONE_FAILED"}'; exit 2 ;;
 *) echo 'must not prepare after clone failure' >&2; exit 2 ;;
esac
"#);
    let (code, error) = rig.json(&args);
    assert_eq!(code, 2);
    // The peer's code maps to a fixed message; its own text, which can hold a secret, is hidden.
    let message = error["error"].as_str().unwrap();
    assert!(
        message.contains("could not clone the repository"),
        "{error}"
    );
    assert!(!message.contains("credentials"), "{error}");
    assert_eq!(rig.log("ssh.log").matches("host prepare ").count(), 1);
    assert!(!rig.dir.join("state/placements.json").exists());
}

#[test]
fn host_launch_matches_workspace_name_or_creates_it_and_validates_cwd() {
    let rig = Rig::new("workspace-name");
    let request = json!({"id":"launch-one","action":{"type":"agent","kind":"claude","name":"worker","message":"hello"},
        "host":"remote","cwd":rig.dir.join("home"),"pane":null,"workspace":null,"workspace_name":"caller","owner":"parent"});
    assert_eq!(
        rig.json(&[
            "host",
            "launch",
            "--job-validity",
            "2",
            "--request",
            &request.to_string()
        ])
        .0,
        0
    );
    assert!(rig.log("herdr.log").contains("tab create --workspace w1"));
    let mut next = request.clone();
    next["id"] = json!("launch-two");
    next["workspace_name"] = json!("new-name");
    assert_eq!(
        rig.json(&[
            "host",
            "launch",
            "--job-validity",
            "2",
            "--request",
            &next.to_string()
        ])
        .0,
        0
    );
    assert!(
        rig.log("herdr.log")
            .contains("workspace create --label new-name")
    );
    next["id"] = json!("launch-three");
    next["cwd"] = json!("/missing-dogsled-cwd");
    assert_eq!(
        rig.json(&[
            "host",
            "launch",
            "--job-validity",
            "2",
            "--request",
            &next.to_string()
        ])
        .0,
        2
    );
}
