//! Independent publication and action lanes. A slow ticket never owns the probe clock.

use crate::config::Policy;
use crate::error::{Error, Result};
use crate::hosts::Fleet;
use crate::ledger::{Ledger, now_ms};
use crate::node::Node;
use crate::shell::System;
use crate::{items, work};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn actions(ledger: &Ledger, policy: &Policy, home: String) -> Result<()> {
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
    work::tick(&fleet, now_ms())?;
    crate::cleanup::auto(&fleet.node(), now_ms())
}

pub fn serve(node: &Node<'_>, policy: &Policy, interval: Duration, once: bool) -> Result<()> {
    let (send, recv) = mpsc::channel();
    let mut worker = None;
    let mut next = Instant::now();
    loop {
        node.tick(now_ms())?;
        if worker.is_none() {
            {
                let mut ledger = node.ledger.clone();
                ledger.budget = crate::budget::Budget::default();
                let policy = policy.clone();
                let home = node.home.to_owned();
                let send = send.clone();
                worker = Some(std::thread::spawn(move || {
                    let _ = send.send(actions(&ledger, &policy, home));
                }));
            }
        }
        next += interval;
        match recv.recv_timeout(next.saturating_duration_since(Instant::now())) {
            Ok(result) => {
                if let Some(handle) = worker.take() {
                    handle
                        .join()
                        .map_err(|_| Error::Invalid("service action worker failed".into()))?;
                }
                if let Err(error) = result {
                    eprintln!("wrangle service actions: {error}");
                }
                if once {
                    return Ok(());
                }
                std::thread::sleep(next.saturating_duration_since(Instant::now()));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(Error::Invalid("service worker disconnected".into()));
            }
        }
    }
}
