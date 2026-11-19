//! Versioned origin and execution evidence. Only the host that observed a fact writes it.

use crate::board::TodoSections;
use crate::commands::Status;
use crate::jobs::{ExecutionScope, Job};
use crate::ledger::Ledger;
use crate::node::{Snapshot, SnapshotReport};
use crate::work::{Action, Request};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const VERSION: u64 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct Evidence {
    pub hostname: String,
    pub owned: Vec<Owned>,
    pub executions: Vec<Execution>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct Owned {
    pub request_id: String,
    pub kind: Kind,
    pub owner_pane: String,
    pub owner_session: Option<String>,
    pub pane: Option<String>,
    pub phase: Phase,
    pub name: String,
    pub repository: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct Execution {
    pub request_id: String,
    pub kind: Kind,
    pub owner_hostname: String,
    pub owner_pane: String,
    pub pane: String,
    pub title: String,
    pub job_label: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    Agent,
    Job,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Phase {
    Pending,
    Admitted,
    Retained,
}

pub(crate) fn decode(host: &Value) -> Option<Evidence> {
    let value = host.get("owned_work")?;
    if value.get("version")?.as_u64() != Some(VERSION)
        || value.get("status")?.as_str() != Some("ready")
    {
        return None;
    }
    Evidence::deserialize(value).ok()
}

pub(super) fn serialize_host(
    ledger: &Ledger,
    report: &SnapshotReport,
    status: Option<&Status>,
    todos: Option<&[TodoSections]>,
) -> crate::error::Result<Value> {
    let mut value = serde_json::to_value(report)?;
    let extension = match capture(ledger, status, report.snapshot.as_ref(), todos) {
        Ok(Evidence {
            hostname,
            owned,
            executions,
        }) => json!({
            "version": VERSION, "status": "ready", "hostname": hostname,
            "owned": owned, "executions": executions,
        }),
        // Do not export database, command or path details in a diagnostic.
        Err(reason) => json!({"version": VERSION, "status": "unavailable", "reason": reason}),
    };
    value["owned_work"] = extension;
    Ok(value)
}

fn capture(
    ledger: &Ledger,
    status: Option<&Status>,
    snapshot: Option<&Snapshot>,
    todos: Option<&[TodoSections]>,
) -> Result<Evidence, &'static str> {
    let status = status.ok_or("status unavailable")?;
    let snapshot = snapshot.ok_or("host sample unavailable")?;
    let todos = todos.ok_or("todos unavailable")?;
    let hostname = crate::inbox::hostname();
    if hostname.is_empty() || hostname == "unknown" {
        return Err("hostname unavailable");
    }
    let mut executions = agent_executions(ledger, snapshot, todos)?;
    executions.extend(job_executions(ledger, &snapshot.jobs)?);
    executions.sort_by(|a, b| a.request_id.cmp(&b.request_id).then(a.pane.cmp(&b.pane)));
    Ok(Evidence {
        hostname,
        owned: origin(status),
        executions,
    })
}

fn repository(request: &Request) -> Option<String> {
    crate::checkout::identity(&request.repository.as_ref()?.origin)
}

fn program(path: &str) -> Option<String> {
    let name = std::path::Path::new(path).file_name()?.to_str()?;
    // argv[0] is a program path, not a free-form label or command line.
    name.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'+'))
        .then(|| name.to_owned())
}

fn pending(request: &Request) -> Option<Owned> {
    let (kind, name) = match &request.action {
        Action::Agent { name, .. } => (Kind::Agent, name.clone()),
        Action::Job { cmd, .. } => (
            Kind::Job,
            cmd.first()
                .and_then(|arg| program(arg))
                .unwrap_or_else(|| "job".into()),
        ),
        Action::Place => return None,
    };
    Some(Owned {
        request_id: request.id.clone(),
        kind,
        owner_pane: request.owner.clone(),
        owner_session: None, // A ticket does not record the originating harness session.
        pane: None,
        phase: Phase::Pending,
        name,
        repository: repository(request),
    })
}

fn origin(status: &Status) -> Vec<Owned> {
    let requests = status
        .queue
        .iter()
        .map(|ticket| (ticket.request.id.as_str(), &ticket.request))
        .collect::<BTreeMap<_, _>>();
    let mut owned = status
        .queue
        .iter()
        .filter(|ticket| ticket.state == "pending")
        .filter_map(|ticket| pending(&ticket.request).map(|row| (ticket.request.id.as_str(), row)))
        .collect::<BTreeMap<_, _>>();
    for item in &status.items {
        let request_id = item.id.as_str();
        // An already admitted request cannot become pending again because a ticket was
        // sampled before settlement, even when the item is no longer board-visible.
        owned.remove(request_id);
        let kind = match item.kind.as_str() {
            "agent" => Kind::Agent,
            "job" => Kind::Job,
            _ => continue,
        };
        if !crate::board::visible_item(item) {
            continue;
        }
        let retained = item.closed_ms.is_some();
        owned.insert(
            request_id,
            Owned {
                request_id: item.id.clone(),
                kind,
                owner_pane: item.owner_pane.clone(),
                owner_session: item.owner_session.clone(),
                pane: (!retained && !item.pane.is_empty()).then(|| item.pane.clone()),
                phase: if retained {
                    Phase::Retained
                } else {
                    Phase::Admitted
                },
                name: match kind {
                    Kind::Agent => item.name.clone().unwrap_or_else(|| item.id.clone()),
                    Kind::Job => item
                        .name
                        .as_deref()
                        .and_then(program)
                        .unwrap_or_else(|| "job".into()),
                },
                repository: requests
                    .get(request_id)
                    .and_then(|request| repository(request))
                    .or_else(|| {
                        if retained {
                            None
                        } else {
                            crate::board::item_repository(&status.hosts, item)
                        }
                    }),
            },
        );
    }
    owned.into_values().collect()
}

fn agent_executions(
    ledger: &Ledger,
    snapshot: &Snapshot,
    todos: &[TodoSections],
) -> Result<Vec<Execution>, &'static str> {
    let mut executions = Vec::new();
    for agent in &snapshot.probe.agent_panes {
        if agent.pane.is_empty() {
            continue;
        }
        let Some(launch) = crate::launch::observed_agent_launch(ledger, agent)
            .map_err(|_| "launch evidence unavailable")?
        else {
            continue;
        };
        let (Action::Agent { name, .. }, Some(owner_hostname)) = (launch.action, launch.owner_host)
        else {
            continue;
        };
        if owner_hostname.is_empty() || launch.owner.is_empty() {
            continue;
        }
        let title = todos
            .iter()
            .find(|todo| todo.pane == agent.pane && !todo.current.trim().is_empty())
            .map_or(name, |todo| todo.current.clone());
        executions.push(Execution {
            request_id: launch.id,
            kind: Kind::Agent,
            owner_hostname,
            owner_pane: launch.owner,
            pane: agent.pane.clone(),
            title,
            job_label: None,
        });
    }
    Ok(executions)
}

fn job_executions(ledger: &Ledger, jobs: &[Job]) -> Result<Vec<Execution>, &'static str> {
    if jobs.is_empty() {
        return Ok(Vec::new());
    }
    let owners = crate::job_store::owners(ledger).map_err(|_| "job owner evidence unavailable")?;
    let owners = owners
        .iter()
        .map(|owner| (owner.id.as_str(), owner))
        .collect::<BTreeMap<_, _>>();
    Ok(jobs
        .iter()
        .filter_map(|job| {
            let owner = owners.get(job.id.as_str())?;
            let owner_hostname = owner.owner_host.as_ref().filter(|host| !host.is_empty())?;
            if job.owner != owner.owner || job.owner.is_empty() || job.pane.is_empty() {
                return None;
            }
            let title = job
                .cmd
                .first()
                .and_then(|arg| program(arg))
                .unwrap_or_else(|| "job".into());
            let branch = job
                .startup
                .as_ref()
                .and_then(|startup| match &startup.scope {
                    ExecutionScope::Git { checkout } => Some(checkout.branch.as_str()),
                    _ => None,
                });
            let job_label = branch
                .filter(|branch| !branch.is_empty() && !branch.chars().any(char::is_control))
                .map_or_else(|| title.clone(), |branch| format!("{title} · {branch}"));
            Some(Execution {
                request_id: job.id.clone(),
                kind: Kind::Job,
                owner_hostname: owner_hostname.clone(),
                owner_pane: owner.owner.clone(),
                pane: job.pane.clone(),
                title,
                job_label: Some(job_label),
            })
        })
        .collect())
}
