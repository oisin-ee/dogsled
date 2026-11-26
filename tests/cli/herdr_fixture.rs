use super::{PROCESS_DEADLINE, Value, fs, json};
use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

const PROCESS_INFO: &str = r#"{"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":10,"name":"zsh","argv":["zsh"]}]}}}"#;

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

fn read(dir: &Path, agents: &Path, method: &str, params: &Value) -> Value {
    let file = |path: &Path| {
        fs::read(path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_else(|| {
            json!({"error":{"code":"fixture_unavailable","message":format!("{} is missing or invalid JSON", path.display())}})
        })
    };
    let (call, reply) = match method {
        "agent.list" => ("agent list".into(), file(agents)),
        "pane.list" => ("pane list".into(), file(&dir.join("panes.json"))),
        "pane.process_info" => (
            format!(
                "pane process-info --pane {}",
                params["pane_id"].as_str().unwrap()
            ),
            serde_json::from_str(PROCESS_INFO).unwrap(),
        ),
        _ => {
            return json!({"error":{"code":"unsupported","message":format!("fixture does not implement {method}")}});
        }
    };
    append(&dir.join("herdr.log"), &format!("{call}\n"));
    append(&dir.join("herdr-roles.log"), &format!("observer {call}\n"));
    reply
}

fn append(path: &Path, line: &str) {
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap()
        .write_all(line.as_bytes())
        .unwrap();
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
