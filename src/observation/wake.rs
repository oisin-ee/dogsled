//! Service-owned input invalidation. Notifications carry no state; the fallback repairs
//! lost datagrams or unavailable event sources. One dirty bitset bounds queued collection.

use crate::error::Result;
use crate::herdr_socket::{Client, Next};
use crate::ledger::Ledger;
use crate::service::Event;
use serde_json::json;
use std::collections::BTreeSet;
use std::os::unix::net::UnixDatagram;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

const SOCKET: &str = "observation-wake.sock";
pub(crate) const LEDGER: u8 = 1;
pub(crate) const LOCAL: u8 = 3;
pub(crate) const PEERS: u8 = 4;
pub(crate) const BRANCHES: u8 = 8;

/// Best-effort post-commit notification. Never wait for the service or run collection.
pub(crate) fn notify(ledger: &Ledger) {
    if let Ok(socket) = UnixDatagram::unbound()
        && socket.set_nonblocking(true).is_ok()
    {
        let _ = socket.send_to(b"ledger", ledger.dir.join(SOCKET));
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Dirty {
    pending: Arc<AtomicU8>,
    send: mpsc::Sender<Event>,
    peers: Arc<std::sync::Mutex<BTreeSet<String>>>,
}

impl Dirty {
    pub(super) fn new(send: mpsc::Sender<Event>) -> Self {
        Self {
            pending: Arc::new(AtomicU8::new(0)),
            send,
            peers: Arc::default(),
        }
    }

    pub(crate) fn retry_peer(&self, host: String) {
        self.peers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(host);
        self.mark(PEERS);
    }

    pub(crate) fn take_peers(&self) -> BTreeSet<String> {
        std::mem::take(
            &mut *self
                .peers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    pub(crate) fn mark(&self, domains: u8) {
        if self.pending.fetch_or(domains, Ordering::AcqRel) == 0 {
            let _ = self.send.send(Event::Observe);
        }
    }

    pub(super) fn pending(&self) -> bool {
        self.pending.load(Ordering::Acquire) != 0
    }

    pub(super) fn take(&self) -> u8 {
        self.pending.swap(0, Ordering::AcqRel)
    }
}

pub(super) struct Commits {
    socket: UnixDatagram,
    path: std::path::PathBuf,
    stopped: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Commits {
    pub(super) fn start(ledger: &Ledger, dirty: Dirty) -> Result<Self> {
        let path = ledger.dir.join(SOCKET);
        match std::fs::remove_file(&path) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        let socket = UnixDatagram::bind(&path)?;
        let reader = socket.try_clone()?;
        let stopped = Arc::new(AtomicBool::new(false));
        let done = Arc::clone(&stopped);
        let thread = std::thread::spawn(move || {
            let mut bytes = [0; 16];
            while reader.recv(&mut bytes).is_ok() {
                if done.load(Ordering::Acquire) {
                    break;
                }
                dirty.mark(LEDGER);
            }
        });
        Ok(Self {
            socket,
            path,
            stopped,
            thread: Some(thread),
        })
    }
}

impl Drop for Commits {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self.socket.send_to(b"stop", &self.path);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Herdr 0.9.5's schema exposes global topology events, but agent status requires a
/// `pane_id`. Replace that interest set only when the collector's known panes change.
pub(super) struct Herdr {
    panes: mpsc::Sender<BTreeSet<String>>,
    interests: BTreeSet<String>,
    stopped: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn filters(panes: &BTreeSet<String>) -> Vec<serde_json::Value> {
    let mut filters: Vec<_> = [
        "pane.created",
        "pane.closed",
        "pane.updated",
        "pane.exited",
        "pane.agent_detected",
        "worktree.created",
        "worktree.opened",
        "worktree.removed",
    ]
    .into_iter()
    .map(|event| json!({"type": event}))
    .collect();
    filters.extend(
        panes
            .iter()
            .map(|pane| json!({"type": "pane.agent_status_changed", "pane_id": pane})),
    );
    filters
}

impl Herdr {
    pub(super) fn start(dirty: Dirty, fallback: Duration) -> Self {
        let (panes, receive) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let done = Arc::clone(&stopped);
        let thread = std::thread::spawn(move || {
            let mut interests = BTreeSet::new();
            let mut subscription = None;
            let mut retry = std::time::Instant::now();
            let mut diagnostic = None;
            while !done.load(Ordering::Acquire) {
                while let Ok(panes) = receive.try_recv() {
                    interests = panes;
                    subscription = None;
                    retry = std::time::Instant::now();
                }
                if subscription.is_none() && std::time::Instant::now() >= retry {
                    let opened = Client::discover()
                        .and_then(|client| client.subscribe(&filters(&interests)));
                    match opened {
                        Ok(opened) => {
                            subscription = Some(opened);
                            diagnostic = None;
                            // Reconcile changes lost during disconnection or interest replacement.
                            dirty.mark(LOCAL);
                        }
                        Err(error) => {
                            let error = error.to_string();
                            if diagnostic.as_ref() != Some(&error) {
                                eprintln!(
                                    "dogsled service: Herdr observation events unavailable: {error}"
                                );
                                diagnostic = Some(error);
                            }
                            retry = std::time::Instant::now() + fallback;
                        }
                    }
                }
                if let Some(active) = &mut subscription {
                    match active.next() {
                        Ok(Next::Event(_)) => dirty.mark(LOCAL),
                        Ok(Next::Idle) => (),
                        outcome @ (Ok(Next::Closed) | Err(_)) => {
                            let error = match outcome {
                                Err(error) => error.to_string(),
                                Ok(_) => "Herdr observation subscription closed".into(),
                            };
                            if diagnostic.as_ref() != Some(&error) {
                                eprintln!(
                                    "dogsled service: Herdr observation events unavailable: {error}"
                                );
                                diagnostic = Some(error);
                            }
                            subscription = None;
                            retry = std::time::Instant::now() + fallback;
                            dirty.mark(LOCAL);
                        }
                    }
                } else {
                    // This is cancellation polling, not another observation timer.
                    std::thread::park_timeout(Duration::from_millis(100));
                }
            }
        });
        Self {
            panes,
            interests: BTreeSet::new(),
            stopped,
            thread: Some(thread),
        }
    }

    pub(super) fn interests(&mut self, panes: BTreeSet<String>) {
        if self.interests != panes {
            self.interests.clone_from(&panes);
            let _ = self.panes.send(panes);
        }
    }
}

impl Drop for Herdr {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_domains_coalesce_and_preserve_a_change_during_collection() {
        let (send, receive) = mpsc::channel();
        let dirty = Dirty::new(send);
        for _ in 0..100 {
            dirty.mark(LEDGER);
        }
        dirty.mark(LOCAL);
        assert!(matches!(receive.try_recv(), Ok(Event::Observe)));
        assert!(receive.try_recv().is_err());
        // Starting collection consumes the current domains. Changes made while that
        // collection is in flight get one further notification, not one per commit.
        assert_eq!(dirty.take(), LOCAL);
        for _ in 0..100 {
            dirty.mark(LEDGER);
        }
        assert!(matches!(receive.try_recv(), Ok(Event::Observe)));
        assert!(receive.try_recv().is_err());
        assert_eq!(dirty.take(), LEDGER);
        assert!(!dirty.pending());
    }
}
