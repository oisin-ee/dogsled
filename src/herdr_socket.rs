//! A small blocking client for Herdr's socket API: newline-delimited JSON over
//! the Unix socket at `HERDR_SOCKET_PATH`, one request and one response per
//! connection. It carries the methods that have no CLI form:
//! `layout.export`, `pane.layout`, `layout.set_split_ratio`, and `pane.focus`,
//! and the agent methods the portal reads and writes: `agent.list`,
//! `pane.read`, `agent.prompt`, `agent.send_keys`, and `agent.focus`.
//! A subscription keeps one connection open and pushes events over it.

use crate::error::{Error, Result};
use serde_json::{Value, json};
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);
/// How long a subscription waits for one event before it reports that it is
/// idle. A short wait lets a reader notice that its caller went away.
const SUBSCRIBE_POLL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Client {
    path: PathBuf,
}

impl Client {
    /// The socket Herdr injects into every pane and plugin command.
    pub fn from_env() -> Result<Self> {
        std::env::var_os("HERDR_SOCKET_PATH")
            .filter(|path| !path.is_empty())
            .map(Self::at)
            .ok_or_else(|| Error::Invalid("HERDR_SOCKET_PATH is required to dock the board".into()))
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Send one request and return its `result`. A Herdr error response, a
    /// transport failure, or a malformed reply is `Error::Invalid` with the detail.
    pub fn call(&self, method: &str, params: &Value) -> Result<Value> {
        let fail = |detail: String| Error::Invalid(format!("herdr {method}: {detail}"));
        let mut stream = UnixStream::connect(&self.path)
            .map_err(|error| fail(format!("{}: {error}", self.path.display())))?;
        stream
            .set_read_timeout(Some(TIMEOUT))
            .and_then(|()| stream.set_write_timeout(Some(TIMEOUT)))
            .map_err(|error| fail(error.to_string()))?;
        let id = format!("dogsled_{}", crate::ledger::new_id());
        let mut line =
            serde_json::to_string(&json!({"id": id, "method": method, "params": params}))?;
        line.push('\n');
        stream
            .write_all(line.as_bytes())
            .map_err(|error| fail(error.to_string()))?;
        let mut reply = String::new();
        BufReader::new(stream)
            .read_line(&mut reply)
            .map_err(|error| fail(error.to_string()))?;
        if reply.trim().is_empty() {
            return Err(fail("connection closed without a response".into()));
        }
        let reply: Value = serde_json::from_str(&reply)
            .map_err(|error| fail(format!("invalid response: {error}")))?;
        if let Some(error) = reply.get("error") {
            return Err(fail(format!(
                "{}: {}",
                error["code"].as_str().unwrap_or("error"),
                error["message"].as_str().unwrap_or_default()
            )));
        }
        if reply["id"] != id.as_str() {
            return Err(fail("response id does not match the request".into()));
        }
        reply
            .get("result")
            .cloned()
            .ok_or_else(|| fail("response has no result".into()))
    }

    /// The portable layout tree (`root` of `pane`/`split` nodes) of the tab holding `pane_id`.
    pub fn layout_export(&self, pane_id: &str) -> Result<Value> {
        layout(
            "layout.export",
            &self.call("layout.export", &json!({"pane_id": pane_id}))?,
        )
    }

    /// The rect snapshot (`area`, `panes`, `splits`) of the tab holding `pane_id`.
    pub fn pane_layout(&self, pane_id: &str) -> Result<Value> {
        layout(
            "pane.layout",
            &self.call("pane.layout", &json!({"pane_id": pane_id}))?,
        )
    }

    /// Set the first-child ratio of the split at `path` (`false` = first, `true` = second).
    pub fn set_split_ratio(&self, tab_id: &str, path: &[bool], ratio: f32) -> Result<Value> {
        layout(
            "layout.set_split_ratio",
            &self.call(
                "layout.set_split_ratio",
                &json!({"tab_id": tab_id, "path": path, "ratio": ratio}),
            )?,
        )
    }

    /// Focus `pane_id`, switching to its tab. The reply is a `pane_info` with no layout.
    pub fn focus_pane(&self, pane_id: &str) -> Result<Value> {
        self.call("pane.focus", &json!({"pane_id": pane_id}))
    }

    /// The agents Herdr sees, as the `agents` array of an `agent_list` reply.
    pub fn agent_list(&self) -> Result<Vec<Value>> {
        let result = self.call("agent.list", &json!({}))?;
        Ok(result["agents"].as_array().cloned().unwrap_or_default())
    }

    /// The text on a pane, as a `pane_read` reply.
    pub fn pane_read(&self, pane_id: &str, source: &str, lines: Option<u32>) -> Result<Value> {
        let mut params = json!({"pane_id": pane_id, "source": source});
        if let Some(lines) = lines {
            params["lines"] = json!(lines);
        }
        let result = self.call("pane.read", &params)?;
        Ok(result["read"].clone())
    }

    /// Submit a prompt to the agent in `target`, which is a pane id or an
    /// agent name.
    pub fn agent_prompt(&self, target: &str, text: &str) -> Result<Value> {
        self.call("agent.prompt", &json!({"target": target, "text": text}))
    }

    /// Send key presses to the agent in `target`.
    pub fn agent_send_keys(&self, target: &str, keys: &[String]) -> Result<Value> {
        self.call("agent.send_keys", &json!({"target": target, "keys": keys}))
    }

    /// Focus the agent in `target`, and mark its completion as seen.
    pub fn agent_focus(&self, target: &str) -> Result<Value> {
        self.call("agent.focus", &json!({"target": target}))
    }

    /// Open a subscription to Herdr's events.
    ///
    /// The reply acknowledges the subscription. Later lines carry the events
    /// themselves. The connection stays open until the subscription is dropped.
    pub fn subscribe(&self, subscriptions: &[Value]) -> Result<Subscription> {
        Subscription::open(self, subscriptions)
    }
}

/// One event from a subscription, or the fact that none arrived yet.
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    /// An event envelope, with its `event` name and its `data`.
    Event(Value),
    /// No event arrived before the poll ended.
    Idle,
    /// Herdr closed the connection.
    Closed,
}

/// A live subscription to Herdr's events.
///
/// A thread reads the socket and hands the lines over, so a slow event never
/// sits in a socket buffer that a read timeout would cut in half.
#[derive(Debug)]
pub struct Subscription {
    lines: std::sync::mpsc::Receiver<std::result::Result<String, String>>,
    /// Closing this handle wakes the reader even when Herdr sends no events.
    socket: UnixStream,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl Subscription {
    /// Open the subscription and read the acknowledgement.
    fn open(client: &Client, subscriptions: &[Value]) -> Result<Self> {
        let fail = |detail: String| Error::Invalid(format!("herdr events.subscribe: {detail}"));
        let mut stream = UnixStream::connect(&client.path)
            .map_err(|error| fail(format!("{}: {error}", client.path.display())))?;
        stream
            .set_write_timeout(Some(TIMEOUT))
            .map_err(|error| fail(error.to_string()))?;
        let id = format!("dogsled_{}", crate::ledger::new_id());
        let mut line = serde_json::to_string(
            &json!({"id": id, "method": "events.subscribe", "params": {"subscriptions": subscriptions}}),
        )?;
        line.push('\n');
        stream
            .write_all(line.as_bytes())
            .map_err(|error| fail(error.to_string()))?;
        let mut reader = BufReader::new(stream);
        let mut reply = String::new();
        reader
            .read_line(&mut reply)
            .map_err(|error| fail(error.to_string()))?;
        if reply.trim().is_empty() {
            return Err(fail("connection closed without a response".into()));
        }
        let reply: Value = serde_json::from_str(&reply)
            .map_err(|error| fail(format!("invalid response: {error}")))?;
        if let Some(error) = reply.get("error") {
            return Err(fail(format!(
                "{}: {}",
                error["code"].as_str().unwrap_or("error"),
                error["message"].as_str().unwrap_or_default()
            )));
        }
        // Keep a socket handle so dropping the subscription interrupts its reader.
        let socket = reader.get_ref().try_clone()?;
        let (sender, receiver) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in reader.lines() {
                let message = match line {
                    Ok(line) => Ok(line),
                    Err(error) => Err(error.to_string()),
                };
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            lines: receiver,
            socket,
            reader: Some(reader),
        })
    }

    /// The next event, or that none arrived yet.
    pub fn next(&mut self) -> Result<Next> {
        match self.lines.recv_timeout(SUBSCRIBE_POLL) {
            // The thread read a line.
            Ok(Ok(line)) => {
                if line.trim().is_empty() {
                    return Ok(Next::Idle);
                }
                Self::parse(&line)
            }
            // The thread failed to read the socket.
            Ok(Err(detail)) => Err(Error::Invalid(format!("herdr events.subscribe: {detail}"))),
            // No event arrived before the poll ended. The connection stays open.
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(Next::Idle),
            // Herdr closed the subscription.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Ok(Next::Closed),
        }
    }

    /// One event line, as the API reads it.
    fn parse(line: &str) -> Result<Next> {
        let value: Value = serde_json::from_str(line)
            .map_err(|error| Error::Invalid(format!("herdr events.subscribe: {error}")))?;
        // Herdr reports a lost event history as an error on the same line.
        if let Some(error) = value.get("error") {
            return Err(Error::Invalid(format!(
                "herdr events.subscribe: {}: {}",
                error["code"].as_str().unwrap_or("error"),
                error["message"].as_str().unwrap_or_default()
            )));
        }
        Ok(Next::Event(value))
    }
}

fn layout(method: &str, result: &Value) -> Result<Value> {
    result
        .get("layout")
        .filter(|layout| layout.is_object())
        .cloned()
        .ok_or_else(|| Error::Invalid(format!("herdr {method}: response has no layout")))
}

/// A scripted Herdr socket in a temp dir. `answer` maps `(method, params)` to
/// the response body without its `id` (`{"result": …}` or `{"error": …}`).
#[cfg(test)]
pub(crate) mod fake {
    use super::Client;
    use serde_json::Value;
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    pub struct Server {
        pub path: PathBuf,
        pub requests: Arc<Mutex<Vec<Value>>>,
        stop: Arc<AtomicBool>,
    }

    impl Server {
        pub fn start(answer: impl Fn(&str, &Value) -> Value + Send + 'static) -> Self {
            // Unix socket paths are short (~104 bytes on macOS); keep the name small.
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "hs-{}-{}.sock",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_file(&path);
            let listener = UnixListener::bind(&path).expect("bind fake Herdr socket");
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let (log, halt) = (Arc::clone(&requests), Arc::clone(&stop));
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if halt.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(mut stream) = stream else { continue };
                    let mut line = String::new();
                    if BufReader::new(&stream).read_line(&mut line).is_err() || line.is_empty() {
                        continue;
                    }
                    let request: Value = serde_json::from_str(&line).expect("request is JSON");
                    log.lock().unwrap().push(request.clone());
                    let mut body = answer(
                        request["method"].as_str().unwrap_or_default(),
                        &request["params"],
                    );
                    body["id"] = request["id"].clone();
                    let _ = writeln!(stream, "{body}");
                }
            });
            Self {
                path,
                requests,
                stop,
            }
        }

        pub fn client(&self) -> Client {
            Client::at(&self.path)
        }

        pub fn methods(&self) -> Vec<String> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .map(|request| request["method"].as_str().unwrap_or_default().to_owned())
                .collect()
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            let _ = UnixStream::connect(&self.path);
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// A fake Herdr socket that also serves subscriptions. The `events` lines
    /// are pushed after the acknowledgement of `events.subscribe`.
    pub struct EventServer {
        pub path: PathBuf,
        pub requests: Arc<Mutex<Vec<Value>>>,
        stop: Arc<AtomicBool>,
    }

    impl EventServer {
        pub fn start(events: Vec<String>) -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "hs-e-{}-{}.sock",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_file(&path);
            let listener = UnixListener::bind(&path).expect("bind fake Herdr socket");
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let (log, halt) = (Arc::clone(&requests), Arc::clone(&stop));
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if halt.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(mut stream) = stream else { continue };
                    let mut line = String::new();
                    if BufReader::new(&stream).read_line(&mut line).is_err() || line.is_empty() {
                        continue;
                    }
                    let request: Value = serde_json::from_str(&line).expect("request is JSON");
                    log.lock().unwrap().push(request.clone());
                    if request["method"] == "events.subscribe" {
                        let mut body =
                            serde_json::json!({"result": {"type": "subscription_started"}});
                        body["id"] = request["id"].clone();
                        let _ = writeln!(stream, "{body}");
                        for event in &events {
                            let _ = writeln!(stream, "{event}");
                        }
                        continue;
                    }
                    let mut body = serde_json::json!({"result": {"type": "ok"}});
                    body["id"] = request["id"].clone();
                    let _ = writeln!(stream, "{body}");
                }
            });
            Self {
                path,
                requests,
                stop,
            }
        }

        pub fn client(&self) -> Client {
            Client::at(&self.path)
        }
    }

    impl Drop for EventServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            let _ = UnixStream::connect(&self.path);
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn herdr_socket_sends_one_request_line_and_returns_its_result() {
        let server = fake::Server::start(|method, params| {
            assert_eq!(method, "layout.set_split_ratio");
            assert_eq!(params["tab_id"], "w1:t1");
            assert_eq!(params["path"], json!([false, true]));
            json!({"result": {"type": "layout_split_ratio_set", "layout": {"tab_id": "w1:t1"}}})
        });
        let layout = server
            .client()
            .set_split_ratio("w1:t1", &[false, true], 0.25)
            .unwrap();
        assert_eq!(layout, json!({"tab_id": "w1:t1"}));
        let requests = server.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        assert!((requests[0]["params"]["ratio"].as_f64().unwrap() - 0.25).abs() < 1e-6);
        assert!(requests[0]["id"].as_str().unwrap().starts_with("dogsled_"));
    }

    #[test]
    fn herdr_socket_returns_herdrs_error_message_as_invalid() {
        let server = fake::Server::start(
            |_, _| json!({"error": {"code": "split_not_found", "message": "split path not found"}}),
        );
        let error = server
            .client()
            .set_split_ratio("w1:t1", &[], 0.5)
            .unwrap_err();
        assert!(matches!(&error, Error::Invalid(message)
            if message == "herdr layout.set_split_ratio: split_not_found: split path not found"));
    }

    #[test]
    fn herdr_socket_reads_layouts_by_pane() {
        let server = fake::Server::start(|method, params| {
            assert_eq!(params["pane_id"], "w1:p1");
            json!({"result": {"type": method.replace('.', "_"), "layout": {"method": method}}})
        });
        let client = server.client();
        assert_eq!(
            client.layout_export("w1:p1").unwrap()["method"],
            "layout.export"
        );
        assert_eq!(
            client.pane_layout("w1:p1").unwrap()["method"],
            "pane.layout"
        );
        assert_eq!(server.methods(), ["layout.export", "pane.layout"]);
    }

    #[test]
    fn herdr_socket_focuses_a_pane_by_its_id() {
        let server = fake::Server::start(|method, params| {
            assert_eq!(method, "pane.focus");
            assert_eq!(params["pane_id"], "w1:p1");
            json!({"result": {"type": "pane_info", "pane": {"pane_id": "w1:p1"}}})
        });
        let pane = server.client().focus_pane("w1:p1").unwrap();
        assert_eq!(pane["type"], "pane_info");
        assert_eq!(pane["pane"]["pane_id"], "w1:p1");
        assert_eq!(server.methods(), ["pane.focus"]);
    }

    #[test]
    fn herdr_socket_reads_the_agent_list_and_the_pane_screen() {
        let server = fake::Server::start(|method, params| match method {
            "agent.list" => json!({"result": {"type": "agent_list", "agents": [
                {"pane_id": "w1:p1", "agent_status": "working", "agent": "pi", "name": "worker"}
            ]}}),
            "pane.read" => {
                assert_eq!(params["pane_id"], "w1:p1");
                assert_eq!(params["source"], "visible");
                assert_eq!(params["lines"], 200);
                json!({"result": {"type": "pane_read", "read": {
                    "pane_id": "w1:p1", "text": "hello", "revision": 7
                }}})
            }
            other => panic!("unexpected method {other}"),
        });
        let client = server.client();
        let agents = client.agent_list().unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0]["pane_id"], "w1:p1");
        assert_eq!(agents[0]["agent_status"], "working");
        let read = client.pane_read("w1:p1", "visible", Some(200)).unwrap();
        assert_eq!(read["text"], "hello");
        assert_eq!(read["revision"], 7);
        assert_eq!(server.methods(), ["agent.list", "pane.read"]);
    }

    #[test]
    fn herdr_socket_prompts_sends_keys_and_focuses_an_agent() {
        let server = fake::Server::start(|method, params| match method {
            "agent.prompt" => {
                assert_eq!(params["target"], "w1:p1");
                assert_eq!(params["text"], "go on");
                json!({"result": {"type": "agent_prompted"}})
            }
            "agent.send_keys" => {
                assert_eq!(params["target"], "w1:p1");
                assert_eq!(params["keys"], json!(["enter"]));
                json!({"result": {"type": "ok"}})
            }
            "agent.focus" => {
                assert_eq!(params["target"], "w1:p1");
                json!({"result": {"type": "agent_info"}})
            }
            other => panic!("unexpected method {other}"),
        });
        let client = server.client();
        assert_eq!(
            client.agent_prompt("w1:p1", "go on").unwrap()["type"],
            "agent_prompted"
        );
        assert_eq!(
            client
                .agent_send_keys("w1:p1", &["enter".to_owned()])
                .unwrap()["type"],
            "ok"
        );
        assert_eq!(client.agent_focus("w1:p1").unwrap()["type"], "agent_info");
        assert_eq!(
            server.methods(),
            ["agent.prompt", "agent.send_keys", "agent.focus"]
        );
    }

    #[test]
    fn a_subscription_reads_the_events_herdr_pushes() {
        let server = fake::EventServer::start(vec![
            r#"{"event":"pane.agent_status_changed","data":{"pane_id":"w1:p1","agent_status":"blocked"}}"#
                .to_owned(),
            r#"{"event":"pane.agent_status_changed","data":{"pane_id":"w1:p1","agent_status":"idle"}}"#
                .to_owned(),
        ]);
        let mut subscription = server
            .client()
            .subscribe(&[json!({"type": "pane.agent_status_changed", "pane_id": "w1:p1"})])
            .expect("a subscription");
        let first = subscription.next().expect("an event");
        let Next::Event(event) = first else {
            panic!("expected an event, got {first:?}");
        };
        assert_eq!(event["event"], "pane.agent_status_changed");
        assert_eq!(event["data"]["agent_status"], "blocked");
        let second = subscription.next().expect("an event");
        let Next::Event(event) = second else {
            panic!("expected an event, got {second:?}");
        };
        assert_eq!(event["data"]["agent_status"], "idle");
        // The subscription sent its request with the pane it watches.
        let requests = server.requests.lock().unwrap().clone();
        assert_eq!(requests[0]["method"], "events.subscribe");
        assert_eq!(
            requests[0]["params"]["subscriptions"][0]["pane_id"],
            "w1:p1"
        );
    }

    #[test]
    fn a_closed_subscription_reports_that_it_is_closed() {
        let server = fake::EventServer::start(Vec::new());
        let mut subscription = server
            .client()
            .subscribe(&[json!({"type": "pane.created"})])
            .expect("a subscription");
        // The fake closes the connection once its events are written.
        let mut closed = false;
        for _ in 0..20 {
            if let Next::Closed = subscription.next().expect("a read") {
                closed = true;
                break;
            }
        }
        assert!(closed);
        assert_eq!(subscription.next().expect("a read"), Next::Closed);
    }

    #[test]
    fn a_refused_subscription_reports_herdrs_error() {
        let server = fake::Server::start(
            |_, _| json!({"error": {"code": "pane_not_found", "message": "pane w9:p9 not found"}}),
        );
        let error = server
            .client()
            .subscribe(&[json!({"type": "pane.created"})])
            .unwrap_err();
        assert!(matches!(&error, Error::Invalid(message)
            if message == "herdr events.subscribe: pane_not_found: pane w9:p9 not found"));
    }

    #[test]
    fn herdr_socket_reports_a_missing_socket() {
        let client = Client::at(std::env::temp_dir().join("dogsled-no-such.sock"));
        assert!(
            matches!(client.layout_export("w1:p1"), Err(Error::Invalid(message))
            if message.starts_with("herdr layout.export: "))
        );
    }
}
