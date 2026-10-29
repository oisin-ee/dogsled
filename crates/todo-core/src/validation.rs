//! Validation of declared contracts and their dependency graph.
use crate::{Acceptance, Assignment, ContractDraft, Status, TodoError, TodoList, Unit};
use std::collections::{HashMap, HashSet};

pub(crate) fn error(message: &str) -> TodoError {
    TodoError::Contract(message.to_owned())
}

pub(crate) fn nonempty(value: &str) -> Result<(), TodoError> {
    if value.trim().is_empty() {
        Err(error("CONTRACT_INCOMPLETE: empty contract field"))
    } else {
        Ok(())
    }
}

fn path(value: &str, cwd: bool) -> Result<(), TodoError> {
    if cwd && value == "." {
        return Ok(());
    }
    nonempty(value)?;
    if value.starts_with('/')
        || value.contains(['\\', ':', '*', '?', '[', ']', '{', '}'])
        || value
            .trim_end_matches('/')
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(error(
            "INVALID_SCOPE: use repository-relative files or directory prefixes",
        ));
    }
    Ok(())
}

fn paths(values: &[String]) -> Result<(), TodoError> {
    let mut seen = HashSet::new();
    for value in values {
        path(value, false)?;
        if !seen.insert(value.trim_end_matches('/')) {
            return Err(error("INVALID_SCOPE: duplicate path"));
        }
    }
    Ok(())
}

pub(crate) fn draft(draft: &ContractDraft) -> Result<(), TodoError> {
    if let Some(value) = &draft.deliverable {
        nonempty(value)?;
    }
    if let Some(scope) = &draft.scope {
        nonempty(&scope.repository_id)?;
        paths(&scope.owns)?;
        paths(&scope.reads)?;
        paths(&scope.generates)?;
    }
    if let Some(Assignment::Worker { profile }) = &draft.assignment {
        nonempty(profile)?;
    }
    match &draft.acceptance {
        Some(Acceptance::Command {
            cwd,
            argv,
            expected,
        }) => {
            path(cwd, true)?;
            if argv.is_empty() {
                return Err(error("CONTRACT_INCOMPLETE: acceptance argv is empty"));
            }
            for arg in argv {
                nonempty(arg)?;
            }
            nonempty(expected)?;
        }
        Some(Acceptance::Review {
            artifact,
            criterion,
        }) => {
            nonempty(artifact)?;
            nonempty(criterion)?;
        }
        None => {}
    }
    Ok(())
}

pub(crate) fn units(list: &TodoList) -> Vec<(&Unit, Status, bool)> {
    list.tasks
        .iter()
        .flat_map(|task| {
            task.unit
                .iter()
                .map(|unit| (unit, task.status, task.subtasks.is_empty()))
                .chain(
                    task.subtasks.iter().filter_map(|child| {
                        child.unit.as_ref().map(|unit| (unit, child.status, true))
                    }),
                )
        })
        .collect()
}

fn cycle<'a>(
    id: &'a str,
    graph: &HashMap<&'a str, &'a Unit>,
    visiting: &mut HashSet<&'a str>,
    done: &mut HashSet<&'a str>,
) -> Result<(), TodoError> {
    if done.contains(id) {
        return Ok(());
    }
    if !visiting.insert(id) {
        return Err(error("DEPENDENCY_CYCLE: prerequisites contain a cycle"));
    }
    let unit = graph
        .get(id)
        .ok_or_else(|| error("UNIT_NOT_FOUND: unknown prerequisite"))?;
    let mut unique = HashSet::new();
    if unit.imported.is_some() {
        visiting.remove(id);
        done.insert(id);
        return Ok(());
    }
    for dependency in &unit.draft.depends_on {
        if !unique.insert(dependency) {
            return Err(error("DEPENDENCY_DUPLICATE: repeated prerequisite"));
        }
        cycle(dependency, graph, visiting, done)?;
    }
    visiting.remove(id);
    done.insert(id);
    Ok(())
}

pub(crate) fn prerequisites(list: &TodoList, unit: &Unit) -> Result<(), TodoError> {
    if unit.imported.is_some() {
        return Ok(()); // Frozen prerequisite evidence is validated with the import.
    }
    let entries = units(list);
    for id in &unit.draft.depends_on {
        if !entries.iter().any(|(other, status, _)| {
            other.reference.unit_id == *id && *status == Status::Completed
        }) {
            return Err(error(
                "DEPENDENCY_OPEN: prerequisite must be completed, not dropped",
            ));
        }
    }
    Ok(())
}

fn held_binding(unit: &Unit, status: Status, leaf: bool) -> Result<(), TodoError> {
    let Some(binding) = &unit.binding else {
        return Ok(());
    };
    binding.execution.validate()?;
    let exported = unit
        .frozen_export_json
        .as_deref()
        .ok_or_else(|| error("BINDING_MISMATCH: missing frozen export"))?;
    let exported = crate::TaskExport::parse(exported)?;
    if exported.contract != unit.contract(&exported.contract.title)?
        || exported.digest != binding.export_digest
        || unit.attempted_revision != Some(unit.reference.revision)
    {
        return Err(error(
            "BINDING_MISMATCH: frozen export differs from held contract",
        ));
    }
    if binding.contract != unit.reference
        || status != Status::InProgress
        || !leaf
        || !matches!(unit.draft.assignment, Some(Assignment::Worker { .. }))
        || unit.local_session_id.is_some()
    {
        return Err(error("BINDING_MISMATCH: invalid held contract"));
    }
    Ok(())
}

pub(crate) fn validate_units(list: &TodoList) -> Result<(), TodoError> {
    let entries = units(list);
    let mut graph = HashMap::new();
    let mut local_active = false;
    for (unit, status, leaf) in &entries {
        nonempty(&unit.reference.unit_id)?;
        nonempty(&unit.reference.session_id)?;
        if unit.reference.revision == 0
            || graph
                .insert(unit.reference.unit_id.as_str(), *unit)
                .is_some()
        {
            return Err(error("INVALID_IDENTITY: duplicate ID or zero revision"));
        }
        draft(&unit.draft)?;
        held_binding(unit, *status, *leaf)?;
        if let Some(session) = &unit.local_session_id {
            nonempty(session)?;
            if *status != Status::InProgress
                || !leaf
                || (unit.draft.assignment != Some(Assignment::Self_) && unit.imported.is_none())
            {
                return Err(error(
                    "INVALID_ACTIVATION: local work must be an active self-assigned leaf",
                ));
            }
            unit.contract("validation")?;
            prerequisites(list, unit)?;
            if local_active {
                return Err(error(
                    "LOCAL_UNIT_ACTIVE: only one local executable unit is allowed",
                ));
            }
            local_active = true;
        }
    }
    let mut done = HashSet::new();
    for id in graph.keys() {
        cycle(id, &graph, &mut HashSet::new(), &mut done)?;
    }
    Ok(())
}
