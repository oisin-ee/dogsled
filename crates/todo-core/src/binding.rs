//! Frozen handoff data and evidence. Executor lifecycle remains outside todo-core.
use crate::{ExecutionRef, TodoError, UnitContract, UnitRef};
use serde::{Deserialize, Serialize};

/// Exact revision/execution correlation, not runtime worker state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskBinding {
    pub contract: UnitRef,
    pub execution: ExecutionRef,
    pub export_digest: String,
}

/// Completed prerequisite revision frozen at claim time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Prerequisite {
    pub unit_id: String,
    pub completed_revision: u32,
}

/// Ordered digest projection. Struct declaration order is the wire order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(deny_unknown_fields)]
pub struct ExportData {
    pub version: u32,
    pub provider: String,
    pub contract: UnitContract,
    pub prerequisites: Vec<Prerequisite>,
    pub brief: String,
}

/// Export envelope; the digest excludes only its own field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(deny_unknown_fields)]
pub struct TaskExport {
    pub version: u32,
    pub provider: String,
    pub contract: UnitContract,
    pub prerequisites: Vec<Prerequisite>,
    pub brief: String,
    pub digest: String,
}

/// Untrusted worker result reference, never acceptance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidenceRef {
    pub execution: ExecutionRef,
    pub result_id: String,
    pub summary: String,
}

/// Durable accepted event identity survives a new binding or revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcceptedEvent {
    pub origin: UnitRef,
    pub execution: ExecutionRef,
    pub event_id: String,
}

/// Evidence for an execution which obtained an export digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BoundResult {
    Returned {
        evidence: EvidenceRef,
    },
    Stopped {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        evidence: Option<EvidenceRef>,
        reason: String,
    },
}

/// Precommit abandonment has no export digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UnstartedResult {
    NotStarted { reason: String },
}

/// Executor-owned release facts; unknown outcomes never release a hold.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(untagged, deny_unknown_fields)]
pub enum Settlement {
    Bound {
        version: u32,
        #[serde(rename = "eventId")]
        event_id: String,
        binding: TaskBinding,
        result: BoundResult,
    },
    NotStarted {
        version: u32,
        #[serde(rename = "eventId")]
        event_id: String,
        origin: UnitRef,
        execution: ExecutionRef,
        result: UnstartedResult,
    },
}

impl Settlement {
    pub(crate) fn identity(&self) -> Result<AcceptedEvent, TodoError> {
        let (version, event_id, origin, execution) = match self {
            Self::Bound {
                version,
                event_id,
                binding,
                ..
            } => (*version, event_id, &binding.contract, &binding.execution),
            Self::NotStarted {
                version,
                event_id,
                origin,
                execution,
                ..
            } => (*version, event_id, origin, execution),
        };
        if version != 1 {
            return Err(crate::validation::error(
                "VERSION_UNSUPPORTED: expected version 1",
            ));
        }
        crate::validation::nonempty(event_id)?;
        execution.validate()?;
        Ok(AcceptedEvent {
            origin: origin.clone(),
            execution: execution.clone(),
            event_id: event_id.clone(),
        })
    }
}
