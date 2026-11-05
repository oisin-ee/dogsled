use super::*;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::net::UnixListener;
use std::sync::mpsc;

#[path = "observation_portal.rs"]
mod portal;
#[path = "observation_sidebar.rs"]
mod sidebar;

/// A supplied wire publication, not an observer. Its data deliberately differs from the
/// local inputs, so a reader that reconstructs even one section fails these assertions.
pub(super) struct Supplied {
    path: PathBuf,
    updates: mpsc::Sender<Option<Value>>,
    requests: mpsc::Receiver<Value>,
    interests: mpsc::Receiver<(String, bool)>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Supplied {
    pub(super) fn start(rig: &Rig, frame: Value) -> Self {
        Self::start_details(rig, frame, std::collections::BTreeMap::new())
    }

    fn start_details(
        rig: &Rig,
        mut frame: Value,
        details: std::collections::BTreeMap<String, Value>,
    ) -> Self {
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
        let (interest_events, interests) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut subscribers = Vec::new();
            loop {
                match incoming.recv_timeout(Duration::from_millis(5)) {
                    Ok(Some(next)) => {
                        frame = next;
                        subscribers.retain_mut(
                            |(stream, pane): &mut (
                                std::os::unix::net::UnixStream,
                                Option<String>,
                            )| {
                                if pane.is_some() {
                                    return true;
                                }
                                if writeln!(stream, "{frame}").is_ok() {
                                    true
                                } else {
                                    if let Some(pane) = pane {
                                        let _ = interest_events.send((pane.clone(), false));
                                    }
                                    false
                                }
                            },
                        );
                    }
                    Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                subscribers.retain_mut(|(stream, pane)| {
                    let mut byte = [0];
                    stream.set_nonblocking(true).unwrap();
                    let result = stream.read(&mut byte);
                    stream.set_nonblocking(false).unwrap();
                    if matches!(result, Err(error) if error.kind() == std::io::ErrorKind::WouldBlock) { return true; }
                    if let Some(pane) = pane { let _ = interest_events.send((pane.clone(), false)); }
                    false
                });
                while let Ok((mut stream, _)) = listener.accept() {
                    // Darwin streams inherit the listener's O_NONBLOCK; Linux streams do not.
                    stream.set_nonblocking(false).unwrap();
                    stream.set_read_timeout(Some(PROCESS_DEADLINE)).unwrap();
                    stream.set_write_timeout(Some(PROCESS_DEADLINE)).unwrap();
                    let mut line = String::new();
                    BufReader::new(&stream).read_line(&mut line).unwrap();
                    let request: Value = serde_json::from_str(&line).unwrap();
                    assert_eq!(request["version"], 1);
                    let selected = if request["operation"] == "detail" {
                        &details[request["pane"].as_str().unwrap()]
                    } else {
                        &frame
                    };
                    writeln!(stream, "{selected}").unwrap();
                    if request["operation"] == "subscribe" || request["operation"] == "detail" {
                        let pane = (request["operation"] == "detail")
                            .then(|| request["pane"].as_str().unwrap().to_owned());
                        if let Some(pane) = &pane {
                            interest_events.send((pane.clone(), true)).unwrap();
                        }
                        subscribers.push((stream, pane));
                    }
                    requests.send(request).unwrap();
                }
            }
        });
        Self {
            path,
            updates,
            requests: received,
            interests,
            thread: Some(thread),
        }
    }

    pub(super) fn replace(&self, frame: Value) {
        self.updates.send(Some(frame)).unwrap();
    }

    fn interest(&self, pane: &str, registered: bool) {
        assert_eq!(
            self.interests.recv_timeout(PROCESS_DEADLINE).unwrap(),
            (pane.to_owned(), registered)
        );
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
            "jobs":view(json!([job.clone()])),"active_jobs":view(json!([job.clone()])),
            "job_history":view(json!({"w1:p1":{"jobs":[job],"hosts":[{"id":"foreign","label":format!("published-{revision}")}]}})),
            "empty_job_history":view(json!({"jobs":[],"hosts":[]}))},"gaps":[]})
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

    pub(super) fn isolate(rig: &Rig) -> Vec<fs::File> {
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

    /// Pi's one subscription: each revision carries the owner's board and job history together,
    /// an unpublished domain is an explicit frame, and a stopped publisher ends the watch.
    #[test]
    fn board_watch_streams_the_owners_board_and_history_per_revision() {
        let rig = Rig::short("readers-board-watch");
        let server = Supplied::start(&rig, publication(1));
        let _locks = isolate(&rig);
        // A watch is one owner's subscription, never the whole fleet's board.
        let all = rig.run(&["board", "--watch", "--all"]);
        assert!(!all.status.success());
        assert!(String::from_utf8_lossy(&all.stderr).contains("cannot be used with"));
        let mut command = rig.cmd(&["board", "--watch", "--json", "--owner", "w1:p1"]);
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
        let next = || -> Value {
            serde_json::from_str(&lines.recv_timeout(PROCESS_DEADLINE).unwrap()).unwrap()
        };
        server.requested("subscribe");
        for revision in [1, 2] {
            if revision == 2 {
                server.replace(publication(2));
            }
            let frame = publication(revision);
            assert_eq!(
                next(),
                json!({
                    "revision": revision,
                    "board": frame["views"]["boards"]["data"]["w1:p1"],
                    "history": frame["views"]["job_history"]["data"]["w1:p1"],
                })
            );
        }
        let mut missing = publication(3);
        missing["views"]["job_history"]["data"] = Value::Null;
        missing["views"]["job_history"]["gaps"] =
            json!([{"domain":"status","availability":"failed","detail":"capture refused"}]);
        server.replace(missing);
        let unpublished = next();
        assert_eq!(unpublished["revision"], 3);
        assert!(unpublished["board"].is_null(), "{unpublished}");
        assert!(
            unpublished["error"]
                .as_str()
                .unwrap()
                .contains("observation unavailable"),
            "{unpublished}"
        );
        drop(server);
        assert!(!child.wait().success());
        reader.join().unwrap();
        assert_eq!(rig.log("reader-access.log"), "");
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

/// O13: the integrated boundary. Each group is one claim of plans/briefs/observation.md.
mod boundary {
    use super::readers::isolate;
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    type Observer = super::super::service_lanes::Observer;

    /// A PATH where every observation tool records its caller and fails. Readers need none.
    fn reader_path(rig: &Rig) -> PathBuf {
        let bin = rig.dir.join("reader-bin");
        fs::create_dir_all(&bin).unwrap();
        for name in [
            "git", "herdr", "ssh", "df", "ps", "sysctl", "vm_stat", "uptime", "nproc",
        ] {
            let path = bin.join(name);
            fs::write(
                &path,
                "#!/bin/sh\nprintf '%s %s\\n' \"${0##*/}\" \"$*\" >> \"$RIG/reader-access.log\"\nexit 97\n",
            )
            .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        bin
    }

    fn reader(rig: &Rig, path: &Path, args: &[&str]) -> Command {
        let mut command = rig.cmd(args);
        command.env("PATH", path);
        command
    }

    /// Record serve's Git calls while still running the real Git the collector needs.
    fn count_git(rig: &Rig) {
        let output = std::process::Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap();
        let git = String::from_utf8(output.stdout).unwrap();
        rig.script(
            "git",
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$RIG/git.log\"\nexec {} \"$@\"\n",
                git.trim()
            ),
        );
    }

    /// One change-driven collection: rename the first pane and wait until it is published.
    /// Returns the Herdr and Git calls serve made for it.
    fn collect(observer: &mut Observer, name: &str) -> (Vec<String>, Vec<String>) {
        let calls = |log: &str| observer_lines(observer, log);
        let (herdr, git) = (calls("herdr.log"), calls("git.log"));
        let mut panes = observer.rig.read("panes.json");
        panes["result"]["panes"][0]["name"] = json!(name);
        observer.rig.put("panes.json", &panes);
        observer.event("pane.agent_status_changed");
        observer.wait(|frame| frame["local"]["host"]["probe"]["panes"][0]["name"] == name);
        observer.quiet();
        let after = |log: &str, before: usize| {
            observer
                .rig
                .log(log)
                .lines()
                .skip(before)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        (after("herdr.log", herdr), after("git.log", git))
    }

    fn observer_lines(observer: &Observer, log: &str) -> usize {
        observer.rig.log(log).lines().count()
    }

    /// Group 5. Readers and subscribers never observe, and they neither cause nor enlarge
    /// serve's collections.
    #[test]
    fn readers_and_subscriptions_add_no_observation_work() {
        let mut observer = Observer::start("boundary-scale", "3600");
        count_git(&observer.rig);
        let path = reader_path(&observer.rig);
        let (mut herdr, alone) = collect(&mut observer, "alone");
        assert!(!herdr.is_empty(), "a collection must reach Herdr");
        let rig = &observer.rig;
        let quiet = observer.quiet();
        let samples = observer.samples();
        let (herdr_before, git_before) = (rig.log("herdr.log"), rig.log("git.log"));

        let mut watchers = Vec::new();
        let (send, lines) = mpsc::channel();
        for args in [
            &["board", "--watch", "--json", "--owner", "w1:p1"][..],
            &["board", "--watch", "--json", "--owner", "w1:p1"],
            &["board", "--watch", "--json", "--owner", "w2:p1"],
            &["status", "--watch", "--json"],
            &["status", "--watch", "--json"],
        ] {
            let mut command = reader(rig, &path, args);
            command.stdout(Stdio::piped()).stderr(Stdio::null());
            let mut child = Owned::spawn(&mut command);
            let stdout = child.child_mut().stdout.take().unwrap();
            let send = send.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    if send.send(line.unwrap()).is_err() {
                        break;
                    }
                }
            });
            watchers.push(child);
        }
        for _ in &watchers {
            let first: Value =
                serde_json::from_str(&lines.recv_timeout(PROCESS_DEADLINE).unwrap()).unwrap();
            assert!(first.get("error").is_none(), "{first}");
        }
        for _ in 0..4 {
            for args in [
                &["status", "--json"][..],
                &["status", "--line"],
                &["board", "--all", "--json"],
                &["board", "--owner", "w1:p1", "--json"],
                &["items", "--json"],
                &["ps", "--json"],
                &["host", "snapshot", "--json"],
                &["host", "todos", "--json"],
            ] {
                let output = reader(rig, &path, args).output().unwrap();
                assert!(output.status.success(), "{args:?}: {output:?}");
            }
        }
        let mut mcp = super::super::mcp::Client::start(rig);
        for _ in 0..4 {
            let (_, failed) = mcp.call("status", &json!({"all":true}));
            assert!(!failed);
        }
        drop(mcp);

        // Nothing a reader did reached a tool, a collection, or a new publication.
        assert_eq!(rig.log("reader-access.log"), "");
        assert_eq!(rig.log("herdr.log"), herdr_before);
        assert_eq!(rig.log("git.log"), git_before);
        assert_eq!(observer.samples(), samples);
        assert_eq!(observer.current()["revision"], quiet["revision"]);

        // With every subscriber attached, the next change costs what it cost with none.
        let (mut herdr_busy, busy) = collect(&mut observer, "busy");
        // Serve probes panes in parallel, so only the set of calls is stable.
        herdr_busy.sort();
        herdr.sort();
        assert_eq!(herdr_busy, herdr);
        assert_eq!(busy.len(), alone.len(), "{alone:?}\n{busy:?}");
        assert_eq!(observer.samples(), samples + 1);
        for _ in &watchers {
            let next: Value =
                serde_json::from_str(&lines.recv_timeout(PROCESS_DEADLINE).unwrap()).unwrap();
            assert!(next.get("error").is_none(), "{next}");
        }
        assert_eq!(observer.rig.log("reader-access.log"), "");
        drop(watchers);
    }

    /// Group 2. A reader never accepts a publication it cannot trust, and a subscriber never
    /// prints a replacement from another epoch or an older revision.
    #[test]
    fn unsupported_or_out_of_order_publications_are_unavailable() {
        let rig = Rig::short("boundary-versions");
        let mut future = publication(5);
        future["version"] = json!(2);
        let server = Supplied::start(&rig, future);
        let _locks = isolate(&rig);
        for args in [
            vec!["status"],
            vec!["board", "--all"],
            vec!["items"],
            vec!["ps"],
        ] {
            let (code, error) = rig.json(&args);
            assert_ne!(code, 0, "{args:?}");
            assert!(
                error
                    .to_string()
                    .contains("unsupported observation protocol version"),
                "{args:?}: {error}"
            );
        }
        drop(server);
        for replacement in [
            {
                let mut restarted = publication(9);
                restarted["epoch"] = json!("restarted");
                restarted
            },
            publication(4),
        ] {
            let server = Supplied::start(&rig, publication(5));
            let mut command = rig.cmd(&["board", "--watch", "--json", "--owner", "w1:p1"]);
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
            assert_eq!(first["revision"], 5);
            server.replace(replacement.clone());
            assert!(!child.wait().success(), "{replacement}");
            reader.join().unwrap();
            let rest: Vec<String> = lines.try_iter().collect();
            assert!(
                rest.iter().all(|line| !line.contains("\"board\"")),
                "{replacement}: {rest:?}"
            );
            drop(server);
        }
        assert_eq!(rig.log("reader-access.log"), "");
    }

    /// Group 3. A host's paths are its own: a local symlink at the same path cannot rewrite
    /// a foreign path, and readers print it exactly as its host published it.
    #[test]
    fn foreign_paths_survive_local_filesystem_changes() {
        let rig = Rig::short("boundary-foreign");
        let target = rig.dir.join("local-target");
        let link = rig.dir.join("foreign-tree");
        fs::create_dir_all(&target).unwrap();
        let foreign = link.display().to_string();
        let mut frame = publication(1);
        let job = json!({"id":"remote-job","cmd":["true"],"cwd":foreign,"host":"foreign",
            "owner":"w1:p1","pane":"foreign:p1","state":"running"});
        frame["views"]["jobs"]["data"] = json!([job.clone()]);
        frame["views"]["active_jobs"]["data"] = json!([job]);
        let _server = Supplied::start(&rig, frame);
        let _locks = isolate(&rig);
        let cwd = |rig: &Rig| {
            let (code, jobs) = rig.json(&["ps", "--all"]);
            assert_eq!(code, 0, "{jobs}");
            jobs[0]["cwd"].clone()
        };
        assert_eq!(cwd(&rig), json!(foreign));
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(cwd(&rig), json!(foreign));
        fs::remove_file(&link).unwrap();
        fs::create_dir(&link).unwrap();
        assert_eq!(cwd(&rig), json!(foreign));
        assert_eq!(rig.log("reader-access.log"), "");
    }

    /// Group 4. An observation that found a checkout safe to remove authorizes nothing once
    /// the checkout holds new work: the removal revalidates and keeps it.
    #[test]
    fn a_safe_observation_cannot_remove_work_added_after_it() {
        use super::super::close::{close_rig, git};
        let (rig, tree) = close_rig("boundary-safety");
        let repo = rig.dir.join("home/repo");
        fs::write(tree.join("feature"), "work\n").unwrap();
        git(&tree, &["add", "feature"]);
        git(&tree, &["commit", "-q", "-m", "feature"]);
        git(&repo, &["merge", "-q", "--squash", "feat/x"]);
        git(&repo, &["commit", "-q", "-m", "feature (#1)"]);
        rig.put(
            "panes.json",
            &json!({"result":{"panes":[{"pane_id":"w2:p2","workspace_id":"w2","agent":null,"cwd":tree}]}}),
        );
        let (code, plan) = rig.json(&["doctor"]);
        assert_eq!(code, 0, "{plan}");
        let entry = plan["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["class"] == "worktrees")
            .cloned()
            .unwrap_or_else(|| panic!("{plan}"));
        assert_eq!(entry["removable"], true, "{entry}");
        // New work arrives after the safe observation and before the destructive act.
        fs::write(tree.join("unsaved"), "after observation\n").unwrap();
        let plan = json!({"entries":[entry]});
        let (code, applied) = rig.json(&["host", "prune", "--plan", &plan.to_string()]);
        assert_eq!(code, 0, "{applied}");
        assert_eq!(
            fs::read_to_string(tree.join("unsaved")).unwrap(),
            "after observation\n",
            "{applied}"
        );
        assert!(
            !rig.log("herdr.log").contains("workspace close w2"),
            "{applied}"
        );
        assert_eq!(applied["skipped"].as_array().unwrap().len(), 1, "{applied}");
        assert_eq!(applied["failures"], json!([]), "{applied}");
    }
}
