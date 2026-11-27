use super::{PROCESS_DEADLINE, Value, fs, json};
use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

type Handlers = Arc<Mutex<BTreeMap<&'static str, Box<dyn FnMut(UnixStream, Value) + Send>>>>;

pub(super) struct Server {
    pub(super) path: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    handlers: Handlers,
}

impl Server {
    pub(super) fn start(dir: &Path, name: &str, agents: PathBuf) -> Self {
        let path = dir.join(name);
        let listener = UnixListener::bind(&path).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let handlers = Handlers::default();
        let (dir, halt, methods) = (dir.to_owned(), Arc::clone(&stop), Arc::clone(&handlers));
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if halt.load(Ordering::Acquire) {
                    break;
                }
                let mut stream = stream.unwrap();
                stream.set_read_timeout(Some(PROCESS_DEADLINE)).unwrap();
                stream.set_write_timeout(Some(PROCESS_DEADLINE)).unwrap();
                let mut line = String::new();
                if !matches!(BufReader::new(&stream).read_line(&mut line), Ok(n) if n > 0) {
                    continue;
                }
                let request: Value = serde_json::from_str(&line).unwrap();
                let method = request["method"].as_str().unwrap();
                if let Some(handler) = methods.lock().unwrap().get_mut(method) {
                    handler(stream, request);
                    continue;
                }
                let reply = read(&dir, &agents, method, &request["params"]);
                Self::reply(&mut stream, &request, reply);
            }
        });
        Self {
            path,
            stop,
            thread: Some(thread),
            handlers,
        }
    }

    pub(super) fn on(
        &self,
        method: &'static str,
        handler: impl FnMut(UnixStream, Value) + Send + 'static,
    ) {
        assert!(
            self.handlers
                .lock()
                .unwrap()
                .insert(method, Box::new(handler))
                .is_none()
        );
    }

    pub(super) fn reply(stream: &mut UnixStream, request: &Value, mut reply: Value) -> bool {
        reply["id"] = request["id"].clone();
        writeln!(stream, "{reply}").is_ok()
    }
}

/// Answer a read with the rig's own `herdr` command, so the socket and the command are one
/// fake Herdr: a test that scripts the command (an answer, a failure, a log line) scripts
/// the socket too.
fn read(dir: &Path, agents: &Path, method: &str, params: &Value) -> Value {
    let args: Vec<String> = match method {
        "agent.list" => vec!["agent".into(), "list".into()],
        "pane.list" => vec!["pane".into(), "list".into()],
        "pane.process_info" => vec![
            "pane".into(),
            "process-info".into(),
            "--pane".into(),
            params["pane_id"].as_str().unwrap().into(),
        ],
        _ => {
            return json!({"error":{"code":"unsupported","message":format!("fixture does not implement {method}")}});
        }
    };
    let path = format!(
        "{}:{}",
        dir.join("bin").display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = super::isolated(dir.join("bin").join("herdr"))
        .args(&args)
        .env("PATH", path)
        .env("RIG", dir)
        .env("RIG_AGENTS", agents)
        .env("HOME", dir.join("home"))
        .env("DOGSLED_TEST_ROLE", "observer")
        .current_dir(dir)
        .output()
        .unwrap();
    // The command prints Herdr's reply. A failure carries Herdr's error on either stream.
    let reply = [&output.stdout, &output.stderr]
        .into_iter()
        .find_map(|bytes| serde_json::from_slice::<Value>(bytes).ok())
        .filter(|reply| output.status.success() || reply.get("error").is_some());
    reply.unwrap_or_else(|| {
        json!({"error":{"code":"command_failed","message":format!(
            "herdr {}: {}: {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )}})
    })
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = UnixStream::connect(&self.path);
        let result = self.thread.take().unwrap().join();
        if !std::thread::panicking() {
            result.unwrap();
        }
        let _ = fs::remove_file(&self.path);
    }
}
