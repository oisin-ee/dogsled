//! Publication and routine lanes have independent clocks. Stop requests wake cleanup.
//! Publication runs on the loop itself; each action lane runs on its own thread, so a slow sweep
//! never delays a check-in or an admission.

use crate::config::Policy;
use crate::error::{Error, Result};
use crate::hosts::Fleet;
use crate::ledger::{Ledger, now_ms};
use crate::node::Node;
use crate::shell::System;
use crate::{items, work};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The action lanes, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lane {
    Watchdog,
    Admission,
    Children,
    Inbox,
    Quota,
}

const LANES: [Lane; 5] = [
    Lane::Watchdog,
    Lane::Admission,
    Lane::Children,
    Lane::Inbox,
    Lane::Quota,
];

enum Event {
    Finished(usize, Result<()>),
    Sweep,
    Swept(Result<()>),
}

/// Wake the loop without waiting for its publication interval. The ledger request is durable;
/// this socket is only a local notification. Dropping the listener joins its thread.
struct Listener {
    socket: std::os::unix::net::UnixDatagram,
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    path: std::path::PathBuf,
}

impl Listener {
    fn start(ledger: &Ledger, send: mpsc::Sender<Event>) -> Result<Self> {
        std::fs::create_dir_all(&ledger.dir)?;
        let path = ledger.dir.join(crate::sweep::SOCKET);
        match std::fs::remove_file(&path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
        let socket = std::os::unix::net::UnixDatagram::bind(&path)?;
        let reader = socket.try_clone()?;
        let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done = stopped.clone();
        let thread = std::thread::spawn(move || {
            let mut bytes = [0; 16];
            while reader.recv(&mut bytes).is_ok() {
                if done.load(std::sync::atomic::Ordering::Acquire)
                    || send.send(Event::Sweep).is_err()
                {
                    break;
                }
            }
        });
        Ok(Self {
            socket,
            stopped,
            thread: Some(thread),
            path,
        })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stopped
            .store(true, std::sync::atomic::Ordering::Release);
        let _ = self.socket.send_to(b"stop", &self.path);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn actions(ledger: &Ledger, policy: &Policy, home: String, lane: Lane) -> Result<()> {
    let shell = System::default();
    if lane == Lane::Quota {
        return crate::quota::read(&shell, ledger, policy, &home, false, false).map(|_| ());
    }
    let fleet = if items::list(ledger)?.is_empty()
        && work::queue(ledger)?.is_empty()
        && !crate::inbox::pending(ledger)?
    {
        Fleet {
            shell: &shell,
            ledger,
            policy,
            home,
            hosts: vec![crate::hosts::Host::local()],
        }
    } else {
        Fleet::discover(&shell, ledger, policy, home)?
    };
    match lane {
        Lane::Watchdog => {
            let now = now_ms();
            let reports = fleet.status(now)?;
            crate::watchdog::tick(&fleet, &reports, now)?;
            items::reconcile(ledger, &reports, now)
        }
        Lane::Admission => work::tick(&fleet, now_ms()),
        Lane::Children => crate::sweep::children(&fleet, now_ms()),
        Lane::Inbox => crate::inbox::tick(&fleet, now_ms()),
        Lane::Quota => Ok(()),
    }
}

pub fn serve(node: &Node<'_>, policy: &Policy, interval: Duration, once: bool) -> Result<()> {
    node.ledger
        .with_lock("service", || run(node, policy, interval, once))
}

fn run(node: &Node<'_>, policy: &Policy, interval: Duration, once: bool) -> Result<()> {
    let (send, recv) = mpsc::channel();
    let _listener = match Listener::start(node.ledger, send.clone()) {
        Ok(listener) => Some(listener),
        Err(error) => {
            eprintln!("dogsled service: warning: cleanup wake socket unavailable: {error}");
            None
        }
    };
    let mut sweep: Option<std::thread::JoinHandle<()>> = None;
    let mut requested = false;
    let mut workers: [Option<std::thread::JoinHandle<()>>; LANES.len()] = Default::default();
    let mut completed = [false; LANES.len()];
    let mut next = Instant::now();
    loop {
        // The socket only reduces latency. Recover pending requests on every publication
        // iteration, including when the socket cannot bind or a notification was lost.
        requested |= crate::sweep::pending(node.ledger)?;
        node.tick(now_ms())?;
        for (lane, worker) in workers.iter_mut().enumerate() {
            if worker.is_none() && (!once || !completed[lane]) {
                let mut ledger = node.ledger.clone();
                ledger.budget = crate::budget::Budget::default();
                let policy = policy.clone();
                let home = node.home.to_owned();
                let send = send.clone();
                *worker = Some(std::thread::spawn(move || {
                    let _ = send.send(Event::Finished(
                        lane,
                        actions(&ledger, &policy, home, LANES[lane]),
                    ));
                }));
            }
        }
        next += interval;
        loop {
            if requested && sweep.is_none() {
                requested = false;
                let mut ledger = node.ledger.clone();
                ledger.budget = crate::budget::Budget::default();
                let policy = policy.clone();
                let home = node.home.to_owned();
                let send = send.clone();
                sweep = Some(std::thread::spawn(move || {
                    let shell = System::default();
                    let node = Node {
                        shell: &shell,
                        ledger: &ledger,
                        thresholds: (&policy).into(),
                        home: &home,
                        cleanup: &policy.cleanup,
                    };
                    let _ = send.send(Event::Swept(crate::sweep::requested(&node)));
                }));
            }
            if once && completed.iter().all(|done| *done) && sweep.is_none() {
                return Ok(());
            }
            match recv.recv_timeout(next.saturating_duration_since(Instant::now())) {
                Ok(Event::Sweep) => requested = true,
                Ok(Event::Swept(result)) => {
                    if let Some(handle) = sweep.take() {
                        handle
                            .join()
                            .map_err(|_| Error::Invalid("service sweep worker failed".into()))?;
                    }
                    if let Err(error) = result {
                        eprintln!("dogsled service sweep: {error}");
                    }
                }
                Ok(Event::Finished(lane, result)) => {
                    if let Some(handle) = workers[lane].take() {
                        handle
                            .join()
                            .map_err(|_| Error::Invalid("service action worker failed".into()))?;
                    }
                    completed[lane] = true;
                    if let Err(error) = result {
                        eprintln!("dogsled service lane {:?}: {error}", LANES[lane]);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(Error::Invalid("service worker disconnected".into()));
                }
            }
        }
    }
}
