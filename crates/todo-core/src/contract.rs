//! Executable task data. Runtime launch facts do not belong in this module.
use serde::{Deserialize, Serialize};

/// Branch provenance of one immutable contract revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnitRef {
    pub session_id: String,
    pub unit_id: String,
    pub revision: u32,
    /// Empty only until the containing Pi entry has been appended and read.
    pub contract_entry_id: String,
}

/// Desired executor, not a process or a model override.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Assignment {
    #[serde(rename = "self")]
    Self_,
    Worker {
        profile: String,
    },
}

/// Repository-relative declared scope, not a filesystem permission grant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    pub repository_id: String,
    pub owns: Vec<String>,
    pub reads: Vec<String>,
    pub generates: Vec<String>,
}

/// Evidence requirement. Creating a command does not execute it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Acceptance {
    Command {
        cwd: String,
        argv: Vec<String>,
        expected: String,
    },
    Review {
        artifact: String,
        criterion: String,
    },
}

/// Author-editable fields. A draft can omit fields but cannot supply invalid values.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContractDraft {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deliverable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Scope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<Acceptance>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignment: Option<Assignment>,
}

/// Complete contract projection used at the component boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnitContract {
    pub r#ref: UnitRef,
    pub title: String,
    pub deliverable: String,
    pub scope: Scope,
    pub acceptance: Acceptance,
    pub depends_on: Vec<String>,
    pub assignment: Assignment,
}

/// Opaque executor identity; no process interpretation belongs here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionRef {
    pub executor: String,
    pub owner_id: String,
    pub request_id: String,
}

impl ExecutionRef {
    pub(crate) fn validate(&self) -> Result<(), crate::TodoError> {
        if self.executor != "dogsled" {
            return Err(crate::validation::error(
                "BINDING_MISMATCH: unsupported executor",
            ));
        }
        crate::validation::nonempty(&self.owner_id)?;
        crate::validation::nonempty(&self.request_id)
    }
}

/// Stored task metadata, independent of process execution state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Unit {
    pub reference: UnitRef,
    pub draft: ContractDraft,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance_evidence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<crate::TaskBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_export_json: Option<String>,
    #[serde(default)]
    pub evidence: Vec<crate::EvidenceRef>,
    #[serde(default)]
    pub accepted_events: Vec<crate::AcceptedEvent>,
    /// The last claimed revision cannot launch again after settlement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempted_revision: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported: Option<crate::ImportedUnit>,
    /// Executions whose binding the owning lead released by closing the unit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub released: Vec<ExecutionRef>,
}

/// Trusted runtime identity, separate from agent-authored tool parameters.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MutationContext {
    pub session_id: String,
    pub mutation_id: String,
}

impl Unit {
    /// Compact contract detail shared by the model and terminal presentations.
    #[must_use]
    pub fn summary(&self) -> String {
        let assignment = match &self.draft.assignment {
            Some(Assignment::Self_) => "self".to_owned(),
            Some(Assignment::Worker { profile }) => format!("worker · {profile}"),
            None => "draft · assignment missing".to_owned(),
        };
        let scope = self.draft.scope.as_ref().map_or_else(
            || "scope missing".to_owned(),
            |scope| {
                format!(
                    "{}: {}",
                    scope.repository_id,
                    if scope.owns.is_empty() {
                        "read-only".to_owned()
                    } else {
                        scope.owns.join(", ")
                    }
                )
            },
        );
        format!(
            "{} r{} · {assignment} · {scope}",
            self.reference.unit_id, self.reference.revision
        )
    }

    pub(crate) fn new(context: &MutationContext, index: usize, draft: ContractDraft) -> Self {
        Self {
            reference: UnitRef {
                session_id: context.session_id.clone(),
                unit_id: format!("u-{}-{index}", context.mutation_id),
                revision: 1,
                contract_entry_id: String::new(),
            },
            draft,
            local_session_id: None,
            acceptance_evidence: None,
            binding: None,
            frozen_export_json: None,
            evidence: vec![],
            accepted_events: vec![],
            attempted_revision: None,
            imported: None,
            released: vec![],
        }
    }

    /// Projects a complete contract without changing or activating it.
    ///
    /// # Errors
    /// Rejects missing executable fields.
    pub fn contract(&self, title: &str) -> Result<UnitContract, crate::TodoError> {
        let incomplete = || {
            crate::TodoError::Contract(
                "CONTRACT_INCOMPLETE: supply deliverable, scope, acceptance and assignment"
                    .to_owned(),
            )
        };
        Ok(UnitContract {
            r#ref: self.reference.clone(),
            title: title.to_owned(),
            deliverable: self.draft.deliverable.clone().ok_or_else(incomplete)?,
            scope: self.draft.scope.clone().ok_or_else(incomplete)?,
            acceptance: self.draft.acceptance.clone().ok_or_else(incomplete)?,
            depends_on: self.draft.depends_on.clone(),
            assignment: self.draft.assignment.clone().ok_or_else(incomplete)?,
        })
    }
}
