//! Strict map deserialization enumerates supplied keys at the JavaScript boundary.
use crate::{Request, Subtask, Task, TodoList};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::fmt;

fn field<'de, M, T>(map: &mut M, slot: &mut Option<T>, name: &'static str) -> Result<(), M::Error>
where
    M: MapAccess<'de>,
    T: Deserialize<'de>,
{
    if slot.is_some() {
        return Err(de::Error::duplicate_field(name));
    }
    *slot = Some(map.next_value()?);
    Ok(())
}

struct ListVisitor;
impl<'de> Visitor<'de> for ListVisitor {
    type Value = TodoList;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a todo list map")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut tasks = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "tasks" => field(&mut map, &mut tasks, "tasks")?,
                _ => return Err(de::Error::unknown_field(&key, &["tasks"])),
            }
        }
        Ok(TodoList {
            tasks: tasks.ok_or_else(|| de::Error::missing_field("tasks"))?,
        })
    }
}

impl<'de> Deserialize<'de> for TodoList {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(ListVisitor)
    }
}

struct TaskVisitor;
impl<'de> Visitor<'de> for TaskVisitor {
    type Value = Task;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an outcome map")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let (mut content, mut status, mut subtasks, mut unit) = (None, None, None, None);
        let mut description = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "content" => field(&mut map, &mut content, "content")?,
                "description" => field(&mut map, &mut description, "description")?,
                "status" => field(&mut map, &mut status, "status")?,
                "subtasks" => field(&mut map, &mut subtasks, "subtasks")?,
                "unit" => field(&mut map, &mut unit, "unit")?,
                _ => {
                    return Err(de::Error::unknown_field(
                        &key,
                        &["content", "description", "status", "subtasks", "unit"],
                    ));
                }
            }
        }
        Ok(Task {
            content: content.ok_or_else(|| de::Error::missing_field("content"))?,
            description: description.unwrap_or_default(),
            status: status.ok_or_else(|| de::Error::missing_field("status"))?,
            subtasks: subtasks.unwrap_or_default(),
            unit: unit.unwrap_or_default(),
        })
    }
}

impl<'de> Deserialize<'de> for Task {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(TaskVisitor)
    }
}

struct SubtaskVisitor;
impl<'de> Visitor<'de> for SubtaskVisitor {
    type Value = Subtask;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a leaf work unit map")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let (mut content, mut status, mut unit) = (None, None, None);
        let mut description = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "content" => field(&mut map, &mut content, "content")?,
                "description" => field(&mut map, &mut description, "description")?,
                "status" => field(&mut map, &mut status, "status")?,
                "unit" => field(&mut map, &mut unit, "unit")?,
                _ => {
                    return Err(de::Error::unknown_field(
                        &key,
                        &["content", "description", "status", "unit"],
                    ));
                }
            }
        }
        Ok(Subtask {
            content: content.ok_or_else(|| de::Error::missing_field("content"))?,
            description: description.unwrap_or_default(),
            status: status.ok_or_else(|| de::Error::missing_field("status"))?,
            unit: unit.unwrap_or_default(),
        })
    }
}

impl<'de> Deserialize<'de> for Subtask {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(SubtaskVisitor)
    }
}

struct RequestVisitor;
impl<'de> Visitor<'de> for RequestVisitor {
    type Value = Request;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a todo operation map")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let (mut op, mut items, mut task, mut parent) = (None, None, None, None);
        let (mut contract, mut start, mut evidence) = (None, None, None);
        let mut description = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "op" => field(&mut map, &mut op, "op")?,
                "items" => field(&mut map, &mut items, "items")?,
                "task" => field(&mut map, &mut task, "task")?,
                "parent" => field(&mut map, &mut parent, "parent")?,
                "contract" => field(&mut map, &mut contract, "contract")?,
                "start" => field(&mut map, &mut start, "start")?,
                "evidence" => field(&mut map, &mut evidence, "evidence")?,
                "description" => field(&mut map, &mut description, "description")?,
                _ => {
                    return Err(de::Error::unknown_field(
                        &key,
                        &[
                            "op",
                            "items",
                            "task",
                            "parent",
                            "contract",
                            "start",
                            "evidence",
                            "description",
                        ],
                    ));
                }
            }
        }
        Ok(Request {
            op: op.ok_or_else(|| de::Error::missing_field("op"))?,
            items: items.unwrap_or_default(),
            task: task.unwrap_or_default(),
            parent: parent.unwrap_or_default(),
            contract: contract.unwrap_or_default(),
            start: start.unwrap_or_default(),
            evidence: evidence.unwrap_or_default(),
            description: description.unwrap_or_default(),
        })
    }
}

impl<'de> Deserialize<'de> for Request {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(RequestVisitor)
    }
}
