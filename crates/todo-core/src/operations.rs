//! Atomic operations over a branch snapshot.
use crate::selection::{Selected, matches, migrate_request, root, selected};
use crate::validation::{error, nonempty};
use crate::{
    Assignment, ContractDraft, MutationContext, Status, Subtask, Task, TodoError, TodoList, Unit,
    validate,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Supported model-facing operations.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Init,
    Start,
    Done,
    Drop,
    Append,
    View,
    Update,
}

/// A draft leaf; local keys are resolved only within its creation request.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(deny_unknown_fields)]
pub struct DraftItem {
    pub content: String,
    #[serde(default)]
    #[cfg_attr(feature = "wasm", tsify(optional))]
    pub description: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub contract: ContractDraft,
}

/// Child creation never accepts another outcome.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(untagged, deny_unknown_fields)]
pub enum LeafItem {
    Text(String),
    Draft(Box<DraftItem>),
}

/// Root creation accepts standalone outcomes or outcomes with leaf work units.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(untagged, deny_unknown_fields)]
pub enum Item {
    Text(String),
    Outcome {
        content: String,
        #[serde(default)]
        #[cfg_attr(feature = "wasm", tsify(optional))]
        description: Option<String>,
        subtasks: Vec<LeafItem>,
    },
    Draft(Box<DraftItem>),
}

/// IDs address migrated tasks; exact labels address only legacy records.
#[derive(Clone, Debug, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub op: Operation,
    #[cfg_attr(
        feature = "wasm",
        tsify(optional, type = "readonly Item[] | undefined")
    )]
    pub items: Option<Vec<Item>>,
    #[cfg_attr(feature = "wasm", tsify(optional, type = "string | undefined"))]
    pub task: Option<String>,
    #[cfg_attr(feature = "wasm", tsify(optional, type = "string | undefined"))]
    pub parent: Option<String>,
    #[cfg_attr(feature = "wasm", tsify(optional, type = "ContractDraft | undefined"))]
    pub contract: Option<ContractDraft>,
    #[cfg_attr(feature = "wasm", tsify(optional, type = "string | undefined"))]
    pub start: Option<String>,
    #[cfg_attr(feature = "wasm", tsify(optional, type = "string | undefined"))]
    pub evidence: Option<String>,
    /// For `update`: sets the description; blank clears it; absent leaves it.
    #[cfg_attr(feature = "wasm", tsify(optional, type = "string | undefined"))]
    pub description: Option<String>,
}

/// Trims a description; blank means none.
fn clean(description: Option<String>) -> Option<String> {
    description
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// A successful operation; views never persist a snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct Change {
    pub list: TodoList,
    pub persist: bool,
}

pub(crate) fn legacy(items: Vec<String>) -> Result<TodoList, TodoError> {
    if items.is_empty() {
        return Err(TodoError::MissingItems);
    }
    let list = TodoList {
        tasks: items
            .into_iter()
            .map(|content| Task {
                content: content.trim().to_owned(),
                description: None,
                status: Status::Pending,
                subtasks: vec![],
                unit: None,
            })
            .collect(),
    };
    validate(&list)?;
    Ok(list)
}

const fn changes_nothing(request: &Request) -> bool {
    request.contract.is_none() && request.description.is_none()
}

const fn arguments(request: &Request) -> Result<(), TodoError> {
    let invalid = match request.op {
        Operation::Init => request.parent.is_some() || request.task.is_some(),
        Operation::Append => request.task.is_some(),
        Operation::Start => {
            request.items.is_some() || request.task.is_none() || request.start.is_some()
        }
        Operation::Update => {
            request.items.is_some() || request.task.is_none() || changes_nothing(request)
        }
        Operation::Done | Operation::Drop => {
            request.items.is_some()
                || request.start.is_some()
                || matches!((&request.parent, &request.task), (Some(_), None))
        }
        Operation::View => {
            request.items.is_some()
                || request.task.is_some()
                || request.parent.is_some()
                || request.start.is_some()
        }
    };
    if invalid
        || ((request.contract.is_some() || request.description.is_some())
            && !matches!(request.op, Operation::Update))
        || (request.evidence.is_some() && !matches!(request.op, Operation::Done))
    {
        Err(TodoError::InvalidRequest)
    } else {
        Ok(())
    }
}

fn draft_item(
    item: LeafItem,
    runtime: &MutationContext,
    index: usize,
    keys: &mut HashMap<String, String>,
) -> Result<Subtask, TodoError> {
    let (content, description, draft, key) = match item {
        LeafItem::Text(content) => (content, None, ContractDraft::default(), None),
        LeafItem::Draft(item) => (item.content, item.description, item.contract, item.key),
    };
    let unit = Unit::new(runtime, index, draft);
    if let Some(key) = key {
        nonempty(&key)?;
        if key.starts_with("u-") || keys.insert(key, unit.reference.unit_id.clone()).is_some() {
            return Err(error(
                "DUPLICATE_KEY: creation keys must be unique and cannot start with u-",
            ));
        }
    }
    Ok(Subtask {
        content: content.trim().to_owned(),
        description: clean(description),
        status: Status::Pending,
        unit: Some(unit),
    })
}

struct Created {
    tasks: Vec<Task>,
    keys: HashMap<String, String>,
}

fn resolve_dependencies(unit: &mut Unit, keys: &HashMap<String, String>) {
    for dependency in &mut unit.draft.depends_on {
        if let Some(id) = keys.get(dependency) {
            dependency.clone_from(id);
        }
    }
}

fn create(
    items: Vec<Item>,
    runtime: &MutationContext,
    offset: usize,
) -> Result<Created, TodoError> {
    if items.is_empty() {
        return Err(TodoError::MissingItems);
    }
    let mut keys = HashMap::new();
    let mut tasks = Vec::new();
    let mut index = offset;
    for item in items {
        let (item, subtasks) = match item {
            Item::Text(content) => (LeafItem::Text(content), vec![]),
            Item::Draft(draft) => (LeafItem::Draft(draft), vec![]),
            Item::Outcome {
                content,
                description,
                subtasks,
            } => {
                let root = DraftItem {
                    content,
                    description,
                    key: None,
                    contract: ContractDraft::default(),
                };
                (LeafItem::Draft(Box::new(root)), subtasks)
            }
        };
        let root = draft_item(item, runtime, index, &mut keys)?;
        index += 1;
        let mut children = Vec::new();
        for child in subtasks {
            children.push(draft_item(child, runtime, index, &mut keys)?);
            index += 1;
        }
        tasks.push(Task {
            content: root.content,
            description: root.description,
            status: root.status,
            unit: root.unit,
            subtasks: children,
        });
    }
    for task in &mut tasks {
        for unit in task.unit.iter_mut().chain(
            task.subtasks
                .iter_mut()
                .filter_map(|child| child.unit.as_mut()),
        ) {
            resolve_dependencies(unit, &keys);
        }
    }
    Ok(Created { tasks, keys })
}

fn add(
    list: &mut TodoList,
    request: &Request,
    context: &MutationContext,
) -> Result<HashMap<String, String>, TodoError> {
    if matches!(request.op, Operation::Init) {
        if crate::validation::units(list)
            .iter()
            .any(|(unit, _, _)| unit.local_session_id.is_some())
        {
            return Err(error("LOCAL_UNIT_ACTIVE: close active work before init"));
        }
        list.tasks.clear();
    }
    let offset = list.tasks.iter().map(|task| 1 + task.subtasks.len()).sum();
    let items = request.items.clone().unwrap_or_default();
    if request.parent.is_some()
        && items
            .iter()
            .any(|item| matches!(item, Item::Outcome { .. }))
    {
        return Err(TodoError::InvalidRequest);
    }
    let Created { tasks, keys } = create(items, context, offset)?;
    if let Some(parent) = &request.parent {
        let parent = root(list, parent)?;
        if !parent.status.is_open() {
            return Err(error("UNIT_CLOSED: create a new outcome for new scope"));
        }
        if parent
            .unit
            .as_ref()
            .is_some_and(|unit| unit.local_session_id.is_some())
        {
            return Err(error(
                "LOCAL_UNIT_ACTIVE: cannot add children to executing work",
            ));
        }
        parent
            .subtasks
            .extend(tasks.into_iter().map(|task| Subtask {
                content: task.content,
                description: task.description,
                status: task.status,
                unit: task.unit,
            }));
    } else {
        list.tasks.extend(tasks);
    }
    Ok(keys)
}

fn start(
    list: &mut TodoList,
    request: &Request,
    context: &MutationContext,
) -> Result<(), TodoError> {
    let Selected {
        content: title,
        status,
        unit,
        leaf,
        ..
    } = selected(list, request)?;
    if !leaf {
        return Err(error("NOT_EXECUTABLE: a parent is not a work unit"));
    }
    if !status.is_open() {
        return Err(error("UNIT_CLOSED: completed or dropped work cannot start"));
    }
    let unit = unit
        .as_mut()
        .ok_or_else(|| error("CONTRACT_INCOMPLETE: update legacy work before start"))?;
    let contract = unit.contract(title)?;
    if contract.assignment != Assignment::Self_ {
        return Err(error(
            "WRONG_ASSIGNMENT: start requires self; workers use the external executor",
        ));
    }
    unit.local_session_id = Some(context.session_id.clone());
    *status = Status::InProgress;
    for task in &mut list.tasks {
        if task.subtasks.iter().any(|child| {
            child
                .unit
                .as_ref()
                .is_some_and(|unit| unit.local_session_id.is_some())
        }) {
            task.status = Status::InProgress;
        }
    }
    validate(list)
}

fn update(list: &mut TodoList, request: &Request) -> Result<(), TodoError> {
    let Selected {
        status,
        unit,
        description,
        ..
    } = selected(list, request)?;
    let unit = unit
        .as_mut()
        .ok_or_else(|| error("INVALID_IDENTITY: migration required"))?;
    if !status.is_open() {
        return Err(error(
            "UNIT_FROZEN: only pending or non-executing legacy contracts can change",
        ));
    }
    if request.description.is_some() {
        *description = clean(request.description.clone());
    }
    let Some(draft) = request.contract.clone() else {
        return Ok(());
    };
    if unit.local_session_id.is_some() {
        return Err(error(
            "UNIT_FROZEN: only pending or non-executing legacy contracts can change",
        ));
    }
    if unit.draft != draft || unit.attempted_revision == Some(unit.reference.revision) {
        unit.draft = draft;
        unit.reference.revision = unit
            .reference
            .revision
            .checked_add(1)
            .ok_or_else(|| error("REVISION_EXHAUSTED"))?;
        unit.reference.contract_entry_id.clear();
        unit.acceptance_evidence = None;
    }
    Ok(())
}

fn close_leaf(
    status: &mut Status,
    unit: &mut Option<Unit>,
    request: &Request,
) -> Result<(), TodoError> {
    if !status.is_open() {
        return Ok(());
    }
    if unit.as_ref().is_some_and(|unit| unit.binding.is_some()) && request.task.is_none() {
        return Err(error(
            "UNIT_ALREADY_BOUND: close a held worker unit by its explicit ID",
        ));
    }
    if matches!(request.op, Operation::Done) {
        let evidence = request
            .evidence
            .as_ref()
            .ok_or_else(|| error("ACCEPTANCE_REQUIRED: done needs a short note"))?;
        nonempty(evidence)?;
        if let Some(unit) = unit {
            unit.acceptance_evidence = Some(evidence.clone());
        }
        *status = Status::Completed;
    } else {
        *status = Status::Abandoned;
    }
    if let Some(unit) = unit {
        unit.local_session_id = None;
        if let Some(binding) = unit.binding.take() {
            unit.released.push(binding.execution);
        }
        if let Some(imported) = &mut unit.imported {
            imported.seal = Some(crate::Seal {
                entry_id: String::new(),
                disposition: *status,
            });
        }
    }
    Ok(())
}

fn close_root(task: &mut Task, request: &Request) -> Result<(), TodoError> {
    if matches!(request.op, Operation::Done)
        && task.subtasks.iter().any(|child| child.status.is_open())
    {
        return Err(TodoError::OpenChildren(task.content.clone()));
    }
    if matches!(request.op, Operation::Drop) {
        for child in &mut task.subtasks {
            close_leaf(&mut child.status, &mut child.unit, request)?;
        }
    }
    close_leaf(&mut task.status, &mut task.unit, request)
}

fn completion(list: &mut TodoList, request: &Request) -> Result<(), TodoError> {
    if !matches!(request.op, Operation::Done) {
        return Ok(());
    }
    if request.task.is_none() {
        return Err(error("REVIEW_REQUIRED: done requires an explicit unit ID"));
    }
    let Selected {
        content: title,
        status,
        unit,
        leaf,
        ..
    } = selected(list, request)?;
    if !status.is_open() || !leaf {
        return Ok(());
    }
    let unit = unit
        .as_ref()
        .ok_or_else(|| error("CONTRACT_INCOMPLETE: update legacy work before completion"))?;
    unit.contract(title)?;
    let unit = unit.clone();
    crate::validation::prerequisites(list, &unit)
}

fn close(list: &mut TodoList, request: &Request) -> Result<(), TodoError> {
    completion(list, request)?;
    if let Some(selector) = &request.task {
        if request.parent.is_none()
            && list
                .tasks
                .iter()
                .any(|task| matches(&task.content, task.unit.as_ref(), selector))
        {
            close_root(root(list, selector)?, request)
        } else {
            let Selected { status, unit, .. } = selected(list, request)?;
            close_leaf(status, unit, request)
        }
    } else {
        if matches!(request.op, Operation::Done)
            && list
                .tasks
                .iter()
                .any(|task| task.status.is_open() && !task.subtasks.is_empty())
        {
            return Err(error(
                "REVIEW_REQUIRED: complete each parent explicitly after integration review",
            ));
        }
        for task in list.tasks.iter_mut().filter(|task| task.status.is_open()) {
            close_root(task, request)?;
        }
        Ok(())
    }
}

/// A bound unit is protected unless this request closed it and recorded its release.
fn held_units(previous: &TodoList, list: &TodoList) -> Vec<crate::UnitRef> {
    let after = crate::validation::units(list);
    crate::validation::units(previous)
        .into_iter()
        .filter(|(unit, _, _)| {
            unit.binding.as_ref().is_some_and(|binding| {
                !after.iter().any(|(next, status, _)| {
                    next.reference == unit.reference
                        && !status.is_open()
                        && next.binding.is_none()
                        && next.released.last() == Some(&binding.execution)
                })
            })
        })
        .map(|(unit, _, _)| unit.reference.clone())
        .collect()
}

/// Applies one atomic request using identity supplied by the trusted runtime.
///
/// # Errors
/// Rejects invalid contracts, transitions, prerequisites, or selectors without partial mutation.
pub fn apply(
    mut list: TodoList,
    mut request: Request,
    context: &MutationContext,
) -> Result<Change, TodoError> {
    validate(&list)?;
    arguments(&request)?;
    if matches!(request.op, Operation::View) {
        return Ok(Change {
            list,
            persist: false,
        });
    }
    nonempty(&context.session_id)?;
    nonempty(&context.mutation_id)?;
    if let Some(imported) = list
        .tasks
        .first()
        .and_then(|task| task.unit.as_ref())
        .and_then(|unit| unit.imported.as_ref())
        && (!matches!(request.op, Operation::Done | Operation::Drop)
            || imported.local_session_id != context.session_id)
    {
        return Err(error(
            "SEEDED_SCOPE_LOCKED: only closure of the imported unit is allowed",
        ));
    }
    let previous = list.clone();
    migrate_request(&mut list, &mut request, context)?;
    let keys = match request.op {
        Operation::Init | Operation::Append => add(&mut list, &request, context)?,
        Operation::Update => {
            update(&mut list, &request)?;
            HashMap::new()
        }
        Operation::Start => {
            start(&mut list, &request, context)?;
            HashMap::new()
        }
        Operation::Done | Operation::Drop => {
            close(&mut list, &request)?;
            HashMap::new()
        }
        Operation::View => HashMap::new(),
    };
    if let Some(selector) = &request.start {
        let selector = keys.get(selector).unwrap_or(selector).clone();
        request.task = Some(selector);
        request.parent = None;
        start(&mut list, &request, context)?;
    }
    let held = held_units(&previous, &list);
    crate::provider::protect(&previous, &list, &held)?;
    validate(&list)?;
    Ok(Change {
        list,
        persist: true,
    })
}
