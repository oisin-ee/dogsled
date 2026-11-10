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
use std::sync::Arc;
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
    /// A durable request (cleanup or quota refresh) may be pending.
    Wake,
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
                    || send.send(Event::Wake).is_err()
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

fn actions(
    ledger: &Ledger,
    policy: &Policy,
    home: String,
    lane: Lane,
    published: Arc<crate::observation::Published>,
    release_retries: &crate::launch::ReleaseRetries,
) -> Result<()> {
    let shell = System::default();
    if lane == Lane::Handoff {
        return crate::done::handoff(ledger, &shell, now_ms());
    }
    let fleet = crate::hosts::ActionFleet::new(&shell, ledger, policy, home, published)?;
    if lane == Lane::Quota {
        return crate::quota::lane(&fleet, now_ms());
    }
    if lane == Lane::Retention {
        // Retention owns its synchronization too; no other service lane is required.
        crate::launch::release_owned(&fleet, release_retries, now_ms())?;
        return crate::jobs::expire(ledger, now_ms());
    }
    match lane {
        Lane::Watchdog => {
            let now = now_ms();
            let reports = fleet.reports()?;
            crate::watchdog::tick(&fleet, &reports, now)?;
            items::reconcile(ledger, &reports, now)
        }
        Lane::Admission => work::tick(&fleet, release_retries, now_ms()),
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

struct Workers {
    release_retries: Arc<crate::launch::ReleaseRetries>,
    handles: [Option<std::thread::JoinHandle<()>>; LANES.len()],
    completed: [bool; LANES.len()],
}

impl Workers {
    fn new(release_retries: Arc<crate::launch::ReleaseRetries>) -> Self {
        Self {
            release_retries,
            handles: Default::default(),
            completed: [false; LANES.len()],
        }
    }

    /// Start eligible lanes without coupling their scheduling to publication or input.
    fn start(
        &mut self,
        node: &Node<'_>,
        policy: &Policy,
        once: bool,
        send: &mpsc::Sender<Event>,
        source: &crate::observation::Publisher,
    ) -> Result<()> {
        let published = source.current()?;
        for (index, lane) in LANES.iter().enumerate() {
            if !once || (!self.completed[index] && ready(*lane, self.completed)) {
                self.spawn(index, node, policy, send, &published);
            }
        }
        Ok(())
    }

    /// Durable requests outlive their wake, so they are reread on every wake and every pass.
    /// A quota refresh starts the quota lane now unless it is running; the result says whether
    /// a cleanup pass is requested.
    fn requests(
        &mut self,
        node: &Node<'_>,
        policy: &Policy,
        once: bool,
        send: &mpsc::Sender<Event>,
        source: &crate::observation::Publisher,
    ) -> Result<bool> {
        if !once && crate::quota::pending(node.ledger)? {
            let index = LANES
                .iter()
                .position(|lane| *lane == Lane::Quota)
                .ok_or_else(|| Error::Invalid("quota lane missing".into()))?;
            if self.handles[index].is_none() {
                self.spawn(index, node, policy, send, &source.current()?);
            }
        }
        crate::sweep::pending(node.ledger)
    }

    fn spawn(
        &mut self,
        lane: usize,
        node: &Node<'_>,
        policy: &Policy,
        send: &mpsc::Sender<Event>,
        published: &Arc<crate::observation::Published>,
    ) {
        if self.handles[lane].is_some() {
            return;
        }
        let mut ledger = node.ledger.clone();
        ledger.budget = crate::budget::Budget::default();
        let policy = policy.clone();
        let home = node.home.to_owned();
        let send = send.clone();
        let published = Arc::clone(published);
        let release_retries = Arc::clone(&self.release_retries);
        self.handles[lane] = Some(std::thread::spawn(move || {
            let _ = send.send(Event::Finished(
                lane,
                actions(
                    &ledger,
                    &policy,
                    home,
                    LANES[lane],
                    published,
                    &release_retries,
                ),
            ));
        }));
    }

    fn finished(&mut self, lane: usize) -> Result<()> {
        if let Some(handle) = self.handles[lane].take() {
            handle
                .join()
                .map_err(|_| Error::Invalid("service action worker failed".into()))?;
        }
        self.completed[lane] = true;
        Ok(())
    }
}

struct Exchange {
    peers: crate::observation::peers_runtime::Observers,
    branches: crate::observation::peers::Branches,
    interests: crate::observation::peers::Interests,
    diagnostics: crate::observation::diagnostics::Diagnostics,
    details: crate::observation::details::Collector,
    activity: crate::observation::live_activity::Handle,
    dirty: wake::Dirty,
    timings: crate::observation::timing::PassTimings,
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
        details,
        activity,
        dirty,
        timings,
    } = exchange;
    let now = now_ms();
    let collection = crate::store::observing(|| -> Result<_> {
        let started = Instant::now();
        let gaps = if domains & (wake::LOCAL | wake::BRANCHES) != 0 {
            branches.observe(node, &interests.current()?, now)?
        } else {
            branches.gaps()
        };
        timings.phase("branches", started.elapsed());
        let started = Instant::now();
        let mut collection = if domains & wake::LOCAL == wake::LOCAL {
            collector.collect(node, now)
        } else if domains & (wake::LEDGER | wake::BRANCHES) != 0 {
            collector.refresh(node, now, false)
        } else {
            crate::observation::collect::Collection {
                input: collector.publication(now),
                sample: Ok(()),
                timings: Vec::new(),
            }
        };
        timings.phase("collect", started.elapsed());
        for (name, elapsed) in collection.timings.drain(..) {
            timings.phase(name, elapsed);
        }
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
        let started = Instant::now();
        let result = peers.compose(fleet, &mut collection.input, now, collector.reports());
        timings.phase("peers.compose", started.elapsed());
        result?;
        activity.observe(&collection.input)?;
        let started = Instant::now();
        let result = details.refresh(&mut collection.input, dirty);
        timings.phase("details.refresh", started.elapsed());
        result?;
        Ok(collection)
    })?;
    let started = Instant::now();
    herdr.interests(collector.panes());
    timings.phase("herdr.interests", started.elapsed());
    let started = Instant::now();
    let result = diagnostics.publish(publisher, collection.input, now, |line| eprintln!("{line}"));
    timings.phase("publish.replace", started.elapsed());
    result?;
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
        let mut publisher = crate::observation::Publisher::new()?;
        let mut server = crate::observation::ipc::Server::new(&node.ledger.dir)?;
        let interests = crate::observation::peers::Interests::default();
        server.branches(interests.clone(), dirty.clone());
        let details = crate::observation::details::Collector::default();
        let (commands, feed, activity) = crate::observation::live_activity::Authority::spawn(
            publisher.epoch().into(),
            System::default(),
        )?;
        server.activity(commands, feed);
        server.details(details.interests.clone());
        publisher.details(details.payloads.clone());
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
                details,
                activity,
                dirty: dirty.clone(),
                timings: crate::observation::timing::PassTimings::default(),
            },
            _commits: commits,
            herdr: wake::Herdr::start(dirty.clone(), interval),
        })
    }

    /// The earliest of the schedule and the first baseline sample.
    fn due(next: Instant, baseline: Option<Instant>) -> Instant {
        baseline.map_or(next, |at| next.min(at))
    }

    fn timed_refresh(
        &mut self,
        node: &Node<'_>,
        policy: &Policy,
        dirty: &wake::Dirty,
        cycle: (u8, bool),
        scheduled: bool,
    ) -> Result<()> {
        let (domains, once) = cycle;
        self.exchange.timings.pass(
            domains
                | if scheduled {
                    crate::observation::timing::SCHEDULED
                } else {
                    0
                },
        );
        let result = self.refresh(
            node,
            policy,
            dirty,
            (
                if scheduled {
                    domains | wake::LOCAL
                } else {
                    domains
                },
                once,
            ),
        );
        self.exchange
            .timings
            .flush(Instant::now(), |line| eprintln!("{line}"));
        result
    }

    fn refresh(
        &mut self,
        node: &Node<'_>,
        policy: &Policy,
        dirty: &wake::Dirty,
        cycle: (u8, bool),
    ) -> Result<()> {
        let (domains, once) = cycle;
        let fleet = Fleet::producer(
            node.shell,
            node.ledger,
            policy,
            node.home.to_owned(),
            vec![crate::hosts::Host::local()],
        );
        let started = Instant::now();
        self.exchange.peers.retry(dirty.take_peers());
        self.exchange
            .timings
            .phase("peers.retry", started.elapsed());
        let started = Instant::now();
        let discovery = self.exchange.peers.discover(&fleet, dirty, once);
        self.exchange.timings.phase("discover", started.elapsed());
        if let Err(error) = discovery {
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

const BASELINE_SAMPLE_DELAY: Duration = Duration::from_secs(1);

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
    let mut workers = Workers::new(Arc::new(crate::launch::ReleaseRetries::default()));
    let mut diagnostics = LaneDiagnostics::default();
    let mut next = Instant::now();
    let mut baseline = (!once).then(|| next + BASELINE_SAMPLE_DELAY);
    loop {
        let scheduled = Instant::now() >= next;
        let baseline_due = baseline.take_if(|at| Instant::now() >= *at).is_some();
        let domains = dirty.take();
        observation.timed_refresh(
            node,
            policy,
            &dirty,
            (domains, once),
            scheduled || baseline_due,
        )?;
        if baseline_due {
            eprintln!("dogsled service: baseline sample published");
        }
        if scheduled {
            workers.start(node, policy, once, &send, &observation.publisher)?;
            next += interval;
        }
        // The socket only reduces latency. Recover pending requests on every publication
        // iteration, including when the socket cannot bind or a notification was lost.
        requested |= workers.requests(node, policy, once, &send, &observation.publisher)?;
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
            if once && workers.completed.iter().all(|done| *done) && sweep.is_none() {
                return Ok(());
            }
            let observation_due = Observation::due(next, baseline);
            match recv.recv_timeout(observation_due.saturating_duration_since(Instant::now())) {
                Ok(Event::Wake) => {
                    requested |=
                        workers.requests(node, policy, once, &send, &observation.publisher)?;
                }
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
                    workers.finished(lane)?;
                    diagnostics.record(node.ledger, LANES[lane], result);
                    dirty.mark(wake::LEDGER);
                    if once {
                        if LANES[lane] == Lane::Handoff {
                            observation.timed_refresh(
                                node,
                                policy,
                                &dirty,
                                (wake::LEDGER, once),
                                false,
                            )?;
                        }
                        workers.start(node, policy, once, &send, &observation.publisher)?;
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
