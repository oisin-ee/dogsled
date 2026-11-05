use super::*;
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixListener;
use std::sync::mpsc;

/// A supplied wire publication, not an observer. Its data deliberately differs from the
/// local inputs, so a reader that reconstructs even one section fails these assertions.
pub(super) struct Supplied {
    path: PathBuf,
    updates: mpsc::Sender<Option<Value>>,
    requests: mpsc::Receiver<Value>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Supplied {
    pub(super) fn start(rig: &Rig, mut frame: Value) -> Self {
        let path = rig.dir.join("state/observation.sock");
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            use std::os::unix::fs::FileTypeExt as _;
            assert!(metadata.file_type().is_socket(), "{}", path.display());
            assert_eq!(
                std::os::unix::net::UnixStream::connect(&path)
                    .unwrap_err()
                    .kind(),
                std::io::ErrorKind::ConnectionRefused
            );
            fs::remove_file(&path).unwrap();
        }
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let (updates, incoming) = mpsc::channel();
        let (requests, received) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut subscribers = Vec::new();
            loop {
                match incoming.recv_timeout(Duration::from_millis(5)) {
                    Ok(Some(next)) => {
                        frame = next;
                        subscribers.retain_mut(|stream: &mut std::os::unix::net::UnixStream| {
                            writeln!(stream, "{frame}").is_ok()
                        });
                    }
                    Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                while let Ok((mut stream, _)) = listener.accept() {
                    stream.set_read_timeout(Some(PROCESS_DEADLINE)).unwrap();
                    stream.set_write_timeout(Some(PROCESS_DEADLINE)).unwrap();
                    let mut line = String::new();
                    BufReader::new(&stream).read_line(&mut line).unwrap();
                    let request: Value = serde_json::from_str(&line).unwrap();
                    assert_eq!(request["version"], 1);
                    writeln!(stream, "{frame}").unwrap();
                    if request["operation"] == "subscribe" {
                        subscribers.push(stream);
                    }
                    requests.send(request).unwrap();
                }
            }
        });
        Self {
            path,
            updates,
            requests: received,
            thread: Some(thread),
        }
    }

    pub(super) fn replace(&self, frame: Value) {
        self.updates.send(Some(frame)).unwrap();
    }

    fn requested(&self, operation: &str) {
        let deadline = Instant::now() + PROCESS_DEADLINE;
        loop {
            let request = self
                .requests
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            if request["operation"] == operation {
                return;
            }
        }
    }
}

impl Drop for Supplied {
    fn drop(&mut self) {
        let _ = self.updates.send(None);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
        fs::remove_file(&self.path).unwrap();
    }
}

/// Capacity fixtures publish the producer's projection of the exact seeded rows that
/// reserve revalidates. No live sampler may replace those rows during this guard.
pub(super) fn resolved(rig: &Rig) -> Supplied {
    let publisher = seeded(rig);
    let (code, frame) = rig.json(&["observation", "current"]);
    assert_eq!(code, 0, "{frame}");
    assert!(
        frame["gaps"].as_array().unwrap().is_empty(),
        "resolve projection gap: {}",
        frame["gaps"]
    );
    for domain in ["hosts", "agents", "findings"] {
        assert!(
            !frame["views"][domain]["data"].is_null(),
            "{domain}: {frame}"
        );
    }
    publisher
}

pub(super) fn seeded(rig: &Rig) -> Supplied {
    let mut command = rig.cmd(&["state", "project"]);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    serde_json::to_writer(child.stdin.take().unwrap(), &rig.read("machines.json")).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let frame: Value = serde_json::from_slice(&output.stdout).unwrap();
    let publisher = Supplied::start(rig, frame);
    wait_publication(rig, |frame| {
        frame["views"]["hosts"]["data"][0]["probe"] == rig.host()["probe"]
    });
    publisher
}

#[path = "../fixtures/observation/load.rs"]
pub(super) mod fixtures;

pub(super) fn host_display(
    label: &str,
    load: &str,
    state: &str,
    sampled_ms: Option<u64>,
    condition: &str,
) -> Value {
    json!({"id":label,"brief":{"label":label,"load":load,"state":state,"measured":true},"condition":condition,"sampled_ms":sampled_ms,
        "metrics":null,"states":{},"agents":[],"jobs":[],"reservations":0,"lanes":[],"reason":null,"error":null})
}

pub(super) fn display(hosts: Vec<Value>) -> Value {
    let hosts = Value::Array(hosts);
    json!({"hosts":hosts,"counts":{"open":0,"finished":0,"orphans":null,"queued":0,"stalls":0},"holds":[],"sweep":null,"items":[],"queue":[]})
}

pub(super) fn publication(revision: u64) -> Value {
    let board = json!({"hosts":[{"label":format!("published-{revision}"),"service":"up","reachable":true,"cores":8,"eligible":false}],"rows":[]});
    let status = json!({"hosts":[],"items":[],"queue":[],"orphans":[],"sweep":null,"stopped_activity":{"revision":revision.to_string()},"holds":[]});
    let host = json!({"service_lanes":[],"running":true,"handoffs":[],"job_validity":2,"request_validity":1,"placement_replay":3,"holds":[]});
    let todos = json!([]);
    let job = json!({"id":format!("job-{revision}"),"cmd":["true"],"cwd":"/foreign/tree","host":"foreign","owner":"w1:p1","pane":"foreign:p1","state":"running"});
    let inventory = json!({"items":[{"id":format!("item-{revision}")}],"queue":[{"id":format!("queue-{revision}")}]});
    let view = |data: Value| json!({"data":data,"gaps":[]});
    json!({"version":1,"epoch":"supplied","revision":revision,"local_revision":1,"published_ms":revision,"sources":{"local":{},"peers":{}},
        "local":{"host":host,"todos":["not-the-todos-view"],"status":{"items":["not-the-status-view"]}},
        "views":{"status":view(status),"status_display":view(display(vec![host_display(&format!("published-{revision}"), "0/8", "eligible", Some(1), "service: up")])), "board":view(board.clone()),"boards":view(json!({"w1:p1":board.clone()})),"empty_board":view(board),
            "todos":view(todos),"inventory":view(inventory.clone()),"inventories":view(json!({"w1:p1":inventory})),"empty_inventory":view(json!({"items":[],"queue":[]})),
            "jobs":view(json!([job.clone()])),"active_jobs":view(json!([job]))},"gaps":[]})
}

mod readers {
    use super::*;

    #[test]
    fn stored_projection_uses_only_read_only_herdr_and_preserves_seeded_capacity_and_store_bytes() {
        for (load, eligible) in [(0.0, true), (3.0, false)] {
            let rig = Rig::short(&format!("stored-project-{eligible}"));
            rig.snapshot("state", now(), load, 2);
            let database = rig.database();
            database
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .unwrap();
            let before = fs::read(rig.dir.join("state/state.db")).unwrap();
            for name in ["git", "ssh", "df"] {
                rig.script(
                    name,
                    "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$RIG/projection-probes.log\"\nexit 97\n",
                );
            }
            let publisher = seeded(&rig);
            let (_, frame) = rig.json(&["observation", "current"]);
            assert_eq!(
                frame["views"]["hosts"]["data"][0]["probe"],
                rig.host()["probe"]
            );
            assert_eq!(
                frame["views"]["hosts"]["data"][0]["headroom"]["eligible"],
                eligible
            );
            let reads = rig.log("herdr.log");
            assert!(reads.lines().any(|line| line == "agent list"), "{reads}");
            assert!(reads.lines().any(|line| line == "pane list"), "{reads}");
            assert!(
                reads
                    .lines()
                    .all(|line| matches!(line, "agent list" | "pane list")
                        || line.starts_with("pane process-info --pane ")),
                "{reads}"
            );
            assert_eq!(rig.log("projection-probes.log"), "");
            assert_eq!(fs::read(rig.dir.join("state/state.db")).unwrap(), before);
            drop(publisher);
            drop(database);
        }
    }

    fn isolate(rig: &Rig) -> Vec<fs::File> {
        for command in ["git", "herdr", "ssh"] {
            rig.script(
                command,
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$RIG/reader-access.log\"\nexit 97\n",
            );
        }
        // Any store open or repair fails, even when the test runs as a privileged user.
        fs::create_dir(rig.dir.join("state/state.db")).unwrap();
        [
            "observation",
            "service",
            "state-db",
            "host",
            "branch-checks",
            "peer-8103",
        ]
        .into_iter()
        .map(|name| {
            let file = fs::File::options()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(rig.dir.join(format!("state/{name}.lock")))
                .unwrap();
            file.try_lock().unwrap();
            file
        })
        .collect()
    }

    #[test]
    fn each_reader_selects_one_supplied_revision_without_observation() {
        let rig = Rig::short("readers-supplied");
        let frame = publication(41);
        let _server = Supplied::start(&rig, frame.clone());
        let _locks = isolate(&rig);
        let before = rig.log("reader-access.log");
        for (args, expected) in [
            (vec!["status"], frame["views"]["status"]["data"].clone()),
            (
                vec!["board", "--all"],
                frame["views"]["board"]["data"].clone(),
            ),
            (
                vec!["board", "--owner", "w1:p1"],
                frame["views"]["boards"]["data"]["w1:p1"].clone(),
            ),
            (
                vec!["board", "--owner", "unknown:p1"],
                frame["views"]["empty_board"]["data"].clone(),
            ),
            (vec!["host", "snapshot"], frame["local"]["host"].clone()),
            (
                vec!["host", "todos"],
                frame["views"]["todos"]["data"].clone(),
            ),
            (vec!["items"], frame["views"]["inventory"]["data"].clone()),
            (
                vec!["items", "--owner", "w1:p1"],
                frame["views"]["inventories"]["data"]["w1:p1"].clone(),
            ),
        ] {
            let (code, actual) = rig.json(&args);
            assert_eq!(code, 0, "{args:?}: {actual}");
            assert_eq!(actual, expected, "{args:?}");
        }
        for args in [vec!["ps"], vec!["ps", "--all"]] {
            let (code, actual) = rig.json(&args);
            assert_eq!(code, 0, "{actual}");
            assert_eq!(actual[0]["id"], "job-41");
            assert_eq!(actual[0]["host"], "foreign");
        }
        for args in [
            vec!["status", "--line"],
            vec!["status", "--brief"],
            vec!["status"],
        ] {
            let output = rig.run(&args);
            assert!(output.status.success(), "{output:?}");
            let text = String::from_utf8(output.stdout).unwrap();
            assert!(text.contains("published-41 0/8"), "{text}");
        }
        let mut mcp = super::super::mcp::Client::start(&rig);
        for args in [json!({"all":true}), json!({"all":false})] {
            let (board, failed) = mcp.call("status", &args);
            assert!(!failed, "{board}");
            assert_eq!(board, frame["views"]["board"]["data"]);
        }
        assert_eq!(rig.log("reader-access.log"), before);
        assert!(rig.dir.join("state/state.db").is_dir());
    }

    #[test]
    fn cold_readers_return_unavailable_without_reconstruction() {
        let rig = Rig::short("readers-cold");
        let _locks = isolate(&rig);
        for args in [
            vec!["status"],
            vec!["status", "--watch"],
            vec!["board", "--all"],
            vec!["host", "snapshot"],
            vec!["host", "todos"],
            vec!["items"],
            vec!["ps"],
        ] {
            let (code, error) = rig.json(&args);
            assert_ne!(code, 0, "{args:?}");
            assert!(
                error["error"]
                    .as_str()
                    .unwrap()
                    .contains("observation unavailable: dogsled serve is not running"),
                "{args:?}: {error}"
            );
        }
        let mut mcp = super::super::mcp::Client::start(&rig);
        let (error, failed) = mcp.call("status", &json!({"all":true}));
        assert!(failed);
        assert!(
            error
                .as_str()
                .unwrap()
                .contains("observation unavailable: dogsled serve is not running"),
            "{error}"
        );
        assert_eq!(rig.log("reader-access.log"), "");
    }

    #[test]
    fn cli_readers_bypass_configuration_loading() {
        let rig = Rig::short("readers-config");
        let _server = Supplied::start(&rig, publication(1));
        rig.config("not valid TOML = [");
        for args in [
            vec!["status"],
            vec!["board", "--all"],
            vec!["host", "snapshot"],
            vec!["host", "todos"],
            vec!["items"],
            vec!["ps"],
        ] {
            assert_eq!(rig.json(&args).0, 0, "{args:?}");
        }
    }

    #[test]
    fn display_uses_published_classification_and_reader_clock() {
        let rig = Rig::short("readers-display");
        fs::write(rig.dir.join("clock"), "61000").unwrap();
        let mut frame = publication(1);
        // Raw status says no hosts, items or agents. Display deliberately contradicts it.
        let mut host = host_display("published", "0/8", "no_headroom", Some(1000), "service: up");
        host["agents"] =
            json!([{"pane":"worker","state":"blocked","name":"published state","stalled_ms":null}]);
        host["states"] = json!({"blocked":1});
        let mut facts = display(vec![host]);
        facts["counts"] = json!({"open":9,"finished":3,"orphans":2,"queued":7,"stalls":5});
        frame["views"]["status_display"]["data"] = facts;
        let server = Supplied::start(&rig, frame);
        let _locks = isolate(&rig);
        let line = rig.run(&["status", "--line"]);
        assert_eq!(String::from_utf8(line.stdout).unwrap(), "published 0/8 ▲\n");
        let brief = rig.run(&["status", "--brief"]);
        let brief = String::from_utf8(brief.stdout).unwrap();
        assert!(brief.contains("service: up (snapshot 1m ago)"), "{brief}");
        assert!(
            brief.contains("9 open items (3 finished) · 2 orphaned · 7 queued · 5 stalls"),
            "{brief}"
        );
        let full = rig.run(&["status"]);
        assert!(
            String::from_utf8(full.stdout)
                .unwrap()
                .contains("worker blocked published state")
        );
        fs::write(rig.dir.join("clock"), "121000").unwrap();
        let brief = rig.run(&["status", "--brief"]);
        assert!(
            String::from_utf8(brief.stdout)
                .unwrap()
                .contains("snapshot 2m ago")
        );
        assert_eq!(rig.log("reader-access.log"), "");
        drop(server);
    }

    #[test]
    fn absent_views_and_retained_data_show_explicit_gaps() {
        let rig = Rig::short("readers-gaps");
        let mut frame = publication(1);
        let gap = json!({"domain":"status","availability":"failed","detail":"capture refused"});
        frame["views"]["status"]["gaps"] = json!([gap.clone()]);
        frame["views"]["board"]["data"] = Value::Null;
        frame["views"]["board"]["gaps"] = json!([gap.clone()]);
        frame["gaps"] = json!([gap]);
        let _server = Supplied::start(&rig, frame);
        let out = rig.run(&["status", "--json"]);
        assert!(out.status.success(), "{out:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("capture refused"));
        let out = rig.run(&["board", "--all", "--json"]);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("capture refused"));
        assert!(String::from_utf8_lossy(&out.stdout).contains("observation unavailable"));
    }

    #[test]
    fn watch_subscribes_to_complete_replacements_and_marks_disconnect() {
        let rig = Rig::short("readers-watch");
        let server = Supplied::start(&rig, publication(1));
        let _locks = isolate(&rig);
        let mut command = rig.cmd(&["status", "--watch", "--json"]);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = Owned::spawn(&mut command);
        let stdout = child.child_mut().stdout.take().unwrap();
        let (send, lines) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if send.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        server.requested("subscribe");
        let first: Value =
            serde_json::from_str(&lines.recv_timeout(PROCESS_DEADLINE).unwrap()).unwrap();
        assert_eq!(first, publication(1)["views"]["status"]["data"]);
        server.updates.send(Some(publication(2))).unwrap();
        let next: Value =
            serde_json::from_str(&lines.recv_timeout(PROCESS_DEADLINE).unwrap()).unwrap();
        assert_eq!(next, publication(2)["views"]["status"]["data"]);
        drop(server);
        assert!(!child.wait().success());
        let error: Value =
            serde_json::from_str(&lines.recv_timeout(PROCESS_DEADLINE).unwrap()).unwrap();
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("disconnected/stale; retained revision 2"),
            "{error}"
        );
        reader.join().unwrap();
        assert_eq!(rig.log("reader-access.log"), "");
    }
}
