use super::*;
use std::io::{BufRead as _, BufReader, Write as _};

const NEW_VERSION: &str = "0.31.0";

fn replacement(rig: &Rig) -> PathBuf {
    let path = rig.dir.join("new-dogsled");
    fs::copy(env!("CARGO_BIN_EXE_dogsled"), &path).unwrap();
    fs::canonicalize(path).unwrap()
}

fn override_version(command: &mut Command, executable: &Path) {
    command
        .env("DOGSLED_TEST_VERSION_EXE", executable)
        .env("DOGSLED_TEST_VERSION", NEW_VERSION);
}

fn record(rig: &Rig, executable: &Path, version: &str) {
    let connection = rig.database();
    connection.execute(
        "INSERT INTO store_writer (singleton, version, executable) VALUES (1, ?1, ?2)
         ON CONFLICT (singleton) DO UPDATE SET version = excluded.version, executable = excluded.executable",
        rusqlite::params![version, executable.to_str().unwrap()],
    ).unwrap();
}

fn command(rig: &Rig, args: &[&str], executable: &Path) -> Command {
    let mut command = rig.cmd(args);
    override_version(&mut command, executable);
    command
}

fn output(mut command: Command) -> (std::process::ExitStatus, String) {
    let path = std::env::temp_dir().join(format!(
        "upgrade-output-{}",
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let error_path = path.with_extension("err");
    command
        .stdout(fs::File::create(&path).unwrap())
        .stderr(fs::File::create(&error_path).unwrap());
    let mut owned = Owned::spawn(&mut command);
    let status = owned.wait();
    let stdout = fs::read_to_string(&path).unwrap();
    fs::remove_file(path).unwrap();
    let stderr = fs::read_to_string(&error_path).unwrap();
    fs::remove_file(error_path).unwrap();
    (status, format!("{stdout}{stderr}"))
}

#[test]
fn old_cli_executes_the_newest_writer_and_preserves_arguments() {
    let rig = Rig::short("upgrade-cli");
    let executable = replacement(&rig);
    // The replacement becomes the recorded owner by opening the real store.
    drop(rig.database());
    let mut newer = isolated(&executable);
    rig.rig_env(&mut newer);
    override_version(&mut newer, &executable);
    newer.args(["state", "items"]);
    let (status, text) = output(newer);
    assert!(status.success(), "{text}");
    let (status, text) = output(command(&rig, &["state", "items"], &executable));
    assert!(status.success(), "{text}");
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), json!([]));
    let path: String = rig
        .database()
        .query_row("SELECT executable FROM store_writer", [], |row| row.get(0))
        .unwrap();
    assert_eq!(path, executable.to_str().unwrap());
}

#[test]
fn equal_version_updates_the_canonical_writer_path() {
    let rig = Rig::short("upgrade-equal");
    let executable = replacement(&rig);
    record(&rig, &executable, env!("CARGO_PKG_VERSION"));
    let (status, text) = output(rig.cmd(&["state", "items"]));
    assert!(status.success(), "{text}");
    let path: String = rig
        .database()
        .query_row("SELECT executable FROM store_writer", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        PathBuf::from(path),
        fs::canonicalize(env!("CARGO_BIN_EXE_dogsled")).unwrap()
    );
}

#[test]
fn a_missing_replacement_fails_with_its_path_and_version() {
    let rig = Rig::short("upgrade-missing");
    let executable = rig.dir.join("missing-dogsled");
    record(&rig, &executable, NEW_VERSION);
    let (status, text) = output(rig.cmd(&["state", "items"]));
    assert!(!status.success(), "{text}");
    assert!(text.contains(NEW_VERSION), "{text}");
    assert!(text.contains(executable.to_str().unwrap()), "{text}");
    assert!(text.contains("handover failed"), "{text}");
}

#[test]
fn a_mismatching_replacement_version_is_refused() {
    let rig = Rig::short("upgrade-mismatch");
    let executable = replacement(&rig);
    record(&rig, &executable, NEW_VERSION);
    let (status, text) = output(rig.cmd(&["state", "items"]));
    assert!(!status.success(), "{text}");
    assert!(text.contains("--version does not match"), "{text}");
    assert!(text.contains(NEW_VERSION), "{text}");
    assert!(text.contains(executable.to_str().unwrap()), "{text}");
}

#[test]
fn an_equal_or_older_writer_is_never_executed() {
    for version in [env!("CARGO_PKG_VERSION"), "0.29.0"] {
        let rig = Rig::short("upgrade-no-downgrade");
        record(&rig, &rig.dir.join("does-not-exist"), version);
        let (status, text) = output(rig.cmd(&["state", "items"]));
        assert!(status.success(), "{version}: {text}");
    }
}

#[test]
fn newer_schema_without_newer_writer_keeps_the_schema_refusal() {
    let rig = Rig::short("upgrade-no-writer");
    let connection = rig.database();
    connection.execute("DELETE FROM store_writer", []).unwrap();
    connection
        .execute_batch("PRAGMA user_version = 999")
        .unwrap();
    let (status, text) = output(rig.cmd(&["state", "items"]));
    assert!(!status.success(), "{text}");
    assert!(
        text.contains("schema version 999 is newer than supported version 2"),
        "{text}"
    );
}

#[test]
fn newer_schema_is_read_only_until_the_recorded_binary_takes_over() {
    let rig = Rig::short("upgrade-schema");
    let executable = replacement(&rig);
    record(&rig, &executable, NEW_VERSION);
    rig.database()
        .execute_batch("PRAGMA user_version = 3")
        .unwrap();
    let mut command = command(&rig, &["state", "items"], &executable);
    command.env("DOGSLED_TEST_SCHEMA_VERSION", "3");
    let (status, text) = output(command);
    assert!(status.success(), "{text}");
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), json!([]));
    let version: i64 = rig
        .database()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 3);
}

#[test]
fn a_lying_version_probe_cannot_loop_into_an_older_image() {
    let rig = Rig::short("upgrade-loop");
    let executable = rig.dir.join("bin/lying-dogsled");
    rig.script(
        "lying-dogsled",
        &format!(
            "#!/bin/sh\nif test \"$1\" = --version; then echo 'dogsled {NEW_VERSION}'; exit; fi\nexec '{}' \"$@\"\n",
            env!("CARGO_BIN_EXE_dogsled")
        ),
    );
    record(&rig, &executable, NEW_VERSION);
    let (status, text) = output(rig.cmd(&["state", "items"]));
    assert!(!status.success(), "{text}");
    assert!(text.contains("different version after exec"), "{text}");
    assert!(text.contains(NEW_VERSION), "{text}");
    assert!(text.contains(executable.to_str().unwrap()), "{text}");
}

#[test]
fn an_inherited_exec_guard_does_not_block_a_new_child_cli() {
    let rig = Rig::short("upgrade-child-guard");
    let executable = replacement(&rig);
    record(&rig, &executable, NEW_VERSION);
    let mut command = command(&rig, &["state", "items"], &executable);
    command.env(
        "DOGSLED_UPGRADE_TARGET",
        json!({
            "pid":std::process::id(),
            "writer":{"version":NEW_VERSION,"executable":executable}
        })
        .to_string(),
    );
    let (status, text) = output(command);
    assert!(status.success(), "{text}");
}

#[test]
fn a_hung_version_probe_fails_within_its_budget() {
    let rig = Rig::short("upgrade-probe-timeout");
    let executable = rig.dir.join("bin/hung-version");
    rig.script("hung-version", "#!/bin/sh\nexec sleep 60\n");
    record(&rig, &executable, NEW_VERSION);
    let started = Instant::now();
    let (status, text) = output(rig.cmd(&["state", "items"]));
    assert!(!status.success(), "{text}");
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "probe exceeded its budget: {text}"
    );
    assert!(text.contains("deadline"), "{text}");
    assert!(text.contains(executable.to_str().unwrap()), "{text}");
    assert!(text.contains(NEW_VERSION), "{text}");
}

#[test]
fn noncanonical_replacement_path_is_refused() {
    let rig = Rig::short("upgrade-noncanonical");
    let executable = replacement(&rig);
    let alias = rig.dir.join("new-link");
    std::os::unix::fs::symlink(&executable, &alias).unwrap();
    record(&rig, &alias, NEW_VERSION);
    let (status, text) = output(command(&rig, &["state", "items"], &executable));
    assert!(!status.success(), "{text}");
    assert!(text.contains("absolute and canonical"), "{text}");
}

fn process_command(pid: u32) -> String {
    let mut command = isolated("ps");
    command.args(["-p", &pid.to_string(), "-o", "command="]);
    let (status, text) = output(command);
    assert!(status.success(), "{text}");
    text
}

fn wait_for_image(owned: &mut Owned, executable: &Path) {
    let deadline = Instant::now() + PROCESS_DEADLINE;
    loop {
        assert!(!process_gone(owned.pid), "server exited before handover");
        let text = process_command(owned.pid);
        if text.starts_with(executable.to_str().unwrap()) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "server did not hand over: {text}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn running_serve_hands_over_between_passes_with_the_same_pid() {
    let rig = Rig::short("upgrade-serve");
    let executable = replacement(&rig);
    let mut command = command(&rig, &["serve", "--interval", "1"], &executable);
    let journal = rig.dir.join("service-stderr");
    command
        .stdout(Stdio::null())
        .stderr(fs::File::create(&journal).unwrap());
    let mut owned = Owned::spawn(&mut command);
    let pid = owned.pid;
    let deadline = Instant::now() + PROCESS_DEADLINE;
    while rig.log("state/samples.jsonl").is_empty() {
        assert!(
            !process_gone(owned.pid),
            "service exited: {}",
            fs::read_to_string(&journal).unwrap()
        );
        assert!(
            Instant::now() < deadline,
            "service did not start: {}",
            fs::read_to_string(&journal).unwrap()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    record(&rig, &executable, NEW_VERSION);
    wait_for_image(&mut owned, &executable);
    assert_eq!(owned.child_mut().id(), pid);
    let before = rig.log("state/samples.jsonl").lines().count();
    let deadline = Instant::now() + PROCESS_DEADLINE;
    while rig.log("state/samples.jsonl").lines().count() <= before {
        assert!(!process_gone(owned.pid), "new service exited");
        assert!(Instant::now() < deadline, "new service did not publish");
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Mcp {
    owned: Owned,
    lines: std::sync::mpsc::Receiver<String>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl Mcp {
    fn start(rig: &Rig, executable: &Path) -> Self {
        let mut command = command(rig, &["mcp"], executable);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut owned = Owned::spawn(&mut command);
        let stdout = owned.child_mut().stdout.take().unwrap();
        let (send, lines) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            owned,
            lines,
            reader: Some(reader),
        }
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.owned.child_mut().stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    fn next(&self) -> Value {
        let line = self.lines.recv_timeout(PROCESS_DEADLINE).unwrap();
        let message: Value = serde_json::from_str(&line).unwrap();
        assert!(message.get("error").is_none(), "{message}");
        message
    }

    fn receive(&self, id: u64) -> Value {
        let message = self.next();
        assert_eq!(message["id"], id, "{message}");
        message["result"].clone()
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        self.owned.terminate();
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
    }
}

#[test]
fn live_mcp_answers_the_pending_second_tool_call_without_initialize() {
    let rig = Rig::short("upgrade-mcp");
    let executable = replacement(&rig);
    let mut client = Mcp::start(&rig, &executable);
    let pid = client.owned.pid;
    client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"upgrade-test","version":"0"}
    }}));
    assert_eq!(client.receive(1)["serverInfo"]["name"], "dogsled");
    client.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    let call = |id| json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"status","arguments":{}}});
    client.send(&call(2));
    let first = client.receive(2);
    assert_ne!(first["isError"], true, "{first}");
    record(&rig, &executable, NEW_VERSION);
    client.send(&call(3));
    client.send(&call(4));
    // The pending line and the pipelined unread line must both survive, regardless
    // of the order in which their tool handlers finish.
    let mut answered = Vec::new();
    for _ in 0..2 {
        let answer = client.next();
        assert_ne!(answer["result"]["isError"], true, "{answer}");
        answered.push(answer["id"].as_u64().unwrap());
    }
    answered.sort_unstable();
    assert_eq!(answered, [3, 4]);
    wait_for_image(&mut client.owned, &executable);
    assert_eq!(client.owned.child_mut().id(), pid);
    // Later calls use the same session, not only the resumed pending request.
    client.send(&call(5));
    assert_ne!(client.receive(5)["isError"], true);
    let version: String = rig
        .database()
        .query_row("SELECT version FROM store_writer", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, NEW_VERSION);
}
