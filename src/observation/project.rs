//! Shared reader projections. Capture replaces one immutable local input value; peer-only
//! composition borrows that value and a single frozen peer set, without observation or I/O.

use super::{Availability, Domains, Gap, Publication};
use crate::board::{self, Board, TodoSections};
use crate::commands::Status;
use crate::config::Thresholds;
use crate::error::{Error, Result};
use crate::hosts::{Fleet, HostReport};
use portal_types::agents::AgentView;
use portal_types::questions::Pending;
use portal_types::reviews::Review;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Every domain carries its gaps with its data. `None` means no usable input, not empty
/// success. Retained source values remain useful, but their failures travel with them.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct View<T> {
    pub data: Option<T>,
    pub gaps: Vec<Gap>,
}

#[derive(Debug)]
pub(crate) struct LocalInputs {
    pub now: u64,
    captured_ms: u64,
    inventory: Option<Value>,
    todos: Option<Vec<TodoSections>>,
    enrichment: Option<board::Enrichment>,
    watchdog: Option<crate::watchdog::State>,
    findings: Option<BTreeMap<String, crate::gate::Findings>>,
    agents: Option<Vec<AgentView>>,
    questions: Option<Vec<Pending>>,
    reviews: Option<Vec<Review>>,
    gaps: Vec<Gap>,
}

fn capture<T>(domain: &str, result: Result<T>, gaps: &mut Vec<Gap>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            gaps.push(Gap {
                domain: domain.into(),
                availability: Availability::Failed,
                detail: error.to_string(),
            });
            None
        }
    }
}

fn domain<T: serde::de::DeserializeOwned>(local: &Domains, name: &str) -> Result<T> {
    let value = local
        .get(name)
        .ok_or_else(|| Error::Invalid(format!("{name} observation unavailable")))?;
    Ok(serde_json::from_value(value.clone())?)
}

impl LocalInputs {
    fn capture(
        fleet: &Fleet<'_>,
        input: &Publication,
        titles: &mut crate::web::agents::Titles,
    ) -> Self {
        let mut gaps = Vec::new();
        let host_failed = input.gaps.iter().any(|gap| gap.domain == "host");
        let status = capture(
            "status",
            if host_failed {
                super::collect::status(fleet.ledger, Vec::new(), input.published_ms)
            } else {
                domain::<Status>(&input.local, "status")
            },
            &mut gaps,
        );
        // Ages use the collection time. A peer-only wake must not alter local-derived views.
        let now = input
            .sources
            .local
            .get("status")
            .copied()
            .unwrap_or(input.published_ms);
        let todos = capture("todos", domain(&input.local, "todos"), &mut gaps);
        let enrichment = status.as_ref().and_then(|status| {
            let messages = capture(
                "board",
                crate::store::read(fleet.ledger, crate::web_store::inbox::all_records),
                &mut gaps,
            )?;
            Some(board::capture_enrichment(
                &board::build(status, None, now),
                status,
                messages,
                fleet.shell,
            ))
        });
        let watchdog = capture(
            "stopped_activity",
            crate::host_store::watchdog(fleet.ledger).map(Option::unwrap_or_default),
            &mut gaps,
        );
        let launches = capture("findings", crate::launch::requests(fleet.ledger), &mut gaps);
        let custody = capture("findings", crate::custody::list(fleet.ledger), &mut gaps);
        let terminals = capture::<crate::probe::Terminals>(
            "agents",
            domain(&input.local, "terminals"),
            &mut gaps,
        );
        let agents = capture(
            "agents",
            terminals
                .as_ref()
                .ok_or_else(|| Error::Invalid("agent observation unavailable".into()))
                .and_then(|terminals| crate::web::agents::capture(&terminals.agents, titles)),
            &mut gaps,
        );
        let questions = capture(
            "questions",
            crate::web::store::pending(fleet.ledger),
            &mut gaps,
        );
        let reviews = capture(
            "reviews",
            crate::web::reviews::pending(fleet.ledger),
            &mut gaps,
        );
        let findings = status.as_ref().and_then(|status| {
            capture(
                "findings",
                captured_findings(
                    status,
                    todos.as_deref().unwrap_or_default(),
                    launches.as_deref()?,
                    custody.as_deref()?,
                    terminals.as_ref(),
                ),
                &mut gaps,
            )
        });
        Self {
            now,
            captured_ms: input.published_ms,
            inventory: status.and_then(|status| {
                capture(
                    "status",
                    serde_json::to_value(status).map_err(Into::into),
                    &mut gaps,
                )
            }),
            todos,
            enrichment,
            watchdog,
            findings,
            agents,
            questions,
            reviews,
            gaps,
        }
    }
}

/// Gate ownership can compare local checkout paths. Resolve it during capture only.
fn captured_findings(
    status: &Status,
    todos: &[TodoSections],
    launches: &[crate::work::Launch],
    custody: &[crate::custody::Custody],
    terminals: Option<&crate::probe::Terminals>,
) -> Result<BTreeMap<String, crate::gate::Findings>> {
    let terminals = terminals
        .ok_or_else(|| Error::Invalid("owner findings: local panes unavailable".into()))?;
    owners(status, todos)
        .into_iter()
        .map(|owner| {
            let finding = if let Some(report) = &status.sweep {
                let session = terminals
                    .agents
                    .iter()
                    .find(|agent| agent.pane == owner)
                    .and_then(|agent| agent.agent_session.as_deref());
                crate::gate::findings_from(
                    owner,
                    report,
                    &status.items,
                    &terminals.panes,
                    launches,
                    custody,
                    session,
                )
            } else {
                crate::gate::Findings::default()
            };
            Ok((owner.to_owned(), finding))
        })
        .collect()
}

#[derive(Default)]
pub(crate) struct Projector {
    cached: Option<(Value, Arc<LocalInputs>)>,
    titles: crate::web::agents::Titles,
}

impl Projector {
    pub fn inputs(&mut self, fleet: &Fleet<'_>, input: &Publication) -> Arc<LocalInputs> {
        let key = json!({"local":input.local,"measured":input.sources.local,"gaps":input.gaps.iter().filter(|gap| !gap.domain.starts_with("peer:")).collect::<Vec<_>>()});
        if let Some((previous, inputs)) = &self.cached
            && *previous == key
        {
            return Arc::clone(inputs);
        }
        let inputs = Arc::new(LocalInputs::capture(fleet, input, &mut self.titles));
        self.cached = Some((key, Arc::clone(&inputs)));
        inputs
    }
}

fn insert<T: Serialize>(
    views: &mut Domains,
    key: &str,
    data: Option<T>,
    gaps: &[Gap],
) -> Result<()> {
    views.insert(
        key.into(),
        serde_json::to_value(View {
            data,
            gaps: gaps.to_vec(),
        })?,
    );
    Ok(())
}

fn owners<'a>(status: &'a Status, todos: &'a [TodoSections]) -> BTreeSet<&'a str> {
    status
        .items
        .iter()
        .map(|item| item.owner_pane.as_str())
        .chain(
            status
                .queue
                .iter()
                .map(|ticket| ticket.request.owner.as_str()),
        )
        .chain(todos.iter().map(|todo| todo.pane.as_str()))
        .chain(
            status
                .hosts
                .iter()
                .filter(|report| report.host.is_local())
                .filter_map(|report| report.probe.as_ref())
                .flat_map(|probe| probe.agent_panes.iter().map(|agent| agent.pane.as_str())),
        )
        .chain(
            status
                .holds
                .iter()
                .filter_map(|hold| hold.owner_pane.as_deref()),
        )
        .collect()
}

fn enriched(status: &Status, owner: Option<&str>, local: &LocalInputs) -> Option<Board> {
    let mut board = board::build(status, owner, local.now);
    board::enrich_captured(
        &mut board,
        status,
        local.todos.as_ref()?,
        local.enrichment.as_ref()?,
    );
    Some(board)
}

/// All derivation below is pure: no ledger, shell, socket, transcript or filesystem handle.
/// The local owner namespace is this publication's host; foreign jobs retain execution IDs.
pub(crate) fn derive(
    local: &LocalInputs,
    mut reports: Vec<HostReport>,
    limits: &Thresholds,
    input: &mut Publication,
) -> Result<()> {
    for report in &mut reports {
        for job in &mut report.jobs {
            job.host.clone_from(&report.host.id);
        }
    }
    let mut gaps = input.gaps.clone();
    gaps.extend(local.gaps.iter().cloned());
    let status = local
        .inventory
        .as_ref()
        .map(|value| -> Result<_> {
            let mut status: Status = serde_json::from_value(value.clone())?;
            status.hosts.clone_from(&reports);
            Ok(status)
        })
        .transpose()?;
    let status = match (status, local.watchdog.as_ref()) {
        (Some(status), Some(state)) => {
            let mut status = super::collect::assemble(status);
            status.stopped_activity =
                crate::watchdog::stopped_activity_from(state, &status, local.now);
            Some(status)
        }
        _ => None,
    };
    for (domain, available) in [
        ("board", local.enrichment.is_some()),
        ("agents", local.agents.is_some()),
        ("questions", local.questions.is_some()),
        ("reviews", local.reviews.is_some()),
        ("findings", local.findings.is_some()),
        ("stopped_activity", local.watchdog.is_some()),
    ] {
        if available {
            input.sources.local.insert(domain.into(), local.captured_ms);
        }
    }
    let views = &mut input.views;
    views.clear();
    insert(views, "hosts", Some(&reports), &gaps)?;
    insert(views, "status", status.as_ref(), &gaps)?;
    insert(
        views,
        "status_display",
        status.as_ref().map(super::display::Display::capture),
        &gaps,
    )?;
    insert(views, "todos", local.todos.as_ref(), &gaps)?;
    let board = status
        .as_ref()
        .and_then(|status| enriched(status, None, local));
    insert(views, "board", board.as_ref(), &gaps)?;
    let empty_board = board.as_ref().map(|board| Board {
        hosts: board.hosts.clone(),
        rows: Vec::new(),
    });
    insert(views, "empty_board", empty_board, &gaps)?;
    insert(
        views,
        "inventory",
        status
            .as_ref()
            .map(|status| json!({"items":status.items,"queue":status.queue})),
        &gaps,
    )?;
    insert(
        views,
        "empty_inventory",
        status.as_ref().map(|_| json!({"items":[],"queue":[]})),
        &gaps,
    )?;
    let sidebar: Vec<_> = reports
        .iter()
        .map(|report| crate::sidebar::model::HostRow::from_report(report, limits, local.now))
        .collect();
    insert(views, "sidebar_hosts", Some(sidebar), &gaps)?;
    let jobs: Vec<_> = reports
        .iter()
        .flat_map(|report| report.jobs.iter())
        .collect();
    insert(views, "jobs", Some(&jobs), &gaps)?;
    let active_jobs: Vec<_> = jobs.iter().filter(|job| job.ended_ms.is_none()).collect();
    insert(views, "active_jobs", Some(active_jobs), &gaps)?;
    scoped_views(views, status.as_ref(), local, &reports, &jobs, &gaps)?;
    insert(views, "findings", local.findings.as_ref(), &gaps)?;
    agents_view(views, local, &gaps)?;
    let portal = portal(
        local,
        board.as_ref(),
        status.as_ref(),
        &gaps,
        input.published_ms,
    );
    insert(views, "portal_board", portal.board.as_ref(), &gaps)?;
    insert(views, "inbox", portal.inbox.as_ref(), &gaps)?;
    insert(views, "portal", Some(portal), &gaps)?;
    input.gaps = gaps;
    Ok(())
}

fn agents_view(views: &mut super::Domains, local: &LocalInputs, gaps: &[Gap]) -> Result<()> {
    let gaps: Vec<_> = gaps
        .iter()
        .filter(|gap| matches!(gap.domain.as_str(), "agents" | "terminals"))
        .cloned()
        .collect();
    insert(views, "agents", local.agents.as_ref(), &gaps)
}

fn scoped_views(
    views: &mut super::Domains,
    status: Option<&Status>,
    local: &LocalInputs,
    reports: &[HostReport],
    jobs: &[&crate::jobs::Job],
    gaps: &[Gap],
) -> Result<()> {
    let hosts: Vec<_> = reports.iter().map(|report| report.host.clone()).collect();
    let mut boards = BTreeMap::new();
    let mut histories = BTreeMap::new();
    let mut inventories = BTreeMap::new();
    if let Some(status) = &status {
        for owner in owners(status, local.todos.as_deref().unwrap_or_default()) {
            boards.insert(owner, enriched(status, Some(owner), local));
            let owned: BTreeSet<_> = status
                .items
                .iter()
                .filter(|item| item.kind == "job" && item.owner_pane == owner)
                .map(|item| (item.host.as_str(), item.id.as_str()))
                .collect();
            let history: Vec<_> = jobs
                .iter()
                .filter(|job| owned.contains(&(job.host.as_str(), job.id.as_str())))
                .collect();
            histories.insert(owner, json!({"jobs":history,"hosts":hosts}));
            let items: Vec<_> = status
                .items
                .iter()
                .filter(|item| item.owner_pane == owner)
                .collect();
            let queue: Vec<_> = status
                .queue
                .iter()
                .filter(|ticket| ticket.request.owner == owner)
                .collect();
            inventories.insert(owner, Some(json!({"items":items,"queue":queue})));
        }
    }
    insert(views, "boards", status.as_ref().map(|_| boards), gaps)?;
    insert(
        views,
        "inventories",
        status.as_ref().map(|_| inventories),
        gaps,
    )?;
    insert(
        views,
        "job_history",
        status.as_ref().map(|_| histories),
        gaps,
    )?;
    Ok(())
}

fn portal(
    local: &LocalInputs,
    board: Option<&Board>,
    status: Option<&Status>,
    gaps: &[Gap],
    published_ms: u64,
) -> portal_types::PortalSnapshot {
    let portal_board = board.zip(local.todos.as_ref()).map(|(board, todos)| {
        crate::web::board::view(
            board,
            status.map_or(&[][..], |status| status.items.as_slice()),
            todos,
            local.now,
        )
    });
    let inbox = match (
        &portal_board,
        &local.agents,
        &local.reviews,
        &local.questions,
    ) {
        (Some(board), Some(agents), Some(reviews), Some(questions)) => Some(
            crate::web::inbox::view(board, reviews, questions, agents, local.now),
        ),
        _ => None,
    };
    portal_types::PortalSnapshot {
        version: super::VERSION,
        epoch: String::new(),
        revision: 0,
        published_ms,
        board: portal_board,
        agents: local.agents.clone(),
        inbox,
        gaps: gaps.to_vec(),
    }
}
