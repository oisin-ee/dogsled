//! Debug fixture boundary: stored capacity and read-only Herdr terminal observations enter
//! the production projector. No native sampling, lanes, locks or store writes are available.

use super::{
    Availability, Domains, Gap, Publication, Published, Publisher, Sources, collect, project,
};
use crate::error::{Error, Result};
use crate::hosts::{Cached, Fleet, Host};
use crate::ledger::Ledger;
use crate::shell::{Output, Shell, System};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

#[derive(Debug, Default)]
struct StoredOnly {
    system: System,
    denied: Mutex<BTreeSet<String>>,
}

impl StoredOnly {
    fn unavailable(&self, operation: &str) -> Error {
        self.denied
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(operation.into());
        Error::Invalid(format!(
            "external observation unavailable in state project: {operation}"
        ))
    }

    fn gaps(&self) -> Vec<Gap> {
        self.denied
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|operation| Gap {
                domain: format!("fixture:{operation}"),
                availability: Availability::Unavailable,
                detail: format!("external observation unavailable in state project: {operation}"),
            })
            .collect()
    }
}

impl Shell for StoredOnly {
    fn run(&self, program: &str, args: &[String]) -> Result<Output> {
        let read = match args {
            [entity, verb] => matches!(entity.as_str(), "agent" | "pane") && verb == "list",
            [entity, verb, flag, _pane] => {
                entity == "pane" && verb == "process-info" && flag == "--pane"
            }
            _ => false,
        };
        if program == "herdr" && read {
            self.system.run(program, args)
        } else {
            Err(self.unavailable(program))
        }
    }

    fn read_file(&self, path: &str) -> Result<String> {
        Err(self.unavailable(path))
    }
}

/// Topology is an explicit fixture input, never discovered from Herdr or another host.
pub(crate) fn project(ledger: &Ledger) -> Result<Arc<Published>> {
    let peers: Vec<Host> = serde_json::from_reader(std::io::stdin().lock())?;
    let policy = crate::config::load(&crate::config::default_path())?;
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    let shell = StoredOnly {
        system: System {
            budget: ledger.budget,
        },
        ..Default::default()
    };
    let fleet = Fleet::producer(
        &shell,
        ledger,
        &policy,
        home,
        std::iter::once(Host::local())
            .chain(peers.into_iter().filter(|host| !host.is_local()))
            .collect(),
    );
    crate::store::read_only(ledger, || {
        publication(&fleet, &shell, crate::ledger::now_ms())
    })
}

fn publication(fleet: &Fleet<'_>, shell: &StoredOnly, now: u64) -> Result<Arc<Published>> {
    let mut collector = collect::Collector::default();
    collector.record("terminals", now, crate::probe::terminals(shell));
    let terminals = collector.publication(now);
    let local = collect::host(
        &fleet.node(),
        crate::host_store::snapshot(fleet.ledger)?,
        true,
        now,
    )?;
    let mut reports = vec![fleet.cached_report(&Host::local(), Cached::from(local.clone()), now)];
    for host in fleet.hosts.iter().filter(|host| !host.is_local()) {
        let cached = crate::host_store::peer(fleet.ledger, &host.id)?.unwrap_or_default();
        reports.push(fleet.cached_report(host, cached, now));
    }
    let status = collect::status(fleet.ledger, reports.clone(), now)?;
    let mut input = Publication {
        published_ms: now,
        sources: Sources {
            local: BTreeMap::from([
                ("host".into(), now),
                ("headroom".into(), now),
                ("status".into(), now),
            ]),
            peers: BTreeMap::new(),
        },
        local: BTreeMap::from([
            ("host".into(), serde_json::to_value(local)?),
            (
                "headroom".into(),
                serde_json::to_value(&reports[0].headroom)?,
            ),
            ("status".into(), serde_json::to_value(status)?),
            (
                "todos".into(),
                serde_json::to_value(crate::board::cached_todos(fleet.ledger)?)?,
            ),
        ]),
        views: Domains::new(),
        gaps: Vec::new(),
    };
    input.local.extend(terminals.local);
    input.sources.local.extend(terminals.sources.local);
    input.gaps.extend(terminals.gaps);
    let captured = project::Projector::default().inputs(fleet, &input);
    input.gaps.extend(shell.gaps());
    project::derive(&captured, reports, &fleet.policy.into(), &mut input)?;
    Publisher::new()?.replace(input)
}

#[cfg(test)]
#[path = "stored_tests.rs"]
mod tests;
