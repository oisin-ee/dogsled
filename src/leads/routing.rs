//! Published host selection and single-delivery lead transport.

use super::{CreateContext, Failure, FailureKind, Phase, Started};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
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
    let launch = |error| correlated(&id, error);
    let validity = crate::jobs::issue_unowned_validity(context.ledger, &id).map_err(launch)?;
    // Persist the choice before transport. A lost reply never causes another selection,
    // reservation, queue entry, or second delivery.
    crate::job_store::put_selection(context.ledger, &id, Some(&host.label)).map_err(launch)?;
    let delivery = Delivery {
        version: 1,
        id,
        repo: slug.into(),
        prompt: prompt.into(),
        model,
        thinking,
        validity,
        host: host.label.clone(),
    };
    if host.is_local() {
        return start(context, &delivery);
    }
    send(&fleet, host, &delivery)
}

fn send(
    fleet: &crate::hosts::ActionFleet<'_>,
    host: &crate::hosts::Host,
    delivery: &Delivery,
) -> std::result::Result<Started, Failure> {
    let args = ["lead".into(), "--input".into(), "-".into()];
    let input =
        serde_json::to_vec(&delivery).map_err(|error| correlated(&delivery.id, error.into()))?;
    let timeout = fleet
        .shell
        .budget()
        .with_limit("ssh", crate::budget::SETUP_LIMIT)
        .map_err(|error| correlated(&delivery.id, error))?;
    let output = fleet
        .shell
        .run_input_bounded("ssh", &fleet.remote_argv(host, &args), &input, timeout)
        .map_err(|_| {
            correlated(
                &delivery.id,
                Error::Invalid(
                    "lead peer delivery failed; start may be uncertain; not retried".into(),
                ),
            )
        })?;
    if !output.ok() {
        return Err(correlated(
            &delivery.id,
            Error::Invalid("lead peer delivery failed; start may be uncertain; not retried".into()),
        ));
    }
    let reply: Reply = serde_json::from_str(&output.stdout).map_err(|_| {
        correlated(
            &delivery.id,
            Error::Invalid(
                "invalid lead peer response; start may be uncertain; not retried".into(),
            ),
        )
    })?;
    match reply {
        Reply::Started { lead }
            if lead.id == delivery.id
                && lead.host == delivery.host
                && lead.name == format!("lead-{}", delivery.id)
                && !lead.pane.is_empty() =>
        {
            Ok(lead)
        }
        Reply::Started { .. } => Err(correlated(
            &delivery.id,
            Error::Invalid("lead peer response identity does not match; not retried".into()),
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
        }),
        Reply::Failed { .. } => Err(correlated(
            &delivery.id,
            Error::Invalid("lead peer failure identity does not match; not retried".into()),
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
    }
}

fn start(
    context: &CreateContext<'_>,
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
    super::agent_name(&delivery.id)?;
    if delivery.version != 1 || delivery.host.is_empty() {
        return Err(Error::Invalid("unsupported lead delivery".into()));
    }
    let locks = context.ledger.child("lead-ingress");
    let _lock = locks.try_lock(&delivery.id)?.ok_or_else(|| {
        Error::Invalid("lead delivery is already in progress; not retried".into())
    })?;
    let outcome = receive(context, &delivery);
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
    context: &CreateContext<'_>,
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
