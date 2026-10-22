//! A small blocking client for Herdr's socket API: newline-delimited JSON over
//! the Unix socket at `HERDR_SOCKET_PATH`, one request and one response per
//! connection. It carries only the methods that have no CLI form:
//! `layout.export`, `pane.layout`, `layout.set_split_ratio`, and `pane.focus`.

use crate::error::{Error, Result};
use serde_json::{Value, json};
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

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
    fn herdr_socket_reports_a_missing_socket() {
        let client = Client::at(std::env::temp_dir().join("dogsled-no-such.sock"));
        assert!(
            matches!(client.layout_export("w1:p1"), Err(Error::Invalid(message))
            if message.starts_with("herdr layout.export: "))
        );
    }
}
