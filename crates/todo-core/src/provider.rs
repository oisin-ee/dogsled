//! Atomic task capability operations; no executor calls or runtime state.
use crate::selection::{Selected, selected};
use crate::validation::{error, nonempty, prerequisites, units};
use crate::{
    AcceptedEvent, Assignment, BoundResult, ExecutionRef, Operation, Prerequisite, Request,
    Settlement, Status, TaskBinding, TaskExport, TodoError, TodoList, Unit, UnitContract, UnitRef,
    UnstartedResult, validate,
};
use serde::{Deserialize, Serialize};

/// Parses untrusted extension input with strict JSON unknown-field validation.
///
/// # Errors
/// Rejects malformed claim fields.
pub fn parse_claim(input: &str) -> Result<ClaimInput, TodoError> {
    serde_json::from_str(input)
        .map_err(|error| crate::validation::error(&format!("CONTRACT_INCOMPLETE: {error}")))
}

/// Parses settlement without weakening tagged result validation at the WASM boundary.
///
/// # Errors
/// Rejects malformed settlement fields.
pub fn parse_settlement(input: &str) -> Result<Settlement, TodoError> {
    serde_json::from_str(input)
        .map_err(|error| crate::validation::error(&format!("BINDING_MISMATCH: {error}")))
}

/// Optimistic claim identity, supplied by a cooperating extension.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(deny_unknown_fields)]
pub struct ClaimInput {
    pub expected: UnitRef,
    pub execution: ExecutionRef,
}

/// Exactly the binding and export persisted in the claim snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(deny_unknown_fields)]
pub struct ClaimResult {
    pub binding: TaskBinding,
    pub exported: TaskExport,
}

/// Whether settlement changed this branch.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(deny_unknown_fields)]
pub struct SettlementResult {
    pub applied: bool,
}

/// Atomic claim snapshot and its exported result.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct ClaimChange {
    pub list: TodoList,
    pub persist: bool,
    pub value: ClaimResult,
}

/// Concrete WASM settlement response.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct SettlementChange {
    pub list: TodoList,
    pub persist: bool,
    pub value: SettlementResult,
}

/// Validated local activation, not a worker-state projection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActiveUnit {
    pub r#ref: UnitRef,
    pub local_session_id: String,
    pub seeded: bool,
}

/// Guards held contracts and leased local calls against replacement or closure.
///
/// # Errors
/// Rejects changes affecting a protected revision or its completion state.
pub fn protect(before: &TodoList, after: &TodoList, leases: &[UnitRef]) -> Result<(), TodoError> {
    for (unit, status, leaf) in units(before) {
        if leases.contains(&unit.reference) {
            let Ok((_, next, next_status, next_leaf)) = entry(after, &unit.reference.unit_id)
            else {
                return Err(error(
                    "UNIT_ALREADY_BOUND: protected work cannot be replaced",
                ));
            };
            if unit != next || status != next_status || leaf != next_leaf {
                return Err(error(
                    "UNIT_ALREADY_BOUND: held or executing call protects this contract",
                ));
            }
        }
    }
    Ok(())
}

fn select<'a>(list: &'a mut TodoList, id: &str) -> Result<Selected<'a>, TodoError> {
    selected(
        list,
        &Request {
            op: Operation::View,
            task: Some(id.to_owned()),
            parent: None,
            items: None,
            contract: None,
            start: None,
            evidence: None,
            description: None,
        },
    )
    .map_err(|cause| error(&format!("UNIT_NOT_FOUND: {cause}")))
}

fn entry<'a>(list: &'a TodoList, id: &str) -> Result<(&'a str, &'a Unit, Status, bool), TodoError> {
    for task in &list.tasks {
        if let Some(unit) = &task.unit
            && unit.reference.unit_id == id
        {
            return Ok((&task.content, unit, task.status, task.subtasks.is_empty()));
        }
        for child in &task.subtasks {
            if let Some(unit) = &child.unit
                && unit.reference.unit_id == id
            {
                return Ok((&child.content, unit, child.status, true));
            }
        }
    }
    Err(error("UNIT_NOT_FOUND: no unit on the current branch"))
}

/// Reads a complete contract from this branch without freezing it.
///
/// # Errors
/// Rejects unknown IDs or incomplete contracts.
pub fn inspect(list: &TodoList, id: &str) -> Result<UnitContract, TodoError> {
    validate(list)?;
    nonempty(id)?;
    let (title, unit, _, _) = entry(list, id)?;
    unit.contract(title)
}

/// Returns only this session's executing local leaf.
///
/// # Errors
/// Rejects malformed branch snapshots.
pub fn active(list: &TodoList, session_id: &str) -> Result<Option<ActiveUnit>, TodoError> {
    validate(list)?;
    Ok(units(list).into_iter().find_map(|(unit, status, leaf)| {
        (leaf
            && status == Status::InProgress
            && unit.binding.is_none()
            && unit.local_session_id.as_deref() == Some(session_id))
        .then(|| ActiveUnit {
            r#ref: unit.reference.clone(),
            local_session_id: session_id.to_owned(),
            seeded: unit.imported.is_some(),
        })
    }))
}

fn matching_origin(unit: &Unit, expected: &UnitRef, session_id: &str) -> Result<(), TodoError> {
    if unit.reference != *expected
        || expected.session_id != session_id
        || expected.contract_entry_id.is_empty()
    {
        return Err(error(
            "STALE_ORIGIN: revision is not on the active origin branch",
        ));
    }
    Ok(())
}

fn frozen(unit: &Unit, execution: &ExecutionRef) -> Result<Option<ClaimResult>, TodoError> {
    if let Some(binding) = &unit.binding {
        if binding.execution != *execution {
            return Err(error(
                "UNIT_ALREADY_BOUND: another execution holds the contract",
            ));
        }
        let json = unit
            .frozen_export_json
            .as_ref()
            .ok_or_else(|| error("BINDING_MISMATCH: missing frozen export"))?;
        return Ok(Some(ClaimResult {
            binding: binding.clone(),
            exported: TaskExport::parse(json)?,
        }));
    }
    if unit.attempted_revision == Some(unit.reference.revision) {
        return Err(error(
            "UNIT_ALREADY_BOUND: update to a new revision before another execution",
        ));
    }
    Ok(None)
}

fn claimed_result(
    list: &TodoList,
    input: &ClaimInput,
    session_id: &str,
) -> Result<ClaimResult, TodoError> {
    let (title, unit, status, leaf) = entry(list, &input.expected.unit_id)?;
    matching_origin(unit, &input.expected, session_id)?;
    if let Some(result) = frozen(unit, &input.execution)? {
        return Ok(result);
    }
    if !leaf || !status.is_open() {
        return Err(error(
            "WRONG_ASSIGNMENT: claim requires an open executable leaf",
        ));
    }
    let contract = unit.contract(title)?;
    if !matches!(contract.assignment, Assignment::Worker { .. }) || unit.local_session_id.is_some()
    {
        return Err(error(
            "WRONG_ASSIGNMENT: claim requires a worker assignment",
        ));
    }
    prerequisites(list, unit)?;
    let dependencies = unit
        .draft
        .depends_on
        .iter()
        .map(|id| {
            let (_, other, _, _) = entry(list, id)?;
            Ok(Prerequisite {
                unit_id: id.clone(),
                completed_revision: other.reference.revision,
            })
        })
        .collect::<Result<Vec<_>, TodoError>>()?;
    let exported = TaskExport::create(contract, dependencies)?;
    let binding = TaskBinding {
        contract: input.expected.clone(),
        execution: input.execution.clone(),
        export_digest: exported.digest.clone(),
    };
    Ok(ClaimResult { binding, exported })
}

/// Freezes contract, prerequisite evidence and binding in one snapshot.
///
/// # Errors
/// Rejects stale origins, competing claims, incomplete contracts and scope conflicts.
pub fn claim(
    mut list: TodoList,
    input: &ClaimInput,
    session_id: &str,
) -> Result<ClaimChange, TodoError> {
    validate(&list)?;
    input.execution.validate()?;
    let value = claimed_result(&list, input, session_id)?;
    let Selected { unit, status, .. } = select(&mut list, &input.expected.unit_id)?;
    let unit = unit.as_mut().ok_or_else(|| error("UNIT_NOT_FOUND"))?;
    if unit.binding.is_some() {
        return Ok(ClaimChange {
            list,
            persist: false,
            value,
        });
    }
    unit.frozen_export_json = Some(value.exported.json()?);
    unit.binding = Some(value.binding.clone());
    unit.attempted_revision = Some(unit.reference.revision);
    *status = Status::InProgress;
    for parent in &mut list.tasks {
        if parent.subtasks.iter().any(|child| {
            child
                .unit
                .as_ref()
                .is_some_and(|unit| unit.binding.is_some())
        }) {
            parent.status = Status::InProgress;
        }
    }
    validate(&list)?;
    Ok(ClaimChange {
        list,
        persist: true,
        value,
    })
}

fn validate_result(input: &Settlement, identity: &AcceptedEvent) -> Result<(), TodoError> {
    let evidence = match input {
        Settlement::Bound {
            result: BoundResult::Returned { evidence },
            ..
        } => Some(evidence),
        Settlement::Bound {
            result: BoundResult::Stopped { evidence, reason },
            ..
        } => {
            nonempty(reason)?;
            evidence.as_ref()
        }
        Settlement::NotStarted {
            result: UnstartedResult::NotStarted { reason },
            ..
        } => {
            nonempty(reason)?;
            None
        }
    };
    if let Some(evidence) = evidence {
        evidence.execution.validate()?;
        nonempty(&evidence.result_id)?;
        nonempty(&evidence.summary)?;
        if evidence.execution != identity.execution || evidence.summary.chars().count() > 4096 {
            return Err(error(
                "BINDING_MISMATCH: result evidence does not match execution or exceeds 4096 characters",
            ));
        }
    }
    Ok(())
}

fn release(
    unit: &mut Unit,
    status: &mut Status,
    input: &Settlement,
    identity: AcceptedEvent,
) -> Result<bool, TodoError> {
    let Some(binding) = &unit.binding else {
        if matches!(input, Settlement::NotStarted { .. }) {
            return Ok(false);
        }
        return Err(error("BINDING_MISMATCH: no matching held binding"));
    };
    if (&binding.execution, &binding.contract) != (&identity.execution, &identity.origin) {
        return Err(error(
            "BINDING_MISMATCH: another execution holds this revision",
        ));
    }
    let evidence = match input {
        Settlement::Bound {
            binding: expected,
            result,
            ..
        } => {
            if binding != expected {
                return Err(error("BINDING_MISMATCH: digest or execution differs"));
            }
            match result {
                BoundResult::Returned { evidence } => {
                    *status = Status::InProgress;
                    Some(evidence)
                }
                BoundResult::Stopped { evidence, .. } => {
                    *status = Status::Pending;
                    evidence.as_ref()
                }
            }
        }
        Settlement::NotStarted { .. } => {
            *status = Status::Pending;
            None
        }
    };
    if let Some(evidence) = evidence {
        if let Some(old) = unit
            .evidence
            .iter()
            .find(|old| old.execution == evidence.execution && old.result_id == evidence.result_id)
        {
            if old != evidence {
                return Err(error(
                    "BINDING_MISMATCH: result identity has contradictory evidence",
                ));
            }
        } else {
            unit.evidence.push(evidence.clone());
        }
    }
    unit.accepted_events.push(identity);
    unit.binding = None;
    Ok(true)
}

/// Attaches correlated evidence and releases a hold; never accepts work.
///
/// # Errors
/// Rejects another origin, execution or digest without partial mutation.
pub fn settle(
    mut list: TodoList,
    input: &Settlement,
    session_id: &str,
) -> Result<SettlementChange, TodoError> {
    validate(&list)?;
    let identity = input.identity()?;
    validate_result(input, &identity)?;
    if identity.origin.session_id != session_id {
        return Err(error("STALE_ORIGIN: another session owns this result"));
    }
    if units(&list)
        .iter()
        .any(|(unit, _, _)| unit.imported.is_some())
    {
        return Err(error(
            "SEEDED_SCOPE_LOCKED: worker cannot settle its own execution",
        ));
    }
    let (_, unit, _, _) = entry(&list, &identity.origin.unit_id)
        .map_err(|cause| error(&format!("STALE_ORIGIN: {cause}")))?;
    if unit.accepted_events.contains(&identity) {
        return Ok(SettlementChange {
            list,
            persist: false,
            value: SettlementResult { applied: false },
        });
    }
    matching_origin(unit, &identity.origin, session_id)?;
    if unit.binding.is_none() && unit.released.contains(&identity.execution) {
        return Ok(SettlementChange {
            list,
            persist: false,
            value: SettlementResult { applied: false },
        });
    }
    let Selected { unit, status, .. } = select(&mut list, &identity.origin.unit_id)?;
    let unit = unit.as_mut().ok_or_else(|| error("STALE_ORIGIN"))?;
    let applied = release(unit, status, input, identity)?;
    validate(&list)?;
    Ok(SettlementChange {
        list,
        persist: applied,
        value: SettlementResult { applied },
    })
}
