//! Join the origin's immutable request to its execution host, only for the portal.

use crate::hosts::{HostReport, LOCAL};
use crate::observation::owned_work::{Execution, Kind, Owned, Phase};
use crate::observation::peers::Ownership;
use portal_types::{BoardView, GroupView, RepoGroup, RowView};
use std::collections::BTreeMap;

type Key<'a> = (&'a str, &'a str, &'a str, bool);
type Match<'a> = (&'a HostReport, &'a Execution, bool);

struct Index<'a> {
    executions: BTreeMap<Key<'a>, Vec<Match<'a>>>,
    names: BTreeMap<&'a str, usize>,
}

impl<'a> Index<'a> {
    fn build(reports: &'a [HostReport], sources: &'a BTreeMap<String, Ownership>) -> Self {
        let mut index = Self {
            executions: BTreeMap::new(),
            names: BTreeMap::new(),
        };
        for report in reports {
            let Some(source) = sources.get(&report.host.id) else {
                continue;
            };
            let Some(data) = &source.evidence else {
                continue;
            };
            *index.names.entry(data.hostname.as_str()).or_default() += 1;
            for exec in &data.executions {
                let key = (
                    exec.owner_hostname.as_str(),
                    exec.owner_pane.as_str(),
                    exec.request_id.as_str(),
                    exec.kind == Kind::Job,
                );
                index
                    .executions
                    .entry(key)
                    .or_default()
                    .push((report, exec, source.current));
            }
        }
        index
    }

    fn execution(&self, hostname: &str, item: &Owned) -> Option<Match<'a>> {
        if item.phase != Phase::Admitted || self.names.get(hostname) != Some(&1) {
            return None;
        }
        let key = (
            hostname,
            item.owner_pane.as_str(),
            item.request_id.as_str(),
            item.kind == Kind::Job,
        );
        let [found] = self.executions.get(&key)?.as_slice() else {
            return None;
        };
        item.pane
            .as_ref()
            .is_none_or(|pane| pane == &found.1.pane)
            .then_some(*found)
    }
}

fn owner_matches(item: &Owned, report: &HostReport) -> bool {
    item.owner_session.as_ref().is_none_or(|session| {
        report.probe.as_ref().is_some_and(|probe| {
            probe.agent_panes.iter().any(|agent| {
                agent.pane == item.owner_pane && agent.agent_session.as_ref() == Some(session)
            })
        })
    })
}

fn imported(item: &Owned) -> RowView {
    let (state, group) = match item.phase {
        Phase::Admitted => ("unknown", GroupView::Attention),
        Phase::Pending => ("queued", GroupView::Waiting),
        Phase::Retained => ("closed", GroupView::ToLand),
    };
    RowView {
        id: item.request_id.clone(),
        kind: if item.kind == Kind::Job {
            "job"
        } else {
            "agent"
        }
        .into(),
        group,
        host: "unknown".into(),
        pane: None,
        owner_pane: None,
        owner_host: None,
        title: None,
        cleanup: None,
        unseen: false,
        name: item.name.clone(),
        what: if item.kind == Kind::Job {
            "job"
        } else {
            "worker"
        }
        .into(),
        state: state.into(),
        activity: String::new(),
        age_ms: 0,
        quiet_ms: None,
        turn_clock: None,
        ahead: None,
        dirty: false,
        repository: item.repository.clone(),
        ownership_stale: false,
    }
}

fn apply(
    row: &mut RowView,
    item: &Owned,
    origin: &HostReport,
    current: bool,
    execution: Option<Match<'_>>,
) {
    let valid_owner = owner_matches(item, origin);
    row.owner_host = Some(origin.host.label.clone());
    row.owner_pane = valid_owner.then(|| item.owner_pane.clone());
    row.ownership_stale = !current || !valid_owner;
    row.pane = None;
    // A todo on the viewing host cannot name a pane on another host.
    if !origin.host.is_local() || row.host != LOCAL {
        row.title = None;
    }
    let Some((host, exec, fresh)) = execution else {
        if item.phase == Phase::Admitted {
            row.ownership_stale = true;
        }
        return;
    };
    row.host.clone_from(&host.host.label);
    row.ownership_stale |= !fresh;
    row.title = Some(exec.job_label.as_ref().unwrap_or(&exec.title).clone());
    row.pane = Some(exec.pane.clone()); // identity for de-duplication; stale is never actionable
    match exec.kind {
        Kind::Agent => {
            if let Some(agent) = host
                .probe
                .as_ref()
                .and_then(|p| p.agent_panes.iter().find(|a| a.pane == exec.pane))
            {
                if !row.ownership_stale {
                    row.turn_clock = exec.turn_clock;
                }
                if !origin.host.is_local() {
                    row.state.clone_from(&agent.state);
                }
                if let Some(repo) = &agent.repository {
                    row.repository = Some(repo.clone());
                }
            } else {
                row.ownership_stale = true;
            }
        }
        Kind::Job => {
            if let Some(job) = host
                .jobs
                .iter()
                .find(|j| j.id == exec.request_id && j.pane == exec.pane)
            {
                row.state = match (job.ended_ms, job.exit) {
                    (Some(_), Some(0)) => "done".into(),
                    (Some(_), Some(exit)) => format!("exit {exit}"),
                    _ => job.state.clone(),
                };
            } else {
                row.ownership_stale = true;
            }
        }
    }
    if !origin.host.is_local() && matches!(row.state.as_str(), "working" | "running") {
        row.group = GroupView::Running;
    }
}

/// Input board contains local-owner rows only. No imported row enters another observer's export.
pub(crate) fn merge(
    board: &mut BoardView,
    reports: &[HostReport],
    sources: &BTreeMap<String, Ownership>,
    tickets: &[crate::work::Ticket],
) {
    let index = Index::build(reports, sources);
    let ids: BTreeMap<_, _> = tickets
        .iter()
        .map(|t| (t.id.as_str(), t.request.id.as_str()))
        .collect();
    let mut positions = BTreeMap::new();
    for (r, repo) in board.repos.iter().enumerate() {
        for (i, row) in repo.rows.iter().enumerate() {
            positions.insert(
                ids.get(row.id.as_str())
                    .copied()
                    .unwrap_or(&row.id)
                    .to_owned(),
                (r, i),
            );
        }
    }
    for report in reports {
        let Some(source) = sources.get(&report.host.id) else {
            continue;
        };
        let Some(evidence) = &source.evidence else {
            continue;
        };
        if let Some(host) = board
            .hosts
            .iter_mut()
            .find(|h| h.label == report.host.label)
        {
            host.hostname = Some(evidence.hostname.clone());
        }
        for item in &evidence.owned {
            let execution = index.execution(&evidence.hostname, item);
            if report.host.is_local() {
                if let Some(&(r, i)) = positions.get(&item.request_id) {
                    apply(
                        &mut board.repos[r].rows[i],
                        item,
                        report,
                        source.current,
                        execution,
                    );
                }
                continue;
            }
            let mut row = imported(item);
            apply(&mut row, item, report, source.current, execution);
            if row.repository.is_none() {
                row.repository = report
                    .probe
                    .as_ref()
                    .and_then(|p| p.agent_panes.iter().find(|a| a.pane == item.owner_pane))
                    .and_then(|a| a.repository.clone());
            }
            if let Some(repo) = board
                .repos
                .iter_mut()
                .find(|repo| repo.repository == row.repository)
            {
                repo.rows.push(row);
            } else {
                board.repos.push(RepoGroup {
                    repository: row.repository.clone(),
                    rows: vec![row],
                    todos: vec![],
                });
            }
        }
    }
}
