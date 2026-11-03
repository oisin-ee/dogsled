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
    Handoff,
    Inbox,
    Retention,
    Quota,
}

const LANES: [Lane; 7] = [
    Lane::Watchdog,
    Lane::Admission,
    Lane::Children,
    Lane::Handoff,
    Lane::Inbox,
    Lane::Retention,
    Lane::Quota,
];

impl Lane {
    const fn name(self) -> &'static str {
        match self {
            Self::Watchdog => "watchdog",
            Self::Admission => "admission",
            Self::Children => "children",
            Self::Handoff => "handoff",
            Self::Inbox => "inbox",
            Self::Retention => "retention",
            Self::Quota => "quota",
        }
    }
}

#[derive(Default)]
struct LaneDiagnostics {
    unrecorded: std::collections::BTreeMap<&'static str, (Option<String>, String)>,
}

impl LaneDiagnostics {
    fn record(&mut self, ledger: &Ledger, lane: Lane, result: Result<()>) {
        let error = result.err().map(|error| error.to_string());
        match crate::store::write(ledger, |tx| {
            crate::tables::record_lane(tx, lane.name(), error.as_deref(), now_ms())
        }) {
            Ok(changed) => {
                self.unrecorded.remove(lane.name());
                if changed {
                    if let Some(error) = error {
                        eprintln!("dogsled service lane {}: {error}", lane.name());
                    } else {
                        eprintln!("dogsled service lane {}: recovered", lane.name());
                    }
                }
            }
            Err(recording_error) => {
                let current = (error, recording_error.to_string());
                let previous = self.unrecorded.get(lane.name());
                if previous.map(|old| &old.0) != Some(&current.0)
                    && let Some(error) = &current.0
                {
                    eprintln!("dogsled service lane {}: {error}", lane.name());
                }
                if previous.map(|old| &old.1) != Some(&current.1) {
                    eprintln!(
                        "dogsled service lane {}: cannot record lane status: {}",
                        lane.name(),
                        current.1
                    );
                }
                self.unrecorded.insert(lane.name(), current);
            }
        }
    }
}

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
    if lane == Lane::Handoff {
        return crate::done::handoff(ledger, &shell, now_ms());
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
    if lane == Lane::Retention {
        // Retention owns its synchronization too; no other service lane is required.
        crate::launch::release_owned(&fleet)?;
        return crate::jobs::expire(ledger, now_ms());
    }
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
        Lane::Quota | Lane::Handoff | Lane::Retention => Ok(()),
    }
}

pub fn serve(node: &Node<'_>, policy: &Policy, interval: Duration, once: bool) -> Result<()> {
    loop {
        let writer = node
            .ledger
            .with_lock("service", || run(node, policy, interval, once))?;
        let Some(writer) = writer else {
            return Ok(());
        };
        match crate::upgrade::handover(&writer, None) {
            Err(error) if matches!(error.root(), Error::NewerWriter(_)) => {
                eprintln!("dogsled service upgrade: {error}");
            }
            result => {
                return result;
            }
        }
    }
}

/// Start eligible action lanes without coupling their scheduling to publication or input.
fn start_lanes(
    node: &Node<'_>,
    policy: &Policy,
    once: bool,
    workers: &mut [Option<std::thread::JoinHandle<()>>; LANES.len()],
    completed: [bool; LANES.len()],
    send: &mpsc::Sender<Event>,
) {
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
}

/// A refusal aborts publication before starting more work; only the loop owns retries.
fn publish(node: &Node<'_>, requested: &mut bool) -> Result<()> {
    // The socket only reduces latency. Recover requests on every publication pass.
    *requested |= crate::sweep::pending(node.ledger)?;
    node.tick(now_ms())?;
    if let Err(error) = crate::board::collect_todos(node.ledger) {
        if matches!(error.root(), Error::NewerWriter(_)) {
            return Err(error);
        }
        eprintln!("dogsled service: todo intake failed: {error}");
    }
    Ok(())
}

fn run(
    node: &Node<'_>,
    policy: &Policy,
    interval: Duration,
    once: bool,
) -> Result<Option<crate::tables::Writer>> {
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
    let mut diagnostics = LaneDiagnostics::default();
    let mut next = Instant::now();
    let outcome = (|| {
        loop {
            let mut current = match crate::upgrade::newer(node.ledger)? {
                crate::upgrade::Boundary::Ready(writer) => return Ok(Some(writer)),
                crate::upgrade::Boundary::Current => true,
                crate::upgrade::Boundary::Pending(error) => {
                    eprintln!("dogsled service upgrade: {error}");
                    next = Instant::now();
                    false
                }
            };
            if current {
                match publish(node, &mut requested) {
                    Ok(()) => {
                        start_lanes(node, policy, once, &mut workers, completed, &send);
                    }
                    Err(error) if matches!(error.root(), Error::NewerWriter(_)) => {
                        eprintln!("dogsled service pass: {error}");
                        current = false;
                        next = Instant::now();
                    }
                    Err(error) => return Err(error),
                }
            }
            next += interval;
            loop {
                if current && requested && sweep.is_none() {
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
                if current && once && completed.iter().all(|done| *done) && sweep.is_none() {
                    return Ok(None);
                }
                match recv.recv_timeout(next.saturating_duration_since(Instant::now())) {
                    Ok(Event::Sweep) => requested = true,
                    Ok(Event::Swept(result)) => {
                        if let Some(handle) = sweep.take() {
                            handle.join().map_err(|_| {
                                Error::Invalid("service sweep worker failed".into())
                            })?;
                        }
                        if let Err(error) = result {
                            eprintln!("dogsled service sweep: {error}");
                        }
                    }
                    Ok(Event::Finished(lane, result)) => {
                        if let Some(handle) = workers[lane].take() {
                            handle.join().map_err(|_| {
                                Error::Invalid("service action worker failed".into())
                            })?;
                        }
                        completed[lane] = true;
                        diagnostics.record(node.ledger, LANES[lane], result);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Err(Error::Invalid("service worker disconnected".into()));
                    }
                }
            }
        }
    })();
    // No worker or service lock can cross exec: their external effects must finish.
    for handle in workers.into_iter().flatten().chain(sweep) {
        handle
            .join()
            .map_err(|_| Error::Invalid("service worker failed while draining".into()))?;
    }
    outcome
}
