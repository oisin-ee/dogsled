use super::*;

fn profile(rig: &Rig, name: &str, json: &Value) {
    let dir = rig.dir.join(format!("profiles/{name}"));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("profile.json"), json.to_string()).unwrap();
    fs::write(dir.join("prompt.md"), "Review only.\n").unwrap();
}

#[test]
fn a_profile_sets_the_exact_pi_argv_and_the_task_is_an_argument_or_a_file() {
    let rig = Rig::new("profile-argv");
    rig.snapshot("state/host.json", now(), 0.0, 8);
    profile(
        &rig,
        "reviewer",
        &json!({"description":"Read-only review","model":"alias/sol","thinking":"medium",
            "prompt":"prompt.md","accepts":["base"],"pi_args":[],"role":"reader"}),
    );
    let (code, result) = rig.json(&[
        "agent",
        "--profile",
        "reviewer",
        "--name",
        "rev",
        "--message",
        "Review the diff.",
    ]);
    assert_eq!(code, 0, "{result}");
    assert_eq!(result["profile"], "reviewer");
    assert_eq!(result["model"], "alias/sol");
    let prompt = rig.dir.join("profiles/reviewer/prompt.md");
    let log = rig.log("herdr.log");
    assert!(
        log.contains(&format!(
            "agent start rev --kind pi --pane w2:p1 -- --name rev --model alias/sol --thinking medium --append-system-prompt {} Review the diff.\n",
            prompt.display()
        )),
        "{log}"
    );
    assert!(!log.contains("agent prompt"), "{log}");
    let item = &rig.read("state/items.json")[0];
    assert_eq!(
        (
            &item["harness"],
            &item["profile"],
            &item["model"],
            &item["role"]
        ),
        (
            &json!("pi"),
            &json!("reviewer"),
            &json!("alias/sol"),
            &json!("reader")
        )
    );
    // A multi-line task goes through a file: Herdr refuses an argument with a newline.
    let mut cmd = rig.cmd(&["agent", "--name", "multi", "--message-file", "-", "--json"]);
    let mut child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), b"Line one.\nLine two.\n").unwrap();
    let out = child.wait_with_output().unwrap();
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    let log = rig.log("herdr.log");
    let line = log
        .lines()
        .find(|l| l.starts_with("agent start multi"))
        .unwrap_or_else(|| panic!("{result} {log}"));
    let task = line.rsplit_once(" @").unwrap().1;
    assert!(
        line.starts_with("agent start multi --kind pi --pane w2:p1 -- --name multi @"),
        "{line}"
    );
    assert_eq!(fs::read_to_string(task).unwrap(), "Line one.\nLine two.\n");
    // Profiles are for Pi; an unknown profile fails before anything starts.
    let (code, refused) = rig.json(&[
        "agent",
        "--kind",
        "claude",
        "--profile",
        "reviewer",
        "--name",
        "x",
        "--message",
        "hi",
    ]);
    assert_eq!(code, 2, "{refused}");
    let (code, missing) = rig.json(&[
        "agent",
        "--profile",
        "absent",
        "--name",
        "x",
        "--message",
        "hi",
    ]);
    assert_eq!(code, 2, "{missing}");
    assert!(
        missing["error"]
            .as_str()
            .unwrap()
            .contains("profile absent"),
        "{missing}"
    );
    let (_, listed) = rig.json(&["profiles"]);
    assert_eq!(listed[0]["name"], "reviewer");
    assert_eq!(listed[0]["thinking"], "medium");
}

#[test]
fn a_failed_launch_is_queued_and_retried_with_backoff() {
    let rig = Rig::new("launch-retry");
    rig.snapshot("state/host.json", now(), 0.0, 8);
    fs::write(rig.dir.join("clock"), now().to_string()).unwrap();
    // Herdr's start fails once.
    rig.script(
        "herdr",
        &fs::read_to_string(rig.dir.join("bin/herdr")).unwrap().replace(
            "  'agent start'*|",
            "  'agent start'*) if ! test -f \"$RIG/started-once\"; then touch \"$RIG/started-once\"; echo 'timed out waiting for agent startup' >&2; exit 1; fi; printf '{}\\n' ;;\n  ",
        ),
    );
    let (code, queued) = rig.json(&["agent", "--name", "w", "--message", "go"]);
    assert_eq!(code, 1, "{queued}");
    assert_eq!(queued["queued"], true);
    assert!(
        queued["retrying"].as_str().unwrap().contains("timed out"),
        "{queued}"
    );
    // The failed tab is closed, so the retry starts clean.
    assert!(rig.log("herdr.log").contains("pane close w2:p1"));
    let ticket = &rig.read("state/queue.json")[0];
    assert_eq!(ticket["state"], "pending");
    assert_eq!(ticket["retry"]["attempts"], 1);
    // Before the backoff ends, admission leaves it.
    rig.run(&["serve", "--once", "--interval", "1"]);
    assert_eq!(rig.read("state/queue.json")[0]["state"], "pending");
    let next = rig.read("state/queue.json")[0]["retry"]["next_ms"]
        .as_u64()
        .unwrap();
    fs::write(rig.dir.join("clock"), next.to_string()).unwrap();
    rig.run(&["serve", "--once", "--interval", "1"]);
    let ticket = &rig.read("state/queue.json")[0];
    assert_eq!(ticket["state"], "admitted", "{ticket}");
    assert_eq!(rig.log("herdr.log").matches("agent start w ").count(), 2);
}
