use super::*;
use std::io::{BufRead as _, BufReader, Write as _};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// A `dogsled mcp` client: one JSON-RPC line in, one response line out.
struct Client {
    child: std::process::Child,
    out: BufReader<std::process::ChildStdout>,
    next: u64,
}

impl Client {
    fn start(rig: &Rig) -> Self {
        let mut child = rig
            .cmd(&["mcp"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let out = BufReader::new(child.stdout.take().unwrap());
        let mut client = Self {
            child,
            out,
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
        let stdin = self.child.stdin.as_mut().unwrap();
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
        loop {
            let mut line = String::new();
            assert!(self.out.read_line(&mut line).unwrap() > 0, "server closed");
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] == id {
                return message["result"].clone();
            }
        }
    }
    /// A tool's JSON result, and whether it is a tool error.
    fn call(&mut self, tool: &str, arguments: &Value) -> (Value, bool) {
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
        drop(self.child.stdin.take());
        let _ = self.child.wait();
    }
}

#[test]
fn mcp_lists_the_eleven_tools_with_their_schemas() {
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
            expect("cancel", &["id"]),
            expect("close", &["pane"]),
            expect("done", &[]),
            expect("inbox", &[]),
            expect("place", &[]),
            expect("read", &["target"]),
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
    let (cancelled, _) = client.call("cancel", &json!({"id":result["ticket"]}));
    assert_eq!(cancelled["cancelled"], result["ticket"]);
    assert_eq!(rig.read("state/queue.json")[0]["state"], "cancelled");
}
