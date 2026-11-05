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

#[path = "observation/wake.rs"]
pub(crate) mod wake;

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

#[derive(Debug)]
pub(crate) enum Event {
    Finished(usize, Result<()>),
    Sweep,
    Swept(Result<()>),
    Observe,
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
    node.ledger
        .with_lock("service", || run(node, policy, interval, once))
}

/// One `--once` pass imports the handoffs this pass made: children follows the handoff lane.
/// The continuous service needs no order; the next children pass imports later handoffs.
fn ready(lane: Lane, completed: [bool; LANES.len()]) -> bool {
    lane != Lane::Children
        || LANES
            .iter()
            .zip(completed)
            .any(|(other, done)| *other == Lane::Handoff && done)
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
        if worker.is_none() && (!once || (!completed[lane] && ready(LANES[lane], completed))) {
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

struct Exchange {
    peers: crate::observation::peers_runtime::Observers,
    branches: crate::observation::peers::Branches,
    interests: crate::observation::peers::Interests,
    diagnostics: crate::observation::diagnostics::Diagnostics,
}

fn publish(
    node: &Node<'_>,
    collector: &mut crate::observation::collect::Collector,
    publisher: &mut crate::observation::Publisher,
    herdr: &mut wake::Herdr,
    cycle: (u8, bool),
    exchange: &mut Exchange,
    fleet: &Fleet<'_>,
) -> Result<()> {
    let (domains, once) = cycle;
    let Exchange {
        peers,
        branches,
        interests,
        diagnostics,
    } = exchange;
    let now = now_ms();
    let collection = crate::store::observing(|| -> Result<_> {
        let gaps = if domains & !wake::PEERS != 0 {
            branches.observe(node, &interests.current()?, now)?
        } else {
            branches.gaps()
        };
        let mut collection = if domains & wake::LOCAL == wake::LOCAL {
            collector.collect(node, now)
        } else if domains & (wake::LEDGER | wake::BRANCHES) != 0 {
            collector.refresh(node, now, false)
        } else {
            crate::observation::collect::Collection {
                input: collector.publication(now),
                sample: Ok(()),
            }
        };
        let results = branches.results();
        if !results.is_empty() {
            collection
                .input
                .local
                .insert("branches".into(), serde_json::to_value(&results)?);
            if let Some(measured) = results.iter().map(|result| result.measured_ms).max() {
                collection
                    .input
                    .sources
                    .local
                    .insert("branches".into(), measured);
            }
        }
        collection.input.gaps.extend(gaps);
        peers.compose(fleet, &mut collection.input, now, collector.reports())?;
        Ok(collection)
    })?;
    herdr.interests(collector.panes());
    let frame = publisher.replace(collection.input)?;
    diagnostics.record(&frame.gaps, now, |line| eprintln!("{line}"));
    if once {
        collection.sample?;
    }
    Ok(())
}

struct Observation {
    publisher: crate::observation::Publisher,
    _server: crate::observation::ipc::Server,
    collector: crate::observation::collect::Collector,
    exchange: Exchange,
    _commits: Option<wake::Commits>,
    herdr: wake::Herdr,
}

impl Observation {
    fn new(node: &Node<'_>, interval: Duration, dirty: &wake::Dirty) -> Result<Self> {
        let publisher = crate::observation::Publisher::new()?;
        let mut server = crate::observation::ipc::Server::new(&node.ledger.dir)?;
        let interests = crate::observation::peers::Interests::default();
        server.branches(interests.clone(), dirty.clone());
        if let Err(error) = server.bind(publisher.subscribe()) {
            eprintln!("observation socket unavailable: {error}");
        }
        let commits = match wake::Commits::start(node.ledger, dirty.clone()) {
            Ok(listener) => Some(listener),
            Err(error) => {
                eprintln!("dogsled service: observation commit wake unavailable: {error}");
                None
            }
        };
        Ok(Self {
            publisher,
            _server: server,
            collector: crate::observation::collect::Collector::default(),
            exchange: Exchange {
                peers: crate::observation::peers_runtime::Observers::default(),
                branches: crate::observation::peers::Branches::default(),
                interests,
                diagnostics: crate::observation::diagnostics::Diagnostics::default(),
            },
            _commits: commits,
            herdr: wake::Herdr::start(dirty.clone(), interval),
        })
    }

    fn refresh(
        &mut self,
        node: &Node<'_>,
        policy: &Policy,
        dirty: &wake::Dirty,
        cycle: (u8, bool),
    ) -> Result<()> {
        let (domains, once) = cycle;
        let fleet = Fleet {
            shell: node.shell,
            ledger: node.ledger,
            policy,
            home: node.home.to_owned(),
            hosts: vec![crate::hosts::Host::local()],
        };
        self.exchange.peers.retry(dirty.take_peers());
        if let Err(error) = self.exchange.peers.discover(&fleet, dirty, once) {
            eprintln!("dogsled peer discovery: {error}");
        }
        publish(
            node,
            &mut self.collector,
            &mut self.publisher,
            &mut self.herdr,
            (domains, once),
            &mut self.exchange,
            &fleet,
        )
    }
}

fn run(node: &Node<'_>, policy: &Policy, interval: Duration, once: bool) -> Result<()> {
    let (send, recv) = mpsc::channel();
    let dirty = wake::Dirty::new(send.clone());
    let mut observation = Observation::new(node, interval, &dirty)?;
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
    loop {
        let scheduled = Instant::now() >= next;
        let domains = dirty.take();
        // The socket only reduces latency. Recover pending requests on every publication
        // iteration, including when the socket cannot bind or a notification was lost.
        requested |= crate::sweep::pending(node.ledger)?;
        observation.refresh(
            node,
            policy,
            &dirty,
            (
                if scheduled {
                    domains | wake::LOCAL
                } else {
                    domains
                },
                once,
            ),
        )?;
        if scheduled {
            start_lanes(node, policy, once, &mut workers, completed, &send);
            next += interval;
        }
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
                    let node = Node::new(&shell, &ledger, &policy, &home);
                    let _ = send.send(Event::Swept(crate::sweep::requested(&node)));
                }));
            }
            if once && completed.iter().all(|done| *done) && sweep.is_none() {
                return Ok(());
            }
            match recv.recv_timeout(next.saturating_duration_since(Instant::now())) {
                Ok(Event::Sweep) => requested = true,
                Ok(Event::Observe) => {
                    if dirty.pending() && !once {
                        break;
                    }
                }
                Ok(Event::Swept(result)) => {
                    dirty.mark(wake::LOCAL);
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
                    diagnostics.record(node.ledger, LANES[lane], result);
                    dirty.mark(wake::LEDGER);
                    if once {
                        start_lanes(node, policy, once, &mut workers, completed, &send);
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
