//! Compose receiver-relative autonomy and decision marks from host-owned facts.

use crate::autonomy::{self, PublishedAutonomy};
use crate::hosts::{HostReport, LOCAL};
use crate::observation::{Availability, Gap, PeerSource, peers::Ownership};
use portal_types::agents::AgentView;
use portal_types::autonomy::{AutonomyView, DecisionMark, DecisionView};
use std::collections::BTreeMap;

pub(super) struct Projection {
    pub(super) snapshot: Option<AutonomyView>,
    pub(super) published: PublishedAutonomy,
    pub(super) gaps: Vec<Gap>,
}

pub(super) fn project(
    local_hostname: &str,
    reports: &[HostReport],
    ownership: &BTreeMap<String, Ownership>,
    peers: &BTreeMap<String, PeerSource>,
    agents: &mut [AgentView],
    local_host_current: bool,
) -> Projection {
    let mut snapshot = AutonomyView::default();
    let mut gaps = Vec::new();
    let mut extensions = Vec::new();
    let mut executions = Vec::new();
    let mut labels_by_hostname = BTreeMap::new();
    let mut local_ready = false;
    for report in reports {
        let label = report.host.label.as_str();
        let source = ownership.get(&report.host.id);
        let evidence = source.and_then(|source| source.evidence.as_deref());
        let hostname = evidence
            .map(|evidence| evidence.hostname.as_str())
            .or_else(|| report.host.is_local().then_some(local_hostname));
        if let Some(hostname) = hostname {
            labels_by_hostname
                .entry(hostname)
                .and_modify(|label| *label = None)
                .or_insert(Some(label));
        }
        executions.extend(evidence.into_iter().flat_map(|evidence| {
            evidence
                .executions
                .iter()
                .map(move |execution| (label, execution))
        }));
        let state = source.and_then(|source| source.autonomy_state.as_ref());
        let local = report.host.is_local();
        if local {
            local_ready = local_host_current && state == Some(&Availability::Available);
        }
        if let Some(extension) = source.and_then(|source| source.autonomy.as_deref())
            && (!local || local_ready)
        {
            if extension.fleet {
                snapshot.fleet.push(label.to_owned());
            }
            extensions.push((label, extension));
        }
        if let Some(gap) = host_gap(
            report,
            state,
            peers.get(&report.host.id),
            local_host_current,
        ) {
            if !local && gap.availability == Availability::Unsupported {
                snapshot.unsupported.push(label.to_owned());
            }
            gaps.push(gap);
        }
    }
    let labels_by_hostname = labels_by_hostname
        .into_iter()
        .filter_map(|(hostname, label)| Some((hostname, label?)))
        .collect();
    snapshot.fleet.sort_unstable();
    snapshot.unsupported.sort_unstable();

    let decisions = decision_views(&extensions);
    let (panes, marks) = {
        let causes = autonomy::resolve(
            agents
                .iter()
                .map(|agent| (agent.host.as_str(), agent.pane.as_str())),
            extensions,
            executions,
            &labels_by_hostname,
        );
        let local = causes
            .iter()
            .filter(|((host, _), _)| *host == LOCAL)
            .map(|((_, pane), cause)| ((*pane).to_owned(), *cause))
            .collect();
        let marks = agents
            .iter()
            .map(|agent| {
                causes
                    .get(&(agent.host.as_str(), agent.pane.as_str()))
                    .copied()
            })
            .collect::<Vec<_>>();
        (local, marks)
    };
    let decision_marks = decision_marks(&decisions, &labels_by_hostname);
    for (agent, cause) in agents.iter_mut().zip(marks) {
        agent.autonomous = cause;
        agent.decisions = decision_marks
            .get(&(agent.host.as_str(), agent.pane.as_str()))
            .copied();
    }
    Projection {
        snapshot: local_ready.then_some(snapshot),
        published: PublishedAutonomy { panes, decisions },
        gaps,
    }
}

fn host_gap(
    report: &HostReport,
    state: Option<&Availability>,
    peer: Option<&PeerSource>,
    local_host_current: bool,
) -> Option<Gap> {
    let (availability, detail) = if !report.reachable {
        (Availability::Unavailable, "host unreachable")
    } else if report.host.is_local() && !local_host_current {
        (Availability::Unavailable, "autonomy unavailable")
    } else if state == Some(&Availability::Unsupported)
        || peer.is_some_and(|peer| peer.availability == Availability::Unsupported)
    {
        (Availability::Unsupported, "autonomous unsupported")
    } else if state != Some(&Availability::Available) {
        (Availability::Unavailable, "autonomy unavailable")
    } else if peer.is_some_and(|peer| peer.availability != Availability::Available) {
        (Availability::Stale, "autonomy observation is stale")
    } else {
        return None;
    };
    Some(Gap {
        domain: format!("{}{}", autonomy::GAP_PREFIX, report.host.label),
        availability,
        detail: detail.into(),
    })
}

fn decision_views(extensions: &[(&str, &autonomy::Extension)]) -> Vec<DecisionView> {
    extensions
        .iter()
        .flat_map(|(label, extension)| {
            extension.decisions.iter().cloned().map(|mut decision| {
                // A host can publish only the decisions it recorded.
                (*label).clone_into(&mut decision.host);
                decision
            })
        })
        .collect()
}

fn decision_marks<'a>(
    decisions: &'a [DecisionView],
    labels_by_hostname: &BTreeMap<&str, &'a str>,
) -> BTreeMap<autonomy::Pane<'a>, DecisionMark> {
    let mut marks = BTreeMap::new();
    for decision in decisions {
        let author = (decision.host.as_str(), decision.pane.as_str());
        let owner = autonomy::decision_owner(decision)
            .and_then(|(host, pane)| Some((*labels_by_hostname.get(host)?, pane)));
        for pane in std::iter::once(author).chain(owner.filter(|owner| *owner != author)) {
            let mark = marks.entry(pane).or_insert(DecisionMark {
                count: 0,
                newest_ms: 0,
            });
            mark.count = mark.count.saturating_add(1);
            mark.newest_ms = mark.newest_ms.max(decision.created_ms);
        }
    }
    marks
}
