//! Exercise the running portal against supplied projections and the live observer.

use super::*;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;

struct Portal {
    _process: Owned,
    port: u16,
}

impl Portal {
    fn start(rig: &Rig) -> Self {
        let mut command = rig.cmd(&["web", "--port", "0"]);
        command
            .stdout(Stdio::piped())
            .stderr(fs::File::create(rig.dir.join("portal.stderr")).unwrap());
        let mut process = Owned::spawn(&mut command);
        let stdout = process.child_mut().stdout.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut line = String::new();
            BufReader::new(stdout).read_line(&mut line).unwrap();
            let _ = sender.send(line);
        });
        let line = receiver.recv_timeout(PROCESS_DEADLINE);
        if line.is_err() {
            process.terminate();
        }
        reader.join().unwrap();
        let line = line.unwrap();
        let port = line
            .split("127.0.0.1:")
            .nth(1)
            .unwrap_or_else(|| panic!("portal startup: {line}; {}", rig.log("portal.stderr")))
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        Self {
            _process: process,
            port,
        }
    }

    fn get(&self, path: &str) -> (u16, BTreeMap<String, String>, Value) {
        self.request("GET", path)
    }

    fn post(&self, path: &str) -> (u16, Value) {
        let (status, _, body) = self.request("POST", path);
        (status, body)
    }

    fn request(&self, method: &str, path: &str) -> (u16, BTreeMap<String, String>, Value) {
        let (mut http, status, headers) = Http::request(self.port, method, path);
        let bytes = if headers
            .get("transfer-encoding")
            .is_some_and(|value| value == "chunked")
        {
            let mut bytes = Vec::new();
            loop {
                let chunk = http.chunk();
                if chunk.is_empty() {
                    break;
                }
                bytes.extend(chunk);
            }
            bytes
        } else {
            let length = headers.get("content-length").unwrap().parse().unwrap();
            let mut bytes = vec![0; length];
            http.reader.read_exact(&mut bytes).unwrap();
            bytes
        };
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, headers, body)
    }

    fn stream(&self) -> Http {
        let (http, status, headers) = Http::open(self.port, "/api/stream");
        assert_eq!(status, 200);
        assert_eq!(headers["content-type"], "text/event-stream");
        http
    }
}

struct Http {
    reader: BufReader<TcpStream>,
    pending: String,
}

impl Http {
    fn open(port: u16, path: &str) -> (Self, u16, BTreeMap<String, String>) {
        Self::request(port, "GET", path)
    }

    fn request(port: u16, method: &str, path: &str) -> (Self, u16, BTreeMap<String, String>) {
        let mut socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
        socket.set_read_timeout(Some(PROCESS_DEADLINE)).unwrap();
        socket.set_write_timeout(Some(PROCESS_DEADLINE)).unwrap();
        write!(
            socket,
            "{method} {path} HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: http://localhost:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut reader = BufReader::new(socket);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let status = line.split_whitespace().nth(1).unwrap().parse().unwrap();
        let mut headers = BTreeMap::new();
        loop {
            line.clear();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if line == "\r\n" {
                break;
            }
            let (name, value) = line.trim().split_once(':').unwrap();
            headers.insert(name.to_lowercase(), value.trim().to_owned());
        }
        (
            Self {
                reader,
                pending: String::new(),
            },
            status,
            headers,
        )
    }

    fn chunk(&mut self) -> Vec<u8> {
        let mut line = String::new();
        assert!(
            self.reader.read_line(&mut line).unwrap() > 0,
            "stream closed"
        );
        let length = usize::from_str_radix(line.trim().split(';').next().unwrap(), 16).unwrap();
        let mut bytes = vec![0; length];
        self.reader.read_exact(&mut bytes).unwrap();
        let mut end = [0; 2];
        self.reader.read_exact(&mut end).unwrap();
        assert_eq!(&end, b"\r\n");
        bytes
    }

    fn event(&mut self) -> (String, String) {
        loop {
            if let Some(end) = self.pending.find("\n\n") {
                let event: String = self.pending.drain(..end + 2).collect();
                let name = event.lines().find_map(|line| line.strip_prefix("event: "));
                if let Some(name) = name {
                    let data = event
                        .lines()
                        .filter_map(|line| line.strip_prefix("data: "))
                        .collect::<Vec<_>>()
                        .join("\n");
                    return (name.to_owned(), data);
                }
            } else {
                let chunk = self.chunk();
                assert!(!chunk.is_empty(), "SSE ended");
                self.pending.push_str(std::str::from_utf8(&chunk).unwrap());
            }
        }
    }

    fn publication(&mut self) -> Value {
        loop {
            let (kind, data) = self.event();
            assert_eq!(kind, "portal", "{kind}: {data}");
            let frame: Value = serde_json::from_str(&data).unwrap();
            if frame.get("observation").is_none() {
                return frame;
            }
            assert!(frame["error"].is_string());
        }
    }
}

/// Native Herdr remains a test transport; serve, the Close adapter and web are real.
struct CloseHerdr {
    path: PathBuf,
    stopped: Arc<std::sync::atomic::AtomicBool>,
    sockets: Arc<Mutex<Vec<std::os::unix::net::UnixStream>>>,
    events: Arc<Mutex<Vec<std::os::unix::net::UnixStream>>>,
    subscriptions: mpsc::Receiver<Value>,
    closes: mpsc::Receiver<Value>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CloseHerdr {
    fn start(rig: &Rig, refuse: bool) -> Self {
        use std::os::unix::net::UnixListener;
        use std::sync::atomic::{AtomicBool, Ordering};
        let path = rig.dir.join("herdr.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopped);
        let sockets = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::clone(&sockets);
        let events = Arc::new(Mutex::new(Vec::new()));
        let subscribers = Arc::clone(&events);
        let (subscribed, subscriptions) = mpsc::channel();
        let (actions, closes) = mpsc::channel();
        let dir = rig.dir.clone();
        let thread = std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                stream.set_read_timeout(Some(PROCESS_DEADLINE)).unwrap();
                stream.set_write_timeout(Some(PROCESS_DEADLINE)).unwrap();
                connections
                    .lock()
                    .unwrap()
                    .push(stream.try_clone().unwrap());
                if stop.load(Ordering::Acquire) {
                    break;
                }
                let mut line = String::new();
                if !matches!(BufReader::new(&stream).read_line(&mut line), Ok(n) if n > 0) {
                    continue;
                }
                let request: Value = serde_json::from_str(&line).unwrap();
                let method = request["method"].as_str().unwrap();
                let mut reply = match method {
                    "agent.list" => {
                        serde_json::from_slice::<Value>(&fs::read(dir.join("agents.json")).unwrap())
                            .unwrap()
                    }
                    "events.subscribe" => json!({"result":{"type":"subscription_started"}}),
                    "pane.read" => {
                        json!({"result":{"type":"pane_read","read":{"text":"","revision":1}}})
                    }
                    "pane.close" => {
                        actions.send(request["params"].clone()).unwrap();
                        if refuse {
                            json!({"error":{"code":"confirmation_required","message":"confirm worktree group"}})
                        } else {
                            let pane = request["params"]["pane_id"].as_str().unwrap();
                            remove_native_pane(&dir, pane);
                            Self::notify(&subscribers, "pane.closed", pane);
                            json!({"result":{}})
                        }
                    }
                    other => panic!("unexpected Close transport method: {other}"),
                };
                reply["id"] = request["id"].clone();
                if writeln!(stream, "{reply}").is_ok() && method == "events.subscribe" {
                    subscribers.lock().unwrap().push(stream);
                    let _ = subscribed.send(request["params"]["subscriptions"].clone());
                }
            }
        });
        Self {
            path,
            stopped,
            sockets,
            events,
            subscriptions,
            closes,
            thread: Some(thread),
        }
    }

    fn ready(&self) {
        loop {
            let filters = self.subscriptions.recv_timeout(PROCESS_DEADLINE).unwrap();
            if ["w1:p1", "w1:p2"].iter().all(|pane| {
                filters.as_array().unwrap().iter().any(|filter| {
                    filter["type"] == "pane.agent_status_changed" && filter["pane_id"] == *pane
                })
            }) {
                assert!(
                    filters
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|f| f["type"] == "pane.closed")
                );
                break;
            }
        }
    }

    fn notify(subscribers: &Mutex<Vec<std::os::unix::net::UnixStream>>, event: &str, pane: &str) {
        let event = json!({"event":event,"data":{"pane_id":pane}});
        let mut subscribers = subscribers.lock().unwrap();
        subscribers.retain_mut(|stream| writeln!(stream, "{event}").is_ok());
        assert!(!subscribers.is_empty(), "no held native event subscription");
    }
}

impl Drop for CloseHerdr {
    fn drop(&mut self) {
        use std::net::Shutdown;
        use std::sync::atomic::Ordering;
        self.stopped.store(true, Ordering::Release);
        for socket in self.sockets.lock().unwrap().iter() {
            let _ = socket.shutdown(Shutdown::Both);
        }
        let _ = std::os::unix::net::UnixStream::connect(&self.path);
        if let Some(thread) = self.thread.take() {
            let result = thread.join();
            if !std::thread::panicking() {
                result.unwrap();
            }
        }
        let _ = fs::remove_file(&self.path);
    }
}

fn remove_native_pane(dir: &Path, pane: &str) {
    // The CLI collector and the socket adapter read the same native evidence.
    for (file, key) in [("agents.json", "agents"), ("panes.json", "panes")] {
        let path = dir.join(file);
        let mut evidence: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        evidence["result"][key]
            .as_array_mut()
            .unwrap()
            .retain(|entry| entry["pane_id"] != pane);
        let next = path.with_extension("next");
        fs::write(&next, serde_json::to_vec(&evidence).unwrap()).unwrap();
        fs::rename(next, path).unwrap();
    }
}

fn seed_close_sessions(rig: &Rig) -> Vec<(PathBuf, Vec<u8>)> {
    let checkout = rig.dir.join("home/checkout");
    repository(&checkout);
    let sentinels = [
        ("tracked", b"committed bytes\n".to_vec()),
        ("dirty", b"untracked\0bytes\n".to_vec()),
    ];
    for (name, bytes) in &sentinels {
        fs::write(checkout.join(name), bytes).unwrap();
    }
    close::git(&checkout, &["add", "tracked"]);
    close::git(&checkout, &["commit", "-qm", "sentinel"]);
    fs::write(checkout.join("tracked"), b"dirty tracked bytes\n").unwrap();
    let mut agents = Vec::new();
    let mut panes = Vec::new();
    for (pane, terminal, status) in [
        ("w1:p1", "sibling-terminal", "working"),
        ("w1:p2", "selected-terminal", "blocked"),
    ] {
        let transcript = checkout.join(format!("{terminal}.jsonl"));
        fs::write(
            &transcript,
            format!("{{\"type\":\"session\",\"id\":\"{terminal}\"}}\n"),
        )
        .unwrap();
        agents.push(
            json!({"pane_id":pane,"terminal_id":terminal,"agent":"pi","agent_status":status,
            "cwd":checkout,"agent_session":{"kind":"path","value":transcript}}),
        );
        panes.push(json!({"pane_id":pane,"terminal_id":terminal,"cwd":checkout}));
    }
    rig.put("agents.json", &json!({"result":{"agents":agents}}));
    rig.put("panes.json", &json!({"result":{"panes":panes}}));
    [
        "tracked",
        "dirty",
        "sibling-terminal.jsonl",
        "selected-terminal.jsonl",
        ".git/HEAD",
        ".git/index",
    ]
    .into_iter()
    .map(|name| {
        let path = checkout.join(name);
        let bytes = fs::read(&path).unwrap();
        (path, bytes)
    })
    .collect()
}

fn start_close_observer(rig: &Rig, native: &CloseHerdr) -> Owned {
    // Longer than every test wait: fallback polling cannot satisfy the assertions.
    let mut command = rig.cmd(&["serve", "--interval", "3600"]);
    command
        .stdout(Stdio::null())
        .stderr(fs::File::create(rig.dir.join("close-observer.stderr")).unwrap());
    let observer = Owned::spawn(&mut command);
    native.ready();
    wait_publication(rig, |_| {
        rig.log("close-observer.stderr")
            .contains("baseline sample published")
    });
    wait_publication(rig, |frame| {
        frame["views"]["portal"]["data"]["agents"]
            .as_array()
            .is_some_and(|agents| agents.len() == 2)
    });
    observer
}

fn assert_close_files(sentinels: &[(PathBuf, Vec<u8>)]) {
    for (path, bytes) in sentinels {
        assert_eq!(
            fs::read(path).unwrap(),
            *bytes,
            "changed {}",
            path.display()
        );
    }
}

fn agent_panes(agents: &Value) -> Vec<String> {
    let mut panes: Vec<_> = agents
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| {
            assert_eq!(agent["host"], "local", "{agent}");
            agent["pane"].as_str().unwrap().to_owned()
        })
        .collect();
    panes.sort();
    panes
}

#[test]
fn observation_portal_close_removes_only_selected_session_from_open_stream_and_get() {
    let rig = Rig::short("portal-close-live");
    let sentinels = seed_close_sessions(&rig);
    let native = CloseHerdr::start(&rig, false);
    let _observer = start_close_observer(&rig, &native);
    let portal = Portal::start(&rig);
    let mut client = portal.stream();
    let before = client.publication();
    assert_eq!(agent_panes(&before["agents"]), ["w1:p1", "w1:p2"]);
    let sibling = before["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["pane"] == "w1:p1")
        .unwrap()
        .clone();
    let (status, body) = portal.post("/api/agents/local/w1:p2/close");
    assert_eq!(status, 202, "{body}");
    assert_eq!(
        native.closes.recv_timeout(PROCESS_DEADLINE).unwrap(),
        json!({"pane_id":"w1:p2"})
    );
    let deadline = Instant::now() + PROCESS_DEADLINE;
    let after = loop {
        let frame = client.publication();
        if agent_panes(&frame["agents"]) == ["w1:p1"] {
            break frame;
        }
        assert!(
            Instant::now() < deadline,
            "Close was not published: {frame}"
        );
    };
    assert_eq!(after["epoch"], before["epoch"]);
    assert!(after["revision"].as_u64().unwrap() > before["revision"].as_u64().unwrap());
    assert_eq!(after["agents"], json!([sibling]));
    let (status, _, body) = portal.get("/api/agents");
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["agents"], after["agents"]);
    assert_eq!(
        rig.read("panes.json")["result"]["panes"],
        json!([{"pane_id":"w1:p1","terminal_id":"sibling-terminal","cwd":rig.dir.join("home/checkout")}])
    );
    let remaining = rig.read("agents.json");
    assert_eq!(remaining["result"]["agents"].as_array().unwrap().len(), 1);
    assert_eq!(
        remaining["result"]["agents"][0]["terminal_id"],
        "sibling-terminal"
    );
    assert!(native.closes.try_recv().is_err(), "closed a sibling");
    assert_close_files(&sentinels);
}

#[test]
fn observation_portal_close_native_refusal_preserves_session_in_get_and_open_stream() {
    let rig = Rig::short("portal-close-refused");
    let sentinels = seed_close_sessions(&rig);
    let native = CloseHerdr::start(&rig, true);
    let _observer = start_close_observer(&rig, &native);
    let portal = Portal::start(&rig);
    let mut client = portal.stream();
    let before = client.publication();
    assert_eq!(agent_panes(&before["agents"]), ["w1:p1", "w1:p2"]);
    let agents = rig.read("agents.json");
    let panes = rig.read("panes.json");
    let (status, body) = portal.post("/api/agents/local/w1:p2/close");
    assert_eq!(status, 502, "{body}");
    assert_eq!(
        body,
        json!({"error":"herdr pane.close: confirmation_required: confirm worktree group"})
    );
    assert_eq!(
        native.closes.recv_timeout(PROCESS_DEADLINE).unwrap(),
        json!({"pane_id":"w1:p2"})
    );
    assert_eq!(rig.read("agents.json"), agents);
    assert_eq!(rig.read("panes.json"), panes);
    // A native topology notification proves the held stream still publishes both sessions
    // after a fresh observer capture. It supplies no replacement portal snapshot.
    let sampled = wait_publication(&rig, |_| true)["local"]["host"]["sampled_ms"]
        .as_u64()
        .unwrap();
    CloseHerdr::notify(&native.events, "pane.updated", "w1:p2");
    let captured = wait_publication(&rig, |frame| {
        frame["local"]["host"]["sampled_ms"]
            .as_u64()
            .is_some_and(|at| at > sampled)
    });
    let revision = captured["revision"].as_u64().unwrap();
    let deadline = Instant::now() + PROCESS_DEADLINE;
    loop {
        let frame = client.publication();
        assert_eq!(frame["agents"], before["agents"]);
        if frame["revision"].as_u64().unwrap() >= revision {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "fresh refused session was not streamed"
        );
    }
    let (status, _, body) = portal.get("/api/agents");
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["agents"], before["agents"]);
    assert!(native.closes.try_recv().is_err());
    assert_close_files(&sentinels);
}

/// Every projection is built by state project's real collector/projector and Publisher.
fn capture(rig: &Rig) -> Value {
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
    assert!(
        frame["views"]["portal"]["data"]["board"].is_object(),
        "{frame}"
    );
    assert!(
        frame["views"]["portal"]["data"]["agents"].is_array(),
        "{frame}"
    );
    assert!(
        frame["views"]["portal"]["data"]["inbox"].is_object(),
        "{frame}"
    );
    frame
}

fn isolate(rig: &Rig) -> Vec<fs::File> {
    // Seeding the stored host holds a fake service lock; transfer it to the trap.
    rig.service.lock().unwrap().take();
    for program in ["herdr", "git", "ssh", "gh"] {
        rig.script(
            program,
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$RIG/portal-access.log\"\nexit 97\n",
        );
    }
    // A store open is invalid even as root; locks also trap attempted initialization.
    fs::remove_file(rig.dir.join("state/state.db")).unwrap();
    fs::create_dir(rig.dir.join("state/state.db")).unwrap();
    ["state-db", "service", "host", "branch-checks"]
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
fn catalogue_serve_publishes_github_fixture_to_api_and_repo_list() {
    let rig = Rig::short("repo-catalogue");
    rig.snapshot("state", now(), 0.0, 8);
    rig.script("gh", r#"#!/bin/sh
if test "$DOGSLED_TEST_ROLE" != observer; then
  echo 'reader invoked GitHub' >> "$RIG/catalogue-reader-access"
  exit 97
fi
if test "$#" != 3 || test "$1" != api || test "$2" != 'user/repos?per_page=100&affiliation=owner,collaborator,organization_member' || test "$3" != --paginate; then
  echo 'unexpected gh arguments' >&2
  exit 98
fi
printf 'call\n' >> "$RIG/catalogue-calls"
cat <<'JSON'
[{"full_name":"owner/old","description":"Older repository","pushed_at":"2026-10-01T00:00:00Z","private":false,"archived":false},{"full_name":"owner/archive","description":null,"pushed_at":"2026-12-01T00:00:00Z","private":false,"archived":true}]
[{"full_name":"owner/fork","description":"Newest fork","pushed_at":"2026-11-01T00:00:00Z","private":true,"archived":false,"fork":true},{"full_name":"owner/unpushed","description":null,"pushed_at":null,"private":false,"archived":false}]
JSON
"#);
    let observer = rig.serve();
    let frame = wait_publication(&rig, |frame| {
        frame["views"]["repos"]["data"]["repos"].is_array()
    });
    let expected = json!({"repos":[
        {"slug":"owner/fork","description":"Newest fork","pushed_at":"2026-11-01T00:00:00Z","private":true},
        {"slug":"owner/old","description":"Older repository","pushed_at":"2026-10-01T00:00:00Z","private":false},
        {"slug":"owner/unpushed","description":null,"pushed_at":null,"private":false}
    ],"error":null});
    assert_eq!(frame["views"]["repos"]["data"], expected);
    let portal = Portal::start(&rig);
    let (status, headers, body) = portal.get("/api/repos");
    assert_eq!(status, 200, "{body}");
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(body, expected);
    // Repo readers do not load policy, discover hosts or invoke gh.
    rig.config("not valid TOML");
    let mut list = rig.start(&["repo", "list", "--json"]);
    assert!(list.wait().success());
    let (stdout, stderr) = list.output();
    assert!(stderr.is_empty(), "{stderr}");
    assert_eq!(serde_json::from_str::<Value>(&stdout).unwrap(), expected);
    let mut list = rig.start(&["repo", "list"]);
    assert!(list.wait().success());
    let (stdout, stderr) = list.output();
    assert!(stderr.is_empty(), "{stderr}");
    assert_eq!(
        stdout,
        "owner/fork\tprivate\t2026-11-01T00:00:00Z\tNewest fork\nowner/old\tpublic\t2026-10-01T00:00:00Z\tOlder repository\nowner/unpushed\tpublic\t\t\n"
    );
    assert_eq!(rig.log("catalogue-calls"), "call\n");
    assert!(!rig.dir.join("catalogue-reader-access").exists());
    drop(observer);
    let (status, _, body) = portal.get("/api/repos");
    assert_eq!(status, 503, "{body}");
    assert!(body["error"].as_str().unwrap().contains("unavailable"));
    let mut list = rig.start(&["repo", "list", "--json"]);
    assert!(!list.wait().success());
    let (stdout, _) = list.output();
    assert!(
        serde_json::from_str::<Value>(&stdout).unwrap()["error"]
            .as_str()
            .unwrap()
            .contains("unavailable")
    );
}

#[test]
fn catalogue_recovery_streams_to_all_clients_without_reader_observation() {
    let rig = Rig::short("catalogue-recovery-stream");
    rig.snapshot("state", now(), 0.0, 8);
    rig.script("gh", r#"#!/bin/sh
printf 'call\n' >> "$RIG/catalogue-calls"
if test -f "$RIG/catalogue-failure"; then
  echo 'connection failed' >&2
  exit 1
fi
printf '[{"full_name":"owner/repo","description":"Published choice","pushed_at":null,"private":false,"archived":false}]\n'
"#);
    let collect = || {
        let observer = rig.serve();
        let frame = wait_publication(&rig, |frame| frame["views"]["repos"]["data"].is_object());
        drop(observer);
        frame
    };
    let good = collect();
    rig.put("catalogue-failure", &json!(true));
    let mut failed = collect();
    fs::remove_file(rig.dir.join("catalogue-failure")).unwrap();
    let mut recovered = collect();
    // Freeze unrelated producer views, supplying only the real catalogue projections
    // from the later startups. Supplied transports catalogue-only changes in one epoch.
    for (frame, revision) in [(&mut failed, 2), (&mut recovered, 3)] {
        let catalogue = frame["views"]["repos"].clone();
        let measured = frame["sources"]["local"]["repos"].clone();
        *frame = good.clone();
        frame["local"]["repos"] = catalogue["data"].clone();
        frame["sources"]["local"]["repos"] = measured;
        frame["views"]["repos"] = catalogue.clone();
        frame["views"]["portal"]["data"]["repos"] = catalogue["data"].clone();
        frame["revision"] = json!(revision);
        frame["views"]["portal"]["data"]["revision"] = json!(revision);
        for domain in ["board", "agents", "inbox"] {
            assert_eq!(
                frame["views"]["portal"]["data"][domain],
                good["views"]["portal"]["data"][domain]
            );
        }
    }
    assert_eq!(good["version"], 2);
    assert_eq!(
        good["views"]["portal"]["data"]["version"],
        portal_types::PORTAL_VERSION
    );
    assert_eq!(rig.log("catalogue-calls"), "call\ncall\ncall\n");
    let supplied = Supplied::start(&rig, good.clone());
    let _locks = isolate(&rig);
    let portal = Portal::start(&rig);
    let mut clients: Vec<_> = (0..4).map(|_| portal.stream()).collect();
    for client in &mut clients {
        assert_eq!(client.publication(), good["views"]["portal"]["data"]);
    }
    supplied.requested("subscribe");
    for frame in [&failed, &recovered] {
        supplied.replace(frame.clone());
        for client in &mut clients {
            assert_eq!(client.publication(), frame["views"]["portal"]["data"]);
        }
        let expected = &frame["views"]["repos"]["data"];
        assert_eq!(frame["views"]["portal"]["data"]["repos"], *expected);
        if frame["revision"] == 2 {
            assert!(
                expected["error"]
                    .as_str()
                    .unwrap()
                    .contains("connection failed")
            );
        } else {
            assert!(expected["error"].is_null());
        }
        let (status, headers, body) = portal.get("/api/repos");
        assert_eq!(status, 200);
        assert_eq!(headers["cache-control"], "no-store");
        assert_eq!(body, *expected);
        // REST keeps its existing one-shot read; it must not add a subscription.
        let request = supplied.requests.recv_timeout(PROCESS_DEADLINE).unwrap();
        assert_eq!(request["operation"], "current");
        assert!(
            supplied.requests.try_recv().is_err(),
            "REST added an IPC subscription"
        );
        // The independent CLI retains its current-publication operation and wire shape.
        let mut list = rig.start(&["repo", "list", "--json"]);
        assert!(list.wait().success());
        let (stdout, stderr) = list.output();
        assert!(stderr.is_empty(), "{stderr}");
        assert_eq!(serde_json::from_str::<Value>(&stdout).unwrap(), *expected);
        let request = supplied.requests.recv_timeout(PROCESS_DEADLINE).unwrap();
        assert_eq!(request["operation"], "current");
    }
    assert!(supplied.requests.try_recv().is_err());
    assert_eq!(
        rig.log("portal-access.log"),
        "",
        "zero reader gh, Git, Herdr or SSH calls"
    );
    assert_eq!(rig.log("catalogue-calls"), "call\ncall\ncall\n");
    assert!(rig.dir.join("state/state.db").is_dir());
}

#[test]
fn portal_readers_share_one_revision_and_one_subscription_without_observation() {
    let rig = Rig::short("portal-publication");
    rig.snapshot("state", now(), 0.0, 8);
    let first = capture(&rig);
    rig.snapshot("state", now(), 2.0, 8);
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{
            "pane_id":"w1:p1", "agent":"pi", "agent_status":"working", "name":"New agent"
        }]}}),
    );
    rig.put_state(&["state", "question", "waiting", "--put"], &json!({"pending":{
        "id":"waiting", "pane":"w1:p1", "session":"session", "cwd":"/repo", "created_ms":now(),
        "questions":{"title":"Choose", "questions":[{"id":"q", "type":"text", "question":"Which?"}]}
    }}));
    let mut second = capture(&rig);
    for domain in ["board", "agents", "inbox"] {
        assert_ne!(
            first["views"]["portal"]["data"][domain],
            second["views"]["portal"]["data"][domain]
        );
    }
    // Supplied is a transport fixture: retain one producer epoch and advance its revision.
    second["epoch"] = first["epoch"].clone();
    second["revision"] = json!(2);
    second["views"]["portal"]["data"]["epoch"] = first["epoch"].clone();
    second["views"]["portal"]["data"]["revision"] = json!(2);
    let supplied = Supplied::start(&rig, first.clone());
    let _locks = isolate(&rig);
    let portal = Portal::start(&rig);
    let mut clients = vec![portal.stream()];
    assert_eq!(clients[0].publication(), first["views"]["portal"]["data"]);
    supplied.requested("subscribe");
    for path in ["/api/board", "/api/agents", "/api/inbox"] {
        let (status, headers, data) = portal.get(path);
        assert_eq!(status, 200, "{path}: {data}");
        assert_eq!(headers["x-observation-revision"], "1");
        assert_eq!(
            headers["x-observation-epoch"],
            first["epoch"].as_str().unwrap()
        );
        let expected = &first["views"]["portal"]["data"];
        match path {
            "/api/board" => assert_eq!(data, expected["board"]),
            "/api/agents" => assert_eq!(data["agents"], expected["agents"]),
            _ => assert_eq!(data, expected["inbox"]),
        }
    }
    for _ in 0..7 {
        let mut client = portal.stream();
        assert_eq!(client.publication(), first["views"]["portal"]["data"]);
        clients.push(client);
    }
    supplied.replace(second.clone());
    for client in &mut clients {
        let event = client.publication();
        assert_eq!(event, second["views"]["portal"]["data"]);
        assert_eq!(event["revision"], 2);
    }
    let unexpected = supplied.requests.try_recv();
    assert!(
        unexpected.is_err(),
        "eight clients share the one subscription; unexpected request: {unexpected:?}"
    );
    assert_eq!(
        rig.log("portal-access.log"),
        "",
        "zero Herdr, Git or SSH calls"
    );
    assert!(rig.dir.join("state/state.db").is_dir());
    drop(supplied);
    for client in &mut clients {
        let (kind, data) = client.event();
        assert_eq!(kind, "portal");
        let status: Value = serde_json::from_str(&data).unwrap();
        assert_eq!(status["observation"], "disconnected");
        assert!(status["error"].as_str().unwrap().contains("observation"));
    }
    for path in ["/api/board", "/api/agents", "/api/inbox"] {
        let (status, _, data) = portal.get(path);
        assert_eq!(status, 503);
        assert!(data["error"].as_str().unwrap().contains("observation"));
    }
    assert_eq!(rig.log("portal-access.log"), "");
}

/// Capture detail through the actual serve interest path before trapping every reader tool.
fn capture_agent_detail(rig: &Rig) -> (Value, Value) {
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let transcript = rig.dir.join("conversation.jsonl");
    fs::write(&transcript, "{\"type\":\"session\",\"id\":\"detail-session\"}\n{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Fixture title\\npublished conversation\"}]}}\n").unwrap();
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{
            "pane_id":"w1:p1", "agent":"pi", "agent_status":"working",
            "terminal_id":"detail-terminal", "agent_session":{"kind":"path","value":transcript}
        }]}}),
    );
    let path = rig.dir.join("detail-herdr.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&stopped);
    let agents = rig.dir.join("agents.json");
    let thread = std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            if stop.load(Ordering::Acquire) {
                break;
            }
            stream.set_read_timeout(Some(PROCESS_DEADLINE)).unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            let mut reply = match request["method"].as_str().unwrap() {
                "agent.list" => {
                    serde_json::from_slice::<Value>(&fs::read(&agents).unwrap()).unwrap()
                }
                "pane.read" => {
                    json!({"result":{"type":"pane_read","read":{"text":"published screen","revision":11}}})
                }
                "events.subscribe" => {
                    json!({"error":{"code":"unsupported","message":"fixture uses the fallback"}})
                }
                other => panic!("unexpected detail observation: {other}"),
            };
            reply["id"] = request["id"].clone();
            writeln!(stream, "{reply}").unwrap();
        }
    });
    let mut command = rig.cmd(&["serve", "--interval", "60"]);
    command.env("HERDR_SOCKET_PATH", &path);
    let observer = Rig::background(command);
    wait_publication(rig, |frame| {
        frame["views"]["portal"]["data"]["agents"][0]["pane"] == "w1:p1"
    });
    let mut interest = UnixStream::connect(rig.dir.join("state/observation.sock")).unwrap();
    writeln!(
        interest,
        "{}",
        json!({"version":2,"operation":"detail","pane":"w1:p1"})
    )
    .unwrap();
    let frame = wait_publication(rig, |frame| {
        frame["views"]["agent_details"]["data"]["w1:p1"]["screen_revision"] == 11
    });
    interest.set_read_timeout(Some(PROCESS_DEADLINE)).unwrap();
    let mut reader = BufReader::new(interest);
    let content = loop {
        let mut line = String::new();
        assert!(reader.read_line(&mut line).unwrap() > 0);
        let detail: Value = serde_json::from_str(&line).unwrap();
        if detail["detail"]["conversation"]["availability"] == "available" {
            break detail;
        }
    };
    assert_eq!(
        content["detail"]["conversation"]["data"]["turns"][0]["text"],
        "Fixture title\npublished conversation"
    );
    assert_eq!(
        frame["views"]["portal"]["data"]["agents"][0]["title"],
        "Fixture title"
    );
    assert_eq!(
        frame["local"]["terminals"]["agents"][0]["session"]["activity"],
        "Fixture title"
    );
    assert!(!frame.to_string().contains("published conversation"));
    assert!(!frame.to_string().contains("published screen"));
    drop(observer);
    drop(reader);
    stopped.store(true, Ordering::Release);
    let _ = UnixStream::connect(&path);
    thread.join().unwrap();
    fs::remove_file(transcript).unwrap();
    (frame, content)
}

#[test]
fn detail_readers_use_session_qualified_publication_without_observation() {
    let rig = Rig::short("portal-detail-isolation");
    let (frame, content) = capture_agent_detail(&rig);
    let supplied = Supplied::start_details(
        &rig,
        frame.clone(),
        BTreeMap::from([("w1:p1".into(), content.clone())]),
    );
    let _locks = isolate(&rig);
    let portal = Portal::start(&rig);
    assert_eq!(
        portal.stream().publication(),
        frame["views"]["portal"]["data"]
    );
    let expected = &content["detail"];
    for field in ["conversation", "screen"] {
        let (status, headers, data) = portal.get(&format!("/api/agents/local/w1:p1/{field}"));
        assert_eq!(status, 200, "{data}");
        assert_eq!(data, expected[field]["data"]);
        supplied.interest("w1:p1", true);
        supplied.interest("w1:p1", false);
        assert_ne!(data["session"].as_str().unwrap(), "");
        assert_eq!(
            headers["x-observation-revision"],
            content["revision"].to_string()
        );
    }
    let (mut events, status, _) = Http::open(portal.port, "/api/agents/local/w1:p1/events");
    assert_eq!(status, 200);
    let (kind, data) = events.event();
    let data: Value = serde_json::from_str(&data).unwrap();
    assert_eq!(kind, "agent");
    supplied.interest("w1:p1", true);
    assert_eq!(data["state"], "working");
    assert_eq!(data["session"]["availability"], "available");
    assert_eq!(
        data["session"]["key"],
        expected["conversation"]["data"]["session"]
    );
    assert_eq!(rig.log("portal-access.log"), "");
    assert!(rig.dir.join("state/state.db").is_dir());
    drop(events);
    supplied.interest("w1:p1", false);
    assert!(supplied.interests.try_recv().is_err());
    drop(supplied);
}

#[test]
fn cold_portal_reports_unavailable_and_connects_when_publication_arrives() {
    let rig = Rig::short("portal-unavailable");
    rig.snapshot("state", now(), 0.0, 8);
    let frame = capture(&rig);
    let _locks = isolate(&rig);
    let portal = Portal::start(&rig);
    let mut client = portal.stream();
    let (kind, data) = client.event();
    assert_eq!(kind, "portal");
    let status: Value = serde_json::from_str(&data).unwrap();
    assert_eq!(status["observation"], "unavailable");
    assert!(
        status["error"]
            .as_str()
            .unwrap()
            .contains("observation unavailable")
    );
    for path in ["/api/board", "/api/agents", "/api/inbox"] {
        let (status, _, data) = portal.get(path);
        assert_eq!(status, 503);
        assert!(
            data["error"]
                .as_str()
                .unwrap()
                .contains("observation unavailable")
        );
    }
    let _publisher = Supplied::start(&rig, frame.clone());
    assert_eq!(client.publication(), frame["views"]["portal"]["data"]);
    assert_eq!(rig.log("portal-access.log"), "");
}
