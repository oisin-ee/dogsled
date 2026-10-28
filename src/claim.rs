//! `dogsled claim` and `dogsled accept`: the commands a lead uses to take the custody of an
//! orphaned child and to accept the exact result that child returned.
//!
//! Both commands act as the calling pane in `HERDR_PANE_ID` and its readable harness
//! session. Caller-supplied identity is never trusted. The custody stays on the host that
//! records the item, and an acceptance is a review fact about a result: it never completes
//! the child's original task.

use crate::custody::{self, ClaimError, ClaimOutcome, ClaimRequest, Refusal};
use crate::error::{Error, Result};
use crate::hosts::Fleet;
use crate::ledger::now_ms;
use crate::unit_transport::{ExecutionRef, Review};
use serde::Serialize;

/// The receipt of a claim. The Pi extension records it as an audit receipt; it is never
/// task state.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimReceipt {
    /// Always `claim`.
    pub kind: &'static str,
    /// The item the receipt is about.
    pub item: String,
    /// `claimed` when the custody moved, `held` when the caller already held it.
    pub outcome: &'static str,
    /// The claim that holds the custody now.
    pub claim_id: Option<String>,
    /// The revision of the custody record.
    pub revision: u64,
    /// The host that records the item.
    pub host: String,
    /// The pane that is responsible now.
    pub custodian_pane: String,
    /// The session of the responsible pane.
    pub custodian_session: Option<String>,
    /// The custodian the claim replaced.
    pub previous_custodian: Option<String>,
}

impl ClaimReceipt {
    pub fn line(&self) -> String {
        format!(
            "{} {} (custody revision {}, claim {})\n",
            self.outcome,
            self.item,
            self.revision,
            self.claim_id.as_deref().unwrap_or("none")
        )
    }
}

/// The receipt of an acceptance.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptReceipt {
    /// Always `accept`.
    pub kind: &'static str,
    /// The item the receipt is about.
    pub item: String,
    /// The review the custody recorded.
    #[serde(flatten)]
    pub review: Review,
    /// `false` when the same review was already recorded.
    pub applied: bool,
}

impl AcceptReceipt {
    pub fn line(&self) -> String {
        format!(
            "accepted {} (result {}, review {}, applied {})\n",
            self.item, self.review.result_id, self.review.review_id, self.applied
        )
    }
}

/// Render the user-facing custody commands.
pub fn command(
    fleet: &Fleet<'_>,
    command: crate::cli::Command,
    json: bool,
) -> Result<std::process::ExitCode> {
    match command {
        crate::cli::Command::Claim { item, revision } => {
            let receipt = claim(fleet, &item, revision)?;
            crate::print(json, &receipt, || receipt.line())
        }
        crate::cli::Command::Accept {
            item,
            result,
            evidence,
        } => {
            let receipt = accept(fleet, &item, &result, &evidence)?;
            crate::print(json, &receipt, || receipt.line())
        }
        _ => Err(Error::Invalid("not a custody command".into())),
    }?;
    Ok(std::process::ExitCode::SUCCESS)
}

/// The item id is a file name in the custody and admission stores, so it stays a plain id.
fn validate_item(item: &str) -> Result<()> {
    if item.is_empty() || !item.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(Error::Invalid(format!("invalid item id {item}")));
    }
    Ok(())
}

/// The caller as the custody knows it: its pane and its readable harness session.
///
/// The pane comes from the environment and the session from the live agent list. Nothing
/// the caller supplies is identity.
fn identity(fleet: &Fleet<'_>) -> Result<(String, String)> {
    let pane = crate::inbox::own_pane(None)?;
    let agents = crate::probe::agent_list(fleet.shell).map_err(|error| {
        Error::Invalid(format!(
            "unproven: cannot verify the caller session in {pane}: {error}"
        ))
    })?;
    let agent = agents
        .into_iter()
        .find(|agent| agent.pane == pane)
        .ok_or_else(|| {
            Error::Invalid(format!(
                "unproven: no agent is recorded in the caller pane {pane}"
            ))
        })?;
    let session = agent.agent_session.ok_or_else(|| {
        Error::Invalid(format!(
            "unproven: the agent in caller pane {pane} has no session"
        ))
    })?;
    Ok((pane, session))
}

/// The typed reason a claim was refused.
fn reason(refusal: &Refusal) -> &'static str {
    match refusal {
        Refusal::Conflict { .. } => "revision_conflict",
        Refusal::Moved { .. } => "moved",
        Refusal::NotOrphan { .. } => "not_orphan",
        Refusal::Unproven { .. } => "unproven",
        Refusal::WrongHost { .. } => "wrong_host",
        Refusal::UnknownItem { .. } => "unknown_item",
    }
}

/// A refusal keeps its typed reason, so a caller reads why without parsing prose.
fn refusal_error(refusal: &Refusal) -> Error {
    Error::Invalid(format!("{}: {refusal}", reason(refusal)))
}

fn claim_error(error: ClaimError) -> Error {
    match error {
        ClaimError::Refusal(refusal) => refusal_error(&refusal),
        ClaimError::Host(error) => error,
    }
}

/// Claim the custody of an orphaned item for the calling pane.
///
/// `revision` is the custody revision the caller read. Without it the current revision is
/// used, so a claim still loses to a claim that moved the custody first.
pub fn claim(fleet: &Fleet<'_>, item: &str, revision: Option<u64>) -> Result<ClaimReceipt> {
    validate_item(item)?;
    let (pane, session) = identity(fleet)?;
    let expected_revision = match revision {
        Some(revision) => revision,
        None => custody::get(fleet.ledger, item)?.map_or(0, |record| record.revision),
    };
    let request = ClaimRequest {
        item: item.to_owned(),
        host: crate::inbox::hostname(),
        pane,
        session: Some(session),
        expected_revision,
        now: now_ms(),
    };
    let outcome = custody::claim(fleet.ledger, &request, fleet.shell).map_err(claim_error)?;
    Ok(match outcome {
        ClaimOutcome::Claimed { custody } => receipt(item, "claimed", &custody),
        ClaimOutcome::Held { custody } => receipt(item, "held", &custody),
    })
}

fn receipt(item: &str, outcome: &'static str, custody: &custody::Custody) -> ClaimReceipt {
    let latest = custody.latest_claim();
    ClaimReceipt {
        kind: "claim",
        item: item.to_owned(),
        outcome,
        claim_id: latest.map(|claim| claim.id.clone()),
        revision: custody.revision,
        host: custody.host.clone(),
        custodian_pane: custody.custodian_pane.clone(),
        custodian_session: custody.custodian_session.clone(),
        previous_custodian: latest.and_then(|claim| claim.previous_custodian.clone()),
    }
}

/// The execution of the unit request the item records.
fn execution(fleet: &Fleet<'_>, item: &str) -> Result<ExecutionRef> {
    let attempts = fleet.ledger.child("admissions");
    let attempt: crate::work::Attempt = attempts
        .read(&format!("{item}.json"))?
        .ok_or_else(|| Error::Invalid(format!("{item} has no unit request")))?;
    attempt
        .unit
        .map(|unit| unit.execution)
        .ok_or_else(|| Error::Invalid(format!("{item} is not a unit request")))
}

/// The review id of an acceptance. It is derived from the review itself, so the same
/// acceptance twice is one review and a different acceptance is a conflict.
fn review_id(claim_id: &str, revision: u64, result_id: &str, evidence: &str) -> Result<String> {
    let bytes = serde_json::to_vec(&(claim_id, revision, result_id, evidence))?;
    Ok(format!(
        "review-{}",
        crate::unit_transport::digest_hex(&bytes)
    ))
}

/// Accept the exact result a claimed child returned.
///
/// The caller must hold the custody. The review names the saved result, and the original
/// task of the child stays open: an acceptance is a fact about a result, never a completion.
pub fn accept(
    fleet: &Fleet<'_>,
    item: &str,
    result_id: &str,
    evidence: &str,
) -> Result<AcceptReceipt> {
    validate_item(item)?;
    let (pane, session) = identity(fleet)?;
    let custody = custody::get(fleet.ledger, item)?.ok_or_else(|| {
        Error::Invalid(format!("{item} has no custody; the acceptance is refused"))
    })?;
    if custody.custodian_pane != pane {
        return Err(Error::Invalid(format!(
            "{item}: the custody is held by {}; the acceptance is refused",
            custody.custodian_pane
        )));
    }
    let claim = custody.latest_claim().ok_or_else(|| {
        Error::Invalid(format!(
            "{item} has no custody claim; the acceptance is refused"
        ))
    })?;
    if custody.custodian_session.as_deref() != Some(session.as_str()) {
        return Err(Error::Invalid(format!(
            "{item}: the custody session is missing or does not match the caller; the acceptance is refused"
        )));
    }
    let review = Review {
        claim_id: claim.id.clone(),
        revision: custody.revision,
        execution: execution(fleet, item)?,
        result_id: result_id.to_owned(),
        review_id: review_id(&claim.id, custody.revision, result_id, evidence)?,
        evidence: evidence.to_owned(),
        reviewer: pane,
    };
    let applied = crate::unit_request::record_review(fleet, &review)?;
    Ok(AcceptReceipt {
        kind: "accept",
        item: item.to_owned(),
        review,
        applied: applied["applied"].as_bool().unwrap_or(false),
    })
}
