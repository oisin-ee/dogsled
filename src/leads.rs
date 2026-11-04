//! Ownerless lead identity and launch state, independent of child work and retention.

use crate::checkout::Repository;
use crate::jobs::Validity;
use crate::probe::Agent;
use serde::{Deserialize, Serialize};

/// Intent is saved before each external effect; its outcome advances the phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Recorded,
    Allocating,
    Allocated,
    Starting,
    Started,
    Prompting,
    Ready,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Record {
    // Immutable launch intent. These fields never change during an update.
    pub id: String,
    pub name: String,
    pub repository: Repository,
    pub prompt: String,
    pub validity: Validity,
    pub created_ms: u64,
    // Observed launch outcomes and explicit closure, not child completion.
    pub phase: Phase,
    pub pane: Option<String>,
    pub terminal: Option<String>,
    pub workspace: Option<String>,
    pub updated_ms: u64,
    pub error: Option<String>,
    pub close_requested_ms: Option<u64>,
    pub closed_ms: Option<u64>,
}

/// A name or pane alone never proves a lead's identity for capacity exemption.
pub(crate) fn matches_agent(record: &Record, agent: &Agent) -> bool {
    record.closed_ms.is_none()
        && record.pane.as_deref() == Some(agent.pane.as_str())
        && matches!(
            (record.terminal.as_deref(), agent.terminal_id.as_deref()),
            (Some(saved), Some(observed)) if !saved.is_empty() && saved == observed
        )
}

/// Missing identity is unknown and must not authorize automatic destruction.
/// Only a positively different terminal proves that this pane has been reused.
pub(crate) fn protects_pane(record: &Record, pane: &str, terminal: Option<&str>) -> bool {
    record.closed_ms.is_none()
        && record.pane.as_deref() == Some(pane)
        && !matches!(
            (record.terminal.as_deref(), terminal),
            (Some(saved), Some(observed))
                if !saved.is_empty() && !observed.is_empty() && saved != observed
        )
}

#[cfg(test)]
mod tests;
