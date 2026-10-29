//! Caller-owned placement journal. The admissions gate spans every transition.
use super::{
    Action, Attempt, Error, Fleet, Launch, Ledger, Placement, Request, Result, Value, jobs, json,
    prelaunch_error, prepare_launch, prepared_launch,
};
use crate::prepare::{self, Envelope, LookupOutcome, LookupRequest, LookupState, disposition};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PlacementProgress {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_ack: Option<bool>,
    #[serde(flatten)]
    pub phase: PlacementPhase,
    pub declined: Vec<crate::node::PlacementReply>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub(crate) enum PlacementPhase {
    Recorded,
    SelectionPending {
        envelope: crate::node::PlacementReservation,
    },
    HostSelected {
        grant: crate::node::PlacementGrant,
    },
    PreparationPending {
        grant: crate::node::PlacementGrant,
        preparation: Envelope,
    },
    PreparationFailed {
        failed: Box<FailedPlacement>,
    },
    Prepared {
        prepared: Box<PreparedPlacement>,
    },
    ResultSaved {
        saved: Box<SavedPlacement>,
    },
    ReleaseAcknowledged {
        saved: Box<SavedPlacement>,
        acknowledgement: Option<disposition::Acknowledgement>,
    },
    Completed {
        saved: Box<SavedPlacement>,
        acknowledgement: Option<disposition::Acknowledgement>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct FailedPlacement {
    grant: crate::node::PlacementGrant,
    preparation: Envelope,
    failure: crate::error::RetainedFailure,
    disposition: Option<disposition::Request>,
    acknowledgement: Option<disposition::Acknowledgement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PreparedPlacement {
    grant: crate::node::PlacementGrant,
    preparation: Option<Envelope>,
    launch: Launch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SavedPlacement {
    prepared: PreparedPlacement,
    result: Value,
    disposition: Option<disposition::Request>,
}

fn conflict(message: &str) -> Error {
    crate::unit_transport::conflict(message)
}

impl PlacementProgress {
    pub(crate) fn terminal_grant(&self) -> Option<&crate::node::PlacementGrant> {
        match &self.phase {
            PlacementPhase::Completed { saved, .. } => Some(&saved.prepared.grant),
            PlacementPhase::PreparationFailed { failed } => Some(&failed.grant),
            _ => None,
        }
    }

    pub(crate) fn expectation(
        &self,
        request: &Request,
    ) -> Result<Option<crate::node::PlacementReplay>> {
        if self.version != crate::node::PLACEMENT_VERSION {
            return Err(conflict("unsupported placement progress version"));
        }
        let expected = serde_json::to_value(request)?;
        for declined in &self.declined {
            declined.validate(&declined.envelope, false)?;
            if declined.envelope.request != expected {
                return Err(conflict("declined placement request changed"));
            }
        }
        let grant = match &self.phase {
            PlacementPhase::Recorded => None,
            PlacementPhase::SelectionPending { envelope } => {
                let transport = crate::node::PlacementReplay {
                    envelope: envelope.clone(),
                    expected_grant: None,
                };
                transport.validate(&request.id)?;
                if envelope.request != serde_json::to_value(request)? {
                    return Err(conflict("placement expectation changed"));
                }
                return Ok(Some(transport));
            }
            PlacementPhase::HostSelected { grant }
            | PlacementPhase::PreparationPending { grant, .. } => Some(grant),
            PlacementPhase::PreparationFailed { failed } => Some(&failed.grant),
            PlacementPhase::Prepared { prepared } => Some(&prepared.grant),
            PlacementPhase::ResultSaved { saved }
            | PlacementPhase::ReleaseAcknowledged { saved, .. }
            | PlacementPhase::Completed { saved, .. } => Some(&saved.prepared.grant),
        };
        let transport = grant.map(|grant| crate::node::PlacementReplay {
            envelope: grant.envelope.clone(),
            expected_grant: Some(grant.clone()),
        });
        if let Some(transport) = &transport {
            transport.validate(&request.id)?;
            if transport.envelope.request != expected {
                return Err(conflict("placement expectation changed"));
            }
        }
        Ok(transport)
    }
}

pub(super) fn record(attempts: &Ledger, request: &Request) -> Result<()> {
    if !matches!(request.action, Action::Place) {
        return Ok(());
    }
    let file = format!("{}.json", request.id);
    if attempts.read::<Attempt>(&file)?.is_none() {
        attempts.write(
            &file,
            &Attempt {
                ordinary_release: None,
                request: serde_json::to_value(request)?,
                finished_ms: None,
                placement: Some(PlacementProgress {
                    version: crate::node::PLACEMENT_VERSION,
                    completion_ack: None,
                    phase: PlacementPhase::Recorded,
                    declined: Vec::new(),
                }),
                host: None,
                launch: None,
                error: None,
                diagnostic: None,
                unit: None,
                result: None,
            },
        )?;
    }
    Ok(())
}

pub(super) fn reserved(attempts: &Ledger, request: &Request) -> Result<bool> {
    if !matches!(request.action, Action::Place) {
        return Ok(false);
    }
    let Some(attempt) = attempts.read::<Attempt>(&format!("{}.json", request.id))? else {
        return Ok(false);
    };
    if attempt.request != serde_json::to_value(request)? {
        return Err(conflict("placement request changed"));
    }
    Ok(attempt
        .placement
        .as_ref()
        .ok_or_else(|| conflict("missing placement progress"))?
        .expectation(request)?
        .is_some_and(|p| p.expected_grant.is_some()))
}

pub(crate) fn resume(ledger: &Ledger, id: &str) -> Result<Request> {
    jobs::validate_id(id)?;
    let attempt = ledger
        .child("admissions")
        .read::<Attempt>(&format!("{id}.json"))?
        .ok_or_else(|| conflict(&format!("unknown placement request {id}")))?;
    let request: Request = serde_json::from_value(attempt.request)?;
    if request.id != id || !matches!(request.action, Action::Place) {
        return Err(conflict(&format!(
            "not the original placement request {id}"
        )));
    }
    attempt
        .placement
        .ok_or_else(|| conflict("missing placement progress"))?
        .expectation(&request)?;
    Ok(request)
}

fn saved_phase(request: &Request, prepared: &PreparedPlacement) -> Result<PlacementPhase> {
    validate_prepared(request, prepared)?;
    Ok(PlacementPhase::ResultSaved {
        saved: Box::new(SavedPlacement {
            prepared: prepared.clone(),
            result: public_result(request, prepared),
            disposition: intent(prepared),
        }),
    })
}

fn public_result(request: &Request, prepared: &PreparedPlacement) -> Value {
    json!({"request_id":request.id,"host":prepared.grant.envelope.host,
        "reservation":prepared.grant.envelope.reservation,
        "path":request.branch.as_ref().map(|_| &prepared.launch.cwd),"pane_id":prepared.launch.pane})
}

fn validate_prepared(request: &Request, prepared: &PreparedPlacement) -> Result<()> {
    let launch = &prepared.launch;
    let grant = &prepared.grant;
    if !matches!(grant.version, 2 | crate::node::PLACEMENT_VERSION)
        || grant.envelope.version != grant.version
        || grant.envelope.request != serde_json::to_value(request)?
        || grant.envelope.reservation != request.id
        || grant.reservation.id != request.id
        || !grant.materialized
        || !grant.headroom.eligible
    {
        return Err(conflict("retained placement grant identity changed"));
    }
    if launch.id != request.id
        || launch.validity != request.validity
        || launch.action != request.action
        || launch.host != prepared.grant.envelope.host.id
        || request.branch.is_some() != launch.preparation.is_some()
        || launch.unit.is_some()
        || launch.owner != request.owner
        || launch.owner_host != request.owner_host
        || launch.workspace_name != request.workspace_name
        || launch.terminal.is_some()
        || launch.workspace
            != prepared
                .grant
                .envelope
                .host
                .is_local()
                .then(|| request.workspace.clone())
                .flatten()
    {
        return Err(conflict("prepared placement identity changed"));
    }
    match (&prepared.preparation, &launch.preparation) {
        (Some(envelope), Some(binding)) => {
            prepare::require_protocol(launch.preparation_protocol)?;
            if !(envelope.version == prepare::ENVELOPE_VERSION
                || (envelope.version == 1 && request.validity.is_none()))
                || envelope.request_id != request.id
                || envelope.validity != request.validity
                || Some(&envelope.branch) != request.branch.as_ref()
                || envelope.base != request.base
                || request.repository.as_ref().is_none_or(|repo| {
                    if prepared.grant.envelope.host.is_local() {
                        !crate::worktree::same(&envelope.repo, &repo.path)
                    } else {
                        envelope.repo != repo.relative
                    }
                })
                || binding.request_id != request.id
                || binding.claim.request_id != request.id
                || binding.request.1 != envelope.branch
                || binding.request.2 != envelope.base
                || binding.prepared.as_ref().is_none_or(|p| {
                    p.path.as_deref() != Some(&launch.cwd) || p.pane_id != launch.pane
                })
            {
                return Err(conflict("placement preparation envelope changed"));
            }
            let mut outcome = binding
                .prepared
                .clone()
                .ok_or_else(|| conflict("missing preparation outcome"))?;
            outcome.binding = Some(binding.clone());
            prepare::validate_prepared(&outcome, &request.id)?;
        }
        (None, None)
            if request.branch.is_none()
                && launch.pane.is_none()
                && launch.preparation_protocol.is_none()
                && (request.repository.is_some() || launch.cwd == request.cwd) => {}
        _ => return Err(conflict("impossible placement preparation state")),
    }
    Ok(())
}

pub(crate) fn validate_saved(request: &Request, saved: &SavedPlacement) -> Result<()> {
    validate_prepared(request, &saved.prepared)?;
    if saved.result != public_result(request, &saved.prepared)
        || saved.disposition != intent(&saved.prepared)
    {
        return Err(conflict("saved placement result or disposition changed"));
    }
    Ok(())
}

fn intent(prepared: &PreparedPlacement) -> Option<disposition::Request> {
    prepared
        .launch
        .preparation
        .as_ref()
        .map(|binding| disposition::Request {
            version: disposition::VERSION,
            binding: *binding.clone(),
            policy: disposition::Policy::Prohibit {
                basis: disposition::Basis::PlacementReturned,
            },
            outcome: disposition::Outcome::Unknown,
        })
}

fn validate_ack(saved: &SavedPlacement, ack: Option<&disposition::Acknowledgement>) -> Result<()> {
    match (&saved.disposition, ack) {
        (Some(intent), Some(ack)) => prepare::validate_ack(intent, ack),
        (None, None) => Ok(()),
        _ => Err(conflict("missing placement release acknowledgement")),
    }
}

fn pending(request: &Request, saved: &SavedPlacement, error: &Error) -> Value {
    json!({"request_id":request.id,"status":"pending_release","recoverable":true,
        "result":saved.result,"error":error.to_string()})
}

fn prepare_selected(
    fleet: &Fleet,
    request: &Request,
    attempt: &mut Attempt,
    grant: &crate::node::PlacementGrant,
    transport: Option<&crate::node::PlacementReplay>,
    selected_now: bool,
) -> Result<PlacementPhase> {
    let attempts = fleet.ledger.child("admissions");
    let file = format!("{}.json", request.id);
    Ok({
        if attempt.launch.is_some() {
            return Err(conflict(
                "historical prepared placement lacks preparation envelope",
            ));
        }
        let selected_host = grant.envelope.host.clone();
        let host = &selected_host;
        if let Some(branch) = &request.branch {
            let repo = request
                .repository
                .as_ref()
                .ok_or_else(|| conflict("branch requires repository"))?;
            let preparation = prepare::envelope(
                fleet,
                host,
                &repo.path,
                branch,
                request.base.as_deref(),
                &request.id,
            )?;
            if !selected_now {
                let query = LookupRequest {
                    version: disposition::VERSION,
                    placement: transport
                        .cloned()
                        .ok_or_else(|| conflict("missing placement grant"))?,
                    preparation: preparation.clone(),
                    binding: None,
                };
                if !matches!(prepare::lookup(fleet, &query)?, LookupState::Absent) {
                    return Err(conflict(
                        "selected placement grant or preparation changed; inspect the host",
                    ));
                }
            }
            let grant = grant.clone();
            attempt
                .placement
                .as_mut()
                .ok_or_else(|| conflict("missing placement progress"))?
                .phase = PlacementPhase::PreparationPending {
                grant: grant.clone(),
                preparation: preparation.clone(),
            };
            attempts.write(&file, attempt)?;
            crate::checkout::ensure(fleet, host, repo, request).map_err(prelaunch_error)?;
            // A replay carries the same acknowledged grant, never a capacity request.
            let replay = transport.ok_or_else(|| conflict("missing acknowledged grant"))?;
            let prepared = prepare::deliver_placement(fleet, host, &preparation, replay)
                .map_err(prelaunch_error)?;
            crate::unit_request::crash_point("placement-preparation-received");
            let launch = prepared_launch(fleet, request, host, None, prepared)?;
            PlacementPhase::Prepared {
                prepared: Box::new(PreparedPlacement {
                    grant,
                    preparation: Some(preparation),
                    launch,
                }),
            }
        } else {
            let launch = prepare_launch(fleet, request, host)?;
            PlacementPhase::Prepared {
                prepared: Box::new(PreparedPlacement {
                    grant: grant.clone(),
                    preparation: None,
                    launch,
                }),
            }
        }
    })
}

fn recover_prepared(
    fleet: &Fleet,
    request: &Request,
    grant: &crate::node::PlacementGrant,
    preparation: &Envelope,
    transport: Option<crate::node::PlacementReplay>,
) -> Result<PlacementPhase> {
    let query = LookupRequest {
        version: disposition::VERSION,
        placement: transport.ok_or_else(|| conflict("missing placement grant"))?,
        preparation: preparation.clone(),
        binding: None,
    };
    let prepared = match prepare::lookup(fleet, &query)? {
        LookupState::ReceiptOnly {
            outcome: LookupOutcome::Prepared { prepared },
        } => prepared,
        LookupState::Absent => {
            let repo = request
                .repository
                .as_ref()
                .ok_or_else(|| conflict("missing original checkout envelope"))?;
            crate::checkout::ensure(fleet, &grant.envelope.host, repo, request)
                .map_err(prelaunch_error)?;
            prepare::deliver_placement(fleet, &grant.envelope.host, preparation, &query.placement)
                .map_err(prelaunch_error)?
        }
        LookupState::ReceiptOnly {
            outcome: LookupOutcome::Failed { failure, binding },
        } => {
            return failed_phase(grant, preparation, failure, binding, None);
        }
        LookupState::Disposition {
            outcome: LookupOutcome::Failed { failure, binding },
            receipt,
        } => {
            return failed_phase(grant, preparation, failure, binding, Some(&receipt.request));
        }
        LookupState::Mismatch => {
            return Err(conflict(&format!(
                "request {} placement grant or preparation binding changed; no dispatch",
                request.id
            )));
        }
        LookupState::UnsupportedVersion => {
            return Err(conflict(&format!(
                "request {} has unsupported preparation evidence; no dispatch",
                request.id
            )));
        }
        _ => {
            return Err(conflict(&format!(
                "request {} preparation remains unresolved on {}; inspect the saved host receipt",
                request.id, grant.envelope.host.label
            )));
        }
    };
    crate::unit_request::crash_point("placement-preparation-recovered");
    let launch = prepared_launch(fleet, request, &grant.envelope.host, None, prepared)?;
    Ok(PlacementPhase::Prepared {
        prepared: Box::new(PreparedPlacement {
            grant: grant.clone(),
            preparation: Some(preparation.clone()),
            launch,
        }),
    })
}

fn failed_phase(
    grant: &crate::node::PlacementGrant,
    preparation: &Envelope,
    failure: crate::error::RetainedFailure,
    binding: Option<Box<disposition::Binding>>,
    saved: Option<&disposition::Request>,
) -> Result<PlacementPhase> {
    let disposition = binding.map(|binding| disposition::Request {
        version: disposition::VERSION,
        binding: *binding,
        policy: disposition::Policy::Prohibit {
            basis: disposition::Basis::PreparationFailed,
        },
        outcome: disposition::Outcome::NotStarted,
    });
    if saved.is_some_and(|saved| Some(saved) != disposition.as_ref()) {
        return Err(conflict("preparation failure disposition changed"));
    }
    Ok(PlacementPhase::PreparationFailed {
        failed: Box::new(FailedPlacement {
            grant: grant.clone(),
            preparation: preparation.clone(),
            failure,
            disposition,
            acknowledgement: None,
        }),
    })
}

fn finish_failure(fleet: &Fleet, request: &Request, mut attempt: Attempt) -> Result<Option<Value>> {
    let PlacementPhase::PreparationFailed { failed } = &mut attempt
        .placement
        .as_mut()
        .ok_or_else(|| conflict("missing placement progress"))?
        .phase
    else {
        return Err(conflict("missing preparation failure"));
    };
    let envelope = &failed.preparation;
    if envelope.version != prepare::ENVELOPE_VERSION
        || envelope.request_id != request.id
        || Some(&envelope.branch) != request.branch.as_ref()
        || envelope.base != request.base
        || failed.failure.request_id.as_deref() != Some(request.id.as_str())
    {
        return Err(conflict("preparation failure identity changed"));
    }
    if let Some(intent) = &failed.disposition {
        if intent.version != disposition::VERSION
            || intent.binding.request_id != request.id
            || intent.binding.claim.request_id != request.id
            || intent.binding.claim.tree.identity.is_none()
            || intent.binding.prepared.is_some()
            || intent.binding.request.1 != envelope.branch
            || intent.binding.request.2 != envelope.base
            || intent.policy
                != (disposition::Policy::Prohibit {
                    basis: disposition::Basis::PreparationFailed,
                })
            || intent.outcome != disposition::Outcome::NotStarted
        {
            return Err(conflict("failure disposition identity changed"));
        }
        if let Some(ack) = &failed.acknowledgement {
            prepare::validate_ack(intent, ack)?;
        } else {
            failed.acknowledgement = Some(prepare::dispose(
                fleet,
                &failed.grant.envelope.host,
                intent,
            )?);
        }
    } else if failed.acknowledgement.is_some() {
        return Err(conflict("unbound failure acknowledgement"));
    }
    let error = failed.failure.restore();
    let grant = failed.grant.clone();
    attempt
        .placement
        .as_mut()
        .ok_or_else(|| conflict("missing placement progress"))?
        .completion_ack = Some(false);
    jobs::finish_once(
        fleet.ledger,
        &mut attempt.finished_ms,
        request.validity.map(|v| v.created_ms),
    )?;
    fleet
        .ledger
        .child("admissions")
        .write(&format!("{}.json", request.id), &attempt)?;
    finish_grant(fleet, &attempt, &grant)?;
    Err(error)
}

fn observe(
    fleet: &Fleet,
    request: &Request,
    attempt: &Attempt,
) -> Result<Option<crate::node::PlacementReplay>> {
    if attempt.request != serde_json::to_value(request)?
        || attempt.unit.is_some()
        || attempt.launch.is_some()
        || attempt.result.is_some()
    {
        return Err(conflict("placement admission changed"));
    }
    let progress = attempt
        .placement
        .as_ref()
        .ok_or_else(|| conflict("missing placement progress"))?;
    let transport = progress.expectation(request)?;
    if transport
        .as_ref()
        .is_none_or(|t| t.expected_grant.is_none())
        && attempt.host.is_some()
    {
        return Err(conflict("unselected placement has a host"));
    }
    if let Some(transport) = &transport
        && ((transport.expected_grant.is_some()
            && attempt
                .host
                .as_ref()
                .is_none_or(|h| *h != transport.envelope.host))
            || *fleet.find(&transport.envelope.host.id)? != transport.envelope.host)
    {
        return Err(conflict("selected placement host or target changed"));
    }
    Ok(transport)
}

/// R1 can use this predicate with release and outstanding-reference predicates separately.
pub(crate) fn validate_completion(proof: &crate::node::GrantCompletion) -> Result<()> {
    let attempt: Attempt = serde_json::from_value(proof.admission.clone())?;
    let request: Request = serde_json::from_value(attempt.request.clone())?;
    if attempt.request != proof.grant.envelope.request || attempt.finished_ms.is_none() {
        return Err(conflict("grant completion has no origin terminal proof"));
    }
    let progress = attempt
        .placement
        .ok_or_else(|| conflict("grant completion has no placement"))?;
    match progress.phase {
        PlacementPhase::Completed {
            saved,
            acknowledgement,
        } => {
            validate_saved(&request, &saved)?;
            validate_ack(&saved, acknowledgement.as_ref())?;
            if saved.prepared.grant != proof.grant {
                return Err(conflict("grant completion identity changed"));
            }
        }
        PlacementPhase::PreparationFailed { failed } => {
            if failed.grant != proof.grant
                || failed.preparation.version != prepare::ENVELOPE_VERSION
                || failed.preparation.request_id != request.id
                || request.branch.as_ref() != Some(&failed.preparation.branch)
                || request.base != failed.preparation.base
                || failed.failure.request_id.as_deref() != Some(&request.id)
            {
                return Err(conflict("failed grant completion changed"));
            }
            let intent = failed
                .disposition
                .as_ref()
                .ok_or_else(|| conflict("failed preparation has no disposition"))?;
            let ack = failed
                .acknowledgement
                .as_ref()
                .ok_or_else(|| conflict("failed preparation has no acknowledgement"))?;
            if intent.outcome != disposition::Outcome::NotStarted
                || intent.policy
                    != (disposition::Policy::Prohibit {
                        basis: disposition::Basis::PreparationFailed,
                    })
                || intent.binding.request_id != request.id
                || intent.binding.prepared.is_some()
            {
                return Err(conflict("failed preparation disposition changed"));
            }
            prepare::validate_ack(intent, ack)?;
        }
        _ => return Err(conflict("grant completion is not terminal")),
    }
    Ok(())
}

pub(super) fn retained(
    fleet: &Fleet,
    attempt: &Attempt,
    request: &Request,
) -> Result<Option<Value>> {
    let Some(progress) = &attempt.placement else {
        return Err(conflict("missing placement progress"));
    };
    if let PlacementPhase::Completed {
        saved,
        acknowledgement,
    } = &progress.phase
    {
        if !matches!(progress.version, 2 | crate::node::PLACEMENT_VERSION) {
            return Err(conflict("unsupported retained placement version"));
        }
        validate_saved(request, saved)?;
        validate_ack(saved, acknowledgement.as_ref())?;
        let host = &saved.prepared.grant.envelope.host;
        if fleet.find(&host.id)? != host {
            return Err(conflict("retained placement host changed"));
        }
        if attempt.finished_ms.is_some() && progress.version == crate::node::PLACEMENT_VERSION {
            finish_grant(fleet, attempt, &saved.prepared.grant)?;
        }
        return Ok(Some(saved.result.clone()));
    }
    Ok(None)
}

fn finish_grant(
    fleet: &Fleet,
    attempt: &Attempt,
    grant: &crate::node::PlacementGrant,
) -> Result<()> {
    if attempt
        .placement
        .as_ref()
        .and_then(|progress| progress.completion_ack)
        != Some(false)
    {
        return Ok(());
    }
    let mut admission = serde_json::to_value(attempt)?;
    admission["placement"]
        .as_object_mut()
        .ok_or_else(|| conflict("missing placement"))?
        .remove("completion_ack");
    let proof = crate::node::GrantCompletion {
        version: 1,
        grant: grant.clone(),
        admission: admission.clone(),
    };
    let _: bool = if grant.envelope.host.is_local() {
        fleet.node().finish_placement(&proof)?
    } else {
        fleet.remote_setup(
            &grant.envelope.host,
            &[
                "placement-finished".into(),
                "--input".into(),
                serde_json::to_string(&proof)?,
            ],
        )?
    };
    admission["placement"]["completion_ack"] = serde_json::json!(true);
    fleet
        .ledger
        .child("admissions")
        .write(&format!("{}.json", grant.envelope.reservation), &admission)?;
    Ok(())
}

fn save_phase(
    fleet: &Fleet,
    request: &Request,
    attempt: &mut Attempt,
    phase: PlacementPhase,
) -> Result<()> {
    if matches!(phase, PlacementPhase::Completed { .. }) {
        attempt
            .placement
            .as_mut()
            .ok_or_else(|| conflict("missing placement progress"))?
            .completion_ack = Some(false);
        jobs::finish_once(
            fleet.ledger,
            &mut attempt.finished_ms,
            request.validity.map(|v| v.created_ms),
        )?;
    }
    let boundary = match &phase {
        PlacementPhase::ResultSaved { .. } => "placement-result-saved",
        PlacementPhase::ReleaseAcknowledged { .. } => "placement-acknowledgement-saved",
        PlacementPhase::Completed { .. } => "placement-completed",
        _ => "placement-prepared",
    };
    attempt
        .placement
        .as_mut()
        .ok_or_else(|| conflict("missing placement progress"))?
        .phase = phase;
    fleet
        .ledger
        .child("admissions")
        .write(&format!("{}.json", request.id), attempt)?;
    if let PlacementPhase::Completed { saved, .. } = &attempt
        .placement
        .as_ref()
        .ok_or_else(|| conflict("placement disappeared"))?
        .phase
    {
        finish_grant(fleet, attempt, &saved.prepared.grant)?;
    }
    crate::unit_request::crash_point(boundary);
    Ok(())
}

pub(super) fn perform(fleet: &Fleet, request: &Request, now: u64) -> Result<Option<Value>> {
    let attempts = fleet.ledger.child("admissions");
    let file = format!("{}.json", request.id);
    record(&attempts, request)?;
    let mut selected_now = false;
    loop {
        let mut attempt = attempts
            .read::<Attempt>(&file)?
            .ok_or_else(|| conflict("placement admission disappeared"))?;
        let transport = observe(fleet, request, &attempt)?;
        let progress = attempt
            .placement
            .as_ref()
            .ok_or_else(|| conflict("missing placement progress"))?;
        let phase = match progress.phase {
            PlacementPhase::Recorded | PlacementPhase::SelectionPending { .. } => {
                super::check_request_validity(fleet.ledger, request)?;
                let Placement::Placed { .. } =
                    super::placement(fleet, request, request.host.as_deref(), now)?
                else {
                    return Ok(None);
                };
                crate::unit_request::crash_point("placement-host-selected");
                selected_now = true;
                continue;
            }
            PlacementPhase::HostSelected { ref grant } => {
                super::check_request_validity(fleet.ledger, request)?;
                let grant = grant.clone();
                prepare_selected(
                    fleet,
                    request,
                    &mut attempt,
                    &grant,
                    transport.as_ref(),
                    selected_now,
                )?
            }
            PlacementPhase::PreparationPending {
                ref grant,
                ref preparation,
            } => recover_prepared(fleet, request, grant, preparation, transport)?,
            PlacementPhase::PreparationFailed { .. } => {
                return finish_failure(fleet, request, attempt);
            }
            PlacementPhase::Prepared { ref prepared } => saved_phase(request, prepared)?,
            PlacementPhase::ResultSaved { ref saved } => {
                validate_saved(request, saved)?;
                let ack = if let Some(intent) = &saved.disposition {
                    match prepare::dispose(fleet, &saved.prepared.grant.envelope.host, intent) {
                        Ok(ack) => Some(ack),
                        Err(error) => return Ok(Some(pending(request, saved, &error))),
                    }
                } else {
                    None
                };
                PlacementPhase::ReleaseAcknowledged {
                    saved: saved.clone(),
                    acknowledgement: ack,
                }
            }
            PlacementPhase::ReleaseAcknowledged {
                ref saved,
                ref acknowledgement,
            } => {
                validate_saved(request, saved)?;
                validate_ack(saved, acknowledgement.as_ref())?;
                PlacementPhase::Completed {
                    saved: saved.clone(),
                    acknowledgement: acknowledgement.clone(),
                }
            }
            PlacementPhase::Completed {
                ref saved,
                ref acknowledgement,
            } => {
                validate_saved(request, saved)?;
                validate_ack(saved, acknowledgement.as_ref())?;
                return Ok(Some(saved.result.clone()));
            }
        };
        save_phase(fleet, request, &mut attempt, phase)?;
    }
}
