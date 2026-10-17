//! Publication, watchdog, admission, and the sweep have independent clocks and budgets.
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
    Sweep,
    Inbox,
}

const LANES: [Lane; 4] = [Lane::Watchdog, Lane::Admission, Lane::Sweep, Lane::Inbox];

fn actions(ledger: &Ledger, policy: &Policy, home: String, lane: Lane) -> Result<()> {
    let shell = System::default();
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
        Lane::Sweep => crate::sweep::pass(&fleet, now_ms()),
        Lane::Inbox => crate::inbox::tick(&fleet, now_ms()),
    }
}

pub fn serve(node: &Node<'_>, policy: &Policy, interval: Duration, once: bool) -> Result<()> {
    let (send, recv) = mpsc::channel();
    let mut workers: [Option<std::thread::JoinHandle<()>>; LANES.len()] = Default::default();
    let mut completed = [false; LANES.len()];
    let mut next = Instant::now();
    loop {
        node.tick(now_ms())?;
        for (lane, worker) in workers.iter_mut().enumerate() {
            if worker.is_none() && (!once || !completed[lane]) {
                let mut ledger = node.ledger.clone();
                ledger.budget = crate::budget::Budget::default();
                let policy = policy.clone();
                let home = node.home.to_owned();
                let send = send.clone();
                *worker = Some(std::thread::spawn(move || {
                    let _ = send.send((lane, actions(&ledger, &policy, home, LANES[lane])));
                }));
            }
        }
        next += interval;
        loop {
            match recv.recv_timeout(next.saturating_duration_since(Instant::now())) {
                Ok((lane, result)) => {
                    if let Some(handle) = workers[lane].take() {
                        handle
                            .join()
                            .map_err(|_| Error::Invalid("service action worker failed".into()))?;
                    }
                    completed[lane] = true;
                    if let Err(error) = result {
                        eprintln!("wrangle service lane {:?}: {error}", LANES[lane]);
                    }
                    if once && completed.iter().all(|done| *done) {
                        return Ok(());
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
