//! Publication, watchdog, and admission have independent clocks and budgets.

use crate::config::Policy;
use crate::error::{Error, Result};
use crate::hosts::Fleet;
use crate::ledger::{Ledger, now_ms};
use crate::node::Node;
use crate::shell::System;
use crate::{items, work};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn actions(ledger: &Ledger, policy: &Policy, home: String, watchdog: bool) -> Result<()> {
    let shell = System::default();
    let fleet = if items::list(ledger)?.is_empty() && work::queue(ledger)?.is_empty() {
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
    if watchdog {
        let now = now_ms();
        let reports = fleet.status(now)?;
        crate::watchdog::tick(&fleet, &reports, now)?;
        items::reconcile(ledger, &reports, now)
    } else {
        work::tick(&fleet, now_ms())
    }
}

pub fn serve(node: &Node<'_>, policy: &Policy, interval: Duration, once: bool) -> Result<()> {
    let (send, recv) = mpsc::channel();
    let mut workers: [Option<std::thread::JoinHandle<()>>; 2] = [None, None];
    let mut completed = [false; 2];
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
                    let _ = send.send((lane, actions(&ledger, &policy, home, lane == 0)));
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
                        eprintln!("wrangle service lane {lane}: {error}");
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
