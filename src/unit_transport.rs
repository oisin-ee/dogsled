//! Frozen task transport. Schema and correlation checks only; task policy stays in pi-todo.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::fmt::Write as _;

use crate::error::{Error, Result};

pub use todo_core::{
    Assignment, BoundResult as ReturnedResult, EvidenceRef as Evidence, ExecutionRef, Seed,
    TaskBinding as Binding, TaskExport as Export, UnitRef, UnstartedResult as NotStartedResult,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}

/// Struct declaration order is the v1 canonical projection, recursively. Never hash a
/// Value map: its sorted keys differ from the provider's declared projection order.
pub fn canonical_export(export: &Export) -> Result<String> {
    todo_core::ExportData {
        version: export.version,
        provider: export.provider.clone(),
        contract: export.contract.clone(),
        prerequisites: export.prerequisites.clone(),
        brief: export.brief.clone(),
    }
    .canonical()
    .map_err(|error| Error::Invalid(error.to_string()))
}

pub fn validate_export<'a>(
    export: &'a Export,
    binding: &Binding,
    origin: &UnitRef,
    execution: &ExecutionRef,
) -> Result<&'a str> {
    validate_origin(origin)?;
    if export.version != 1 || export.provider != "pi-todo" {
        return Err(conflict("unsupported task export envelope"));
    }
    if binding.contract != *origin
        || export.contract.r#ref != *origin
        || binding.execution != *execution
    {
        return Err(conflict(
            "export, binding, and prepared request do not match",
        ));
    }
    let digest = todo_core::export_digest(&canonical_export(export)?)
        .map_err(|error| Error::Invalid(error.to_string()))?;
    if export.digest != digest || binding.export_digest != digest {
        return Err(conflict("export digest does not match"));
    }
    for prerequisite in &export.prerequisites {
        if prerequisite.unit_id.is_empty() || prerequisite.completed_revision == 0 {
            return Err(conflict("invalid prerequisite correlation"));
        }
    }
    match &export.contract.assignment {
        Assignment::Worker { profile } if !profile.is_empty() => Ok(profile),
        _ => Err(Error::Invalid(
            "PROFILE_INVALID: a worker profile is required".into(),
        )),
    }
}

pub fn validate_origin(origin: &UnitRef) -> Result<()> {
    if origin.session_id.is_empty()
        || origin.unit_id.is_empty()
        || origin.contract_entry_id.is_empty()
        || origin.revision == 0
    {
        return Err(conflict(
            "unit reference IDs must be nonempty and revision positive",
        ));
    }
    Ok(())
}

/// Bounded native launch label, not an identity or authorization token. Full binding
/// checks remain authoritative; native name collisions are final refusals, never aliases.
pub fn agent_name(origin: &UnitRef, execution: &ExecutionRef) -> Result<String> {
    let digest = digest_hex(&serde_json::to_vec(&(origin, execution))?);
    Ok(format!("u-{}", &digest[..30]))
}

pub fn digest_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

pub fn conflict(message: &str) -> Error {
    Error::Invalid(format!("REQUEST_CONFLICT: {message}"))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareInput {
    pub origin: UnitRef,
    pub placement: Placement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommitInput {
    pub execution: ExecutionRef,
    pub binding: Binding,
    pub exported: Export,
}

pub fn seed_from_commit(input: CommitInput) -> Seed {
    Seed {
        version: 1,
        execution: input.execution,
        binding: input.binding,
        exported: input.exported,
    }
}

pub fn validate_seed<'a>(seed: &'a Seed, id: &str, owner: &str) -> Result<&'a str> {
    if seed.version != 1
        || seed.execution.executor != "dogsled"
        || seed.execution.request_id != id
        || seed.execution.owner_id != owner
    {
        return Err(conflict("seed does not match launch identity"));
    }
    validate_export(
        &seed.exported,
        &seed.binding,
        &seed.binding.contract,
        &seed.execution,
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalAck {
    pub execution: ExecutionRef,
    pub unit_ref: UnitRef,
    pub digest: String,
    pub seal_entry_id: String,
    pub session_leaf_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Returned {
    pub version: u64,
    pub event_id: String,
    pub binding: Binding,
    pub result: ReturnedResult,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettlementAck {
    pub execution: ExecutionRef,
    pub event_id: String,
    /// Present when a custodian, not the origin owner, acknowledges the delivery.
    #[serde(default)]
    pub custody: Option<CustodyRef>,
}

/// The custody claim a delivery or an acknowledgement is bound to. The server compares it
/// with the live custody record, never with identity the caller supplies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustodyRef {
    pub item_id: String,
    pub claim_id: String,
    pub revision: u64,
    pub recipient: String,
}

/// A custodian's acceptance of the exact returned result. It is a review fact, not task
/// completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Review {
    pub claim_id: String,
    pub revision: u64,
    pub execution: ExecutionRef,
    pub result_id: String,
    pub review_id: String,
    pub evidence: String,
    pub reviewer: String,
}

/// A settlement delivered to the custodian. The settlement is the saved event, unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Delivery {
    pub version: u64,
    pub custody: CustodyRef,
    pub settlement: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionInput {
    pub execution: ExecutionRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotStarted {
    pub version: u64,
    pub event_id: String,
    pub origin: UnitRef,
    pub execution: ExecutionRef,
    pub result: NotStartedResult,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_label_is_bounded_stable_and_uses_every_identity_field() {
        let origin = UnitRef {
            session_id: "01a15a42-78ee-7513-8f13-5005bbbeaf7c".into(),
            unit_id: "u-6ce8048d-2317-4b7e-988e-cf6591d1b7a0-1".into(),
            revision: u32::MAX,
            contract_entry_id: "入口🙂".into(),
        };
        let execution = ExecutionRef {
            executor: "dogsled".into(),
            owner_id: "wFR:p6".into(),
            request_id: "r-1792515815179-47198-0".into(),
        };
        // Independent Python hashlib SHA256 of the ordered UTF-8 JSON tuple.
        let expected = "u-10d4395753ff76990ac04f309e8f17";
        assert_eq!(agent_name(&origin, &execution).unwrap(), expected);
        assert_eq!(agent_name(&origin, &execution).unwrap(), expected);
        let mut names = std::collections::BTreeSet::from([expected.to_owned()]);
        for field in 0..7 {
            let mut origin = origin.clone();
            let mut execution = execution.clone();
            match field {
                0 => origin.session_id.push('x'),
                1 => origin.unit_id.push('🙂'),
                2 => origin.revision -= 1,
                3 => origin.contract_entry_id.push('x'),
                4 => execution.executor.push('x'),
                5 => execution.owner_id.push('x'),
                _ => execution.request_id.push('x'),
            }
            let name = agent_name(&origin, &execution).unwrap();
            assert_eq!(name.len(), 32);
            assert!(name.starts_with("u-"));
            assert!(
                name[2..]
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            );
            assert!(
                names.insert(name),
                "field {field} did not distinguish the label"
            );
        }
    }

    #[test]
    fn canonical_bytes_match_javascript_fixtures() {
        let fixtures: serde_json::Value = serde_json::from_str(include_str!(
            "../crates/todo-core/tests/fixtures/unit-exports.json"
        ))
        .unwrap();
        for fixture in fixtures.as_array().unwrap() {
            let canonical = fixture["canonical"].as_str().unwrap();
            let mut value: serde_json::Value = serde_json::from_str(canonical).unwrap();
            value["digest"] = fixture["digest"].clone();
            let export: Export = serde_json::from_value(value).unwrap();
            assert_eq!(canonical_export(&export).unwrap(), canonical);
            assert_eq!(
                todo_core::export_digest(&canonical_export(&export).unwrap()).unwrap(),
                export.digest
            );
        }
    }
}
