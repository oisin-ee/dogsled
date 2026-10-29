//! Prepare work in the repository checkout, never in the caller's worktree.

pub mod disposition;

use crate::diagnostic::{Cause, Context, Diagnostic, Occupancy, Operation};
use crate::error::{Error, Result};
use crate::hosts::{Fleet, Host};
use crate::repository::Values;
use crate::shell::Shell;
use crate::worktree::{self, AGENT_LOCK};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prepared {
    /// Host-captured authority. Receipt snapshots omit this to avoid recursive authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<Box<disposition::Binding>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Pending {
    #[serde(flatten)]
    tree: crate::cleanup::Tree,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
}

/// Refuse changed or missing authority before ingress can publish validity or dispatch.
/// Capture once under the preparation gate. Callers retain it for their decision.
pub struct LaunchAuthority {
    pub binding: Option<Box<disposition::Binding>>,
    pub historical: bool,
    pub legacy_final: bool,
}

pub fn validate_launch_binding(
    ledger: &crate::ledger::Ledger,
    launch: &crate::work::Launch,
) -> Result<LaunchAuthority> {
    if launch.preparation.is_some() || launch.preparation_protocol.is_some() {
        require_protocol(launch.preparation_protocol)?;
    }
    let receipt = ledger
        .child("preparation-receipts")
        .read::<PreparationReceipt>(&format!("{}.json", launch.id))?;
    if receipt
        .as_ref()
        .is_some_and(|r| r.version.is_some() && r.binding.is_none())
    {
        return Err(worktree::refuse(
            "preparation receipt is missing its binding; claim remains held",
        ));
    }
    let historical = receipt
        .as_ref()
        .is_some_and(|r| r.version.is_none() && r.binding.is_none());
    if let Some(receipt) = &receipt {
        if !historical {
            require_protocol(receipt.version)?;
        }
        if let Some(binding) = &receipt.binding
            && (binding.request != receipt.request
                || binding.prepared != receipt.prepared
                || receipt.prepared.is_none()
                || receipt.failure.is_some())
        {
            return Err(worktree::refuse(
                "preparation binding no longer matches its immutable outcome",
            ));
        }
    }
    let expected = receipt.as_ref().and_then(|r| r.binding.as_ref());
    let legacy_final = if launch.preparation.as_deref() == expected {
        false
    } else if let Some(binding) = expected {
        legacy_bound_final(ledger, launch, binding)?
    } else {
        false
    };
    if launch.preparation.as_deref() != expected && !legacy_final {
        return Err(worktree::refuse(
            "preparation binding changed; ownership remains unresolved",
        ));
    }
    Ok(LaunchAuthority {
        binding: expected.cloned().map(Box::new),
        historical,
        legacy_final,
    })
}

/// C3a saved no launch protocol/binding fields. Correlate its final native receipt
/// format and any transfer proof without changing either or granting fresh dispatch.
fn legacy_bound_final(
    ledger: &crate::ledger::Ledger,
    launch: &crate::work::Launch,
    binding: &disposition::Binding,
) -> Result<bool> {
    if launch.preparation.is_some()
        || launch.preparation_protocol.is_some()
        || binding.request_id != launch.id
        || binding.claim.request_id != launch.id
        || binding.claim.tree.identity.is_none()
        || !worktree::same(&launch.cwd, &binding.claim.tree.path)
    {
        return Ok(false);
    }
    let Some(result) = ledger
        .child("launches")
        .read::<serde_json::Value>(&format!("{}.json", launch.id))?
    else {
        return Ok(false);
    };
    if result["host"].as_str() != Some(&launch.host) {
        return Ok(false);
    }
    let checkout = result
        .get("checkout")
        .cloned()
        .map(serde_json::from_value::<crate::cleanup::Tree>)
        .transpose()?;
    if checkout
        .as_ref()
        .is_none_or(|tree| binding.claim.tree.drift(tree) == crate::cleanup::Drift::Identity)
    {
        return Ok(false);
    }
    if let Some(receipt) = ledger
        .child("preparation-receipts")
        .read::<disposition::Receipt>(&format!("{}.disposition.json", launch.id))?
    {
        let disposition::Policy::Transfer {
            launch: saved,
            pane,
            job,
        } = &receipt.request.policy
        else {
            return Ok(false);
        };
        if receipt.request.version != disposition::VERSION
            || receipt.request.binding != *binding
            || receipt.request.outcome != disposition::Outcome::PossiblyExecuted
            || **saved != *launch
            || result["pane"].as_str() != Some(pane)
            || !matching_transfer_owner(launch, pane, job.as_deref(), binding)
        {
            return Ok(false);
        }
    }
    let Some(prepared) = binding.prepared.as_ref() else {
        return Ok(false);
    };
    match &launch.action {
        crate::work::Action::Agent { .. } => {
            let saved = ledger
                .child("launches")
                .read::<serde_json::Value>(&format!("{}.progress.json", launch.id))?;
            if saved.as_ref().is_none_or(|saved| {
                saved["request"].get("preparation_protocol").is_some()
                    || saved["request"].get("preparation").is_some()
            }) {
                return Ok(false);
            }
            Ok(launch.pane == prepared.pane_id
                && launch.terminal == prepared.terminal_id
                && crate::launch::replay(ledger, launch)?.as_ref() == Some(&result))
        }
        crate::work::Action::Job {
            cmd,
            timeout_ms,
            idle_timeout_ms,
            ..
        } => {
            if result["request"].get("preparation_protocol").is_some() {
                return Ok(false);
            }
            crate::work::correlate(ledger, launch, cmd, *timeout_ms, *idle_timeout_ms, &result)?;
            Ok(true)
        }
        crate::work::Action::Place => Ok(false),
    }
}

/// Retry only this exact transaction. Transport failure never supplies execution evidence.
pub fn dispose(
    fleet: &Fleet,
    host: &Host,
    request: &disposition::Request,
) -> Result<disposition::Acknowledgement> {
    let ack = if host.is_local() {
        disposition::dispose(fleet.ledger, fleet.shell, request)?
    } else {
        fleet.remote_setup(
            host,
            &[
                "prepared".into(),
                "--preparation-protocol".into(),
                disposition::VERSION.to_string(),
                "--input".into(),
                serde_json::to_string(request)?,
            ],
        )?
    };
    validate_ack(request, &ack)?;
    Ok(ack)
}

pub fn validate_ack(
    request: &disposition::Request,
    ack: &disposition::Acknowledgement,
) -> Result<()> {
    if request.version != disposition::VERSION
        || ack.version != request.version
        || ack.binding != request.binding
        || ack.policy != request.policy
        || ack.outcome != request.outcome
        || !ack.released
    {
        return Err(worktree::refuse(
            "unproven preparation disposition acknowledgement; ownership remains unresolved",
        ));
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct LaunchResponse {
    result: serde_json::Value,
    disposition: disposition::Request,
}

/// The wire carries host-produced disposition proof separately from the immutable result.
/// Neither a transport envelope nor its acknowledgement rewrites the launch receipt.
pub fn launch_response(
    ledger: &crate::ledger::Ledger,
    launch: &crate::work::Launch,
    result: serde_json::Value,
) -> Result<serde_json::Value> {
    if launch.preparation.is_none() {
        return Ok(result);
    }
    let receipt = ledger
        .child("preparation-receipts")
        .read::<disposition::Receipt>(&format!("{}.disposition.json", launch.id))?
        .ok_or_else(|| {
            worktree::refuse("launch has no disposition proof; execution remains unresolved")
        })?;
    if receipt.progress != disposition::Progress::Released {
        return Err(worktree::refuse("launch disposition remains held"));
    }
    validate_transfer(launch, &result, &receipt.request)?;
    Ok(serde_json::to_value(LaunchResponse {
        result,
        disposition: receipt.request,
    })?)
}

fn validate_transfer(
    launch: &crate::work::Launch,
    result: &serde_json::Value,
    request: &disposition::Request,
) -> Result<()> {
    if request.version != disposition::VERSION
        || launch.preparation.as_deref() != Some(&request.binding)
        || request.outcome != disposition::Outcome::PossiblyExecuted
    {
        return Err(worktree::refuse(
            "launch disposition binding changed; execution remains unresolved",
        ));
    }
    match &request.policy {
        disposition::Policy::Transfer {
            launch: saved,
            pane,
            job,
        } if **saved == *launch
            && result["pane"].as_str() == Some(pane)
            && matching_transfer_owner(launch, pane, job.as_deref(), &request.binding) =>
        {
            Ok(())
        }
        _ => Err(worktree::refuse(
            "launch disposition owner changed; execution remains unresolved",
        )),
    }
}

/// Dynamic host evidence may vary, but the transferred owner must name this launch.
fn matching_transfer_owner(
    launch: &crate::work::Launch,
    pane: &str,
    job: Option<&crate::jobs::Job>,
    binding: &disposition::Binding,
) -> bool {
    match (&launch.action, job) {
        (crate::work::Action::Agent { .. }, None) => true,
        (
            crate::work::Action::Job {
                cmd,
                timeout_ms,
                idle_timeout_ms,
                validity,
            },
            Some(job),
        ) => {
            job.id == launch.id
                && job.cwd == launch.cwd
                && job.host == launch.host
                && job.owner == launch.owner
                && job.pane == pane
                && job.cmd == *cmd
                && job.timeout_ms == *timeout_ms
                && job.idle_timeout_ms == *idle_timeout_ms
                && job.validity == *validity
                && job.startup.as_ref().is_some_and(|startup| {
                    startup.version == 1
                        && startup.preparation.as_ref() == Some(binding)
                        && matches!(&startup.scope, crate::jobs::ExecutionScope::Git { checkout }
                        if binding.claim.tree.drift(checkout) != crate::cleanup::Drift::Identity)
                })
        }
        _ => false,
    }
}

pub fn accept_launch(
    fleet: &Fleet,
    host: &Host,
    launch: &crate::work::Launch,
    response: serde_json::Value,
) -> Result<serde_json::Value> {
    let response: LaunchResponse = serde_json::from_value(response)?;
    validate_transfer(launch, &response.result, &response.disposition)?;
    dispose(fleet, host, &response.disposition)?;
    Ok(response.result)
}

pub fn main_checkout(shell: &dyn Shell, repo: &str) -> Result<PathBuf> {
    let out = shell.run_ok(
        "git",
        &[
            "-C".into(),
            repo.into(),
            "rev-parse".into(),
            "--path-format=absolute".into(),
            "--git-common-dir".into(),
        ],
    )?;
    let path = Path::new(out.stdout.trim());
    if path.as_os_str().is_empty() {
        return Err(Error::Invalid("git returned no checkout".into()));
    }
    Ok(if path.file_name().is_some_and(|name| name == ".git") {
        path.parent()
            .ok_or_else(|| Error::Invalid("git returned no checkout parent".into()))?
            .to_path_buf()
    } else {
        path.to_path_buf()
    })
}

pub fn run(
    fleet: &Fleet,
    host: &Host,
    repo: &str,
    branch: &str,
    base: Option<&str>,
    request_id: &str,
) -> Result<Prepared> {
    let envelope = envelope(fleet, host, repo, branch, base, request_id)?;
    deliver(fleet, host, &envelope)
}

/// The exact preparation transport is captured before delivery and never rediscovered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub version: u32,
    pub request_id: String,
    pub repo: String,
    pub branch: String,
    pub base: Option<String>,
}

pub fn envelope(
    fleet: &Fleet,
    host: &Host,
    repo: &str,
    branch: &str,
    base: Option<&str>,
    request_id: &str,
) -> Result<Envelope> {
    crate::jobs::validate_id(request_id)?;
    let checkout = main_checkout(fleet.shell, repo)?;
    if host.is_local() {
        return Ok(Envelope {
            version: disposition::VERSION,
            request_id: request_id.into(),
            repo: checkout.display().to_string(),
            branch: branch.into(),
            base: base.map(str::to_owned),
        });
    }
    let home = Path::new(&fleet.home);
    let canonical = home.canonicalize().ok();
    let remote = checkout
        .strip_prefix(home)
        .ok()
        .or_else(|| {
            canonical
                .as_deref()
                .and_then(|home| checkout.strip_prefix(home).ok())
        })
        .unwrap_or(&checkout)
        .display()
        .to_string();
    Ok(Envelope {
        version: disposition::VERSION,
        request_id: request_id.into(),
        repo: remote,
        branch: branch.into(),
        base: base.map(str::to_owned),
    })
}

pub fn deliver(fleet: &Fleet, host: &Host, envelope: &Envelope) -> Result<Prepared> {
    deliver_with(fleet, host, envelope, None)
}

pub fn deliver_placement(
    fleet: &Fleet,
    host: &Host,
    envelope: &Envelope,
    placement: &crate::node::PlacementReplay,
) -> Result<Prepared> {
    if placement.envelope.host != *host {
        return Err(crate::unit_transport::conflict(
            "preparation delivery host changed",
        ));
    }
    deliver_with(fleet, host, envelope, Some(placement))
}

fn deliver_with(
    fleet: &Fleet,
    host: &Host,
    envelope: &Envelope,
    placement: Option<&crate::node::PlacementReplay>,
) -> Result<Prepared> {
    require_protocol(Some(envelope.version))?;
    crate::jobs::validate_id(&envelope.request_id)?;
    if host.is_local() {
        return local_with(
            fleet.shell,
            fleet.ledger,
            &envelope.repo,
            &envelope.branch,
            envelope.base.as_deref(),
            Some(&envelope.request_id),
            placement.map(|p| (p, envelope.repo.as_str())),
        );
    }
    let mut args = vec![
        "prepare".into(),
        "--preparation-protocol".into(),
        envelope.version.to_string(),
        "--repo".into(),
        envelope.repo.clone(),
        "--branch".into(),
        envelope.branch.clone(),
        "--request-id".into(),
        envelope.request_id.clone(),
    ];
    if let Some(base) = &envelope.base {
        args.extend(["--base".into(), base.clone()]);
    }
    if let Some(placement) = placement {
        args.extend([
            "--placement-replay".into(),
            serde_json::to_string(placement)?,
        ]);
    }
    let prepared: Prepared = fleet.remote_setup(host, &args)?;
    validate_prepared(&prepared, &envelope.request_id)?;
    let binding = prepared
        .binding
        .as_deref()
        .ok_or_else(|| worktree::refuse("preparation binding disappeared"))?;
    if binding.request.1 != envelope.branch || binding.request.2 != envelope.base {
        return Err(worktree::refuse(
            "peer returned a different preparation request; ownership remains unresolved",
        ));
    }
    Ok(prepared)
}

/// Read-only lookup authority includes the acknowledged host grant and exact transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookupRequest {
    pub version: u32,
    pub placement: crate::node::PlacementReplay,
    pub preparation: Envelope,
    pub binding: Option<disposition::Binding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LookupState {
    Absent,
    ReceiptOnly {
        outcome: LookupOutcome,
    },
    Disposition {
        outcome: LookupOutcome,
        receipt: Box<disposition::Receipt>,
    },
    UnsupportedVersion,
    Mismatch,
    Unresolved,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum LookupOutcome {
    Incomplete,
    Prepared {
        prepared: Prepared,
    },
    Failed {
        failure: crate::error::RetainedFailure,
        binding: Option<Box<disposition::Binding>>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookupReply {
    pub version: u32,
    pub query: LookupRequest,
    pub result: LookupState,
}

pub fn lookup(fleet: &Fleet, query: &LookupRequest) -> Result<LookupState> {
    let host = &query.placement.envelope.host;
    if *fleet.find(&host.id)? != *host {
        return Err(crate::unit_transport::conflict(
            "preparation lookup host changed",
        ));
    }
    let reply: LookupReply = if host.is_local() {
        lookup_on_host(fleet.ledger, &fleet.home, query)
    } else {
        fleet.remote(
            host,
            &[
                "preparation-lookup".into(),
                "--preparation-protocol".into(),
                disposition::VERSION.to_string(),
                "--input".into(),
                serde_json::to_string(query)?,
            ],
        )?
    };
    require_protocol(Some(reply.version))?;
    if serde_json::to_value(&reply.query)? != serde_json::to_value(query)? {
        return Err(crate::unit_transport::conflict(
            "preparation lookup correlation changed",
        ));
    }
    Ok(reply.result)
}

/// Only immutable record reads. No locks, path discovery, writes, repair, or release.
pub fn lookup_on_host(
    ledger: &crate::ledger::Ledger,
    home: &str,
    query: &LookupRequest,
) -> LookupReply {
    let id = &query.preparation.request_id;
    let result = if crate::jobs::validate_id(id).is_err() {
        LookupState::Mismatch
    } else if query.version != disposition::VERSION
        || query.preparation.version != disposition::VERSION
    {
        LookupState::UnsupportedVersion
    } else if query.placement.validate(id).is_err() {
        LookupState::Mismatch
    } else {
        lookup_guarded(ledger, home, query).unwrap_or(LookupState::Unresolved)
    };
    LookupReply {
        version: disposition::VERSION,
        query: query.clone(),
        result,
    }
}

fn lookup_guarded(
    ledger: &crate::ledger::Ledger,
    home: &str,
    query: &LookupRequest,
) -> Result<LookupState> {
    // Reserve created this gate. Opening read-only cannot create or repair it.
    let gate = std::fs::File::open(
        ledger
            .child("preparation-receipts")
            .dir
            .join(format!("{}.lock", query.preparation.request_id)),
    )?;
    if gate.try_lock().is_err() {
        return Ok(LookupState::Unresolved);
    }
    lookup_records(ledger, home, query)
}

fn validate_placement_delivery(
    ledger: &crate::ledger::Ledger,
    id: &str,
    request: &(PathBuf, String, Option<String>),
    delivery: (&crate::node::PlacementReplay, &str),
) -> Result<()> {
    let (placement, repo) = delivery;
    let envelope = Envelope {
        version: disposition::VERSION,
        request_id: id.into(),
        repo: repo.into(),
        branch: request.1.clone(),
        base: request.2.clone(),
    };
    placement.validate(&envelope.request_id)?;
    let request: crate::work::Request = serde_json::from_value(placement.envelope.request.clone())?;
    let grant = ledger
        .child("placement-grants")
        .read::<crate::node::PlacementGrant>(&format!("{}.json", envelope.request_id))?;
    let query = LookupRequest {
        version: disposition::VERSION,
        placement: placement.clone(),
        preparation: envelope.clone(),
        binding: None,
    };
    if !matching_lookup(&query, &request, grant.as_ref()) {
        return Err(crate::unit_transport::conflict(
            "preparation delivery grant changed; no new capacity",
        ));
    }
    Ok(())
}

fn matching_lookup(
    query: &LookupRequest,
    request: &crate::work::Request,
    grant: Option<&crate::node::PlacementGrant>,
) -> bool {
    query.placement.expected_grant.is_some()
        && grant == query.placement.expected_grant.as_ref()
        && request.branch.as_deref() == Some(query.preparation.branch.as_str())
        && request.base == query.preparation.base
        && request.repository.as_ref().is_some_and(|repo| {
            if query.placement.envelope.host.is_local() {
                worktree::same(&repo.path, &query.preparation.repo)
            } else {
                repo.relative == query.preparation.repo
            }
        })
}

fn lookup_outcome(receipt: &PreparationReceipt, id: &str) -> Result<LookupOutcome> {
    let outcome = match (receipt.prepared.clone(), receipt.failure.clone()) {
        (Some(mut prepared), None) => {
            prepared.binding = receipt.binding.clone().map(Box::new);
            validate_prepared(&prepared, id)?;
            LookupOutcome::Prepared { prepared }
        }
        (None, Some(failure)) if failure.request_id.as_deref() == Some(id) => {
            LookupOutcome::Failed {
                failure,
                binding: receipt.binding.clone().map(Box::new),
            }
        }
        (None, None) => LookupOutcome::Incomplete,
        _ => {
            return Err(crate::unit_transport::conflict(
                "unresolved preparation outcome",
            ));
        }
    };
    Ok(outcome)
}

/// Non-placement preparation identity, captured before transport, never reconstructed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationLookup {
    pub version: u32,
    pub request: crate::work::Request,
    pub host: Host,
    pub preparation: Envelope,
    pub binding: Option<disposition::Binding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparationLookupReply {
    pub version: u32,
    pub query: PreparationLookup,
    pub result: LookupState,
}

/// Untagged only to retain the existing placement wire envelope unchanged.
/// A non-placement query cannot deserialize as the older required-grant envelope.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum LookupQuery {
    Placement(LookupRequest),
    Preparation(PreparationLookup),
}

pub fn lookup_preparation(fleet: &Fleet, query: &PreparationLookup) -> Result<LookupState> {
    if *fleet.find(&query.host.id)? != query.host {
        return Err(crate::unit_transport::conflict(
            "preparation lookup host changed",
        ));
    }
    let reply: PreparationLookupReply = if query.host.is_local() {
        lookup_preparation_on_host(fleet.ledger, &fleet.home, query)
    } else {
        fleet.remote(
            &query.host,
            &[
                "preparation-lookup".into(),
                "--preparation-protocol".into(),
                disposition::VERSION.to_string(),
                "--input".into(),
                serde_json::to_string(query)?,
            ],
        )?
    };
    require_protocol(Some(reply.version))?;
    if serde_json::to_value(&reply.query)? != serde_json::to_value(query)? {
        return Err(crate::unit_transport::conflict(
            "preparation lookup correlation changed",
        ));
    }
    Ok(reply.result)
}

pub fn lookup_preparation_on_host(
    ledger: &crate::ledger::Ledger,
    home: &str,
    query: &PreparationLookup,
) -> PreparationLookupReply {
    let id = &query.preparation.request_id;
    let result = if query.version != disposition::VERSION
        || query.preparation.version != disposition::VERSION
    {
        LookupState::UnsupportedVersion
    } else if crate::jobs::validate_id(id).is_err()
        || query.request.id != *id
        || !matches!(
            query.request.action,
            crate::work::Action::Agent { .. } | crate::work::Action::Job { .. }
        )
        || query.request.branch.as_ref() != Some(&query.preparation.branch)
        || query.request.base != query.preparation.base
        || query.request.repository.as_ref().is_none_or(|repo| {
            if query.host.is_local() {
                !worktree::same(&repo.path, &query.preparation.repo)
            } else {
                repo.relative != query.preparation.repo
            }
        })
    {
        LookupState::Mismatch
    } else {
        (|| {
            let gate = std::fs::File::open(
                ledger
                    .child("preparation-receipts")
                    .dir
                    .join(format!("{id}.lock")),
            )?;
            if gate.try_lock().is_err() {
                return Ok(LookupState::Unresolved);
            }
            lookup_receipt(ledger, home, &query.preparation, query.binding.as_ref())
        })()
        .unwrap_or(LookupState::Unresolved)
    };
    PreparationLookupReply {
        version: disposition::VERSION,
        query: query.clone(),
        result,
    }
}

fn lookup_records(
    ledger: &crate::ledger::Ledger,
    home: &str,
    query: &LookupRequest,
) -> Result<LookupState> {
    let id = &query.preparation.request_id;
    let request: crate::work::Request =
        serde_json::from_value(query.placement.envelope.request.clone())?;
    let grant = ledger
        .child("placement-grants")
        .read::<crate::node::PlacementGrant>(&format!("{id}.json"))?;
    if !matching_lookup(query, &request, grant.as_ref()) {
        return Ok(LookupState::Mismatch);
    }
    let holds: Vec<crate::ledger::Reservation> =
        ledger.read("placements.json")?.unwrap_or_default();
    if let Some(grant) = &grant
        && holds
            .iter()
            .any(|hold| hold.id == *id && *hold != grant.reservation)
    {
        return Ok(LookupState::Mismatch);
    }
    lookup_receipt(ledger, home, &query.preparation, query.binding.as_ref())
}

fn lookup_receipt(
    ledger: &crate::ledger::Ledger,
    home: &str,
    preparation: &Envelope,
    expected_binding: Option<&disposition::Binding>,
) -> Result<LookupState> {
    let id = &preparation.request_id;
    let receipts = ledger.child("preparation-receipts");
    let raw = receipts.read::<serde_json::Value>(&format!("{id}.json"))?;
    let raw_disposition = receipts.read::<serde_json::Value>(&format!("{id}.disposition.json"))?;
    let claims: Vec<Pending> = ledger.read("preparations.json")?.unwrap_or_default();
    let held: Vec<_> = claims
        .iter()
        .filter(|p| p.request_id.as_ref() == Some(id))
        .collect();
    let Some(raw) = raw else {
        return Ok(
            if raw_disposition.is_some() || !held.is_empty() || expected_binding.is_some() {
                LookupState::Unresolved
            } else {
                LookupState::Absent
            },
        );
    };
    match raw.get("version").and_then(serde_json::Value::as_u64) {
        Some(version) if version == u64::from(disposition::VERSION) => {}
        Some(_) => return Ok(LookupState::UnsupportedVersion),
        None => return Ok(LookupState::Unresolved),
    }
    let receipt: PreparationReceipt = serde_json::from_value(raw)?;
    let expected = (
        Path::new(home).join(&preparation.repo),
        preparation.branch.clone(),
        preparation.base.clone(),
    );
    if !worktree::same(
        &receipt.request.0.display().to_string(),
        &expected.0.display().to_string(),
    ) || receipt.request.1 != expected.1
        || receipt.request.2 != expected.2
    {
        return Ok(LookupState::Mismatch);
    }
    if let Some(binding) = &receipt.binding {
        if binding.request_id != *id
            || binding.claim.request_id != *id
            || binding.request != receipt.request
            || binding.prepared != receipt.prepared
            || binding.claim.tree.identity.is_none()
            || held.iter().any(|p| p.tree != binding.claim.tree)
            || expected_binding.is_some_and(|b| b != binding)
        {
            return Ok(LookupState::Mismatch);
        }
    } else if receipt.prepared.is_some()
        || expected_binding.is_some()
        || (!held.is_empty() && receipt.failure.is_some())
    {
        return Ok(LookupState::Unresolved);
    }
    if held.len() > 1 || (raw_disposition.is_none() && receipt.binding.is_some() && held.is_empty())
    {
        return Ok(LookupState::Unresolved);
    }
    let outcome = lookup_outcome(&receipt, id)?;
    if let Some(raw) = raw_disposition {
        match raw
            .pointer("/request/version")
            .and_then(serde_json::Value::as_u64)
        {
            Some(version) if version == u64::from(disposition::VERSION) => {}
            Some(_) => return Ok(LookupState::UnsupportedVersion),
            None => return Ok(LookupState::Unresolved),
        }
        let saved: disposition::Receipt = serde_json::from_value(raw)?;
        if receipt.binding.as_ref() != Some(&saved.request.binding) {
            return Ok(LookupState::Mismatch);
        }
        return Ok(LookupState::Disposition {
            outcome,
            receipt: Box::new(saved),
        });
    }
    Ok(LookupState::ReceiptOnly { outcome })
}

/// The caller retains the exact host capture, never a newly observed checkout identity.
pub fn validate_prepared(prepared: &Prepared, id: &str) -> Result<()> {
    let binding = prepared.binding.as_deref().ok_or_else(|| {
        worktree::refuse("peer returned no preparation binding; ownership remains unresolved")
    })?;
    let mut snapshot = prepared.clone();
    snapshot.binding = None;
    if binding.request_id != id
        || binding.claim.request_id != id
        || binding.claim.tree.identity.is_none()
        || binding.request.0 != Path::new(&binding.claim.tree.repo)
        || binding.request.1 != binding.claim.tree.branch
        || prepared.pane_id.as_deref().is_none_or(str::is_empty)
        || prepared.terminal_id.as_deref().is_none_or(str::is_empty)
        || binding.prepared.as_ref() != Some(&snapshot)
        || prepared.path.is_none()
    {
        return Err(worktree::refuse(
            "peer returned a mismatched preparation binding; ownership remains unresolved",
        ));
    }
    Ok(())
}

/// Path equivalence can be established only where the checkout actually lives.
fn validate_prepared_host(prepared: &Prepared, id: &str) -> Result<()> {
    validate_prepared(prepared, id)?;
    let binding = prepared
        .binding
        .as_deref()
        .ok_or_else(|| worktree::refuse("missing preparation binding"))?;
    if prepared
        .path
        .as_deref()
        .is_none_or(|path| !worktree::same(path, &binding.claim.tree.path))
    {
        return Err(worktree::refuse(
            "prepared path does not name the captured checkout",
        ));
    }
    Ok(())
}

/// Required at every caller mutation boundary. Never retry without the argument.
pub fn require_protocol(version: Option<u32>) -> Result<()> {
    if version != Some(disposition::VERSION) {
        return Err(worktree::refuse(
            "unsupported preparation disposition protocol; ownership remains held",
        ));
    }
    Ok(())
}

/// Create or reuse the branch's linked worktree in the main checkout of `repo`, lock it as an
/// agent worktree, run the repository's `setup` in a new one, and open it as a Herdr workspace
/// linked to the repository. Records the main checkout's workspace as the anchor when the
/// repository had none open.
pub fn local(
    shell: &dyn Shell,
    ledger: &crate::ledger::Ledger,
    repo: &str,
    branch: &str,
    base: Option<&str>,
    request_id: Option<&str>,
) -> Result<Prepared> {
    local_with(shell, ledger, repo, branch, base, request_id, None)
}

fn local_with(
    shell: &dyn Shell,
    ledger: &crate::ledger::Ledger,
    repo: &str,
    branch: &str,
    base: Option<&str>,
    request_id: Option<&str>,
    placement: Option<(&crate::node::PlacementReplay, &str)>,
) -> Result<Prepared> {
    let Some(id) = request_id else {
        if placement.is_some() {
            return Err(crate::unit_transport::conflict(
                "placement preparation requires its original request id",
            ));
        }
        return local_once(shell, ledger, repo, branch, base, None);
    };
    crate::jobs::validate_id(id)?;
    let receipts = ledger.child("preparation-receipts");
    receipts.with_lock(id, || {
        let file = format!("{id}.json");
        let request = (
            main_checkout(shell, repo)?,
            branch.to_owned(),
            base.map(str::to_owned),
        );
        if let Some(placement) = placement {
            validate_placement_delivery(ledger, id, &request, placement)?;
        }
        if let Some(receipt) = receipts.read::<PreparationReceipt>(&file)? {
            if receipt.request != request {
                return Err(crate::unit_transport::conflict(
                    "preparation request changed",
                ));
            }
            return replay_guarded(shell, ledger, id, &receipt);
        }
        disposition::allow_fresh(ledger, id)?;
        // Persist intent before setup, worktree creation, or Herdr can change host state.
        // Without a completed receipt, a later call cannot prove those effects finished.
        receipts.write(&file, &PreparationReceipt::intent(request.clone()))?;
        let repository = request.0.clone();
        worktree::with_lifecycle(shell, ledger, &repository, |guard| {
            let mut captured = Capture::default();
            let result = (|| {
                let settings = crate::repository::load(&repository)?;
                local_guarded(
                    &mut Preparation {
                        guard,
                        shell,
                        ledger,
                        checkout: &repository,
                        settings: &settings,
                        request_id: Some(id),
                        captured: &mut captured,
                    },
                    branch,
                    base,
                )
            })();
            let binding = preparation_binding(id, &request, captured, &result);
            match result {
                Ok(prepared) => {
                    let mut wire = prepared.clone();
                    wire.binding = binding.clone().map(Box::new);
                    validate_prepared_host(&wire, id)?;
                    receipts.write(
                        &file,
                        &PreparationReceipt {
                            version: Some(disposition::VERSION),
                            request,
                            prepared: Some(prepared.clone()),
                            failure: None,
                            binding: binding.clone(),
                        },
                    )?;
                    Ok(wire)
                }
                Err(error) => {
                    // A returned failure is a final outcome. Record it, so a later call replays
                    // the original cause instead of preparing the branch again.
                    let error = error.correlate(Some(id));
                    let failure = error.retained(Operation::Prepare, Some(id));
                    if receipts
                        .write(
                            &file,
                            &PreparationReceipt {
                                version: Some(disposition::VERSION),
                                request,
                                prepared: None,
                                failure: Some(failure),
                                binding: binding.clone(),
                            },
                        )
                        .is_err()
                    {
                        // The outcome is not retained. The intent stays uncertain, and the report
                        // says so instead of claiming a durable cause.
                        return Err(unretained(branch, base, id));
                    }
                    crate::unit_request::crash_point("preparation-failure-recorded");
                    if let Some(binding) = binding.filter(|b| b.claim.tree.identity.is_some()) {
                        disposition::failure_guarded(ledger, shell, guard, binding)?;
                    }
                    Err(error)
                }
            }
        })
    })
}

fn preparation_binding(
    id: &str,
    request: &(PathBuf, String, Option<String>),
    captured: Capture,
    result: &Result<Prepared>,
) -> Option<disposition::Binding> {
    captured
        .checkout
        .or(captured.intended)
        .map(|pending| disposition::Binding {
            request_id: id.into(),
            request: request.clone(),
            claim: disposition::Claim {
                tree: pending.tree,
                request_id: id.into(),
            },
            prepared: result.as_ref().ok().cloned(),
        })
}

/// A preparation receipt holds the outcome of one request. `prepared` and `failure` are
/// outcomes: only one of them can be set. A receipt without either outcome is an uncertain
/// interruption, and stays uncertain.
#[derive(Debug, Serialize, Deserialize)]
struct PreparationReceipt {
    #[serde(
        default,
        deserialize_with = "receipt_version",
        skip_serializing_if = "Option::is_none"
    )]
    version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    binding: Option<disposition::Binding>,
    request: (PathBuf, String, Option<String>),
    prepared: Option<Prepared>,
    /// The safe record of a returned failure. Absent in receipts written before failures
    /// were retained, and in receipts of an interrupted preparation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failure: Option<crate::error::RetainedFailure>,
}

// Absence alone identifies the historical format; an explicit null is not a version.
pub(crate) fn receipt_version<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> std::result::Result<Option<u32>, D::Error> {
    u32::deserialize(decoder).map(Some)
}

impl PreparationReceipt {
    fn intent(request: (PathBuf, String, Option<String>)) -> Self {
        Self {
            version: Some(disposition::VERSION),
            request,
            prepared: None,
            failure: None,
            binding: None,
        }
    }
}

/// Replay a recorded outcome for the same request. A recorded failure returns its original
/// safe cause without preparing again. An old receipt without an outcome proves nothing,
/// so it stays uncertain.
fn replay_guarded(
    shell: &dyn Shell,
    ledger: &crate::ledger::Ledger,
    id: &str,
    receipt: &PreparationReceipt,
) -> Result<Prepared> {
    if receipt.failure.is_some() && receipt.prepared.is_some() {
        return replay(id, receipt);
    }
    if receipt.failure.is_some() && receipt.prepared.is_none() {
        if let Some(binding) = receipt
            .binding
            .clone()
            .filter(|binding| binding.claim.tree.identity.is_some())
        {
            worktree::with_lifecycle(shell, ledger, &receipt.request.0, |guard| {
                disposition::failure_guarded(ledger, shell, guard, binding)
            })?;
        }
    } else {
        disposition::allow_preparation(ledger, id)?;
        if receipt.prepared.is_some() {
            disposition::require_binding(ledger, id)?;
        }
    }
    let mut prepared = replay(id, receipt)?;
    prepared.binding = receipt.binding.clone().map(Box::new);
    validate_prepared_host(&prepared, id)?;
    Ok(prepared)
}

fn replay(id: &str, receipt: &PreparationReceipt) -> Result<Prepared> {
    match (&receipt.prepared, &receipt.failure) {
        (Some(_), Some(_)) => Err(Error::Invalid(format!(
            "request {id} has a contradictory preparation receipt; inspect this host"
        ))),
        (Some(prepared), None) => Ok(prepared.clone()),
        (None, Some(failure)) => Err(failure.restore()),
        (None, None) => Err(Error::Invalid(format!(
            "request {id} stopped during preparation; inspect this host before resubmitting"
        ))),
    }
}

/// The preparation failed and its outcome could not be recorded. The intent stays uncertain,
/// so the report must not claim that the original cause is retained, and it must not carry
/// the write error's text or the discarded preparation cause. It reports its own cause, so a
/// caller on the other side of the peer boundary sees the persistence failure itself.
fn unretained(branch: &str, base: Option<&str>, request_id: &str) -> Error {
    let diagnostic = Diagnostic::new(
        Operation::Prepare,
        Cause::OutcomeUnrecorded,
        Context::new(Some(branch), base, None),
    )
    .request(Some(request_id));
    Error::Invalid(
        "the preparation failed and its cause was not recorded; \
         inspect this host before resubmitting"
            .into(),
    )
    .with_diagnostic(diagnostic)
}

fn local_once(
    shell: &dyn Shell,
    ledger: &crate::ledger::Ledger,
    repo: &str,
    branch: &str,
    base: Option<&str>,
    request_id: Option<&str>,
) -> Result<Prepared> {
    let checkout = main_checkout(shell, repo)?;
    worktree::with_lifecycle(shell, ledger, &checkout, |guard| {
        let settings = crate::repository::load(&checkout)?;
        local_guarded(
            &mut Preparation {
                guard,
                shell,
                ledger,
                checkout: &checkout,
                settings: &settings,
                request_id,
                captured: &mut Capture::default(),
            },
            branch,
            base,
        )
    })
}

fn validate_branch(shell: &dyn Shell, branch: &str) -> Result<()> {
    if branch.is_empty()
        || branch.starts_with('-')
        || !shell
            .run(
                "git",
                &["check-ref-format".into(), "--branch".into(), branch.into()],
            )?
            .ok()
    {
        return Err(Error::Invalid(format!(
            "{branch} is not a valid branch name"
        )));
    }
    Ok(())
}

#[derive(Default)]
struct Capture {
    intended: Option<Pending>,
    checkout: Option<Pending>,
}

fn local_guarded(
    preparation: &mut Preparation<'_>,
    branch: &str,
    base: Option<&str>,
) -> Result<Prepared> {
    let Preparation {
        shell,
        ledger,
        checkout,
        settings,
        request_id,
        captured,
        ..
    } = preparation;
    let (shell, ledger, checkout, settings, request_id) =
        (*shell, *ledger, *checkout, *settings, *request_id);
    validate_branch(shell, branch)?;
    shell
        .run_ok("herdr", &["workspace".into(), "list".into()])
        .map_err(|e| Error::Invalid(format!("no Herdr server is reachable on this host: {e}")))?;
    let existing = worktree::list(shell, checkout)?
        .into_iter()
        .find(|w| w.branch.as_deref() == Some(branch));
    let has_branch = existing.is_some()
        || git(
            checkout,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .run(shell)?
        .ok();
    if has_branch && base.is_some() {
        return Err(
            Error::Invalid(format!("branch {branch} exists; omit --base")).with_diagnostic(
                Diagnostic::new(
                    Operation::Prepare,
                    Cause::BranchExists,
                    Context::new(Some(branch), base, None),
                )
                .request(request_id),
            ),
        );
    }
    // Preflight, after the existing-branch refusal and before any new hold, worktree, or
    // branch. A base this clone has not fetched is fetched once, so a branch pushed after
    // the clone resolves. Nothing else moves: no other ref, and no local branch.
    if !has_branch && let Some(base) = base {
        let mut resolved = resolve_base(shell, checkout, base)?;
        if !resolved && let Some(remote) = remote_branch(base) {
            // One bounded fetch of that one remote branch, into its own remote-tracking ref
            // whatever the clone's configured refspec. The destination is named, so a
            // single-branch clone resolves the base too.
            // A fetch that cannot run, or that exceeds its budget, is a failure of the
            // fetch itself and is reported as itself. A fetch that runs and exits non-zero
            // leaves the base unresolved, and the refusal below names it.
            let refspec = format!("+refs/heads/{remote}:refs/remotes/origin/{remote}");
            git(
                checkout,
                &["fetch", "--no-tags", "--", "origin", refspec.as_str()],
            )
            .run(shell)?;
            resolved = resolve_base(shell, checkout, base)?;
        }
        if !resolved {
            return Err(
                Error::Invalid("requested base unavailable on this host".into()).with_diagnostic(
                    Diagnostic::new(
                        Operation::Prepare,
                        Cause::BaseUnavailable,
                        Context::new(Some(branch), Some(base), None),
                    )
                    .request(request_id),
                ),
            );
        }
    }
    // Publish the intended path before creating or reusing it. Cleanup must not race setup
    // or the transport gap before launch has recorded its receipt.
    let pending = crate::cleanup::Tree {
        repo: checkout.display().to_string(),
        path: existing.as_ref().map_or_else(
            || {
                checkout
                    .join(&settings.worktree.dir)
                    .join(branch.replace(['/', '\\'], "-"))
                    .display()
                    .to_string()
            },
            |tree| tree.path.clone(),
        ),
        branch: branch.into(),
        head: String::new(),
        identity: None,
    };
    record(ledger, pending.clone(), request_id)?;
    captured.intended = Some(Pending {
        tree: pending,
        request_id: request_id.map(str::to_owned),
    });
    make(preparation, existing, branch, base, has_branch)
}

struct Preparation<'a> {
    guard: &'a worktree::Guard,
    shell: &'a dyn Shell,
    ledger: &'a crate::ledger::Ledger,
    checkout: &'a Path,
    settings: &'a crate::repository::Settings,
    request_id: Option<&'a str>,
    captured: &'a mut Capture,
}

/// Create or reuse the worktree, run setup in a new one, and open it in Herdr.
fn make(
    preparation: &mut Preparation<'_>,
    existing: Option<worktree::Listed>,
    branch: &str,
    base: Option<&str>,
    has_branch: bool,
) -> Result<Prepared> {
    let Preparation {
        guard,
        shell,
        ledger,
        checkout,
        settings,
        request_id,
        captured,
    } = preparation;
    let (guard, shell, ledger, checkout, settings, request_id) =
        (*guard, *shell, *ledger, *checkout, *settings, *request_id);
    let (path, created) = if let Some(found) = existing {
        (reuse(shell, checkout, found)?, false)
    } else {
        let path = checkout
            .join(&settings.worktree.dir)
            .join(branch.replace(['/', '\\'], "-"))
            .display()
            .to_string();
        let mut args = vec!["worktree", "add", "--lock", "--reason", AGENT_LOCK];
        let start;
        if has_branch {
            args.extend([path.as_str(), branch]);
        } else {
            start = match base {
                Some(base) => base.to_string(),
                None => default_branch(shell, checkout)?,
            };
            args.extend(["-b", branch, path.as_str(), start.as_str()]);
        }
        if Path::new(&path).exists() {
            return Err(
                Error::Invalid("workspace path is occupied".into()).with_diagnostic(
                    Diagnostic::new(
                        Operation::Prepare,
                        Cause::WorkspaceOccupied,
                        Context::new(Some(branch), base, Some(&path)).occupied(Occupancy::Path),
                    ),
                ),
            );
        }
        git(checkout, &args).setup_ok(shell).map_err(|error| {
            error.with_diagnostic(Diagnostic::new(
                Operation::Prepare,
                Cause::SetupFailed,
                Context::new(Some(branch), base, Some(&path)),
            ))
        })?;
        (path, true)
    };
    let linked = guard
        .discover(shell, &path)?
        .ok_or_else(|| worktree::refuse("prepared checkout disappeared"))?;
    let pending = Pending {
        tree: linked.tree(),
        request_id: request_id.map(str::to_owned),
    };
    record(ledger, pending.tree.clone(), request_id)?;
    captured.checkout = Some(pending);
    crate::cleanup::remember(ledger, linked.tree())?;
    if created {
        let values = Values {
            main: checkout,
            path: &path,
            branch,
        };
        if crate::repository::run(shell, "setup", &settings.worktree.setup, values).is_err() {
            discard(guard, shell, &linked, (!has_branch).then_some(branch)).map_err(|error| {
                error.with_diagnostic(Diagnostic::new(
                    Operation::Prepare,
                    Cause::SetupFailed,
                    Context::new(Some(branch), base, Some(&path)),
                ))
            })?;
            return Err(Error::Invalid(
                "setup command failed; the new worktree was removed".into(),
            )
            .with_diagnostic(Diagnostic::new(
                Operation::Prepare,
                Cause::SetupFailed,
                Context::new(Some(branch), base, Some(&path)).removed(),
            )));
        }
    }
    worktree::validate_identity(shell, &linked)?;
    let prepared = open(shell, checkout, &path)?;
    crate::cleanup::remember(ledger, linked.tree())?;
    Ok(prepared)
}

fn record(
    ledger: &crate::ledger::Ledger,
    pending: crate::cleanup::Tree,
    request_id: Option<&str>,
) -> Result<()> {
    ledger.with_lock("preparations", || {
        let mut preparations: Vec<Pending> = ledger.read("preparations.json")?.unwrap_or_default();
        preparations.retain(|record| {
            !worktree::same(&record.tree.path, &pending.path)
                || record.request_id.as_deref() != request_id
        });
        preparations.push(Pending {
            tree: pending,
            request_id: request_id.map(str::to_owned),
        });
        ledger.write("preparations.json", &preparations)
    })
}

/// Internal remote preparation applies host-relative checkout routing and remembers cleanup.
pub fn on_host(
    node: &crate::node::Node<'_>,
    repo: &str,
    branch: &str,
    base: Option<&str>,
    request_id: Option<&str>,
    placement: Option<&crate::node::PlacementReplay>,
) -> Result<Prepared> {
    let transport_repo = repo;
    let repo = Path::new(node.home).join(repo).display().to_string();
    let prepared = if placement.is_some() {
        local_with(
            node.shell,
            node.ledger,
            &repo,
            branch,
            base,
            request_id,
            placement.map(|p| (p, transport_repo)),
        )
    } else {
        local(node.shell, node.ledger, &repo, branch, base, request_id)
    }
    .map_err(|error| error.correlate(request_id))?;
    Ok(prepared)
}

/// Only an exact disposition transaction removes a preparation hold. Admission failures,
/// final-launch surveys, and disappeared panes are not persisted release authority.
pub fn pending_trees(
    ledger: &crate::ledger::Ledger,
    _launches: &crate::launch::Survey,
) -> Result<Vec<crate::cleanup::Tree>> {
    let records: Vec<Pending> = ledger.read("preparations.json")?.unwrap_or_default();
    Ok(records.into_iter().map(|record| record.tree).collect())
}

/// The captured checkout of a preparation, never rebound from a stale path.
pub fn saved_tree(
    ledger: &crate::ledger::Ledger,
    path: &str,
    request: Option<&str>,
) -> Result<Option<crate::cleanup::Tree>> {
    let records: Vec<Pending> = ledger.read("preparations.json")?.unwrap_or_default();
    Ok(records
        .into_iter()
        .find(|p| {
            worktree::same(&p.tree.path, path)
                && request.is_none_or(|id| p.request_id.as_deref() == Some(id))
        })
        .map(|p| p.tree))
}

/// Exact-match leaf. The caller owns the outer gates and a persisted disposition fence.
fn used(ledger: &crate::ledger::Ledger, claim: &disposition::Claim, resume: bool) -> Result<()> {
    ledger.with_lock("preparations", || {
        let mut records: Vec<Pending> = ledger.read("preparations.json")?.unwrap_or_default();
        let matches: Vec<_> = records
            .iter()
            .enumerate()
            .filter(|(_, record)| record.request_id.as_deref() == Some(&claim.request_id))
            .collect();
        match matches.as_slice() {
            [(index, record)] if record.tree == claim.tree => {
                let index = *index;
                records.remove(index);
            }
            [] if resume => return Ok(()),
            _ => return Err(worktree::refuse("preparation claim changed or disappeared")),
        }
        ledger.write("preparations.json", &records)
    })
}

/// The branch's existing linked worktree, locked as an agent worktree. Another lock, or the
/// main checkout, is someone else's.
fn reuse(shell: &dyn Shell, checkout: &Path, found: worktree::Listed) -> Result<String> {
    if worktree::same(&found.path, &checkout.display().to_string()) {
        return Err(Error::Invalid(format!(
            "branch {} is checked out in the main checkout {}",
            found.branch.as_deref().unwrap_or_default(),
            found.path
        ))
        .with_diagnostic(Diagnostic::new(
            Operation::Prepare,
            Cause::WorkspaceOccupied,
            Context::new(found.branch.as_deref(), None, Some(&found.path))
                .occupied(Occupancy::MainCheckout),
        )));
    }
    match found.lock.as_deref() {
        None => git(
            checkout,
            &["worktree", "lock", "--reason", AGENT_LOCK, &found.path],
        )
        .run_ok(shell)?,
        Some(AGENT_LOCK) => (),
        Some(other) => {
            return Err(Error::Invalid(format!(
                "{} is locked ({other}); it is not an agent worktree",
                found.path
            ))
            .with_diagnostic(Diagnostic::new(
                Operation::Prepare,
                Cause::WorkspaceOccupied,
                Context::new(found.branch.as_deref(), None, Some(&found.path))
                    .occupied(Occupancy::Locked),
            )));
        }
    }
    Ok(found.path)
}

/// Whether the base resolves to a commit on this host.
fn resolve_base(shell: &dyn Shell, checkout: &Path, base: &str) -> Result<bool> {
    Ok(git(
        checkout,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{base}^{{commit}}"),
        ],
    )
    .run(shell)?
    .ok())
}

/// The remote branch of an `origin/<branch>` base, when this host may fetch that name.
/// A name that could be read as an option, or that carries a path, is not fetched.
fn remote_branch(base: &str) -> Option<&str> {
    let branch = base.strip_prefix("origin/")?;
    if branch.is_empty()
        || branch.starts_with('-')
        || branch.contains("..")
        || !branch
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"/_-.".contains(&c))
    {
        return None;
    }
    Some(branch)
}

/// The local name of the repository's default branch, the base of a new branch.
fn default_branch(shell: &dyn Shell, checkout: &Path) -> Result<String> {
    let base = crate::integration::base(shell, &checkout.display().to_string())?;
    Ok(base
        .strip_prefix("refs/remotes/origin/")
        .or_else(|| base.strip_prefix("refs/heads/"))
        .unwrap_or(&base)
        .to_string())
}

/// Remove a worktree that `place` just made and whose setup failed. It holds nothing yet, so
/// the removal is forced through the agent lock, and a branch made for it goes too.
fn discard(
    guard: &worktree::Guard,
    shell: &dyn Shell,
    tree: &worktree::Linked,
    branch: Option<&str>,
) -> Result<()> {
    guard.check_repository(shell, &tree.checkout)?;
    worktree::validate_identity(shell, tree)?;
    git(
        &tree.checkout,
        &["worktree", "remove", "-f", "-f", &tree.path],
    )
    .setup_ok(shell)?;
    if let Some(branch) = branch {
        git(&tree.checkout, &["branch", "-D", branch]).run_ok(shell)?;
    }
    Ok(())
}

/// Open `path` as a Herdr workspace without focus, and record the anchor when Herdr opened the
/// main checkout's workspace for it. A workspace that already showed the path may belong to a
/// running child, so that is an error.
fn open(shell: &dyn Shell, checkout: &Path, path: &str) -> Result<Prepared> {
    let main = checkout.display().to_string();
    let before = worktree::workspace(shell, checkout, &main)?;
    let out = shell.run_ok(
        "herdr",
        &[
            "worktree".into(),
            "open".into(),
            "--cwd".into(),
            main.clone(),
            "--path".into(),
            path.into(),
            "--no-focus".into(),
        ],
    )?;
    let opened: serde_json::Value = serde_json::from_str(&out.stdout)?;
    if before.is_none()
        && let Some(anchor) = worktree::workspace(shell, checkout, &main)?
    {
        worktree::record_anchor(shell, checkout, &anchor)?;
    }
    let workspace_id = opened
        .pointer("/result/workspace/workspace_id")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string);
    if opened.pointer("/result/already_open") == Some(&serde_json::Value::Bool(true)) {
        return Err(Error::Invalid(format!(
            "Herdr workspace {} already shows {path}; a child may own it",
            workspace_id.as_deref().unwrap_or("?")
        ))
        .with_diagnostic(Diagnostic::new(
            Operation::Prepare,
            Cause::WorkspaceOccupied,
            Context::new(None, None, Some(path)).occupied(Occupancy::OpenWorkspace),
        )));
    }
    let pane_id = opened
        .pointer("/result/root_pane/pane_id")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string);
    if pane_id.is_none() {
        return Err(Error::Parse {
            origin: "herdr worktree open".into(),
            detail: out.stdout.trim().into(),
        });
    }
    let terminal_id = opened
        .pointer("/result/root_pane/terminal_id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Error::Parse {
            origin: "herdr worktree open".into(),
            detail: "created pane has no terminal identity".into(),
        })?
        .to_owned();
    Ok(Prepared {
        binding: None,
        pane_id,
        terminal_id: Some(terminal_id),
        workspace_id,
        path: Some(
            opened
                .pointer("/result/worktree/path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(path)
                .to_string(),
        ),
    })
}

/// One Git invocation in `dir`.
struct Git(Vec<String>);

fn git(dir: &Path, args: &[&str]) -> Git {
    Git(["-C".to_string(), dir.display().to_string()]
        .into_iter()
        .chain(args.iter().map(ToString::to_string))
        .collect())
}

impl Git {
    fn run(&self, shell: &dyn Shell) -> Result<crate::shell::Output> {
        shell.run("git", &self.0)
    }

    fn run_ok(&self, shell: &dyn Shell) -> Result<()> {
        shell.run_ok("git", &self.0).map(drop)
    }

    /// Making or removing a checkout can take longer than an ordinary call.
    fn setup_ok(&self, shell: &dyn Shell) -> Result<()> {
        shell.run_setup_ok("git", &self.0).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{Refusal, RetainedKind};
    use crate::hosts::Failure;
    use crate::shell::Fake;

    #[test]
    fn transferred_job_wire_requires_the_exact_execution_owner() {
        use serde_json::json;
        let tree = json!({"repo":"/repo","path":"/repo/tree","branch":"worker","head":"head",
            "identity":{"common":"/repo/.git","gitdir":"/repo/.git/worktrees/worker","generation":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}});
        let binding = json!({"request_id":"a","request":["/repo","worker",null],
            "claim":{"tree":tree,"request_id":"a"},
            "prepared":{"pane_id":"prepared-pane","terminal_id":"prepared-terminal","path":"/repo/tree"}});
        let validity = json!({"version":2,"created_ms":1});
        let launch = json!({"id":"a","preparation":binding,"action":{"type":"job","cmd":["true"],"timeout_ms":10,"idle_timeout_ms":20,"validity":validity},
            "host":"local","cwd":"/repo/tree","pane":null,"terminal":null,"workspace":null,"workspace_name":null,"owner":"owner"});
        let owner = json!({"id":"a","cmd":["true"],"cwd":"/repo/tree","host":"local","owner":"owner","pane":"job-pane","state":"pending",
            "timeout_ms":10,"idle_timeout_ms":20,"validity":validity,
            "startup":{"version":1,"preparation":binding,"scope":"git","checkout":tree}});
        let request = json!({"version":1,"binding":binding,"policy":{"kind":"transfer","launch":launch,"pane":"job-pane","job":owner},"outcome":"possibly_executed"});
        let launch = serde_json::from_value(launch).unwrap();
        let result = json!({"pane":"job-pane"});
        assert!(
            validate_transfer(
                &launch,
                &result,
                &serde_json::from_value(request.clone()).unwrap()
            )
            .is_ok()
        );
        for (field, different) in [
            ("id", json!("other")),
            ("cmd", json!(["false"])),
            ("cwd", json!("/other")),
            ("host", json!("other")),
            ("owner", json!("other")),
            ("pane", json!("other")),
            ("timeout_ms", json!(11)),
            ("idle_timeout_ms", json!(21)),
            ("validity", json!(null)),
            (
                "startup",
                json!({"version":99,"scope":"non_repository","cwd":"/repo/tree"}),
            ),
        ] {
            let mut changed = request.clone();
            changed["policy"]["job"][field] = different;
            assert!(
                validate_transfer(&launch, &result, &serde_json::from_value(changed).unwrap())
                    .is_err(),
                "{field}"
            );
        }
    }

    /// A shell that ends the base fetch at its deadline, and runs every other command
    /// from the scripted fake.
    #[derive(Debug)]
    struct DeadlineFetch(Fake);

    impl Shell for DeadlineFetch {
        fn run(&self, program: &str, args: &[String]) -> Result<crate::shell::Output> {
            let key = std::iter::once(program.to_string())
                .chain(args.iter().cloned())
                .collect::<Vec<_>>()
                .join(" ");
            if key.contains(" fetch ") {
                return Err(Error::Deadline("git".into()));
            }
            self.0.run(program, args)
        }
    }

    /// A fetch that hits the command deadline reports the deadline. The cause is not an
    /// unavailable base, because the base never got its chance to resolve.
    #[test]
    fn a_base_fetch_that_hits_the_deadline_reports_the_deadline() {
        let root = std::env::temp_dir().join(crate::ledger::new_id());
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let git = |args: &str| format!("git -C {} {args}", repo.display());
        let fake = Fake::new();
        fake.on("git check-ref-format --branch worker", "");
        fake.on("herdr workspace list", "");
        fake.on(
            &git("rev-parse --path-format=absolute --git-common-dir"),
            &format!("{}/.git", repo.display()),
        );
        fake.on(&git("worktree list --porcelain"), "");
        fake.on_output(
            &git("show-ref --verify --quiet refs/heads/worker"),
            crate::shell::Output {
                status: 1,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        fake.on_output(
            &git("rev-parse --verify --end-of-options origin/feature^{commit}"),
            crate::shell::Output {
                status: 1,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let shell = DeadlineFetch(fake);
        let ledger = crate::ledger::Ledger::new(root.join("state"));
        let error = local_once(
            &shell,
            &ledger,
            repo.to_str().unwrap(),
            "worker",
            Some("origin/feature"),
            Some("deadline"),
        )
        .unwrap_err();
        assert!(
            matches!(error.root(), Error::Deadline(_)),
            "the fetch deadline is reported as itself: {error}"
        );
        assert_ne!(
            error.diagnostic().map(|diagnostic| diagnostic.cause),
            Some(Cause::BaseUnavailable),
            "the deadline is not an unavailable base: {error}"
        );
        // The retained record keeps the deadline classification, so a replay of the same
        // request reports the deadline too.
        let retained = error.retained(Operation::Prepare, Some("deadline"));
        assert_eq!(retained.kind, RetainedKind::Deadline);
        assert!(matches!(retained.restore().root(), Error::Deadline(_)));
    }

    /// Only an `origin/<branch>` base names a remote branch this host may fetch, and the
    /// name must not read as an option or carry a path.
    #[test]
    fn only_an_origin_branch_base_names_a_fetchable_branch() {
        assert_eq!(remote_branch("origin/feature"), Some("feature"));
        assert_eq!(remote_branch("origin/feature/sub"), Some("feature/sub"));
        for base in [
            "main",
            "origin/",
            "origin/-x",
            "origin/a..b",
            "origin/a b",
            "origin/a^b",
            "upstream/feature",
            "origin/feature^{commit}",
        ] {
            assert_eq!(remote_branch(base), None, "{base}");
        }
    }

    /// A retained failure keeps its classification, so a replay is still a deadline, a
    /// refusal, or a transport failure, and not an opaque error.
    #[test]
    fn a_retained_failure_keeps_its_classification() {
        let deadline = Error::Deadline("ssh netcup: remote deadline".into()).with_diagnostic(
            Diagnostic::new(Operation::Prepare, Cause::SetupFailed, Context::default()),
        );
        let retained = deadline.retained(Operation::Prepare, Some("request-1"));
        assert_eq!(retained.kind, RetainedKind::Deadline);
        assert_eq!(retained.request_id.as_deref(), Some("request-1"));
        assert!(matches!(
            retained.restore().root(),
            Error::Deadline(_) | Error::Diagnostic { .. }
        ));
        let refusal = Error::Refused {
            code: Refusal::CloneFailed,
            detail: "the peer could not clone the repository".into(),
        };
        let retained = refusal.retained(Operation::Prepare, None);
        assert_eq!(retained.kind, RetainedKind::Refused);
        assert_eq!(retained.refusal, Some(Refusal::CloneFailed));
        assert!(matches!(
            retained.restore().root(),
            Error::Refused { code, .. } if *code == Refusal::CloneFailed
        ));
        let transport = Error::Ssh {
            target: "netcup".into(),
            failure: Failure::Timeout,
            refusal: None,
        };
        let retained = transport.retained(Operation::Prepare, None);
        assert_eq!(retained.kind, RetainedKind::Transport);
        assert_eq!(retained.transport, Some(Failure::Timeout));
        assert!(matches!(
            retained.restore().root(),
            Error::Ssh { failure, .. } if *failure == Failure::Timeout
        ));
    }

    /// The retained record never holds the original error text.
    #[test]
    fn a_retained_failure_never_holds_the_original_text() {
        let error = Error::Invalid("command argv with SECRET_TOKEN failed".into());
        let retained = error.retained(Operation::Prepare, Some("request-2"));
        let text = serde_json::to_string(&retained).unwrap();
        assert!(!text.contains("SECRET_TOKEN"), "{text}");
        assert!(retained.detail_withheld, "{text}");
        assert!(
            retained.restore().to_string().contains("withheld"),
            "{text}"
        );
    }

    /// A receipt holds one outcome. A receipt with both is refused, and a receipt with
    /// neither stays uncertain.
    #[test]
    fn a_receipt_holds_exactly_one_outcome() {
        let request = (PathBuf::from("/repo"), "worker".to_owned(), None);
        let prepared = Prepared {
            binding: None,
            pane_id: Some("w8:p1".into()),
            terminal_id: Some("term_tree".into()),
            workspace_id: None,
            path: Some("/repo/.worktrees/worker".into()),
        };
        let success = PreparationReceipt {
            version: None,
            binding: None,
            request: request.clone(),
            prepared: Some(prepared.clone()),
            failure: None,
        };
        assert_eq!(replay("r", &success).unwrap(), prepared);
        let failure = crate::error::RetainedFailure {
            operation: Operation::Prepare,
            diagnostic: None,
            kind: RetainedKind::Other,
            transport: None,
            refusal: None,
            request_id: Some("r".into()),
            detail_withheld: true,
        };
        let recorded = PreparationReceipt {
            version: None,
            binding: None,
            request: request.clone(),
            prepared: None,
            failure: Some(failure.clone()),
        };
        let error = replay("r", &recorded).unwrap_err();
        assert!(error.to_string().contains("withheld"), "{error}");
        let contradictory = PreparationReceipt {
            version: None,
            binding: None,
            request,
            prepared: Some(prepared),
            failure: Some(failure),
        };
        assert!(
            replay("r", &contradictory)
                .unwrap_err()
                .to_string()
                .contains("contradictory preparation receipt")
        );
        let uncertain = PreparationReceipt {
            version: None,
            binding: None,
            request: (PathBuf::from("/repo"), "worker".to_owned(), None),
            prepared: None,
            failure: None,
        };
        assert!(
            replay("r", &uncertain)
                .unwrap_err()
                .to_string()
                .contains("stopped during preparation")
        );
    }
}
