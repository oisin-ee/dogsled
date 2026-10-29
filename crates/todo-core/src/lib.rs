//! Branch-local outcomes and bounded work units, addressed by exact content.

mod binding;
#[cfg(test)]
mod close_tests;
mod contract;
#[cfg(test)]
mod contract_tests;
#[cfg(test)]
mod description_tests;
mod deserialization;
mod export;
mod import;
#[cfg(test)]
mod import_tests;
mod operations;
mod provider;
#[cfg(test)]
mod provider_tests;
mod reminder;
mod selection;
mod validation;
mod widget;

pub use binding::{
    AcceptedEvent, BoundResult, EvidenceRef, ExportData, Prerequisite, Settlement, TaskBinding,
    TaskExport, UnstartedResult,
};
pub use contract::{
    Acceptance, Assignment, ContractDraft, ExecutionRef, MutationContext, Scope, Unit,
    UnitContract, UnitRef,
};
pub use export::export_digest;
pub use import::{ImportReceipt, ImportedUnit, Seal, Seed, import_unit, imported, parse_seed};
pub use operations::{Change, DraftItem, Item, LeafItem, Operation, Request, apply};
pub use provider::{
    ActiveUnit, ClaimChange, ClaimInput, ClaimResult, SettlementChange, SettlementResult, active,
    claim, inspect, parse_claim, parse_settlement, protect, settle,
};
pub use reminder::{ReminderFact, ReminderHistory, completion_reminder};
pub use widget::{SectionChild, SectionItem, TodoSection, WidgetKind, WidgetRow, sections, widget};

use serde::{Deserialize, Serialize};
use std::fmt;

/// The state of one outcome or work unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    InProgress,
    Completed,
    Abandoned,
}

impl Status {
    /// Whether explicit closure is still required.
    #[must_use]
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Pending | Self::InProgress)
    }
}

/// A leaf work unit. Grandchildren are not accepted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct Subtask {
    pub content: String,
    /// Detail behind the short `content` headline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "wasm", tsify(optional))]
    pub description: Option<String>,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<Unit>,
}

/// An outcome, optionally divided into bounded work units.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub content: String,
    /// Detail behind the short `content` headline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "wasm", tsify(optional))]
    pub description: Option<String>,
    pub status: Status,
    #[serde(default)]
    pub subtasks: Vec<Subtask>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<Unit>,
}

impl Task {
    /// Closed children do not automatically complete their parent.
    #[must_use]
    pub fn ready_for_review(&self) -> bool {
        self.status.is_open()
            && !self.subtasks.is_empty()
            && self.subtasks.iter().all(|child| !child.status.is_open())
    }
}

/// The whole todo list of a session branch.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(deny_unknown_fields)]
pub struct TodoList {
    pub tasks: Vec<Task>,
}

/// A rejected operation. The input snapshot remains unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TodoError {
    MissingItems,
    EmptyTask,
    DuplicateTask(String),
    MissingTask,
    TaskNotFound(String),
    OpenChildren(String),
    InvalidRequest,
    Contract(String),
}

impl fmt::Display for TodoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(message) => f.write_str(message),
            Self::MissingItems => f.write_str("Missing items for init or append operation"),
            Self::EmptyTask => f.write_str("Missing task content"),
            Self::DuplicateTask(content) => write!(f, "Duplicate task \"{content}\" in list"),
            Self::MissingTask => f.write_str("Missing task content for start operation"),
            Self::TaskNotFound(content) => {
                write!(f, "Task \"{content}\" not found; use exact task content")
            }
            Self::OpenChildren(content) => write!(
                f,
                "Task \"{content}\" has open subtasks; close them before integration review"
            ),
            Self::InvalidRequest => f.write_str(
                "Invalid arguments for todo operation; children accept string items only",
            ),
        }
    }
}
impl std::error::Error for TodoError {}

/// Validates snapshot labels and sibling identity without changing recorded statuses.
///
/// # Errors
/// Rejects blank labels, duplicate siblings, and closed parents with open children.
pub fn validate(list: &TodoList) -> Result<(), TodoError> {
    validate_labels(list.tasks.iter().map(|task| task.content.as_str()))?;
    for task in &list.tasks {
        validate_labels(task.subtasks.iter().map(|child| child.content.as_str()))?;
        if !task.status.is_open() && task.subtasks.iter().any(|child| child.status.is_open()) {
            return Err(TodoError::OpenChildren(task.content.clone()));
        }
    }
    let imported = validation::units(list)
        .iter()
        .any(|(unit, _, _)| unit.imported.is_some());
    if imported
        && (list.tasks.len() != 1
            || list
                .tasks
                .first()
                .is_some_and(|task| !task.subtasks.is_empty()))
    {
        return Err(validation::error(
            "SEEDED_SCOPE_LOCKED: worker owns exactly one leaf",
        ));
    }
    for task in &list.tasks {
        if let Some(unit) = &task.unit {
            import::validate_import(unit, &task.content, task.status, task.subtasks.is_empty())?;
        }
        for child in &task.subtasks {
            if child
                .unit
                .as_ref()
                .is_some_and(|unit| unit.imported.is_some())
            {
                return Err(validation::error(
                    "SEEDED_SCOPE_LOCKED: worker import must be a standalone leaf",
                ));
            }
        }
    }
    validation::validate_units(list)
}

fn validate_labels<'a>(labels: impl Iterator<Item = &'a str>) -> Result<(), TodoError> {
    let mut seen = std::collections::HashSet::new();
    for label in labels {
        if label.trim().is_empty() {
            return Err(TodoError::EmptyTask);
        }
        if !seen.insert(label) {
            return Err(TodoError::DuplicateTask(label.to_owned()));
        }
    }
    Ok(())
}

/// Creates standalone pending outcomes.
///
/// # Errors
/// Rejects empty lists, blank labels, and duplicates.
pub fn init(items: Vec<String>) -> Result<TodoList, TodoError> {
    operations::legacy(items)
}

const fn mark(status: Status) -> &'static str {
    match status {
        Status::Pending => "[ ]",
        Status::InProgress => "[/]",
        Status::Completed => "[x]",
        Status::Abandoned => "[-]",
    }
}

/// The complete model-facing summary, with separate outcome and work-unit progress.
#[must_use]
pub fn view(list: &TodoList) -> String {
    if list.tasks.is_empty() {
        return "Todo list is empty.".to_owned();
    }
    let done = list
        .tasks
        .iter()
        .filter(|task| task.status == Status::Completed)
        .count();
    let children: Vec<_> = list.tasks.iter().flat_map(|task| &task.subtasks).collect();
    let mut header = format!("Todo list: {done}/{} completed", list.tasks.len());
    if !children.is_empty() {
        let completed = children
            .iter()
            .filter(|child| child.status == Status::Completed)
            .count();
        header = format!(
            "{header} outcomes; {completed}/{} subtasks completed",
            children.len()
        );
    }
    let mut lines = vec![header];
    for task in &list.tasks {
        let review = if task.ready_for_review() {
            " · Ready for review"
        } else {
            ""
        };
        lines.push(format!("{} {}{review}", mark(task.status), task.content));
        if let Some(description) = &task.description {
            lines.push(format!("  {description}"));
        }
        if let Some(unit) = &task.unit {
            lines.push(format!("    {}", unit.summary()));
        }
        for child in &task.subtasks {
            lines.push(format!("  {} {}", mark(child.status), child.content));
            if let Some(description) = &child.description {
                lines.push(format!("    {description}"));
            }
            if let Some(unit) = &child.unit {
                lines.push(format!("      {}", unit.summary()));
            }
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_is_pending_and_validation_is_atomic() {
        let list = init(vec![" a ".to_owned(), "b".to_owned()]).unwrap();
        assert_eq!(
            view(&list),
            "Todo list: 0/2 completed\n[ ] a\n[ ] b",
            "pending, trimmed outcomes"
        );
        assert_eq!(init(vec![]), Err(TodoError::MissingItems), "empty rejected");
        assert_eq!(
            init(vec![" ".to_owned()]),
            Err(TodoError::EmptyTask),
            "blank rejected"
        );
        assert_eq!(
            init(vec!["a".to_owned(), "a ".to_owned()]),
            Err(TodoError::DuplicateTask("a".to_owned())),
            "siblings unique"
        );
    }
}
