//! Published host selection and single-delivery lead transport.

use super::{CreateContext, Failure, FailureKind, LaunchContext, Phase, Started};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Delivery {
    version: u32,
    pub id: String,
    pub repo: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<portal_types::models::ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<portal_types::models::ThinkingLevel>,
    pub validity: crate::jobs::Validity,
    pub host: String,
    // Origin-only transport settings. Never sent to the peer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    route: Option<Route>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Route {
    target: String,
    command: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum Reply {
    Started {
        lead: Started,
    },
    Failed {
        kind: FailureKind,
        id: Option<String>,
        pane: Option<String>,
        phase: Option<Phase>,
    },
}

pub(super) fn create(
    context: &CreateContext<'_>,
    slug: &str,
    prompt: &str,
    model: Option<portal_types::models::ModelRef>,
    thinking: Option<portal_types::models::ThinkingLevel>,
) -> std::result::Result<Started, Failure> {
    let placed = place(context, slug, prompt, model, thinking)?;
    let delivery = crate::store::write(context.ledger, |tx| placed.record_in(tx))
        .map_err(|error| correlated(&placed.id, error))?;
    deliver(&context.launch_context(), &delivery)
}

/// A host choice is read-only until its authority is committed.
pub(super) struct Placement {
    id: String,
    repo: String,
    prompt: String,
    model: Option<portal_types::models::ModelRef>,
    thinking: Option<portal_types::models::ThinkingLevel>,
    floor: u64,
    host: String,
    route: Option<Route>,
}

pub(super) fn place(
    context: &CreateContext<'_>,
    slug: &str,
    prompt: &str,
    model: Option<portal_types::models::ModelRef>,
    thinking: Option<portal_types::models::ThinkingLevel>,
) -> std::result::Result<Placement, Failure> {
    let invalid = |error| super::failure(FailureKind::Invalid, error);
    let unavailable = |error| super::failure(FailureKind::Unavailable, error);
    super::checkout::validate(slug, prompt).map_err(invalid)?;
    let policy = crate::config::load(context.config_path).map_err(unavailable)?;
    let fleet = crate::hosts::ActionFleet::current(
        context.shell,
        context.ledger,
        &policy,
        context.home.into(),
    )
    .map_err(unavailable)?;
    let reports = fleet.reports().map_err(unavailable)?;
    let evaluations: Vec<_> = reports
        .iter()
        .filter(|report| report.reachable)
        .filter_map(|report| {
            report.headroom.clone().map(|mut score| {
                // Leads consume no reservation or capacity. Keep placement's exact score
                // and tie breakers, but do not filter hosts at their admission limits.
                score.host.clone_from(&report.host.id);
                score.eligible = true;
                score
            })
        })
        .collect();
    let selected = crate::plan::choose(&evaluations).ok_or_else(|| {
        unavailable(Error::Invalid(
            "no reachable fleet host has published headroom".into(),
        ))
    })?;
    let host = fleet.find(&selected.host).map_err(unavailable)?;
    let id = super::new_id();
    let floor = crate::jobs::clock_floor(context.ledger).map_err(|error| correlated(&id, error))?;
    let host_label = host.label.clone();
    let route = host.target.as_ref().map(|target| Route {
        target: target.clone(),
        command: policy.remote_command.clone(),
    });
    Ok(Placement {
        id,
        repo: slug.into(),
        prompt: prompt.into(),
        model,
        thinking,
        floor,
        host: host_label,
        route,
    })
}

impl Placement {
    /// Save the choice before transport. A lost reply never causes another selection.
    pub(super) fn record_in(&self, tx: &rusqlite::Transaction<'_>) -> Result<Delivery> {
        let validity = crate::jobs::issue_unowned_validity_in(tx, &self.id, self.floor)?;
        crate::job_store::put_selection_in(tx, &self.id, Some(&self.host))?;
        Ok(Delivery {
            version: 1,
            id: self.id.clone(),
            repo: self.repo.clone(),
            prompt: self.prompt.clone(),
            model: self.model.clone(),
            thinking: self.thinking,
            validity,
            host: self.host.clone(),
            route: self.route.clone(),
        })
    }
}

pub(super) fn deliver(
    context: &LaunchContext<'_>,
    delivery: &Delivery,
) -> std::result::Result<Started, Failure> {
    let Some(route) = &delivery.route else {
        return start(context, delivery);
    };
    let policy = crate::config::Policy {
        remote_command: route.command.clone(),
        ..crate::config::Policy::default()
    };
    let host = crate::hosts::Host {
        id: delivery.host.clone(),
        label: delivery.host.clone(),
        target: Some(route.target.clone()),
    };
    let fleet = crate::hosts::Fleet::cold(
        context.shell,
        context.ledger,
        &policy,
        context.home.into(),
        Vec::new(),
    );
    send(&fleet, &host, delivery)
}

fn send(
    fleet: &crate::hosts::Fleet<'_>,
    host: &crate::hosts::Host,
    delivery: &Delivery,
) -> std::result::Result<Started, Failure> {
    let args = ["lead".into(), "--input".into(), "-".into()];
    let wire = Delivery {
        route: None,
        ..delivery.clone()
    };
    let input =
        serde_json::to_vec(&wire).map_err(|error| correlated(&delivery.id, error.into()))?;
    let uncertain = |message: &str| Failure {
        uncertain: true,
        ..correlated(&delivery.id, Error::Invalid(message.into()))
    };
    let timeout = fleet
        .shell
        .budget()
        .with_limit("ssh", crate::budget::SETUP_LIMIT)
        .map_err(|error| correlated(&delivery.id, error))?;
    let output = fleet
        .shell
        .run_input_bounded("ssh", &fleet.remote_argv(host, &args), &input, timeout)
        .map_err(|_| uncertain("lead peer delivery failed; start may be uncertain; not retried"))?;
    if !output.ok() {
        return Err(uncertain(
            "lead peer delivery failed; start may be uncertain; not retried",
        ));
    }
    let reply: Reply = serde_json::from_str(&output.stdout).map_err(|_| {
        uncertain("invalid lead peer response; start may be uncertain; not retried")
    })?;
    match reply {
        Reply::Started { lead }
            if lead.id == delivery.id
                && lead.host == delivery.host
                && super::valid_name(&lead.name)
                && !lead.pane.is_empty() =>
        {
            Ok(lead)
        }
        Reply::Started { .. } => Err(uncertain(
            "lead peer response identity does not match; not retried",
        )),
        Reply::Failed {
            kind,
            id,
            pane,
            phase,
        } if id.as_deref() == Some(&delivery.id) => Err(Failure {
            kind,
            id,
            pane,
            phase,
            error: Box::new(Error::Invalid(
                "lead creation failed on the selected host; not retried".into(),
            )),
            uncertain: false,
        }),
        Reply::Failed { .. } => Err(uncertain(
            "lead peer failure identity does not match; not retried",
        )),
    }
}

fn correlated(id: &str, error: Error) -> Failure {
    Failure {
        kind: FailureKind::Launch,
        id: Some(id.into()),
        pane: None,
        phase: None,
        error: Box::new(error),
        uncertain: false,
    }
}

fn start(
    context: &LaunchContext<'_>,
    delivery: &Delivery,
) -> std::result::Result<Started, Failure> {
    super::start(context, delivery).map_err(|mut failure| {
        failure.id = Some(delivery.id.clone());
        failure
    })
}

/// A peer receives immutable validity once, before checkout or pane effects. Replaying
/// a finished record is read-only; any other saved ingress remains uncertain.
pub(super) fn peer(context: &CreateContext<'_>, input: &str) -> Result<serde_json::Value> {
    let delivery: Delivery = serde_json::from_str(input)?;
    super::checkout::validate(&delivery.repo, &delivery.prompt)?;
    crate::jobs::validate_id(&delivery.id)?;
    if delivery.version != 1 || delivery.host.is_empty() || delivery.route.is_some() {
        return Err(Error::Invalid("unsupported lead delivery".into()));
    }
    let locks = context.ledger.child("lead-ingress");
    let _lock = locks.try_lock(&delivery.id)?.ok_or_else(|| {
        Error::Invalid("lead delivery is already in progress; not retried".into())
    })?;
    let outcome = receive(&context.launch_context(), &delivery);
    let reply = match outcome {
        Ok(lead) => Reply::Started { lead },
        Err(failure) => Reply::Failed {
            kind: failure.kind,
            id: failure.id,
            pane: failure.pane,
            phase: failure.phase,
        },
    };
    Ok(serde_json::to_value(reply)?)
}

fn receive(
    context: &LaunchContext<'_>,
    delivery: &Delivery,
) -> std::result::Result<Started, Failure> {
    let fail = |error| correlated(&delivery.id, error);
    if let Some(record) = crate::lead_store::get(context.ledger, &delivery.id).map_err(fail)? {
        crate::jobs::match_request_validity(
            crate::jobs::request_validity(context.ledger, &delivery.id).map_err(fail)?,
            Some(delivery.validity),
        )
        .map_err(fail)?;
        if crate::job_store::selection(context.ledger, &delivery.id)
            .map_err(fail)?
            .as_deref()
            != Some(delivery.host.as_str())
            || record.prompt != delivery.prompt
            || record.validity != delivery.validity
            || !crate::repos::same_origin(
                &record.repository.origin,
                &format!("git@github.com:{}.git", delivery.repo),
            )
        {
            return Err(fail(Error::Invalid("lead delivery intent changed".into())));
        }
        if record.phase == Phase::Ready {
            return Ok(Started {
                host: delivery.host.clone(),
                id: record.id,
                name: record.name,
                pane: record.pane.unwrap_or_default(),
                workspace: record.workspace,
            });
        }
        return Err(Failure {
            kind: FailureKind::Launch,
            id: Some(record.id),
            pane: record.pane,
            phase: Some(record.phase),
            error: Box::new(Error::Invalid(
                "lead start is uncertain; not retried".into(),
            )),
            uncertain: false,
        });
    }
    if crate::jobs::request_validity(context.ledger, &delivery.id)
        .map_err(fail)?
        .is_some()
    {
        return Err(fail(Error::Invalid(
            "lead ingress is uncertain; not retried".into(),
        )));
    }
    crate::jobs::receive_request_validity(context.ledger, &delivery.id, Some(delivery.validity))
        .map_err(fail)?;
    crate::job_store::put_selection(context.ledger, &delivery.id, Some(&delivery.host))
        .map_err(fail)?;
    start(context, delivery)
}
