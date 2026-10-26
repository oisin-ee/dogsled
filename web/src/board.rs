//! The board: repositories, their rows grouped by what the owner must do, and their todos.

use dioxus::prelude::*;
use portal_types::{self as wire, GroupView, TodoStatusView};

/// The groups in display order, with their titles and CSS names.
const GROUPS: [(GroupView, &str, &str); 4] = [
    (GroupView::ToLand, "To land", "to_land"),
    (GroupView::Attention, "Attention", "attention"),
    (GroupView::Running, "Running", "running"),
    (GroupView::Waiting, "Waiting", "waiting"),
];

#[component]
pub fn RepoGroup(group: wire::RepoGroup) -> Element {
    let name = group
        .repository
        .clone()
        .unwrap_or_else(|| "Unsorted".to_owned());
    rsx! {
        details { class: "repo", open: true,
            summary { "{name}" }
            for (kind , title , css) in GROUPS {
                {
                    let rows: Vec<wire::RowView> = group
                        .rows
                        .iter()
                        .filter(|row| row.group == kind)
                        .cloned()
                        .collect();
                    rsx! {
                        if !rows.is_empty() {
                            h2 { class: "group-title", "{title}" }
                            for row in rows {
                                Row { key: "{row.kind}/{row.id}", row, css }
                            }
                        }
                    }
                }
            }
            if !group.todos.is_empty() {
                TodoList { todos: group.todos.clone() }
            }
        }
    }
}

#[component]
fn Row(row: wire::RowView, css: &'static str) -> Element {
    let mut selected = use_context::<Signal<Option<String>>>();
    let age = age(row.age_ms);
    let is_selected = row.pane.is_some() && selected() == row.pane;
    let pane = row.pane.clone();
    let body = rsx! {
        span { class: "pill pill-{css}", "{row.state}" }
        span { class: "what", title: "{row.what}", "{row.name}: {row.what}" }
        span { class: "host", "{row.host} · {age}" }
    };
    match pane {
        Some(pane) => rsx! {
            button {
                class: if is_selected { "row selected" } else { "row" },
                onclick: move |_| selected.set(Some(pane.clone())),
                {body}
            }
        },
        None => rsx! {
            div { class: "row", {body} }
        },
    }
}

#[component]
fn TodoList(todos: Vec<wire::TodoView>) -> Element {
    rsx! {
        div { class: "todos",
            h2 { class: "group-title", "Todos" }
            for todo in todos {
                div { key: "{todo.pane}", class: "todo",
                    div { class: "todo-head", "{todo.current}" }
                    for section in todo.sections {
                        ul {
                            for item in section.items {
                                li { class: status_class(item.status), "{item.text}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn status_class(status: TodoStatusView) -> &'static str {
    match status {
        TodoStatusView::InProgress => "todo-in_progress",
        TodoStatusView::Pending => "todo-pending",
        TodoStatusView::Completed => "todo-completed",
        TodoStatusView::Abandoned => "todo-abandoned",
    }
}

/// A short age: seconds, minutes, hours, or days.
fn age(ms: u64) -> String {
    let seconds = ms / 1000;
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86399 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86400),
    }
}
