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
        let mut publisher = Publisher::new().unwrap();
        let mut collector = Collector::default();
        collector.client = Some(socket.clone());
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
                let mut input = captured.lock().unwrap().clone();
                producer
                    .lock()
                    .unwrap()
                    .refresh(&mut input, &dirty)
                    .unwrap();
                publisher.replace(input).unwrap();
                if let Some(ready) = ready.take() {
                    ready.send(()).unwrap();
                }
                if receive.recv().is_err() || stopped.load(Ordering::Acquire) {
                    break;
                }
            }
        });
        started.recv_timeout(Duration::from_secs(3)).unwrap();
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
        let deadline = Instant::now() + Duration::from_secs(3);
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

    pub fn wake(&self) {
        *self.source.lock().unwrap() = input(&self.socket, &self.ledger);
        self.send.send(Event::Observe).unwrap();
    }

    pub fn republish(&self) {
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
