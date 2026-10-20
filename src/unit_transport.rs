//! Frozen task transport. Schema and correlation checks only; task policy stays in pi-todo.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::fmt::Write as _;

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnitRef {
    pub session_id: String,
    pub unit_id: String,
    pub revision: u64,
    pub contract_entry_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionRef {
    pub executor: String,
    pub owner_id: String,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Binding {
    pub contract: UnitRef,
    pub execution: ExecutionRef,
    pub export_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Assignment {
    #[serde(rename = "self")]
    Self_,
    Worker {
        profile: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    pub repository_id: String,
    pub owns: Vec<String>,
    pub reads: Vec<String>,
    pub generates: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Contract {
    #[serde(rename = "ref")]
    pub reference: UnitRef,
    pub title: String,
    pub deliverable: String,
    pub scope: Scope,
    pub acceptance: Acceptance,
    pub depends_on: Vec<String>,
    pub assignment: Assignment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Prerequisite {
    pub unit_id: String,
    pub completed_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Export {
    pub version: u64,
    pub provider: String,
    pub contract: Contract,
    pub prerequisites: Vec<Prerequisite>,
    pub brief: String,
    pub digest: String,
}

impl Export {
    /// Struct declaration order is the v1 canonical projection, recursively. Never hash a
    /// Value map: its sorted keys differ from the provider's declared projection order.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        #[derive(Serialize)]
        struct Projection<'a> {
            version: u64,
            provider: &'a str,
            contract: &'a Contract,
            prerequisites: &'a [Prerequisite],
            brief: &'a str,
        }
        Ok(serde_json::to_vec(&Projection {
            version: self.version,
            provider: &self.provider,
            contract: &self.contract,
            prerequisites: &self.prerequisites,
            brief: &self.brief,
        })?)
    }

    pub fn validate(
        &self,
        binding: &Binding,
        origin: &UnitRef,
        execution: &ExecutionRef,
    ) -> Result<&str> {
        origin.validate()?;
        if self.version != 1 || self.provider != "pi-todo" {
            return Err(conflict("unsupported task export envelope"));
        }
        if binding.contract != *origin
            || self.contract.reference != *origin
            || binding.execution != *execution
        {
            return Err(conflict(
                "export, binding, and prepared request do not match",
            ));
        }
        let digest = digest_hex(&self.canonical_bytes()?);
        if self.digest != digest || binding.export_digest != digest {
            return Err(conflict("export digest does not match"));
        }
        for prerequisite in &self.prerequisites {
            if prerequisite.unit_id.is_empty() || prerequisite.completed_revision == 0 {
                return Err(conflict("invalid prerequisite correlation"));
            }
        }
        match &self.contract.assignment {
            Assignment::Worker { profile } if !profile.is_empty() => Ok(profile),
            _ => Err(Error::Invalid(
                "PROFILE_INVALID: a worker profile is required".into(),
            )),
        }
    }
}

impl UnitRef {
    pub fn validate(&self) -> Result<()> {
        if self.session_id.is_empty()
            || self.unit_id.is_empty()
            || self.contract_entry_id.is_empty()
            || self.revision == 0
        {
            return Err(conflict(
                "unit reference IDs must be nonempty and revision positive",
            ));
        }
        Ok(())
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitInput {
    pub execution: ExecutionRef,
    pub binding: Binding,
    pub exported: Export,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NotStartedResult {
    NotStarted { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_bytes_match_javascript_fixtures() {
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/unit-exports.json")).unwrap();
        for fixture in fixtures.as_array().unwrap() {
            let canonical = fixture["canonical"].as_str().unwrap();
            let mut value: serde_json::Value = serde_json::from_str(canonical).unwrap();
            value["digest"] = fixture["digest"].clone();
            let export: Export = serde_json::from_value(value).unwrap();
            assert_eq!(export.canonical_bytes().unwrap(), canonical.as_bytes());
            assert_eq!(
                digest_hex(&export.canonical_bytes().unwrap()),
                export.digest
            );
        }
    }
}
