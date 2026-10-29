use super::*;
use std::io::{BufRead as _, BufReader, Write as _};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// The time the server gets to end on its own after its stdin closes. A server that
/// outlives this window is killed, so a test never hangs on it.
const EXIT_GRACE: Duration = Duration::from_secs(5);

/// The time a receive may wait for the deadline, or nothing when the deadline has
/// passed. A receive with no time left can still return a queued line, so a server
/// that keeps writing without the answer would outlive the bound.
fn time_left(deadline: Instant) -> Option<Duration> {
    let now = Instant::now();
    (now < deadline).then(|| deadline - now)
}

/// A `dogsled mcp` client: one JSON-RPC line in, one response line out.
pub(super) struct Client {
    /// The server the test owns as its own child. The test reaps it and reads its exit
    /// status. A failed assertion kills the server and its own group.
    owned: Owned,
    /// The server's output lines, in order. A reader thread drains the pipe, so a full
    /// pipe never blocks the server.
    lines: std::sync::mpsc::Receiver<String>,
    /// The thread that reads the server's output. It ends when the server's output
    /// closes.
    reader: Option<std::thread::JoinHandle<()>>,
    next: u64,
}

impl Client {
    pub(super) fn start(rig: &Rig) -> Self {
        let mut command = rig.cmd(&["mcp"]);
        command.stdin(Stdio::piped()).stdout(Stdio::piped());
        let mut owned = Owned::spawn(&mut command);
        let stdout = owned.child_mut().stdout.take().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        // The reader thread sends every line the server writes. A server that never
        // writes leaves the channel empty, so the wait for a response ends on the bound
        // instead of hanging the test.
        let reader = std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let mut client = Self {
            owned,
            lines: receiver,
            reader: Some(reader),
            next: 0,
        };
        let init = client.request(
            "initialize",
            &json!({"protocolVersion":"2025-06-18","capabilities":{},
                "clientInfo":{"name":"test","version":"0"}}),
        );
        assert_eq!(init["serverInfo"]["name"], "dogsled", "{init}");
        client.notify("notifications/initialized");
        client
    }
    fn write(&mut self, message: &Value) {
        let stdin = self.owned.child_mut().stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }
    fn notify(&mut self, method: &str) {
        self.write(&json!({"jsonrpc":"2.0","method":method}));
    }
    fn request(&mut self, method: &str, params: &Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.write(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        // The whole request is bounded, not only each read. A server that keeps writing
        // lines without the answer fails its test on the bound instead of looping for
        // ever.
        let deadline = Instant::now() + PROCESS_DEADLINE;
        loop {
            let line = self.next_line(method, deadline);
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] == id {
                return message["result"].clone();
            }
        }
    }
    /// The server's next line, or fail the test on the bound. A server that never
    /// answers fails its test instead of hanging it. A closed server is a closed
    /// channel, which the reader thread signals by ending.
    fn next_line(&mut self, method: &str, deadline: Instant) -> String {
        // The deadline is checked before each receive. A receive with no time left can
        // still return a queued line, so a server that keeps writing without the answer
        // would outlive the bound.
        let Some(bound) = time_left(deadline) else {
            panic!("the server did not answer {method} within {PROCESS_DEADLINE:?}")
        };
        match self.lines.recv_timeout(bound) {
            Ok(line) => line,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                panic!("the server wrote no line for {method} within {PROCESS_DEADLINE:?}")
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => panic!("server closed"),
        }
    }
    /// A tool's JSON result, and whether it is a tool error.
    pub(super) fn call(&mut self, tool: &str, arguments: &Value) -> (Value, bool) {
        let result = self.request("tools/call", &json!({"name":tool,"arguments":arguments}));
        let text = result["content"][0]["text"].as_str().unwrap_or_default();
        (
            serde_json::from_str(text).unwrap_or_else(|_| json!(text)),
            result["isError"] == true,
        )
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // The server's stdin closes first, so a server that waits for its client ends on
        // its own.
        drop(self.owned.child_mut().stdin.take());
        // The server gets a short grace window to end on its own. A server that does not
        // end is killed below, so the test never hangs on it.
        let deadline = Instant::now() + EXIT_GRACE;
        while Instant::now() < deadline {
            match self.owned.child_mut().try_wait() {
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Ok(Some(_)) | Err(_) => break,
            }
        }
        // The kill comes before the reap, so the process group is never signalled after
        // the host may have given its id to another group.
        self.owned.terminate();
        // The reader ends when the server's output closes, which the kill ensures.
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[test]
fn mcp_retirement_requires_live_custody_and_has_no_cleanup_override() {
    let (rig, _) = preparation_retirement::prepared("mcp-retirement", "a");
    let original = rig.read("state/preparations.json");
    let mut client = Client::start(&rig);
    let (refusal, error) = client.call(
        "retire_preparation",
        &json!({"request_id":"a","reason":"audit"}),
    );
    assert!(error, "{refusal}");
    assert_eq!(rig.read("state/preparations.json"), original);
    let tools = client.request("tools/list", &json!({}));
    let tool = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "retire_preparation")
        .unwrap();
    assert!(tool["inputSchema"]["properties"].get("cleanup").is_none());
    assert!(tool["inputSchema"]["properties"].get("actor").is_none());
    assert_eq!(tool["inputSchema"]["additionalProperties"], false);
    rig.put("agents.json", &json!({"result":{"agents":[{"pane_id":"w1:p1","agent":"pi","agent_session":{"kind":"id","value":"owner-session"}}]}}));
    let host = String::from_utf8(Command::new("hostname").output().unwrap().stdout)
        .unwrap()
        .trim()
        .to_owned();
    rig.put("state/custody.json", &json!([{"item":"a","host":host,"owner_pane":"w1:p1","owner_session":"owner-session","custodian_pane":"w1:p1","custodian_session":"owner-session","revision":4,"claims":[]}]));
    rig.put("agents.json", &json!({"result":{"agents":[{"pane_id":"w1:p1","agent":"pi","agent_session":{"kind":"id","value":"reused-session"}}]}}));
    let (refused, error) = client.call(
        "retire_preparation",
        &json!({"request_id":"a","reason":"audit"}),
    );
    assert!(error, "{refused}");
    assert_eq!(rig.read("state/preparations.json"), original);
    rig.put("agents.json", &json!({"result":{"agents":[{"pane_id":"w1:p1","agent":"pi","agent_session":{"kind":"id","value":"owner-session"}}]}}));
    let (result, error) = client.call(
        "retire_preparation",
        &json!({"request_id":"a","reason":"audit"}),
    );
    assert!(!error, "{result}");
    assert_eq!(result["status"], "retired");
    assert_eq!(result["task"], "unresolved");
    assert_eq!(result["audit"]["authority"]["record"]["revision"], 4);
    assert_eq!(rig.read("state/preparations.json"), json!([]));
    let (duplicate, error) = client.call(
        "retire_preparation",
        &json!({"request_id":"a","reason":"audit"}),
    );
    assert!(!error, "{duplicate}");
    assert_eq!(duplicate, result);
}
#[test]
fn p2_recovery_tools_share_the_runtime() {
    let rig = Rig::short("p2-mcp");
    let repo = rig.dir.join("repo");
    let snapshot = preservation::saved(&repo, false);
    let mut client = Client::start(&rig);
    let (entries, failed) = client.call("recovery_list", &json!({"repo":repo}));
    assert!(!failed, "{entries}");
    assert_eq!(entries[0]["id"], snapshot["id"]);
    let (shown, failed) = client.call("recovery_show", &json!({"repo":repo,"id":snapshot["id"]}));
    assert!(!failed, "{shown}");
    assert_eq!(shown, snapshot);
    let to = rig.dir.join("restored");
    let (result, failed) = client.call(
        "recovery_restore",
        &json!({"repo":repo,"id":snapshot["id"],"to":to}),
    );
    assert!(!failed, "{result}");
    assert_eq!(fs::read(to.join("file")).unwrap(), b"raw\0\xff\n");
}

#[test]
fn mcp_place_resume_replays_original_request_and_rejects_overrides() {
    let (rig, _, id) = place::interrupted_placement("placement-result-saved");
    let original = rig.read(&format!("state/admissions/{id}.json"));
    let mut client = Client::start(&rig);
    for field in ["host", "branch", "base", "cwd"] {
        let mut args = json!({"resume":id});
        args[field] = json!("changed");
        let (error, failed) = client.call("place", &args);
        assert!(failed, "{field}: {error}");
        assert!(error.as_str().unwrap().contains(&id), "{error}");
        assert_eq!(rig.read(&format!("state/admissions/{id}.json")), original);
    }
    let (result, failed) = client.call("place", &json!({"resume":id}));
    assert!(!failed, "{result}");
    assert_eq!(result, original["placement"]["saved"]["result"]);
    let (again, failed) = client.call("place", &json!({"resume":id}));
    assert!(!failed, "{again}");
    assert_eq!(again, result);
    assert_eq!(rig.log("setup.log").lines().count(), 1);
    assert_eq!(rig.log("herdr.log").matches("worktree open").count(), 1);
    let (error, failed) = client.call("place", &json!({"resume":"unknown-mcp-placement"}));
    assert!(failed);
    assert!(error.as_str().unwrap().contains("unknown-mcp-placement"));
}

#[test]
fn review_plain_preparation_error_contains_resumable_request_id() {
    let (rig, repo) = place::place_rig("mcp-plain-error");
    let mut client = Client::start(&rig);
    let (error, failed) = client.call(
        "place",
        &json!({"host":"local","cwd":repo,"branch":"invalid..branch"}),
    );
    assert!(failed, "{error}");
    let text = error.as_str().unwrap();
    let id = text
        .strip_prefix("placement request ")
        .expect("plain preparation error must include request id")
        .split(':')
        .next()
        .unwrap();
    let original = rig.read(&format!("state/admissions/{id}.json"));
    assert_eq!(original["request"]["id"], id);
    let grant = rig.read(&format!("state/placement-grants/{id}.json"));
    let (again, failed) = client.call("place", &json!({"resume":id}));
    assert!(failed, "{again}");
    assert!(again.as_str().unwrap().contains(id), "{again}");
    assert!(
        !again
            .as_str()
            .unwrap()
            .contains("unknown placement request")
    );
    assert_eq!(
        rig.read(&format!("state/placement-grants/{id}.json")),
        grant
    );
    assert_eq!(
        rig.read("state/placements.json").as_array().unwrap().len(),
        1
    );
    assert!(!rig.log("herdr.log").contains("worktree open"));
    assert!(!rig.dir.join("setup.log").exists());
}

#[test]
fn mcp_place_initial_and_resume_share_branchless_result() {
    let rig = Rig::short("mcp-place-resume");
    rig.snapshot("state/host.json", rig.origin_ms, 0.0, 8);
    let mut client = Client::start(&rig);
    let (result, failed) = client.call("place", &json!({"host":"local"}));
    assert!(!failed, "{result}");
    let id = result["request_id"].as_str().unwrap();
    let (replayed, failed) = client.call("place", &json!({"resume":id}));
    assert!(!failed, "{replayed}");
    assert_eq!(result, replayed);
    assert!(result["path"].is_null());
    assert!(result["pane_id"].is_null());
    assert_eq!(
        rig.read("state/placements.json").as_array().unwrap().len(),
        1
    );
}

#[test]
fn mcp_request_deadline_is_checked_before_each_receive() {
    // A receive with no time left can still return a queued line, so a server that keeps
    // writing without the answer would outlive the bound. The deadline is checked
    // before the receive, so a passed deadline gives no time at all.
    let (sender, receiver) = std::sync::mpsc::channel();
    sender.send("queued line").unwrap();
    assert!(time_left(Instant::now() + Duration::from_secs(1)).is_some());
    assert!(time_left(Instant::now()).is_none());
    // The line stays queued, because the check runs before the receive.
    assert_eq!(receiver.try_recv().unwrap(), "queued line");
}

#[test]
fn mcp_lists_the_tools_with_their_schemas() {
    let rig = Rig::new("mcp-list");
    let mut client = Client::start(&rig);
    let tools = client.request("tools/list", &json!({}));
    let mut names: Vec<(String, Vec<String>)> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            let mut required: Vec<String> = t["inputSchema"]["required"]
                .as_array()
                .map(|r| r.iter().map(|v| v.as_str().unwrap().to_owned()).collect())
                .unwrap_or_default();
            required.sort();
            (t["name"].as_str().unwrap().to_owned(), required)
        })
        .collect();
    names.sort();
    let expect = |name: &str, required: &[&str]| {
        (
            name.to_owned(),
            required.iter().map(|r| (*r).to_owned()).collect::<Vec<_>>(),
        )
    };
    assert_eq!(
        names,
        [
            expect("accept", &["evidence", "item", "result"]),
            expect("ask", &["question"]),
            expect("blocked", &["reason"]),
            expect("cancel", &["id"]),
            expect("claim", &["item"]),
            expect("close", &["pane"]),
            expect("done", &[]),
            expect("drop", &["id"]),
            expect("inbox", &[]),
            expect("place", &[]),
            expect("read", &["target"]),
            expect("recovery_list", &["repo"]),
            expect("recovery_restore", &["id", "repo", "to"]),
            expect("recovery_show", &["id", "repo"]),
            expect("retire_preparation", &["reason", "request_id"]),
            expect("run", &["cmd"]),
            expect("send", &["message", "target"]),
            expect("spawn", &[]),
            expect("status", &[]),
            expect("stop", &["id"]),
        ]
    );
    let spawn = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "spawn")
        .unwrap();
    let alternatives = spawn["inputSchema"]["anyOf"].as_array().unwrap();
    assert_eq!(alternatives.len(), 3, "{spawn}");
    let schema = &spawn["inputSchema"];
    let legacy = alternatives[2]["$ref"]
        .as_str()
        .map_or(&alternatives[2], |reference| {
            schema
                .pointer(reference.strip_prefix('#').unwrap())
                .unwrap()
        });
    assert_eq!(legacy["additionalProperties"], false, "{schema}");
    for field in ["profile", "kind", "host", "branch", "base", "cwd"] {
        assert!(legacy["properties"][field].is_object(), "{spawn}");
    }
}

#[test]
fn mcp_custody_tools_claim_and_accept_without_completing_the_item() {
    let rig = super::claim::rig("mcp-custody", "w9:p9");
    super::claim::returned(&rig);
    let before = fs::read(rig.dir.join("state/items.json")).unwrap();
    let mut client = Client::start(&rig);
    let (claim, failed) = client.call("claim", &json!({"item":super::claim::ID,"revision":0}));
    assert!(!failed, "{claim}");
    assert_eq!(claim["custodianPane"], "w1:p1");
    let (receipt, failed) = client.call(
        "accept",
        &json!({
            "item":super::claim::ID,"result":"res-1","evidence":"reviewed exact result"
        }),
    );
    assert!(!failed, "{receipt}");
    assert_eq!(receipt["claimId"], claim["claimId"]);
    assert_eq!(receipt["resultId"], "res-1");
    assert_eq!(receipt["reviewer"], "w1:p1");
    assert_eq!(fs::read(rig.dir.join("state/items.json")).unwrap(), before);
}

#[test]
fn mcp_custody_tools_report_refusals() {
    let rig = super::claim::rig("mcp-custody-refused", "w2:p1");
    let mut client = Client::start(&rig);
    let (error, failed) = client.call("claim", &json!({"item":super::claim::ID}));
    assert!(failed, "{error}");
    assert!(error.as_str().unwrap().contains("not_orphan"));
    let (error, failed) = client.call(
        "accept",
        &json!({
            "item":super::claim::ID,"result":"res-1","evidence":"reviewed"
        }),
    );
    assert!(failed, "{error}");
    assert!(error.as_str().unwrap().contains("custody"));
}

#[test]
fn mcp_spawn_accepts_only_unit_prepared_or_legacy_and_handler_commits() {
    use super::unit_requests::*;
    let rig = rig("mcp-unit", true);
    let execution = prepare(&rig);
    let input = commit_input(&execution, 0);
    let mut client = Client::start(&rig);
    let (error, failed) = client.call("spawn", &json!({"unit":{"id":"u-17"}}));
    assert!(failed, "{error}");
    assert!(error.as_str().unwrap().contains("PROVIDER_UNAVAILABLE"));
    assert!(admission(&rig, &execution)["unit"]["committed"].is_null());
    for invalid in [
        json!({"unit":{"id":"u-17"},"task":"go","name":"w"}),
        json!({"unit":{"id":"u-17"},"cwd":"/tmp"}),
        json!({"preparedUnit":input,"host":"local"}),
        json!({"preparedUnit":input,"name":"w","task":"go"}),
        json!({"preparedUnit":input,"unit":{"id":"u-17"}}),
    ] {
        let response = client.request("tools/call", &json!({"name":"spawn","arguments":invalid}));
        assert!(
            response.is_null() || response["isError"] == true,
            "{response}"
        );
        assert!(admission(&rig, &execution)["unit"]["committed"].is_null());
    }
    let (receipt, failed) = client.call("spawn", &json!({"preparedUnit":input}));
    assert!(!failed, "{receipt}");
    assert_eq!(receipt["disposition"], "queued");
    assert_eq!(receipt["execution"], execution);
    assert_eq!(admission(&rig, &execution)["unit"]["committed"], input);
    assert_eq!(
        client.call("spawn", &json!({"preparedUnit":input})).0,
        receipt
    );
    assert_eq!(rig.read("state/queue.json").as_array().unwrap().len(), 1);
    assert!(!rig.log("herdr.log").contains("agent start"));
}

#[test]
fn mcp_send_writes_one_inbox_record_from_the_callers_pane() {
    let rig = Rig::new("mcp-send");
    let listed = json!([
        {"pane_id":"w1:p1","agent":"pi","agent_status":"idle","name":"lead","terminal_id":"term_lead"},
        {"pane_id":"w1:p2","agent":"claude","agent_status":"idle","name":"worker","terminal_id":"term_work"}
    ]);
    rig.put("agents.json", &json!({"result":{"agents":listed}}));
    rig.put(
        "state/host.json",
        &json!({"sampled_ms":now(),"probe":{"host":"local","label":"local","load1":0.0,"cores":8,
            "memory":{"used_bytes":0,"total_bytes":1},"disk_free_percent":50.0,
            "agents":{"total":2,"by_status":{}},"reservations":[],"panes":[],"agent_panes":[
                {"pane":"w1:p1","agent":"pi","state":"idle","name":"lead","transcript":null,
                 "transcript_modified":null,"terminal_id":"term_lead"},
                {"pane":"w1:p2","agent":"claude","state":"idle","name":"worker","transcript":null,
                 "transcript_modified":null,"terminal_id":"term_work"}]}}),
    );
    let out = rig
        .cmd(&["inbox", "claim", "--harness", "claude"])
        .env("HERDR_PANE_ID", "w1:p2")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut client = Client::start(&rig);
    let (sent, error) = client.call(
        "send",
        &json!({"target":"worker","message":"Line one.\nLine two."}),
    );
    assert!(!error, "{sent}");
    assert_eq!(sent["pane"], "w1:p2");
    let inbox = rig.read("state/inbox/term_work.json");
    assert_eq!(inbox.as_array().unwrap().len(), 1, "{inbox}");
    assert_eq!(inbox[0]["from"]["pane"], "w1:p1");
    assert_eq!(inbox[0]["from"]["name"], "lead");
    assert_eq!(inbox[0]["text"], "Line one.\nLine two.");
    // A refusal is a tool error with the reason, and the server keeps serving.
    let (refused, error) = client.call("send", &json!({"target":"nobody","message":"hi"}));
    assert!(error);
    assert!(
        refused.as_str().unwrap().contains("no agent named"),
        "{refused}"
    );
    let (listing, error) = client.call("inbox", &json!({}));
    assert!(!error, "{listing}");
    assert_eq!(listing["sent"][0]["pane"], "w1:p2");
    assert!(!rig.log("herdr.log").contains("agent prompt"));
}

#[test]
fn mcp_spawn_with_capacity_hands_startup_to_the_service() {
    for fails in [false, true] {
        let rig = Rig::new(&format!("mcp-startup-handoff-{fails}"));
        rig.snapshot("state/host.json", now(), 0.0, 8);
        // Admission behavior is independent of the CI runner's real probe load/disk.
        rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
        if fails {
            launch_retry::npm_start_fails(&rig);
        }
        let mut client = Client::start(&rig);
        let (result, error) = client.call("spawn", &json!({"name":"worker","task":"go"}));
        assert!(!error, "{result}");
        assert_eq!(result["queued"], true, "{result}");
        let ticket = rig.read("state/queue.json")[0].clone();
        assert_eq!(ticket["id"], result["ticket"]);
        assert_eq!(ticket["state"], "pending");
        assert!(!rig.log("herdr.log").contains("agent start"));
        assert!(!rig.log("herdr.log").contains("tab create"));
        let out = rig.run(&["serve", "--once", "--interval", "1"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let ticket = rig.read("state/queue.json")[0].clone();
        assert_eq!(
            rig.log("herdr.log").matches("agent start worker ").count(),
            1
        );
        if fails {
            assert_eq!(ticket["state"], "pending", "{ticket}");
            assert!(
                ticket["error"]
                    .as_str()
                    .unwrap()
                    .contains(launch_retry::NPM_FAILURE),
                "{ticket}"
            );
            let id = ticket["request"]["id"].as_str().unwrap();
            let progress = rig.read(&format!("state/launches/{id}.progress.json"));
            assert!(
                progress["error"]
                    .as_str()
                    .unwrap()
                    .contains(launch_retry::NPM_FAILURE),
                "{progress}"
            );
            assert_eq!(progress["confirmed"], false);
            assert!(!rig.log("herdr.log").contains("pane close"));
        } else {
            assert_eq!(ticket["state"], "admitted", "{ticket}");
            assert_eq!(ticket["result"]["pane"], "w2:p1");
        }
    }
}

#[test]
fn mcp_prepared_spawn_with_capacity_queues_idempotently_before_startup() {
    use super::unit_requests::*;
    let rig = rig("mcp-prepared-handoff", false);
    // Admission behavior is independent of the CI runner's real probe load/disk.
    rig.config("free_core_fraction_min = 0\nreservation_load = 0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0\n");
    let execution = prepare(&rig);
    let input = commit_input(&execution, 0);
    let mut client = Client::start(&rig);
    let (receipt, failed) = client.call("spawn", &json!({"preparedUnit":input}));
    assert!(!failed, "{receipt}");
    assert_eq!(receipt["disposition"], "queued");
    assert_eq!(receipt["execution"], execution);
    assert_eq!(admission(&rig, &execution)["unit"]["committed"], input);
    assert_eq!(
        client.call("spawn", &json!({"preparedUnit":input})).0,
        receipt
    );
    assert_eq!(rig.read("state/queue.json").as_array().unwrap().len(), 1);
    assert!(!rig.log("herdr.log").contains("agent start"));
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ticket = rig.read("state/queue.json")[0].clone();
    assert_eq!(ticket["state"], "admitted", "{ticket}");
    let (launched, failed) = client.call("spawn", &json!({"preparedUnit":input}));
    assert!(!failed, "{launched}");
    assert_eq!(launched["disposition"], "launched");
    assert_eq!(launched["pane"], "w2:p1");
    assert_eq!(rig.log("herdr.log").matches("agent start ").count(), 1);
}

#[test]
fn mcp_spawn_returns_queued_at_once_when_every_host_is_full() {
    let rig = Rig::new("mcp-full");
    rig.snapshot("state/host.json", now(), 9999.0, 1);
    let mut client = Client::start(&rig);
    let started = Instant::now();
    let (result, error) = client.call(
        "spawn",
        &json!({"name":"w","task":"Review the diff.\nThen report.","profile":null}),
    );
    // Alone this takes about 0.2 s. A spawn that waited for admission would hold until its
    // 30 s budget, since no service runs here; 5 s separates the two under parallel test load.
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert!(!error, "{result}");
    assert_eq!(result["queued"], true, "{result}");
    let ticket = &rig.read("state/queue.json")[0];
    assert_eq!(ticket["id"], result["ticket"]);
    assert_eq!(ticket["request"]["owner"], "w1:p1");
    assert_eq!(
        ticket["request"]["action"]["message"],
        "Review the diff.\nThen report."
    );
    assert_eq!(ticket["waiting"], false);
    assert!(!rig.log("herdr.log").contains("agent start"));
    // A profile is for Pi.
    let (refused, error) = client.call(
        "spawn",
        &json!({"name":"c","task":"hi","kind":"claude","profile":"general"}),
    );
    assert!(error, "{refused}");
    let (cancelled, error) = client.call("cancel", &json!({"id":result["ticket"]}));
    assert!(!error, "{cancelled}");
    assert_eq!(cancelled["cancelled"], result["ticket"]);
    assert_eq!(cancelled["future_admission"], "prohibited");
    assert_eq!(cancelled["release"], "not_needed");
    let (duplicate, error) = client.call("cancel", &json!({"id":result["ticket"]}));
    assert!(!error, "{duplicate}");
    assert_eq!(duplicate, cancelled);
    assert_eq!(rig.read("state/queue.json")[0]["state"], "cancelled");
}
