//! Task identity and migration on a selected Pi branch.
use crate::{ContractDraft, MutationContext, Request, Status, Task, TodoError, TodoList, Unit};

pub(crate) struct Selected<'a> {
    pub content: &'a str,
    pub description: &'a mut Option<String>,
    pub status: &'a mut Status,
    pub unit: &'a mut Option<Unit>,
    pub leaf: bool,
}

#[derive(Clone, Copy)]
enum Position {
    Root(usize),
    Child(usize, usize),
}

pub(crate) fn matches(content: &str, unit: Option<&Unit>, selector: &str) -> bool {
    unit.map_or(content == selector, |unit| {
        unit.reference.unit_id == selector
    })
}

fn root_position(list: &TodoList, selector: &str) -> Result<usize, TodoError> {
    list.tasks
        .iter()
        .position(|task| matches(&task.content, task.unit.as_ref(), selector))
        .ok_or_else(|| TodoError::TaskNotFound(selector.to_owned()))
}

pub(crate) fn root<'a>(list: &'a mut TodoList, selector: &str) -> Result<&'a mut Task, TodoError> {
    let index = root_position(list, selector)?;
    list.tasks
        .get_mut(index)
        .ok_or_else(|| TodoError::TaskNotFound(selector.to_owned()))
}

fn position(list: &TodoList, request: &Request) -> Result<Position, TodoError> {
    let selector = request.task.as_ref().ok_or(TodoError::MissingTask)?;
    if let Some(parent) = &request.parent {
        let index = root_position(list, parent)?;
        let child = list
            .tasks
            .get(index)
            .and_then(|task| {
                task.subtasks
                    .iter()
                    .position(|child| matches(&child.content, child.unit.as_ref(), selector))
            })
            .ok_or_else(|| TodoError::TaskNotFound(selector.clone()))?;
        return Ok(Position::Child(index, child));
    }
    for (parent, task) in list.tasks.iter().enumerate() {
        if matches(&task.content, task.unit.as_ref(), selector) {
            return Ok(Position::Root(parent));
        }
        if let Some(child) = task.subtasks.iter().position(|child| {
            child
                .unit
                .as_ref()
                .is_some_and(|unit| unit.reference.unit_id == *selector)
        }) {
            return Ok(Position::Child(parent, child));
        }
    }
    Err(TodoError::TaskNotFound(selector.clone()))
}

fn at(list: &mut TodoList, position: Position) -> Result<Selected<'_>, TodoError> {
    match position {
        Position::Root(index) => {
            let task = list.tasks.get_mut(index).ok_or(TodoError::MissingTask)?;
            Ok(Selected {
                content: &task.content,
                description: &mut task.description,
                status: &mut task.status,
                unit: &mut task.unit,
                leaf: task.subtasks.is_empty(),
            })
        }
        Position::Child(parent, index) => {
            let child = list
                .tasks
                .get_mut(parent)
                .and_then(|task| task.subtasks.get_mut(index))
                .ok_or(TodoError::MissingTask)?;
            Ok(Selected {
                content: &child.content,
                description: &mut child.description,
                status: &mut child.status,
                unit: &mut child.unit,
                leaf: true,
            })
        }
    }
}

pub(crate) fn selected<'a>(
    list: &'a mut TodoList,
    request: &Request,
) -> Result<Selected<'a>, TodoError> {
    let position = position(list, request)?;
    at(list, position)
}

pub(crate) fn migrate_request(
    list: &mut TodoList,
    request: &mut Request,
    context: &MutationContext,
) -> Result<(), TodoError> {
    let parent = request
        .parent
        .as_ref()
        .map(|selector| root_position(list, selector))
        .transpose()?;
    let target = request
        .task
        .as_ref()
        .map(|_| position(list, request))
        .transpose()?;
    let mut index = 0;
    for task in &mut list.tasks {
        for unit in std::iter::once(&mut task.unit)
            .chain(task.subtasks.iter_mut().map(|child| &mut child.unit))
        {
            if unit.is_none() {
                *unit = Some(Unit::new(context, index, ContractDraft::default()));
            }
            index += 1;
        }
    }
    if let Some(target) = target {
        request.task = at(list, target)?
            .unit
            .as_ref()
            .map(|unit| unit.reference.unit_id.clone());
    }
    request.parent = parent
        .and_then(|index| list.tasks.get(index))
        .and_then(|task| task.unit.as_ref())
        .map(|unit| unit.reference.unit_id.clone());
    Ok(())
}
