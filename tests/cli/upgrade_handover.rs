use super::*;
use std::io::{BufRead as _, BufReader, Write as _};

const NEW_VERSION: &str = "0.31.0";

fn replacement(rig: &Rig) -> PathBuf {
    let path = rig.dir.join("new-dogsled");
    // Copy in a child process. An in-process copy holds a write descriptor that a
    // concurrent test's fork inherits until its exec, so Linux refuses to run the copy
    // with ETXTBSY. A `cp` child's descriptor never enters this process.
    let (status, text) = output({
        let mut command = isolated("cp");
        command.arg(env!("CARGO_BIN_EXE_dogsled")).arg(&path);
        command
    });
    assert!(status.success(), "{text}");
    let path = fs::canonicalize(path).unwrap();
    // Synthetic writer rows stand in for an image that has already run and registered.
    // A fresh copy's first launch can exceed the production version-probe budget.
    let mut command = isolated(&path);
    override_version(&mut command, &path);
    command.arg("--version");
    let (status, text) = output(command);
    assert!(status.success(), "{text}");
    assert_eq!(text.trim(), format!("dogsled {NEW_VERSION}"));
    path
}

/// The schema version this build creates, read from its own creation script.
fn supported_schema(rig: &Rig) -> i64 {
    let out = rig.run(&["state", "schema"]);
    assert!(out.status.success(), "{out:?}");
    let script = String::from_utf8(out.stdout).unwrap();
    script
        .rsplit_once("PRAGMA user_version = ")
        .and_then(|(_, rest)| rest.split(';').next())
        .and_then(|version| version.trim().parse().ok())
        .unwrap_or_else(|| panic!("no user_version in the creation script: {script}"))
}

/// Probe samples the service has published. Read `state.db` directly: a CLI read would
/// run the upgrade check this test exercises.
fn samples(rig: &Rig) -> usize {
    let path = rig.dir.join("state/state.db");
    if !path.exists() {
        return 0;
    }
    let connection =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    connection.busy_timeout(Duration::from_secs(5)).unwrap();
    connection
        .query_row("SELECT COUNT(*) FROM samples", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_or(0, |count| usize::try_from(count).unwrap())
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
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "upgrade-output-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
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
fn version_override_requires_the_same_executable() {
    let rig = Rig::short("upgrade-version-identity");
    let executable = replacement(&rig);
    let alias = rig.dir.join("new-link");
    std::os::unix::fs::symlink(&executable, &alias).unwrap();
    for (expected, version) in [
        (&alias, NEW_VERSION),
        (
            &PathBuf::from(env!("CARGO_BIN_EXE_dogsled")),
            env!("CARGO_PKG_VERSION"),
        ),
        (&rig.dir.join("missing"), env!("CARGO_PKG_VERSION")),
    ] {
        let mut command = isolated(&alias);
        override_version(&mut command, expected);
        command.arg("--version");
        let (status, text) = output(command);
        assert!(status.success(), "{text}");
        assert_eq!(text.trim(), format!("dogsled {version}"));
    }
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

fn assert_current_writer(rig: &Rig) {
    let writer: (String, String) = rig
        .database()
        .query_row("SELECT version, executable FROM store_writer", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(writer.0, env!("CARGO_PKG_VERSION"));
    assert_eq!(
        PathBuf::from(writer.1),
        fs::canonicalize(env!("CARGO_BIN_EXE_dogsled")).unwrap()
    );
}

fn assert_stale_recovered(rig: &Rig, executable: &Path, args: &[&str]) -> String {
    let (status, text) = output(rig.cmd(args));
    assert!(status.success(), "{text}");
    assert!(text.contains(NEW_VERSION), "{text}");
    assert!(text.contains(executable.to_str().unwrap()), "{text}");
    assert_eq!(
        text.matches("replaced stale state.db writer").count(),
        1,
        "{text}"
    );
    assert_current_writer(rig);
    text
}

#[test]
fn a_deleted_replacement_is_replaced_by_the_older_cli_and_serve() {
    for args in [
        &["state", "items"][..],
        &["serve", "--once", "--interval", "1"][..],
    ] {
        let rig = Rig::short("upgrade-missing");
        let executable = replacement(&rig);
        record(&rig, &executable, NEW_VERSION);
        fs::remove_file(&executable).unwrap();
        assert_stale_recovered(&rig, &executable, args);
    }
}

#[test]
fn a_mismatching_replacement_is_replaced_by_the_older_cli_and_serve() {
    for args in [
        &["state", "items"][..],
        &["serve", "--once", "--interval", "1"][..],
    ] {
        let rig = Rig::short("upgrade-mismatch");
        let executable = replacement(&rig);
        record(&rig, &executable, NEW_VERSION);
        assert_stale_recovered(&rig, &executable, args);
    }
}

#[test]
fn a_debug_build_does_not_register_in_the_implicit_default_directory() {
    let rig = Rig::short("upgrade-debug-default");
    // Isolate the default directory without setting Dogsled's explicit override.
    let state_home = rig.dir.join("xdg-state");
    let input = rig.dir.join("empty-items");
    fs::write(&input, "[]").unwrap();
    let mut command = rig.cmd(&["state", "items", "--put"]);
    command
        .stdin(fs::File::open(input).unwrap())
        .env_remove("DOGSLED_STATE")
        .env("XDG_STATE_HOME", &state_home);
    let (status, text) = output(command);
    assert!(status.success(), "{text}");
    let path = state_home.join("dogsled/state.db");
    let connection = rusqlite::Connection::open(&path).unwrap();
    let count = || {
        connection
            .query_row("SELECT COUNT(*) FROM store_writer", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
    };
    assert_eq!(count(), 0, "schema creation registered a debug writer");
    let mut command = rig.cmd(&["state", "items"]);
    command
        .env_remove("DOGSLED_STATE")
        .env("XDG_STATE_HOME", &state_home);
    let (status, text) = output(command);
    assert!(status.success(), "{text}");
    assert_eq!(count(), 0, "opening the store registered a debug writer");
    // An explicit override remains eligible, even when it names the same directory.
    let mut command = rig.cmd(&["state", "items"]);
    command.env("DOGSLED_STATE", path.parent().unwrap());
    let (status, text) = output(command);
    assert!(status.success(), "{text}");
    assert_eq!(count(), 1);
}

#[test]
fn a_stale_writer_cannot_replace_a_newer_schema() {
    let rig = Rig::short("upgrade-stale-new-schema");
    let supported = supported_schema(&rig);
    let executable = replacement(&rig);
    record(&rig, &executable, NEW_VERSION);
    fs::remove_file(&executable).unwrap();
    rig.database()
        .execute_batch("PRAGMA user_version = 999")
        .unwrap();
    let (status, text) = output(rig.cmd(&["state", "items"]));
    assert!(!status.success(), "{text}");
    assert!(
        text.contains(&format!(
            "schema version 999 is newer than supported version {supported}"
        )),
        "{text}"
    );
    let version: String = rig
        .database()
        .query_row("SELECT version FROM store_writer", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, NEW_VERSION);
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
    let supported = supported_schema(&rig);
    let connection = rig.database();
    connection.execute("DELETE FROM store_writer", []).unwrap();
    connection
        .execute_batch("PRAGMA user_version = 999")
        .unwrap();
    let (status, text) = output(rig.cmd(&["state", "items"]));
    assert!(!status.success(), "{text}");
    assert!(
        text.contains(&format!(
            "schema version 999 is newer than supported version {supported}"
        )),
        "{text}"
    );
}

#[test]
fn newer_schema_is_read_only_until_the_recorded_binary_takes_over() {
    let rig = Rig::short("upgrade-schema");
    let newer = supported_schema(&rig) + 1;
    let executable = replacement(&rig);
    record(&rig, &executable, NEW_VERSION);
    rig.database()
        .execute_batch(&format!("PRAGMA user_version = {newer}"))
        .unwrap();
    let mut command = command(&rig, &["state", "items"], &executable);
    command.env("DOGSLED_TEST_SCHEMA_VERSION", newer.to_string());
    let (status, text) = output(command);
    assert!(status.success(), "{text}");
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), json!([]));
    let version: i64 = rig
        .database()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, newer);
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
fn a_hung_version_probe_preserves_the_writer_within_its_budget() {
    let rig = Rig::short("upgrade-probe-timeout");
    let executable = rig.dir.join("bin/hung-version");
    rig.script("hung-version", "#!/bin/sh\nexec sleep 60\n");
    record(&rig, &executable, NEW_VERSION);
    let started = Instant::now();
    let (status, text) = output(rig.cmd(&["state", "items"]));
    assert!(!status.success(), "{text}");
    assert!(text.contains("deadline reached"), "{text}");
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "probe exceeded its budget: {text}"
    );
    let writer: (String, String) = rig
        .database()
        .query_row("SELECT version, executable FROM store_writer", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(
        writer,
        (NEW_VERSION.into(), executable.to_str().unwrap().into())
    );
}

#[test]
fn noncanonical_replacement_path_is_replaced() {
    let rig = Rig::short("upgrade-noncanonical");
    let executable = replacement(&rig);
    let alias = rig.dir.join("new-link");
    std::os::unix::fs::symlink(&executable, &alias).unwrap();
    record(&rig, &alias, NEW_VERSION);
    assert_stale_recovered(&rig, &alias, &["state", "items"]);
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
    while samples(&rig) == 0 {
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
    let before = samples(&rig);
    let deadline = Instant::now() + PROCESS_DEADLINE;
    while samples(&rig) <= before {
        assert!(!process_gone(owned.pid), "new service exited");
        assert!(Instant::now() < deadline, "new service did not publish");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn running_serve_retries_an_unsuccessful_probe_without_replacing_the_writer() {
    let rig = Rig::short("upgrade-serve-pending");
    let executable = replacement(&rig);
    let probe = rig.dir.join("bin/pending-version");
    let ready = rig.dir.join("ready");
    rig.script(
        "pending-version",
        &format!(
            "#!/bin/sh\nif test \"$1\" = --version; then\n  if ! test -f '{}'; then exit 1; fi\n  echo 'dogsled {NEW_VERSION}'; exit\nfi\nexec '{}' \"$@\"\n",
            ready.display(), executable.display()
        ),
    );
    let mut command = command(&rig, &["serve", "--interval", "1"], &executable);
    let journal = rig.dir.join("service-stderr");
    command
        .stdout(Stdio::null())
        .stderr(fs::File::create(&journal).unwrap());
    let mut owned = Owned::spawn(&mut command);
    let pid = owned.pid;
    let deadline = Instant::now() + PROCESS_DEADLINE;
    while samples(&rig) == 0 {
        assert!(
            !process_gone(pid),
            "service exited: {}",
            rig.log("service-stderr")
        );
        assert!(Instant::now() < deadline, "service did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    record(&rig, &probe, NEW_VERSION);
    while !rig
        .log("service-stderr")
        .contains("dogsled service upgrade:")
    {
        assert!(
            !process_gone(pid),
            "service exited: {}",
            rig.log("service-stderr")
        );
        assert!(Instant::now() < deadline, "service did not probe");
        std::thread::sleep(Duration::from_millis(10));
    }
    let writer: String = rig
        .database()
        .query_row("SELECT executable FROM store_writer", [], |row| row.get(0))
        .unwrap();
    assert_eq!(writer, probe.to_str().unwrap());
    fs::write(ready, "ready").unwrap();
    wait_for_image(&mut owned, &executable);
    assert_eq!(owned.child_mut().id(), pid);
    let before = samples(&rig);
    let deadline = Instant::now() + PROCESS_DEADLINE;
    while samples(&rig) <= before {
        assert!(!process_gone(pid), "new service exited");
        assert!(Instant::now() < deadline, "new service did not publish");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn running_serve_replaces_stale_writers_without_exiting() {
    let rig = Rig::short("upgrade-serve-stale");
    let executable = replacement(&rig);
    let mut command = rig.cmd(&["serve", "--interval", "1"]);
    let journal = rig.dir.join("service-stderr");
    command
        .stdout(Stdio::null())
        .stderr(fs::File::create(&journal).unwrap());
    let mut owned = Owned::spawn(&mut command);
    let deadline = Instant::now() + PROCESS_DEADLINE;
    while samples(&rig) == 0 {
        assert!(!process_gone(owned.pid), "service exited");
        assert!(Instant::now() < deadline, "service did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    for deleted in [false, true] {
        if deleted {
            fs::remove_file(&executable).unwrap();
        }
        record(&rig, &executable, NEW_VERSION);
        let before = samples(&rig);
        let deadline = Instant::now() + PROCESS_DEADLINE;
        loop {
            assert!(
                !process_gone(owned.pid),
                "service exited: {}",
                fs::read_to_string(&journal).unwrap()
            );
            let version: String = rig
                .database()
                .query_row("SELECT version FROM store_writer", [], |row| row.get(0))
                .unwrap();
            if version == env!("CARGO_PKG_VERSION") && samples(&rig) > before {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "service did not replace the stale writer"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_current_writer(&rig);
    }
    owned.terminate();
    let text = fs::read_to_string(journal).unwrap();
    assert_eq!(
        text.matches("replaced stale state.db writer").count(),
        2,
        "{text}"
    );
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
fn inconclusive_mcp_probe_preserves_the_session_and_retries_next_input() {
    let rig = Rig::short("upgrade-mcp-pending");
    let executable = replacement(&rig);
    let probe = rig.dir.join("bin/pending-version");
    let ready = rig.dir.join("ready");
    rig.script(
        "pending-version",
        &format!(
            "#!/bin/sh\nif test \"$1\" = --version; then\n  if ! test -f '{}'; then exec sleep 60; fi\n  echo 'dogsled {NEW_VERSION}'; exit\nfi\nexec '{}' \"$@\"\n",
            ready.display(), executable.display()
        ),
    );
    let mut client = Mcp::start(&rig, &executable);
    let pid = client.owned.pid;
    client.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"upgrade-test","version":"0"}
    }}));
    assert_eq!(client.receive(1)["serverInfo"]["name"], "dogsled");
    client.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    // This request has no store effects; even a pending probe must preserve the session.
    client.send(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    assert!(client.receive(2)["tools"].is_array());
    record(&rig, &probe, NEW_VERSION);
    client.send(&json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}));
    assert!(client.receive(3)["tools"].is_array());
    let writer: String = rig
        .database()
        .query_row("SELECT executable FROM store_writer", [], |row| row.get(0))
        .unwrap();
    assert_eq!(writer, probe.to_str().unwrap());
    assert_eq!(client.owned.child_mut().id(), pid);
    fs::write(ready, "ready").unwrap();
    client.send(&json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"status","arguments":{}}}));
    assert_ne!(client.receive(4)["isError"], true);
    wait_for_image(&mut client.owned, &executable);
    assert_eq!(client.owned.child_mut().id(), pid);
    let version: String = rig
        .database()
        .query_row("SELECT version FROM store_writer", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, NEW_VERSION);
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
