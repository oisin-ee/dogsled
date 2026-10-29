//! Host-owned preparation disposition. The receipt is the only durable startup fence.

use super::{Pending, Prepared};
use crate::error::{Error, Result};
use crate::ledger::Ledger;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const VERSION: u32 = 1;

/// Captured under preparation and repository protection, never reconstructed from a path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub request_id: String,
    pub request: (PathBuf, String, Option<String>),
    pub claim: Claim,
    pub prepared: Option<Prepared>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    pub tree: crate::cleanup::Tree,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Policy {
    Transfer {
        launch: Box<crate::work::Launch>,
        pane: String,
        job: Option<Box<crate::jobs::Job>>,
    },
    Prohibit {
        basis: Basis,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    PlacementReturned,
    PreparationFailed,
    Retired {
        audit: Box<crate::preparation_retirement::Audit>,
        provenance: Option<serde_json::Value>,
    },
    Cancelled {
        operation_id: String,
        request_id: String,
    },
    UnitRefused {
        launch: Box<crate::work::Launch>,
    },
    AgentRefused {
        launch: Box<crate::work::Launch>,
    },
}

/// Execution evidence is independent of releasing ownership.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    PossiblyExecuted,
    NotStarted,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub version: u32,
    pub binding: Binding,
    pub policy: Policy,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Progress {
    Fenced,
    Released,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_ms: Option<u64>,
    pub request: Request,
    pub progress: Progress,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Acknowledgement {
    pub version: u32,
    pub binding: Binding,
    pub policy: Policy,
    pub outcome: Outcome,
    pub released: bool,
}

impl Claim {
    fn pending(&self) -> Pending {
        Pending {
            tree: self.tree.clone(),
            request_id: Some(self.request_id.clone()),
        }
    }
}

fn conflict() -> Error {
    crate::worktree::refuse("preparation disposition correlation or ownership changed")
}

/// Read-only, request-gated historical capture. Missing checkout identity stays missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Historical {
    pub binding: Binding,
    pub provenance: Option<serde_json::Value>,
    pub disposition: Option<Receipt>,
}

pub fn historical(ledger: &Ledger, id: &str) -> Result<Historical> {
    crate::jobs::validate_id(id)?;
    ledger
        .child("preparation-receipts")
        .with_lock(id, || historical_guarded(ledger, id))
}

fn historical_guarded(ledger: &Ledger, id: &str) -> Result<Historical> {
    let records: Vec<Pending> = ledger.read("preparations.json")?.unwrap_or_default();
    let claims: Vec<_> = records
        .iter()
        .filter(|p| p.request_id.as_deref() == Some(id))
        .collect();
    let receipts = ledger.child("preparation-receipts");
    let provenance = receipts.read::<serde_json::Value>(&format!("{id}.json"))?;
    let disposition = receipts.read::<Receipt>(&file(id))?;
    let binding = match claims.as_slice() {
        [claim] => {
            let receipt = provenance
                .clone()
                .map(serde_json::from_value::<super::PreparationReceipt>)
                .transpose()?;
            if receipt
                .as_ref()
                .is_some_and(|r| r.version.is_some_and(|v| v != VERSION))
            {
                return Err(conflict());
            }
            let captured = if let Some(receipt) = &receipt {
                let captured = receipt.binding.clone().unwrap_or(Binding {
                    request_id: id.to_owned(),
                    request: receipt.request.clone(),
                    claim: Claim {
                        tree: claim.tree.clone(),
                        request_id: id.to_owned(),
                    },
                    prepared: receipt.prepared.clone(),
                });
                if captured.request != receipt.request || captured.prepared != receipt.prepared {
                    return Err(conflict());
                }
                captured
            } else {
                // The identified legacy metadata is the whole provenance. This tuple
                // routes the metadata operation; it supplies no checkout identity.
                Binding {
                    request_id: id.to_owned(),
                    request: (
                        claim.tree.repo.clone().into(),
                        claim.tree.branch.clone(),
                        None,
                    ),
                    claim: Claim {
                        tree: claim.tree.clone(),
                        request_id: id.to_owned(),
                    },
                    prepared: None,
                }
            };
            if captured.request_id != id
                || captured.claim.request_id != id
                || captured.claim.tree != claim.tree
                || !crate::worktree::same(&captured.request.0.to_string_lossy(), &claim.tree.repo)
                || captured.request.1 != claim.tree.branch
            {
                return Err(conflict());
            }
            captured
        }
        [] => {
            let saved = disposition.as_ref().ok_or_else(conflict)?;
            if !matches!(
                saved.request.policy,
                Policy::Prohibit {
                    basis: Basis::Retired { .. }
                }
            ) || !matches_binding(ledger, &saved.request)?
            {
                return Err(conflict());
            }
            saved.request.binding.clone()
        }
        _ => return Err(conflict()),
    };
    if let Some(saved) = &disposition
        && (saved.request.version != VERSION || saved.request.binding != binding)
    {
        return Err(conflict());
    }
    Ok(Historical {
        binding,
        provenance,
        disposition,
    })
}

fn matches_binding(ledger: &Ledger, request: &Request) -> Result<bool> {
    if let Policy::Prohibit {
        basis: Basis::Retired { provenance, audit },
    } = &request.policy
    {
        let id = &request.binding.request_id;
        crate::jobs::validate_id(id)?;
        if audit.request_id != *id || request.binding.claim.request_id != *id {
            return Ok(false);
        }
        let current = ledger
            .child("preparation-receipts")
            .read::<serde_json::Value>(&format!("{id}.json"))?;
        return Ok(current == *provenance);
    }
    Ok(binding(ledger, &request.binding.request_id)?.as_ref() == Some(&request.binding))
}

fn retirement_guarded(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    request: &Request,
    fresh: bool,
) -> Result<Acknowledgement> {
    let operation = || {
        ledger
            .child("launches")
            .with_lock(&request.binding.request_id, || {
                if fresh {
                    let captured = historical_guarded(ledger, &request.binding.request_id)?;
                    if captured.binding != request.binding {
                        return Err(conflict());
                    }
                    validate_policy(ledger, shell, request)?;
                }
                apply(ledger, request)
            })
    };
    if let Some(identity) = &request.binding.claim.tree.identity {
        let path = std::path::Path::new(&identity.common).join("dogsled-lifecycle.lock");
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
        {
            Ok(lock) => {
                let end =
                    std::time::Instant::now() + ledger.budget.remaining("retirement lifecycle")?;
                loop {
                    match lock.try_lock() {
                        Ok(()) => break,
                        Err(std::fs::TryLockError::WouldBlock) => {
                            if std::time::Instant::now() >= end {
                                return Err(Error::Deadline("retirement lifecycle".into()));
                            }
                            std::thread::sleep(std::time::Duration::from_millis(2));
                        }
                        Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
                    }
                }
                let result = operation();
                lock.unlock()?;
                return result;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    operation()
}

fn file(id: &str) -> String {
    format!("{id}.disposition.json")
}

/// The preparation outcome owns the binding. Missing historical identity is never repaired.
fn binding(ledger: &Ledger, id: &str) -> Result<Option<Binding>> {
    crate::jobs::validate_id(id)?;
    let receipt = ledger
        .child("preparation-receipts")
        .read::<super::PreparationReceipt>(&format!("{id}.json"))?;
    let Some(receipt) = receipt else {
        let pending: Vec<Pending> = ledger.read("preparations.json")?.unwrap_or_default();
        if pending.iter().any(|p| p.request_id.as_deref() == Some(id)) {
            return Err(conflict());
        }
        return Ok(None);
    };
    if receipt.version.is_some_and(|version| version != VERSION) {
        return Err(crate::worktree::refuse(
            "unsupported preparation receipt version",
        ));
    }
    let saved = receipt.binding.ok_or_else(|| {
        crate::worktree::refuse("preparation receipt is missing its binding; claim remains held")
    })?;
    if saved.request_id != id
        || saved.claim.request_id != id
        || saved.request != receipt.request
        || saved.prepared != receipt.prepared
        || saved.claim.tree.identity.is_none()
        || receipt.prepared.is_some() == receipt.failure.is_some()
        || saved.prepared.as_ref().is_some_and(|p| {
            p.path
                .as_deref()
                .is_none_or(|path| !crate::worktree::same(path, &saved.claim.tree.path))
                || p.pane_id.as_deref().is_none_or(str::is_empty)
                || p.terminal_id.as_deref().is_none_or(str::is_empty)
        })
    {
        return Err(conflict());
    }
    Ok(Some(saved))
}

pub(super) fn require_binding(ledger: &Ledger, id: &str) -> Result<()> {
    binding(ledger, id)?.ok_or_else(conflict)?;
    Ok(())
}

pub(crate) fn retention_completion(ledger: &Ledger, id: &str) -> Result<Option<u64>> {
    let Some(receipt) = read(ledger, id)? else {
        return Ok(None);
    };
    crate::jobs::completion_time(
        ledger,
        receipt.finished_ms,
        None,
        receipt.progress == Progress::Released,
    )
}

fn read(ledger: &Ledger, id: &str) -> Result<Option<Receipt>> {
    let receipt = ledger
        .child("preparation-receipts")
        .read::<Receipt>(&file(id))?;
    if let Some(receipt) = &receipt
        && ((receipt.finished_ms.is_some() && receipt.progress != Progress::Released)
            || receipt.request.version != VERSION
            || receipt.request.binding.request_id != id
            || !matches_binding(ledger, &receipt.request)?)
    {
        return Err(conflict());
    }
    Ok(receipt)
}

/// Read-only retirement evidence is a resource fence, never a no-launch witness.
pub fn retired(ledger: &Ledger, id: &str) -> Result<bool> {
    let raw = ledger
        .child("preparation-receipts")
        .read::<serde_json::Value>(&file(id))?;
    // Historical non-retirement dispositions keep their existing evidence rules.
    // Only this new basis adds a resource-only exclusion to settlement.
    if raw
        .as_ref()
        .and_then(|value| value.pointer("/request/policy/basis/retired"))
        .is_none()
    {
        return Ok(false);
    }
    let receipt = read(ledger, id)?.ok_or_else(conflict)?;
    crate::preparation_retirement::validate(
        match &receipt.request.policy {
            Policy::Prohibit {
                basis: Basis::Retired { audit, .. },
            } => audit,
            _ => return Err(conflict()),
        },
        &receipt.request,
    )?;
    Ok(true)
}

/// Called under the request's preparation lock, before any discovery or side effect.
pub fn allow_preparation(ledger: &Ledger, id: &str) -> Result<()> {
    if read(ledger, id)?.is_some() {
        return Err(conflict());
    }
    Ok(())
}

/// A missing outcome never authorizes replacing a historical claim or launch.
pub(super) fn allow_fresh(ledger: &Ledger, id: &str) -> Result<()> {
    allow_preparation(ledger, id)?;
    if binding(ledger, id)?.is_some()
        || crate::jobs::record(ledger, id)?.is_some()
        || ledger
            .child("launches")
            .read::<serde_json::Value>(&format!("{id}.json"))?
            .is_some()
        || ledger
            .child("launches")
            .read::<serde_json::Value>(&format!("{id}.progress.json"))?
            .is_some()
    {
        return Err(conflict());
    }
    Ok(())
}

/// A transfer permits only the published wrapper, never another allocation or dispatch.
pub fn allow_launch(ledger: &Ledger, launch: &crate::work::Launch) -> Result<()> {
    allow_preparation(ledger, &launch.id)?;
    if let Some(binding) = binding(ledger, &launch.id)? {
        match_launch(&binding, launch)?;
    }
    Ok(())
}

fn match_launch(binding: &Binding, launch: &crate::work::Launch) -> Result<()> {
    let prepared = binding.prepared.as_ref().ok_or_else(conflict)?;
    if launch.id != binding.request_id
        || !crate::worktree::same(&launch.cwd, &binding.claim.tree.path)
    {
        return Err(conflict());
    }
    match &launch.action {
        crate::work::Action::Agent { .. }
            if launch.pane == prepared.pane_id && launch.terminal == prepared.terminal_id => {}
        crate::work::Action::Job { .. } if launch.pane.is_none() && launch.terminal.is_none() => (),
        _ => return Err(conflict()),
    }
    Ok(())
}

fn validate_tree(
    shell: &dyn crate::shell::Shell,
    guard: &crate::worktree::Guard,
    saved: &Binding,
) -> Result<()> {
    let tree = &saved.claim.tree;
    if crate::worktree::verified_absent(guard, shell, tree) {
        return Ok(());
    }
    let live = guard.observe(shell, &tree.path)?.ok_or_else(conflict)?;
    if tree.drift(&live.tree()) == crate::cleanup::Drift::Identity {
        return Err(conflict());
    }
    Ok(())
}

/// The only fence writer. Outer preparation/repository/launch gates are already held.
/// Validate the exact claim before persisting the fence; a retry may resume an absent claim
/// only when this same receipt proves a prior attempt persisted that fence.
fn apply(ledger: &Ledger, request: &Request) -> Result<Acknowledgement> {
    let id = &request.binding.request_id;
    if request.version != VERSION || !matches_binding(ledger, request)? {
        return Err(conflict());
    }
    let receipts = ledger.child("preparation-receipts");
    let existing = read(ledger, id)?;
    if existing.as_ref().is_some_and(|r| r.request != *request) {
        return Err(conflict());
    }
    ledger.with_lock("preparations", || {
        let pending: Vec<Pending> = ledger.read("preparations.json")?.unwrap_or_default();
        let matching: Vec<_> = pending
            .iter()
            .filter(|p| p.request_id.as_deref() == Some(id))
            .collect();
        match matching.as_slice() {
            [claim] if **claim == request.binding.claim.pending() => (),
            [] if existing.is_some() => (),
            _ => return Err(conflict()),
        }
        Ok(())
    })?;
    if existing
        .as_ref()
        .is_none_or(|r| r.progress != Progress::Released)
    {
        if existing.is_none() {
            receipts.write(
                &file(id),
                &Receipt {
                    finished_ms: None,
                    request: request.clone(),
                    progress: Progress::Fenced,
                },
            )?;
        }
        crate::unit_request::crash_point("preparation-fenced");
        super::used(ledger, &request.binding.claim, existing.is_some())?;
        crate::unit_request::crash_point("preparation-claim-released");
        let mut finished_ms = existing.as_ref().and_then(|receipt| receipt.finished_ms);
        crate::jobs::finish_once(ledger, &mut finished_ms, None)?;
        receipts.write(
            &file(id),
            &Receipt {
                finished_ms,
                request: request.clone(),
                progress: Progress::Released,
            },
        )?;
        let receipt_file = format!("{id}.json");
        let mut preparation = receipts
            .read::<super::PreparationReceipt>(&receipt_file)?
            .ok_or_else(conflict)?;
        if retention_completion(ledger, id)? != finished_ms {
            return Err(conflict());
        }
        if preparation.finished_ms.is_none() {
            preparation.finished_ms = finished_ms;
            receipts.write(&receipt_file, &preparation)?;
        } else if preparation.finished_ms != finished_ms {
            return Err(conflict());
        }
    } else {
        // A released receipt cannot authorize removal of a recreated claim.
        let pending: Vec<Pending> = ledger.read("preparations.json")?.unwrap_or_default();
        if pending.iter().any(|p| p.request_id.as_deref() == Some(id)) {
            return Err(conflict());
        }
    }
    // Recover only the timestamp already proved by release, never the replay time.
    if let Some(finished) = read(ledger, id)?.and_then(|receipt| receipt.finished_ms) {
        let file = format!("{id}.json");
        let mut preparation = receipts
            .read::<super::PreparationReceipt>(&file)?
            .ok_or_else(conflict)?;
        if preparation.finished_ms.is_none() {
            preparation.finished_ms = Some(finished);
            receipts.write(&file, &preparation)?;
        } else if preparation.finished_ms != Some(finished) {
            return Err(conflict());
        }
    }
    Ok(Acknowledgement {
        version: VERSION,
        binding: request.binding.clone(),
        policy: request.policy.clone(),
        outcome: request.outcome.clone(),
        released: true,
    })
}

pub(super) fn failure_guarded(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    guard: &crate::worktree::Guard,
    binding: Binding,
) -> Result<()> {
    validate_tree(shell, guard, &binding)?;
    apply(
        ledger,
        &Request {
            version: VERSION,
            binding,
            policy: Policy::Prohibit {
                basis: Basis::PreparationFailed,
            },
            outcome: Outcome::NotStarted,
        },
    )?;
    Ok(())
}

/// Versioned host transaction for correlated transport. Unsupported or conflicting input
/// refuses before repository observation. Captured expectations are compared, never replaced.
pub fn dispose(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    request: &Request,
) -> Result<Acknowledgement> {
    if request.version != VERSION {
        return Err(conflict());
    }
    let id = &request.binding.request_id;
    crate::jobs::validate_id(id)?;
    ledger
        .child("preparation-receipts")
        .with_lock(id, || dispose_guarded(ledger, shell, request))
}

/// The preparation gate is already held by disposal or final launch replay.
fn dispose_guarded(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    request: &Request,
) -> Result<Acknowledgement> {
    let id = &request.binding.request_id;
    let saved = read(ledger, id)?;
    if !matches_binding(ledger, request)?
        || saved
            .as_ref()
            .is_some_and(|saved| saved.request != *request)
    {
        return Err(conflict());
    }
    if let Some(saved) = &saved {
        let held = ledger.with_lock("preparations", || {
            let records: Vec<Pending> = ledger.read("preparations.json")?.unwrap_or_default();
            Ok(records
                .iter()
                .any(|record| record.request_id.as_deref() == Some(id)))
        })?;
        // The matching fence already captured validated host proof. A removed claim
        // needs no current repository or launch observation to finish acknowledgement.
        // A released receipt never removes even an identical recreated claim.
        if saved.progress == Progress::Released || !held {
            return apply(ledger, request);
        }
    }
    if matches!(
        request.policy,
        Policy::Prohibit {
            basis: Basis::Retired { .. }
        }
    ) {
        // Metadata retirement never observes or repairs the current checkout. Lock the
        // historical lifecycle mutex only if it still exists; do not create a replacement.
        crate::work::launch_test_barrier("RETIREMENT_ENTERED_GATE")?;
        return retirement_guarded(ledger, shell, request, saved.is_none());
    }
    crate::worktree::with_lifecycle(shell, ledger, &request.binding.request.0, |guard| {
        validate_tree(shell, guard, &request.binding)?;
        ledger.child("launches").with_lock(id, || {
            if saved.is_none() {
                validate_policy(ledger, shell, request)?;
            }
            apply(ledger, request)
        })
    })
}

fn validate_policy(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    request: &Request,
) -> Result<()> {
    let binding = &request.binding;
    let id = &binding.request_id;
    let receipts = ledger.child("launches");
    match &request.policy {
        Policy::Prohibit {
            basis: Basis::PlacementReturned,
        } => {
            if request.outcome != Outcome::Unknown
                || binding.prepared.is_none()
                || crate::jobs::record(ledger, id)?.is_some()
                || receipts
                    .read::<serde_json::Value>(&format!("{id}.json"))?
                    .is_some()
                || receipts
                    .read::<serde_json::Value>(&format!("{id}.progress.json"))?
                    .is_some()
            {
                return Err(conflict());
            }
        }
        Policy::Prohibit {
            basis: Basis::PreparationFailed,
        } => {
            if request.outcome != Outcome::NotStarted || binding.prepared.is_some() {
                return Err(conflict());
            }
        }
        Policy::Prohibit {
            basis: Basis::AgentRefused { launch },
        } => {
            match_launch(binding, launch)?;
            if request.outcome != Outcome::NotStarted
                || !crate::launch::refused_before_start(ledger, launch)?
            {
                return Err(conflict());
            }
        }
        Policy::Prohibit {
            basis: Basis::UnitRefused { launch },
        } => {
            match_launch(binding, launch)?;
            let saved = receipts
                .read::<serde_json::Value>(&format!("{id}.json"))?
                .ok_or_else(conflict)?;
            if saved
                .get("failed")
                .and_then(serde_json::Value::as_str)
                .is_none()
            {
                return Err(conflict());
            }
            let execution = &launch.unit.as_ref().ok_or_else(conflict)?.execution;
            let Some(crate::unit_reply::HostEvidence::Refused(evidence)) =
                crate::launch::unit_receipt_locked(ledger, shell, execution)?
            else {
                return Err(conflict());
            };
            if request.outcome != Outcome::NotStarted || evidence.launch != **launch {
                return Err(conflict());
            }
        }
        Policy::Transfer { launch, pane, job } => {
            match_launch(binding, launch)?;
            let saved = receipts
                .read::<serde_json::Value>(&format!("{id}.json"))?
                .ok_or_else(conflict)?;
            if request.outcome != Outcome::PossiblyExecuted || saved["pane"].as_str() != Some(pane)
            {
                return Err(conflict());
            }
            if let crate::work::Action::Job {
                cmd,
                timeout_ms,
                idle_timeout_ms,
                ..
            } = &launch.action
            {
                let owner = job.as_ref().ok_or_else(conflict)?;
                match_job(binding, launch, pane, owner)?;
                let current = crate::jobs::record(ledger, id)?.ok_or_else(conflict)?;
                if !same_job_owner(owner, &current) {
                    return Err(conflict());
                }
                crate::work::correlate(ledger, launch, cmd, *timeout_ms, *idle_timeout_ms, &saved)?;
            } else {
                let (recorded, tree) = crate::launch::checkout_for_request(ledger, launch)?;
                if job.is_some()
                    || !recorded
                    || tree.as_ref().is_none_or(|tree| {
                        binding.claim.tree.drift(tree) == crate::cleanup::Drift::Identity
                    })
                {
                    return Err(conflict());
                }
            }
        }
        prohibited @ Policy::Prohibit { .. } => validate_prohibition(prohibited, request)?,
    }
    Ok(())
}

fn validate_prohibition(policy: &Policy, request: &Request) -> Result<()> {
    if let Policy::Prohibit {
        basis: Basis::Retired { audit, .. },
    } = policy
    {
        return crate::preparation_retirement::validate(audit, request);
    }
    validate_cancelled(policy, &request.binding)
}

fn validate_cancelled(policy: &Policy, binding: &Binding) -> Result<()> {
    let Policy::Prohibit {
        basis: Basis::Cancelled {
            operation_id,
            request_id,
        },
    } = policy
    else {
        return Err(conflict());
    };
    crate::jobs::validate_id(operation_id)?;
    if *request_id != binding.request_id || binding.claim.request_id != *request_id {
        return Err(conflict());
    }
    Ok(())
}

/// Finish a final launch under the preparation gate, before returning its saved result.
/// The persisted fence is the original proof; replay never discovers a new checkout owner.
pub fn replay_guarded(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    launch: &crate::work::Launch,
) -> Result<()> {
    let saved = read(ledger, &launch.id)?;
    let request = if let Some(saved) = saved {
        let (Policy::Transfer {
            launch: expected, ..
        }
        | Policy::Prohibit {
            basis:
                Basis::AgentRefused { launch: expected } | Basis::UnitRefused { launch: expected },
        }) = &saved.request.policy
        else {
            return Err(conflict());
        };
        if **expected != *launch {
            return Err(conflict());
        }
        dispose_guarded(ledger, shell, &saved.request)?;
        return Ok(());
    } else {
        // Historical outcomes carry no release authority. A correlated final launch
        // still replays read-only, leaving its preparation claim held.
        if ledger
            .child("preparation-receipts")
            .read::<super::PreparationReceipt>(&format!("{}.json", launch.id))?
            .is_some_and(|receipt| receipt.version.is_none() && receipt.binding.is_none())
        {
            return Ok(());
        }
        let Some(binding) = binding(ledger, &launch.id)? else {
            return Ok(());
        };
        if !crate::launch::refused_before_start(ledger, launch)? {
            let Some(result) = ledger
                .child("launches")
                .read::<serde_json::Value>(&format!("{}.json", launch.id))?
            else {
                return Ok(());
            };
            return crate::worktree::with_lifecycle(shell, ledger, &binding.request.0, |_guard| {
                ledger.with_lock("done", || {
                    ledger
                        .child("launches")
                        .with_lock(&launch.id, || transfer_guarded(ledger, launch, &result))
                })
            });
        }
        if launch.unit.is_some() {
            return Ok(());
        }
        match_launch(&binding, launch)?;
        Request {
            version: VERSION,
            binding,
            policy: Policy::Prohibit {
                basis: Basis::AgentRefused {
                    launch: Box::new(launch.clone()),
                },
            },
            outcome: Outcome::NotStarted,
        }
    };
    crate::worktree::with_lifecycle(shell, ledger, &request.binding.request.0, |_guard| {
        ledger.with_lock("done", || {
            ledger.child("launches").with_lock(&launch.id, || {
                apply(ledger, &request)?;
                Ok(())
            })
        })
    })
}

/// A failed call is not proof. Only the persisted native refusal can release its claim.
/// Preparation, repository, done, and launch locks are already held in that order.
pub fn agent_refused_guarded(ledger: &Ledger, launch: &crate::work::Launch) -> Result<()> {
    if launch.unit.is_some() {
        return Ok(());
    }
    if !crate::launch::refused_before_start(ledger, launch)? {
        return Ok(());
    }
    let Some(binding) = binding(ledger, &launch.id)? else {
        return Ok(());
    };
    match_launch(&binding, launch)?;
    apply(
        ledger,
        &Request {
            version: VERSION,
            binding,
            policy: Policy::Prohibit {
                basis: Basis::AgentRefused {
                    launch: Box::new(launch.clone()),
                },
            },
            outcome: Outcome::NotStarted,
        },
    )?;
    Ok(())
}

/// Successful dispatch has already published the matching launch owner. The outer gates
/// remain held while ownership passes to it; the first pending wrapper is not prohibited.
pub fn transfer_guarded(
    ledger: &Ledger,
    launch: &crate::work::Launch,
    result: &serde_json::Value,
) -> Result<()> {
    if let Some(saved) = read(ledger, &launch.id)? {
        if !matches!(&saved.request.policy, Policy::Transfer { launch: expected, pane, .. }
            if **expected == *launch && result["pane"].as_str() == Some(pane))
        {
            return Err(conflict());
        }
        apply(ledger, &saved.request)?;
        return Ok(());
    }
    let Some(binding) = binding(ledger, &launch.id)? else {
        return Ok(());
    };
    match_launch(&binding, launch)?;
    let pane = result
        .get("pane")
        .and_then(serde_json::Value::as_str)
        .filter(|p| !p.is_empty())
        .ok_or_else(conflict)?
        .to_owned();
    let saved = ledger
        .child("launches")
        .read::<serde_json::Value>(&format!("{}.json", launch.id))?
        .ok_or_else(conflict)?;
    if saved != *result {
        return Err(conflict());
    }
    let owner = if let crate::work::Action::Job {
        cmd,
        timeout_ms,
        idle_timeout_ms,
        ..
    } = &launch.action
    {
        crate::work::correlate(ledger, launch, cmd, *timeout_ms, *idle_timeout_ms, result)?;
        let job = crate::jobs::record(ledger, &launch.id)?.ok_or_else(conflict)?;
        match_job(&binding, launch, &pane, &job)?;
        Some(Box::new(job))
    } else {
        let (recorded, tree) = crate::launch::checkout_for_request(ledger, launch)?;
        if !recorded
            || tree.as_ref().is_none_or(|tree| {
                binding.claim.tree.drift(tree) == crate::cleanup::Drift::Identity
            })
        {
            return Err(conflict());
        }
        None
    };
    crate::unit_request::crash_point("preparation-before-fence");
    apply(
        ledger,
        &Request {
            version: VERSION,
            binding,
            policy: Policy::Transfer {
                launch: Box::new(launch.clone()),
                pane,
                job: owner,
            },
            outcome: Outcome::PossiblyExecuted,
        },
    )?;
    Ok(())
}

/// Freeze preparation authority into the existing Job before allocation. A copied Job
/// cannot forget its preparation and inherit another request's transfer.
pub fn startup_claim(
    ledger: &Ledger,
    launch: &crate::work::Launch,
    checkout: Option<&crate::cleanup::Tree>,
) -> Result<crate::jobs::Startup> {
    let mut startup = crate::jobs::Startup::captured(&launch.cwd, checkout);
    startup.preparation = binding(ledger, &launch.id)?;
    Ok(startup)
}

fn match_job(
    binding: &Binding,
    launch: &crate::work::Launch,
    pane: &str,
    job: &crate::jobs::Job,
) -> Result<()> {
    match_launch(binding, launch)?;
    let crate::work::Action::Job {
        cmd,
        timeout_ms,
        idle_timeout_ms,
        ..
    } = &launch.action
    else {
        return Err(conflict());
    };
    if job.id != launch.id || job.cwd != launch.cwd || job.host != launch.host || job.owner != launch.owner
        || job.pane != pane || job.cmd != *cmd || job.timeout_ms != *timeout_ms || job.idle_timeout_ms != *idle_timeout_ms
        || job.startup.as_ref().is_none_or(|startup| startup.version != 1 || startup.preparation.as_ref() != Some(binding)
            || !matches!(&startup.scope, crate::jobs::ExecutionScope::Git { checkout } if binding.claim.tree.drift(checkout) != crate::cleanup::Drift::Identity))
    { return Err(conflict()); }
    Ok(())
}

fn same_job_owner(saved: &crate::jobs::Job, current: &crate::jobs::Job) -> bool {
    saved.id == current.id
        && saved.cmd == current.cmd
        && saved.cwd == current.cwd
        && saved.host == current.host
        && saved.owner == current.owner
        && saved.pane == current.pane
        && saved.timeout_ms == current.timeout_ms
        && saved.idle_timeout_ms == current.idle_timeout_ms
        && saved.validity == current.validity
        && saved.startup == current.startup
}

/// C2's final process-start boundary calls this under preparation/repository protection.
pub fn allow_startup(ledger: &Ledger, job: &crate::jobs::Job) -> Result<()> {
    let saved = binding(ledger, &job.id)?;
    let captured = job
        .startup
        .as_ref()
        .and_then(|startup| startup.preparation.as_ref());
    if captured != saved.as_ref() {
        return Err(conflict());
    }
    match read(ledger, &job.id)? {
        Some(Receipt {
            request:
                Request {
                    binding,
                    policy:
                        Policy::Transfer {
                            launch,
                            pane,
                            job: owner,
                        },
                    outcome: Outcome::PossiblyExecuted,
                    ..
                },
            ..
        }) => {
            match_job(&binding, &launch, &pane, job)?;
            let owner = owner.ok_or_else(conflict)?;
            if !same_job_owner(&owner, job) {
                return Err(conflict());
            }
            let receipt = ledger
                .child("launches")
                .read::<serde_json::Value>(&format!("{}.json", job.id))?
                .ok_or_else(conflict)?;
            let crate::work::Action::Job {
                cmd,
                timeout_ms,
                idle_timeout_ms,
                ..
            } = &launch.action
            else {
                return Err(conflict());
            };
            crate::work::correlate(
                ledger,
                &launch,
                cmd,
                *timeout_ms,
                *idle_timeout_ms,
                &receipt,
            )
        }
        Some(_) => Err(conflict()),
        // A preparation without a persisted transfer cannot start, even if dispatch reached
        // the pane before the host crashed. A direct, non-prepared Job keeps C2's authority.
        None if saved.is_some() => Err(conflict()),
        None => Ok(()),
    }
}

/// Called with the launch lock released. Revalidate the correlated native refusal only
/// after acquiring the complete ordered transaction; never invert continuation's done gate.
pub fn refused(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    execution: &crate::unit_transport::ExecutionRef,
    expected: &crate::work::Launch,
) -> Result<()> {
    ledger
        .child("preparation-receipts")
        .with_lock(&execution.request_id, || {
            let Some(binding) = binding(ledger, &execution.request_id)? else {
                return Ok(());
            };
            match_launch(&binding, expected)?;
            let repository = binding.request.0.clone();
            crate::worktree::with_lifecycle(shell, ledger, &repository, |guard| {
                validate_tree(shell, guard, &binding)?;
                ledger
                    .child("launches")
                    .with_lock(&execution.request_id, || {
                        let Some(crate::unit_reply::HostEvidence::Refused(evidence)) =
                            crate::launch::unit_receipt_locked(ledger, shell, execution)?
                        else {
                            return Err(conflict());
                        };
                        if evidence.launch != *expected {
                            return Err(conflict());
                        }
                        let refusal = Request {
                            version: VERSION,
                            binding,
                            policy: Policy::Prohibit {
                                basis: Basis::UnitRefused {
                                    launch: Box::new(expected.clone()),
                                },
                            },
                            outcome: Outcome::NotStarted,
                        };
                        let existing = read(ledger, &execution.request_id)?;
                        let disposition = if let Some(saved) = &existing
                            && matches!(
                                saved.request.policy,
                                Policy::Prohibit {
                                    basis: Basis::Cancelled { .. }
                                }
                            )
                            && saved.request.binding == refusal.binding
                        {
                            &saved.request
                        } else {
                            &refusal
                        };
                        apply(ledger, disposition)?;
                        crate::unit_request::crash_point("refusal-hold-released");
                        Ok(())
                    })
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Request {
        Request {
            version: VERSION,
            binding: Binding {
                request_id: "a".into(),
                request: ("/repo".into(), "worker".into(), None),
                claim: Claim {
                    tree: crate::cleanup::Tree {
                        repo: "/repo".into(),
                        path: "/repo/tree".into(),
                        branch: "worker".into(),
                        head: "saved-head".into(),
                        identity: Some(crate::worktree::Identity {
                            common: "/repo/.git".into(),
                            gitdir: "/repo/.git/worktrees/worker".into(),
                            generation: "a".repeat(32),
                        }),
                    },
                    request_id: "a".into(),
                },
                prepared: Some(Prepared {
                    binding: None,
                    pane_id: Some("w8:p1".into()),
                    terminal_id: Some("term_tree".into()),
                    workspace_id: Some("w8".into()),
                    path: Some("/repo/tree".into()),
                }),
            },
            policy: Policy::Prohibit {
                basis: Basis::PlacementReturned,
            },
            outcome: Outcome::Unknown,
        }
    }

    fn seed(ledger: &Ledger, request: &Request) {
        ledger
            .child("preparation-receipts")
            .write(
                "a.json",
                &super::super::PreparationReceipt {
                    finished_ms: None,
                    validity: None,
                    version: Some(VERSION),
                    request: request.binding.request.clone(),
                    prepared: request.binding.prepared.clone(),
                    failure: None,
                    binding: Some(request.binding.clone()),
                },
            )
            .unwrap();
        ledger
            .write("preparations.json", &vec![request.binding.claim.pending()])
            .unwrap();
    }

    #[test]
    fn disposition_mismatch_never_mutates_ownership() {
        for mismatch in [
            "request",
            "tuple",
            "generation",
            "pane",
            "terminal",
            "version",
            "contradiction",
        ] {
            let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
            let original = request();
            seed(&ledger, &original);
            let mut changed = original.clone();
            match mismatch {
                "request" => changed.binding.request_id = "another".into(),
                "tuple" => changed.binding.request.1 = "another".into(),
                "generation" => {
                    changed
                        .binding
                        .claim
                        .tree
                        .identity
                        .as_mut()
                        .unwrap()
                        .generation = "b".repeat(32);
                }
                "pane" => changed.binding.prepared.as_mut().unwrap().pane_id = Some("other".into()),
                "terminal" => {
                    changed.binding.prepared.as_mut().unwrap().terminal_id = Some("other".into());
                }
                "version" => changed.version += 1,
                "contradiction" => {
                    let mut previous = original.clone();
                    previous.outcome = Outcome::PossiblyExecuted;
                    ledger
                        .child("preparation-receipts")
                        .write(
                            "a.disposition.json",
                            &Receipt {
                                finished_ms: None,
                                request: previous,
                                progress: Progress::Fenced,
                            },
                        )
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let paths = [
                ledger.dir.join("preparations.json"),
                ledger.dir.join("preparation-receipts/a.json"),
                ledger.dir.join("preparation-receipts/a.disposition.json"),
            ];
            let before: Vec<_> = paths
                .iter()
                .map(std::fs::read)
                .map(std::result::Result::ok)
                .collect();
            assert!(
                dispose(&ledger, &crate::shell::Fake::new(), &changed).is_err(),
                "{mismatch}"
            );
            let after: Vec<_> = paths
                .iter()
                .map(std::fs::read)
                .map(std::result::Result::ok)
                .collect();
            assert_eq!(before, after, "{mismatch}");
            std::fs::remove_dir_all(ledger.dir).unwrap();
        }
    }

    #[test]
    fn absent_claim_needs_an_existing_matching_fence() {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let request = request();
        seed(&ledger, &request);
        ledger
            .write("preparations.json", &Vec::<Pending>::new())
            .unwrap();
        assert!(apply(&ledger, &request).is_err());
        assert!(
            !ledger
                .dir
                .join("preparation-receipts/a.disposition.json")
                .exists()
        );
        ledger
            .child("preparation-receipts")
            .write(
                "a.disposition.json",
                &Receipt {
                    finished_ms: None,
                    request: request.clone(),
                    progress: Progress::Fenced,
                },
            )
            .unwrap();
        assert!(apply(&ledger, &request).unwrap().released);
        ledger
            .write("preparations.json", &vec![request.binding.claim.pending()])
            .unwrap();
        assert!(apply(&ledger, &request).is_err());
        assert_eq!(
            ledger
                .read::<Vec<Pending>>("preparations.json")
                .unwrap()
                .unwrap()
                .len(),
            1
        );
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn disposition_releases_only_the_matching_preparation_claim() {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let request = request();
        ledger
            .child("preparation-receipts")
            .write(
                "a.json",
                &super::super::PreparationReceipt {
                    finished_ms: None,
                    validity: None,
                    version: Some(VERSION),
                    request: request.binding.request.clone(),
                    prepared: request.binding.prepared.clone(),
                    failure: None,
                    binding: Some(request.binding.clone()),
                },
            )
            .unwrap();
        let immutable: serde_json::Value = ledger
            .child("preparation-receipts")
            .read("a.json")
            .unwrap()
            .unwrap();
        let first = request.binding.claim.pending();
        let mut other = first.clone();
        other.request_id = Some("b".into());
        ledger
            .write("preparations.json", &vec![first, other.clone()])
            .unwrap();
        let ack = apply(&ledger, &request).unwrap();
        assert!(ack.released);
        assert_eq!(ack.binding, request.binding);
        assert_eq!(
            ledger
                .read::<Vec<Pending>>("preparations.json")
                .unwrap()
                .unwrap(),
            vec![other]
        );
        assert_eq!(
            dispose(&ledger, &crate::shell::Fake::new(), &request).unwrap(),
            ack
        );
        let mut settled: serde_json::Value = ledger
            .child("preparation-receipts")
            .read("a.json")
            .unwrap()
            .unwrap();
        assert!(settled["finished_ms"].as_u64().is_some());
        settled.as_object_mut().unwrap().remove("finished_ms");
        assert_eq!(settled, immutable);
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
}
