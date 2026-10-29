//! Validated worker-local activation of an immutable exported contract.
use crate::validation::{draft, error, nonempty};
use crate::{
    Assignment, Change, ContractDraft, ExecutionRef, Status, Task, TaskBinding, TaskExport,
    TodoError, TodoList, Unit, UnitRef, validate,
};
use serde::{Deserialize, Serialize};

/// Executor-owned transport; task semantics are validated here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(deny_unknown_fields)]
pub struct Seed {
    pub version: u32,
    pub execution: ExecutionRef,
    pub binding: TaskBinding,
    pub exported: TaskExport,
}

/// Immutable local completion evidence. Pi derives the containing entry ID on replay.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Seal {
    pub entry_id: String,
    pub disposition: Status,
}

/// Frozen import and its local session, not an executor lifecycle record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportedUnit {
    pub seed: Seed,
    pub local_session_id: String,
    pub seal: Option<Seal>,
}

/// Provider acknowledgement, including closure after activation ends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportReceipt {
    pub version: u32,
    pub execution: ExecutionRef,
    pub unit_ref: UnitRef,
    pub digest: String,
    pub local_session_id: String,
    pub seal: Option<Seal>,
}

fn contract_draft(seed: &Seed) -> ContractDraft {
    let contract = &seed.exported.contract;
    ContractDraft {
        deliverable: Some(contract.deliverable.clone()),
        scope: Some(contract.scope.clone()),
        acceptance: Some(contract.acceptance.clone()),
        depends_on: contract.depends_on.clone(),
        assignment: Some(contract.assignment.clone()),
    }
}

impl Seed {
    fn validate(&self) -> Result<(), TodoError> {
        if self.version != 1 {
            return Err(error("VERSION_UNSUPPORTED: expected seed version 1"));
        }
        self.execution.validate()?;
        let export = TaskExport::parse(&self.exported.json()?)?;
        let contract = &export.contract;
        let reference = &contract.r#ref;
        nonempty(&reference.session_id)?;
        nonempty(&reference.unit_id)?;
        nonempty(&reference.contract_entry_id)?;
        nonempty(&contract.title)?;
        draft(&contract_draft(self))?;
        if reference.revision == 0
            || !matches!(contract.assignment, Assignment::Worker { .. })
            || self.binding.contract != *reference
            || self.binding.execution != self.execution
            || self.binding.export_digest != export.digest
        {
            return Err(error(
                "BINDING_MISMATCH: seed correlation or assignment differs",
            ));
        }
        let mut seen = std::collections::HashSet::new();
        if contract.depends_on.len() != export.prerequisites.len()
            || contract
                .depends_on
                .iter()
                .zip(&export.prerequisites)
                .any(|(id, evidence)| {
                    id == &reference.unit_id
                        || id != &evidence.unit_id
                        || evidence.completed_revision == 0
                        || !seen.insert(id)
                })
        {
            return Err(error(
                "DEPENDENCY_OPEN: frozen prerequisite evidence differs",
            ));
        }
        Ok(())
    }
}

/// Parses and validates a transport envelope without trusting its digest alone.
///
/// # Errors
/// Rejects malformed, unsupported or mismatched imports.
pub fn parse_seed(input: &str) -> Result<Seed, TodoError> {
    let seed: Seed =
        serde_json::from_str(input).map_err(|cause| error(&format!("IMPORT_FAILED: {cause}")))?;
    seed.validate()?;
    Ok(seed)
}

/// Imports exactly one contract into a fresh local branch. Replay never resets progress.
///
/// # Errors
/// Rejects another repository, execution, session or existing task state.
pub fn import_unit(
    list: TodoList,
    seed: Seed,
    session_id: &str,
    repository_id: &str,
) -> Result<Change, TodoError> {
    validate(&list)?;
    seed.validate()?;
    nonempty(session_id)?;
    if seed.exported.contract.scope.repository_id != repository_id {
        return Err(error("IMPORT_FAILED: seed belongs to another repository"));
    }
    if let Some(imported) = list
        .tasks
        .first()
        .and_then(|task| task.unit.as_ref())
        .and_then(|unit| unit.imported.as_ref())
    {
        if imported.seed != seed || imported.local_session_id != session_id {
            return Err(error("BINDING_MISMATCH: another import owns this session"));
        }
        return Ok(Change {
            list,
            persist: false,
        });
    }
    if !list.tasks.is_empty() {
        return Err(error(
            "SEEDED_SCOPE_LOCKED: import requires an empty task branch",
        ));
    }
    let contract = &seed.exported.contract;
    let title = contract.title.clone();
    let unit = Unit {
        reference: contract.r#ref.clone(),
        draft: contract_draft(&seed),
        local_session_id: Some(session_id.to_owned()),
        acceptance_evidence: None,
        binding: None,
        frozen_export_json: Some(seed.exported.json()?),
        evidence: vec![],
        accepted_events: vec![],
        attempted_revision: None,
        imported: Some(ImportedUnit {
            seed,
            local_session_id: session_id.to_owned(),
            seal: None,
        }),
        released: vec![],
    };
    let list = TodoList {
        tasks: vec![Task {
            content: title,
            description: None,
            status: Status::InProgress,
            subtasks: vec![],
            unit: Some(unit),
        }],
    };
    validate(&list)?;
    Ok(Change {
        list,
        persist: true,
    })
}

pub(crate) fn validate_import(
    unit: &Unit,
    title: &str,
    status: Status,
    leaf: bool,
) -> Result<(), TodoError> {
    let Some(imported) = &unit.imported else {
        return Ok(());
    };
    imported.seed.validate()?;
    nonempty(&imported.local_session_id)?;
    let frozen = imported.seed.exported.json()?;
    if !leaf
        || unit.contract(title)? != imported.seed.exported.contract
        || unit.binding.is_some()
        || !unit.evidence.is_empty()
        || !unit.accepted_events.is_empty()
        || unit.attempted_revision.is_some()
        || unit.frozen_export_json.as_deref() != Some(frozen.as_str())
    {
        return Err(error("BINDING_MISMATCH: imported contract changed"));
    }
    let valid = imported.seal.as_ref().map_or_else(
        || {
            status == Status::InProgress
                && unit.local_session_id.as_deref() == Some(&imported.local_session_id)
        },
        |seal| {
            matches!(seal.disposition, Status::Completed | Status::Abandoned)
                && seal.disposition == status
                && unit.local_session_id.is_none()
        },
    );
    if !valid {
        return Err(error(
            "SEEDED_SCOPE_LOCKED: invalid worker activation or seal",
        ));
    }
    Ok(())
}

/// Reads the validated import receipt even after worker-local completion.
///
/// # Errors
/// Rejects malformed stored task data.
pub fn imported(list: &TodoList, session_id: &str) -> Result<Option<ImportReceipt>, TodoError> {
    validate(list)?;
    Ok(list
        .tasks
        .first()
        .and_then(|task| task.unit.as_ref())
        .and_then(|unit| unit.imported.as_ref())
        .filter(|imported| imported.local_session_id == session_id)
        .map(|imported| ImportReceipt {
            version: 1,
            execution: imported.seed.execution.clone(),
            unit_ref: imported.seed.binding.contract.clone(),
            digest: imported.seed.binding.export_digest.clone(),
            local_session_id: imported.local_session_id.clone(),
            seal: imported.seal.clone(),
        }))
}
