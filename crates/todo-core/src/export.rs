//! Canonical export bytes and the fixed task brief belong to the contract owner.
use crate::binding::{ExportData, Prerequisite, TaskExport};
use crate::{Acceptance, TodoError, UnitContract};
use sha2::{Digest, Sha256};
use std::fmt::Write;

/// SHA-256 over compact UTF-8; arrays retain their authored order.
///
/// # Errors
/// Reports formatting failure without publishing a partial digest.
pub fn export_digest(canonical: &str) -> Result<String, TodoError> {
    let mut output = String::with_capacity(64);
    for byte in Sha256::digest(canonical.as_bytes()) {
        write!(output, "{byte:02x}")
            .map_err(|error| crate::validation::error(&format!("CONTRACT_INCOMPLETE: {error}")))?;
    }
    Ok(output)
}

fn brief(contract: &UnitContract) -> String {
    let acceptance = match &contract.acceptance {
        Acceptance::Command {
            cwd,
            argv,
            expected,
        } => {
            format!("Command in {cwd}: {}\nExpected: {expected}", argv.join(" "))
        }
        Acceptance::Review {
            artifact,
            criterion,
        } => {
            format!("Review {artifact}\nCriterion: {criterion}")
        }
    };
    let scope = &contract.scope;
    let origin = &contract.r#ref;
    format!(
        "Title: {}\nDeliverable: {}\nRepository: {}\nOwns: {}\nReads: {}\nGenerates: {}\nAcceptance: {}\nOrigin: {} / {} / r{} / {}",
        contract.title,
        contract.deliverable,
        scope.repository_id,
        scope.owns.join(", "),
        scope.reads.join(", "),
        scope.generates.join(", "),
        acceptance,
        origin.session_id,
        origin.unit_id,
        origin.revision,
        origin.contract_entry_id,
    )
}

impl ExportData {
    /// Canonical fixed-schema projection, excluding the digest.
    ///
    /// # Errors
    /// Returns serialization errors without changing task state.
    pub fn canonical(&self) -> Result<String, TodoError> {
        serde_json::to_string(self)
            .map_err(|error| crate::validation::error(&format!("CONTRACT_INCOMPLETE: {error}")))
    }
}

impl TaskExport {
    pub(crate) fn create(
        contract: UnitContract,
        prerequisites: Vec<Prerequisite>,
    ) -> Result<Self, TodoError> {
        let data = ExportData {
            version: 1,
            provider: "pi-todo".to_owned(),
            brief: brief(&contract),
            contract,
            prerequisites,
        };
        let digest = export_digest(&data.canonical()?)?;
        Ok(Self {
            version: data.version,
            provider: data.provider,
            contract: data.contract,
            prerequisites: data.prerequisites,
            brief: data.brief,
            digest,
        })
    }

    /// Canonical bytes of the complete frozen envelope.
    ///
    /// # Errors
    /// Returns serialization errors without changing task state.
    pub fn json(&self) -> Result<String, TodoError> {
        serde_json::to_string(self)
            .map_err(|error| crate::validation::error(&format!("CONTRACT_INCOMPLETE: {error}")))
    }

    pub(crate) fn parse(json: &str) -> Result<Self, TodoError> {
        let value: Self = serde_json::from_str(json)
            .map_err(|error| crate::validation::error(&format!("CONTRACT_INCOMPLETE: {error}")))?;
        let data = ExportData {
            version: value.version,
            provider: value.provider.clone(),
            contract: value.contract.clone(),
            prerequisites: value.prerequisites.clone(),
            brief: value.brief.clone(),
        };
        if value.version != 1
            || value.provider != "pi-todo"
            || value.digest != export_digest(&data.canonical()?)?
            || value.json()? != json
        {
            return Err(crate::validation::error(
                "BINDING_MISMATCH: frozen export is not canonical",
            ));
        }
        Ok(value)
    }
}
