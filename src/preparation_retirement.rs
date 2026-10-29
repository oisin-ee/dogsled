//! Request-scoped preparation ownership retirement, independent of execution evidence.

use crate::error::Result;
use crate::hosts::{Fleet, Host};
use crate::ledger::{Ledger, new_id, now_ms};
use crate::prepare::disposition::{self, Basis, Outcome, Policy};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub host: String,
    pub user: String,
    pub pane: Option<String>,
    pub session: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Authority {
    Cleanup,
    Custody { record: crate::custody::Custody },
    Owner { item: Value },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Audit {
    pub operation_id: String,
    pub request_id: String,
    pub host: Host,
    pub actor: Actor,
    pub authority: Authority,
    pub reason: String,
    pub recorded_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Intent {
    version: u32,
    request: Option<Value>,
    disposition: disposition::Request,
    acknowledgement: Option<disposition::Acknowledgement>,
    error: Option<String>,
}

fn conflict(message: &str) -> crate::error::Error {
    crate::unit_transport::conflict(message)
}

fn audit(intent: &Intent) -> Result<&Audit> {
    let Policy::Prohibit {
        basis: Basis::Retired { audit, .. },
    } = &intent.disposition.policy
    else {
        return Err(conflict("not a retirement intent"));
    };
    if intent.version != 1
        || intent.disposition.version != disposition::VERSION
        || intent.disposition.outcome != Outcome::Unknown
        || audit.request_id != intent.disposition.binding.request_id
        || audit.request_id != intent.disposition.binding.claim.request_id
    {
        return Err(conflict("unsupported or mismatched retirement intent"));
    }
    crate::jobs::validate_id(&audit.request_id)?;
    crate::jobs::validate_id(&audit.operation_id)?;
    Ok(audit)
}

pub fn validate(audit: &Audit, request: &disposition::Request) -> Result<()> {
    crate::jobs::validate_id(&audit.operation_id)?;
    if request.outcome != Outcome::Unknown
        || audit.reason.trim().is_empty()
        || audit.actor.host.is_empty()
        || audit.actor.user.is_empty()
        || audit.request_id != request.binding.request_id
    {
        return Err(conflict("retirement audit correlation changed"));
    }
    Ok(())
}

fn actor(fleet: &Fleet) -> Actor {
    let pane = std::env::var("HERDR_PANE_ID").ok();
    let session = pane.as_ref().and_then(|pane| {
        crate::probe::agent_list(fleet.shell)
            .ok()?
            .into_iter()
            .find(|agent| &agent.pane == pane)?
            .agent_session
    });
    Actor {
        host: crate::inbox::hostname(),
        user: nix::unistd::geteuid().to_string(),
        pane,
        session,
    }
}

/// Called only under admission → custody. A pane without historical session provenance
/// is not proof: cleanup authority must be explicit in that case.
fn authority(
    ledger: &Ledger,
    id: &str,
    actor: &Actor,
    cleanup: bool,
    custody: Option<&crate::custody::Custody>,
) -> Result<Authority> {
    if cleanup {
        return Ok(Authority::Cleanup);
    }
    let pane = actor
        .pane
        .as_deref()
        .ok_or_else(|| conflict("retirement requires a live caller"))?;
    let session = actor
        .session
        .as_deref()
        .ok_or_else(|| conflict("missing caller session; use explicit CLI cleanup authority"))?;
    if let Some(record) = custody {
        if record.host != actor.host
            || record.custodian_session.as_deref() != Some(session)
            || !record.held_by(pane, Some(session))
        {
            return Err(conflict("retirement caller is not the current custodian"));
        }
        return Ok(Authority::Custody {
            record: record.clone(),
        });
    }
    let item = crate::items::list(ledger)?
        .into_iter()
        .find(|item| item.id == id)
        .ok_or_else(|| conflict("missing owner provenance; use explicit CLI cleanup authority"))?;
    if item.owner_pane != pane || item.owner_session.as_deref() != Some(session) {
        return Err(conflict("retirement caller is not the recorded owner"));
    }
    let admission = ledger
        .child("admissions")
        .read::<Value>(&format!("{id}.json"))?
        .ok_or_else(|| conflict("missing origin host provenance"))?;
    if admission["request"]["owner_host"].as_str() != Some(actor.host.as_str()) {
        return Err(conflict("retirement owner host changed"));
    }
    Ok(Authority::Owner {
        item: serde_json::to_value(item)?,
    })
}

/// The origin intent, not a terminal ticket, fences every new caller dispatch.
/// This function never supplies execution or no-launch evidence.
pub fn prohibited(ledger: &Ledger, request: &crate::work::Request) -> Result<bool> {
    let Some(intent) = ledger
        .child("preparation-retirements")
        .read::<Intent>(&format!("{}.json", request.id))?
    else {
        return Ok(false);
    };
    let captured = audit(&intent)?;
    if captured.request_id != request.id
        || intent
            .request
            .as_ref()
            .is_some_and(|saved| serde_json::to_value(request).as_ref().ok() != Some(saved))
    {
        return Err(conflict("retirement originating request changed"));
    }
    Ok(true)
}

/// CLI alone may pass cleanup=true. MCP deliberately has no override argument.
pub fn retire(
    fleet: &Fleet,
    id: &str,
    selected: Option<&str>,
    reason: &str,
    cleanup: bool,
) -> Result<Value> {
    crate::jobs::validate_id(id)?;
    if reason.trim().is_empty() {
        return Err(conflict("retirement requires a nonempty reason"));
    }
    fleet.ledger.child("admissions").with_lock(id, || {
        let store = fleet.ledger.child("preparation-retirements");
        let file = format!("{id}.json");
        let mut intent = if let Some(saved) = store.read::<Intent>(&file)? {
            let captured = audit(&saved)?;
            if captured.request_id != id
                || captured.reason != reason
                || selected.is_some_and(|host| fleet.find(host).ok() != Some(&captured.host))
            {
                return Err(conflict(
                    "retirement expectation changed; replay the saved operation",
                ));
            }
            saved
        } else {
            let admission = fleet.ledger.child("admissions").read::<Value>(&file)?;
            let expected_request = admission.as_ref().and_then(|a| a.get("request")).cloned();
            let host = select_host(fleet, admission.as_ref(), selected)?;
            let (actor, captured_authority) =
                crate::custody::locked(fleet.ledger, id, |custody| {
                    let actor = actor(fleet);
                    let authority = authority(fleet.ledger, id, &actor, cleanup, custody)?;
                    Ok((actor, authority))
                })?;
            // Custody is never held over host RPC. Compare the original authority again
            // when saving the intent; never replace it with newly observed custody.
            crate::work::launch_test_barrier("RETIREMENT_CAPTURED")?;
            let historical = crate::prepare::lookup_historical(fleet, &host, id)?;
            validate_origin(fleet, admission.as_ref(), &host, &historical.binding)?;
            let disposition = if let Some(existing) = historical.disposition {
                if !cleanup
                    || !matches!(
                        existing.request.policy,
                        Policy::Prohibit {
                            basis: Basis::Retired { .. }
                        }
                    )
                {
                    return Err(conflict(
                        "historical preparation already has a conflicting disposition",
                    ));
                }
                let Policy::Prohibit {
                    basis: Basis::Retired { audit, .. },
                } = &existing.request.policy
                else {
                    return Err(conflict("not a retirement disposition"));
                };
                if audit.host != host || audit.reason != reason {
                    return Err(conflict("saved retirement host or reason changed"));
                }
                existing.request
            } else {
                disposition::Request {
                    version: disposition::VERSION,
                    binding: historical.binding,
                    policy: Policy::Prohibit {
                        basis: Basis::Retired {
                            audit: Box::new(Audit {
                                operation_id: new_id(),
                                request_id: id.into(),
                                host,
                                actor: actor.clone(),
                                authority: captured_authority.clone(),
                                reason: reason.into(),
                                recorded_ms: now_ms(),
                            }),
                            provenance: historical.provenance,
                        },
                    },
                    outcome: Outcome::Unknown,
                }
            };
            let saved = Intent {
                version: 1,
                request: expected_request,
                disposition,
                acknowledgement: None,
                error: None,
            };
            crate::custody::locked(fleet.ledger, id, |custody| {
                if self::actor(fleet) != actor
                    || authority(fleet.ledger, id, &actor, cleanup, custody)? != captured_authority
                {
                    return Err(conflict("retirement custody changed during capture"));
                }
                store.write(&file, &saved)
            })?;
            crate::unit_request::crash_point("retirement-intent-saved");
            saved
        };
        crate::work::launch_test_barrier("RETIREMENT_BEFORE_DISPOSE")?;
        finish(fleet, &mut intent)?;
        result(&intent)
    })
}

fn select_host(fleet: &Fleet, admission: Option<&Value>, selected: Option<&str>) -> Result<Host> {
    let attempt = admission
        .cloned()
        .map(serde_json::from_value::<crate::work::Attempt>)
        .transpose()?;
    let mut recorded = attempt.as_ref().and_then(|a| a.host.clone());
    if let Some(attempt) = &attempt
        && let Some(progress) = &attempt.placement
    {
        let request = serde_json::from_value(attempt.request.clone())?;
        if let Some(placement) = progress.expectation(&request)? {
            if recorded
                .as_ref()
                .is_some_and(|host| *host != placement.envelope.host)
            {
                return Err(conflict("placement retirement host changed"));
            }
            recorded = Some(placement.envelope.host);
        }
    }
    let host = if let Some(selected) = selected {
        fleet.find(selected)?.clone()
    } else {
        recorded.clone().unwrap_or_else(Host::local)
    };
    if recorded.as_ref().is_some_and(|recorded| recorded != &host) || *fleet.find(&host.id)? != host
    {
        return Err(conflict("retirement selected host changed"));
    }
    Ok(host)
}

fn validate_origin(
    fleet: &Fleet,
    admission: Option<&Value>,
    host: &Host,
    binding: &disposition::Binding,
) -> Result<()> {
    let Some(admission) = admission else {
        return Ok(());
    };
    let attempt: crate::work::Attempt = serde_json::from_value(admission.clone())?;
    let request: crate::work::Request = serde_json::from_value(attempt.request)?;
    if request.id != binding.request_id
        || request.branch.as_ref() != Some(&binding.request.1)
        || request.base != binding.request.2
        || request.repository.as_ref().is_some_and(|repo| {
            host.is_local()
                && !crate::worktree::same(&repo.path, &binding.request.0.to_string_lossy())
        })
        || attempt
            .launch
            .as_ref()
            .and_then(|launch| launch.preparation.as_deref())
            .is_some_and(|saved| saved != binding)
    {
        return Err(conflict(
            "retirement historical preparation does not match its origin",
        ));
    }
    if let Some(query) =
        fleet
            .ledger
            .child("preparation-expectations")
            .read::<crate::prepare::PreparationLookup>(&format!("{}.json", request.id))?
        && (serde_json::to_value(query.request)? != serde_json::to_value(request)?
            || query.host != *host
            || query.binding.as_ref().is_some_and(|saved| saved != binding))
    {
        return Err(conflict("retirement preparation expectation changed"));
    }
    Ok(())
}

fn finish(fleet: &Fleet, intent: &mut Intent) -> Result<()> {
    let captured = audit(intent)?.clone();
    let current = fleet
        .ledger
        .child("admissions")
        .read::<Value>(&format!("{}.json", captured.request_id))?
        .and_then(|a| a.get("request").cloned());
    if current != intent.request {
        return Err(conflict("retirement originating request changed"));
    }
    if captured.host != *fleet.find(&captured.host.id)? {
        return Err(conflict("retirement transport host changed"));
    }
    if let Some(ack) = &intent.acknowledgement {
        crate::prepare::validate_ack(&intent.disposition, ack)?;
        return Ok(());
    }
    match crate::prepare::dispose(fleet, &captured.host, &intent.disposition) {
        Ok(ack) => {
            intent.acknowledgement = Some(ack);
            intent.error = None;
        }
        Err(error) => {
            intent.error = Some(error.to_string());
        }
    }
    fleet
        .ledger
        .child("preparation-retirements")
        .write(&format!("{}.json", captured.request_id), intent)?;
    crate::unit_request::crash_point("retirement-acknowledgement-saved");
    Ok(())
}

fn result(intent: &Intent) -> Result<Value> {
    let captured = audit(intent)?;
    let released = intent.acknowledgement.is_some();
    Ok(json!({
        "resource":"preparation", "status":if released { "retired" } else { "pending" },
        "outcome":"unknown", "future_dispatch":"prohibited",
        "ownership":if released { "released" } else { "pending" }, "task":"unresolved",
        "request_id":captured.request_id, "host":captured.host,
        "receipt_id":captured.operation_id, "audit":captured, "error":intent.error,
    }))
}

/// Service replay uses only saved authority and exact envelopes. One damaged operation
/// never blocks unrelated requests; its durable fence and visible error remain.
pub fn recover(fleet: &Fleet) -> Result<()> {
    let store = fleet.ledger.child("preparation-retirements");
    let entries = match std::fs::read_dir(&store.dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let file = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = file.strip_suffix(".json") else {
            continue;
        };
        if crate::jobs::validate_id(id).is_err() {
            continue;
        }
        let repaired = fleet.ledger.child("admissions").with_lock(id, || {
            let Some(mut saved) = store.read::<Intent>(&file)? else {
                return Ok(());
            };
            if audit(&saved)?.request_id != id {
                return Err(conflict("retirement record identity changed"));
            }
            finish(fleet, &mut saved)
        });
        if let Err(error) = repaired {
            eprintln!("preparation retirement {id} remains pending: {error}");
        }
    }
    Ok(())
}
