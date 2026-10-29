//! Human-facing task groups, separate from the model's complete summary.

use serde::{Deserialize, Serialize};

use crate::{Status, TodoList};

/// Semantic styling for a terminal row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(rename_all = "snake_case")]
pub enum WidgetKind {
    Heading,
    Active,
    Upcoming,
    Done,
    Dropped,
    Summary,
}

/// One row before terminal wrapping and theme application.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct WidgetRow {
    pub kind: WidgetKind,
    pub text: String,
}

/// One subtask of a section item, with its contract summary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct SectionChild {
    pub status: Status,
    pub text: String,
    pub description: Option<String>,
    pub unit: Option<String>,
}

/// One top-level task of a section, with every subtask.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct SectionItem {
    pub status: Status,
    pub text: String,
    pub review: bool,
    pub description: Option<String>,
    pub unit: Option<String>,
    pub children: Vec<SectionChild>,
}

/// The tasks of one status, in list order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct TodoSection {
    pub status: Status,
    pub count: usize,
    pub items: Vec<SectionItem>,
}

/// The section order. Presenters rely on it; `widget` reads the active section first.
const ORDER: [Status; 4] = [
    Status::InProgress,
    Status::Pending,
    Status::Completed,
    Status::Abandoned,
];

/// Groups tasks by status: always four sections, in `ORDER`, empty ones included.
#[must_use]
pub fn sections(list: &TodoList) -> Vec<TodoSection> {
    ORDER
        .into_iter()
        .map(|status| {
            let items: Vec<_> = list
                .tasks
                .iter()
                .filter(|task| task.status == status)
                .map(|task| SectionItem {
                    status: task.status,
                    text: task.content.clone(),
                    review: task.ready_for_review(),
                    description: task.description.clone(),
                    unit: task.unit.as_ref().map(crate::Unit::summary),
                    children: task
                        .subtasks
                        .iter()
                        .map(|child| SectionChild {
                            status: child.status,
                            text: child.content.clone(),
                            description: child.description.clone(),
                            unit: child.unit.as_ref().map(crate::Unit::summary),
                        })
                        .collect(),
                })
                .collect();
            TodoSection {
                status,
                count: items.len(),
                items,
            }
        })
        .collect()
}

const fn style(status: Status) -> (&'static str, WidgetKind) {
    match status {
        Status::Pending => ("○", WidgetKind::Upcoming),
        Status::InProgress => ("●", WidgetKind::Active),
        Status::Completed => ("✓", WidgetKind::Done),
        Status::Abandoned => ("−", WidgetKind::Dropped),
    }
}

fn group(section: &TodoSection, expanded: bool, rows: &mut Vec<WidgetRow>) {
    if section.items.is_empty() {
        return;
    }
    let label = match section.status {
        Status::InProgress => "In progress",
        Status::Pending => "Upcoming",
        Status::Completed => "Done",
        Status::Abandoned => "Dropped",
    };
    let (mark, kind) = style(section.status);
    rows.push(WidgetRow {
        kind: WidgetKind::Heading,
        text: format!("{label} · {}", section.count),
    });
    for item in &section.items {
        let review = if item.review {
            " · Ready for review"
        } else {
            ""
        };
        rows.push(WidgetRow {
            kind,
            text: format!("{mark} {}{review}", item.text),
        });
        if let Some(unit) = &item.unit {
            rows.push(WidgetRow {
                kind: WidgetKind::Summary,
                text: format!("  {unit}"),
            });
        }
        rows.extend(
            item.children
                .iter()
                .filter(|child| expanded || child.status == Status::InProgress)
                .map(|child| {
                    let (mark, kind) = style(child.status);
                    WidgetRow {
                        kind,
                        text: child.unit.as_ref().map_or_else(
                            || format!("  {mark} {}", child.text),
                            |unit| format!("  {mark} {} · {unit}", child.text),
                        ),
                    }
                }),
        );
    }
}

/// Active tasks by default; all status groups when expanded. Empty lists stay hidden.
#[must_use]
pub fn widget(list: &TodoList, expanded: bool) -> Option<Vec<WidgetRow>> {
    if list.tasks.is_empty() {
        return None;
    }
    let mut sections = sections(list).into_iter();
    let mut rows = Vec::new();
    if let Some(active) = sections.next() {
        group(&active, expanded, &mut rows);
    }
    if rows.is_empty() {
        rows.push(WidgetRow {
            kind: WidgetKind::Summary,
            text: "No task in progress".to_owned(),
        });
    }
    if expanded {
        for section in sections {
            group(&section, expanded, &mut rows);
        }
    }
    Some(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContractDraft, MutationContext, Subtask, Task, Unit};

    fn list() -> TodoList {
        TodoList {
            tasks: [
                ("Earlier work", Status::Completed),
                ("Build the widget", Status::InProgress),
                ("Check the layout", Status::Pending),
                ("Old approach", Status::Abandoned),
                ("Ship the change", Status::Pending),
            ]
            .into_iter()
            .map(|(content, status)| Task {
                content: content.to_owned(),
                description: None,
                status,
                subtasks: vec![],
                unit: None,
            })
            .collect(),
        }
    }

    #[test]
    fn collapsed_shows_only_active_task_text() {
        let list = list();
        let rows = widget(&list, false).unwrap();
        assert_eq!(
            rows,
            vec![
                WidgetRow {
                    kind: WidgetKind::Heading,
                    text: "In progress · 1".to_owned()
                },
                WidgetRow {
                    kind: WidgetKind::Active,
                    text: "● Build the widget".to_owned()
                },
            ]
        );
        assert!(
            crate::view(&list).contains("Earlier work"),
            "model still sees every task"
        );
    }

    #[test]
    fn expanded_groups_tasks_in_order_without_conflating_dropped_and_done() {
        let rows = widget(&list(), true).unwrap();
        let text: Vec<_> = rows.iter().map(|row| row.text.as_str()).collect();
        assert_eq!(
            text,
            [
                "In progress · 1",
                "● Build the widget",
                "Upcoming · 2",
                "○ Check the layout",
                "○ Ship the change",
                "Done · 1",
                "✓ Earlier work",
                "Dropped · 1",
                "− Old approach",
            ]
        );
    }

    #[test]
    fn empty_lists_hide_and_closed_lists_remain_discoverable() {
        assert_eq!(widget(&TodoList::default(), false), None);
        assert_eq!(widget(&TodoList::default(), true), None);
        let list = TodoList {
            tasks: vec![Task {
                content: "Finished".to_owned(),
                description: None,
                status: Status::Completed,
                subtasks: vec![],
                unit: None,
            }],
        };
        assert_eq!(
            widget(&list, false).unwrap(),
            vec![WidgetRow {
                kind: WidgetKind::Summary,
                text: "No task in progress".to_owned(),
            }]
        );
        assert_eq!(widget(&list, true).unwrap().len(), 3);
    }

    #[test]
    fn sections_are_four_status_groups_in_fixed_order() {
        let sections = sections(&list());
        let shape: Vec<_> = sections
            .iter()
            .map(|section| {
                (
                    section.status,
                    section.count,
                    section
                        .items
                        .iter()
                        .map(|item| item.text.as_str())
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                (Status::InProgress, 1, vec!["Build the widget"]),
                (
                    Status::Pending,
                    2,
                    vec!["Check the layout", "Ship the change"]
                ),
                (Status::Completed, 1, vec!["Earlier work"]),
                (Status::Abandoned, 1, vec!["Old approach"]),
            ]
        );
    }

    #[test]
    fn section_items_carry_review_unit_and_every_child() {
        let context = MutationContext {
            session_id: "s".to_owned(),
            mutation_id: "m".to_owned(),
        };
        let unit = |index| Unit::new(&context, index, ContractDraft::default());
        let mut list = list();
        list.tasks[1].unit = Some(unit(0));
        list.tasks[1].description = Some("Ship the widget".to_owned());
        list.tasks[1].subtasks = vec![
            Subtask {
                content: "Measure".to_owned(),
                description: Some("Time the cold start".to_owned()),
                status: Status::Completed,
                unit: Some(unit(1)),
            },
            Subtask {
                content: "Skip".to_owned(),
                description: None,
                status: Status::Abandoned,
                unit: None,
            },
        ];
        let sections = sections(&list);
        assert_eq!(
            sections[0].items[0],
            SectionItem {
                status: Status::InProgress,
                text: "Build the widget".to_owned(),
                review: true,
                description: Some("Ship the widget".to_owned()),
                unit: Some("u-m-0 r1 · draft · assignment missing · scope missing".to_owned()),
                children: vec![
                    SectionChild {
                        status: Status::Completed,
                        text: "Measure".to_owned(),
                        description: Some("Time the cold start".to_owned()),
                        unit: Some(
                            "u-m-1 r1 · draft · assignment missing · scope missing".to_owned()
                        ),
                    },
                    SectionChild {
                        status: Status::Abandoned,
                        text: "Skip".to_owned(),
                        description: None,
                        unit: None,
                    },
                ],
            },
            "children are complete, not filtered by expansion"
        );
        assert!(!sections[1].items[0].review, "no children, no review");
        assert_eq!(sections[1].items[0].unit, None);
    }

    #[test]
    fn sections_of_an_empty_list_are_four_empty_groups() {
        let sections = sections(&TodoList::default());
        assert_eq!(
            sections,
            ORDER.map(|status| TodoSection {
                status,
                count: 0,
                items: vec![],
            })
        );
    }
}
