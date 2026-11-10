//! Detail tests capture native agent records and real transcripts before publication.

use super::Collector;
use crate::herdr_socket::Client;
use crate::ledger::Ledger;
use crate::observation::{Domains, Publication, Publisher, Sources, ipc, project};
use crate::service::{Event, wake};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::test_support::WAIT_DEADLINE;

pub(crate) struct Fixture {
    pub ledger: Ledger,
    pub client: ipc::Client,
    pub collector: Arc<Mutex<Collector>>,
    pub interests: super::Interests,
    stop: Arc<AtomicBool>,
    send: mpsc::Sender<Event>,
    thread: Option<std::thread::JoinHandle<()>>,
    server: Option<ipc::Server>,
    socket: Client,
    source: Arc<Mutex<Publication>>,
}

impl Fixture {
    pub fn new(socket: Client) -> Self {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        Self::at(socket, ledger)
    }

    pub fn at(socket: Client, ledger: Ledger) -> Self {
        Self::start(socket, ledger, false, None)
    }

    pub fn paused(socket: Client) -> Self {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        Self::start(socket, ledger, true, None)
    }

    pub fn with_files_shell(socket: Client, shell: Arc<dyn crate::shell::Shell>) -> Self {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        Self::start(socket, ledger, false, Some(shell))
    }

    fn start(
        socket: Client,
        ledger: Ledger,
        paused: bool,
        files_shell: Option<Arc<dyn crate::shell::Shell>>,
    ) -> Self {
        let mut publisher = Publisher::new().unwrap();
        let mut collector = Collector::default();
        collector.client = Some(socket.clone());
        collector.files_shell = files_shell;
        publisher.details(collector.payloads.clone());
        let interests = collector.interests.clone();
        let mut server = ipc::Server::new(&ledger.dir).unwrap();
        let (send, receive) = mpsc::channel();
        let dirty = wake::Dirty::new(send.clone());
        server.branches(
            crate::observation::peers::Interests::default(),
            dirty.clone(),
        );
        server.details(interests.clone());
        let client = ipc::Client::at(&ledger.dir);
        server.bind(publisher.subscribe()).unwrap();
        let (ready, started) = mpsc::sync_channel(1);
        let collector = Arc::new(Mutex::new(collector));
        let producer = Arc::clone(&collector);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let source = Arc::new(Mutex::new(input(&socket, &ledger)));
        let captured = Arc::clone(&source);
        let thread = std::thread::spawn(move || {
            let mut publisher = publisher;
            let mut ready = Some(ready);
            loop {
                let _ = dirty.take();
                if !paused || ready.is_some() {
                    let mut input = captured.lock().unwrap().clone();
                    producer
                        .lock()
                        .unwrap()
                        .refresh(&mut input, &dirty)
                        .unwrap();
                    publisher.replace(input).unwrap();
                }
                if let Some(ready) = ready.take() {
                    ready.send(()).unwrap();
                }
                if receive.recv().is_err() || stopped.load(Ordering::Acquire) {
                    break;
                }
            }
        });
        started.recv_timeout(WAIT_DEADLINE).unwrap();
        Self {
            ledger,
            client,
            collector,
            interests,
            stop,
            send,
            thread: Some(thread),
            server: Some(server),
            socket,
            source,
        }
    }

    pub fn wait(
        &self,
        predicate: impl Fn(&crate::observation::Published) -> bool,
    ) -> Arc<crate::observation::Published> {
        let mut subscription = self.client.subscribe().unwrap();
        let deadline = Instant::now() + WAIT_DEADLINE;
        loop {
            let connection = subscription.current();
            assert_eq!(connection.status, ipc::Status::Connected);
            if predicate(&connection.publication) {
                return connection.publication;
            }
            assert!(
                Instant::now() < deadline,
                "expected detail publication: {:?}",
                connection.publication.views.get("agent_details")
            );
            subscription.changed(Duration::from_millis(100));
        }
    }

    pub fn tick(&self, pane: &str) -> bool {
        self.collector.lock().unwrap().tick(pane)
    }

    pub fn files_tick(&self, pane: &str) -> bool {
        self.collector.lock().unwrap().files_tick(pane)
    }

    pub fn wake(&self) {
        *self.source.lock().unwrap() = input(&self.socket, &self.ledger);
        self.send.send(Event::Observe).unwrap();
    }

    pub fn rename_agent(&self, name: &str) {
        self.source.lock().unwrap().views.get_mut("portal").unwrap()["data"]["agents"][0]["name"] =
            json!(name);
        self.send.send(Event::Observe).unwrap();
    }

    pub fn no_viewers(&self) -> bool {
        self.interests.panes().is_empty()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.take();
        self.stop.store(true, Ordering::Release);
        let _ = self.send.send(Event::Observe);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
        let collector = std::mem::take(&mut *self.collector.lock().unwrap());
        drop(collector);
        std::fs::remove_dir_all(&self.ledger.dir).unwrap();
    }
}

/// A real checkout kept separate from transcript and observer files.
#[derive(Debug)]
pub(crate) struct Repository(pub std::path::PathBuf);

impl Repository {
    pub fn new() -> Self {
        let root = std::env::temp_dir().join(crate::ledger::new_id());
        std::fs::create_dir_all(&root).unwrap();
        let repo = Self(root.canonicalize().unwrap());
        repo.git(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.0.join("file"), "base\n").unwrap();
        repo.git(&["add", "file"]);
        repo.git(&["commit", "-qm", "base"]);
        repo
    }

    fn git(&self, args: &[&str]) {
        use crate::shell::Shell as _;
        let mut words = vec![
            "git".into(),
            "-C".into(),
            self.0.to_str().unwrap().into(),
            "-c".into(),
            "user.name=Fixture".into(),
            "-c".into(),
            "user.email=fixture@example.invalid".into(),
        ];
        words.extend(args.iter().map(ToString::to_string));
        crate::shell::System::default()
            .run_ok("env", &words)
            .unwrap();
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

/// Holds actual Git output at the shell boundary, without blocking the detail thread.
#[derive(Debug, Default)]
pub(crate) struct GitShell {
    holds: Mutex<std::collections::VecDeque<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
    pub fail: AtomicBool,
    pub too_many: AtomicBool,
}

impl GitShell {
    pub fn hold(&self) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered, started) = mpsc::channel();
        let (release, released) = mpsc::channel();
        self.holds.lock().unwrap().push_back((entered, released));
        (started, release)
    }
}

impl crate::shell::Shell for GitShell {
    fn run(&self, program: &str, args: &[String]) -> crate::error::Result<crate::shell::Output> {
        if self.fail.load(Ordering::Acquire) {
            return Err(crate::error::Error::Invalid("fixture Git failure".into()));
        }
        crate::shell::System::default().run(program, args)
    }

    fn run_bounded(
        &self,
        program: &str,
        args: &[String],
        timeout: Duration,
    ) -> crate::error::Result<crate::shell::Output> {
        crate::shell::System::default().run_bounded(program, args, timeout)
    }

    fn run_bytes_bounded(
        &self,
        program: &str,
        args: &[String],
        timeout: Duration,
        limit: usize,
    ) -> crate::error::Result<crate::shell::BytesOutput> {
        if self.too_many.load(Ordering::Acquire) {
            return Err(crate::error::Error::OutputLimit {
                operation: "Git fixture".into(),
                limit,
            });
        }
        if self.fail.load(Ordering::Acquire) {
            return Err(crate::error::Error::Invalid("fixture Git failure".into()));
        }
        let result =
            crate::shell::System::default().run_bytes_bounded(program, args, timeout, limit);
        let hold = self.holds.lock().unwrap().pop_front();
        if let Some((entered, released)) = hold {
            entered.send(()).unwrap();
            released
                .recv_timeout(WAIT_DEADLINE)
                .expect("release Git read");
        }
        result
    }
}

fn input(socket: &Client, ledger: &Ledger) -> Publication {
    let raw = socket.agent_list().unwrap();
    let shell = crate::shell::Fake::new();
    shell.on(
        "herdr agent list",
        &json!({"result":{"agents":raw}}).to_string(),
    );
    shell.on("herdr pane list", r#"{"result":{"panes":[]}}"#);
    let terminals = crate::probe::terminals(&shell).unwrap();
    let agents = crate::web::agents::capture_with(
        &terminals.agents,
        &mut crate::web::agents::Titles::default(),
        Some(socket),
    )
    .unwrap();
    let board = crate::web::board::view(
        &crate::board::Board {
            hosts: vec![],
            rows: vec![],
        },
        &[],
        &[],
        7,
    );
    let inbox = crate::web::inbox::view(
        &board,
        &crate::web::reviews::pending(ledger).unwrap(),
        &crate::web::store::pending(ledger).unwrap(),
        &agents,
        7,
    );
    let snapshot = portal_types::PortalSnapshot {
        version: crate::observation::VERSION,
        epoch: String::new(),
        revision: 0,
        published_ms: 7,
        board: Some(board),
        agents: Some(agents),
        inbox: Some(inbox),
        gaps: vec![],
    };
    Publication {
        published_ms: 7,
        sources: Sources::default(),
        local: Domains::from([("terminals".into(), serde_json::to_value(terminals).unwrap())]),
        views: Domains::from([(
            "portal".into(),
            serde_json::to_value(project::View {
                data: Some(snapshot),
                gaps: vec![],
            })
            .unwrap(),
        )]),
        gaps: vec![],
    }
}
