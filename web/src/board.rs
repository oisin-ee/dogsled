//! Repository navigation. Keep attribution and lead/worker grouping in one place
//! until the board carries the server's explicit owner relationships.

use dioxus::prelude::*;
use portal_types::agents::AgentView;
use portal_types::{BoardView, GroupView, RowView, TodoView};

use crate::app::{InboxItem, InboxKind, InboxSelection};

#[derive(Clone, PartialEq)]
pub struct NavigationRepo {
    pub repository: Option<String>,
    pub rows: Vec<NavigationRow>,
    pub to_land: usize,
}

#[derive(Clone, PartialEq)]
pub struct NavigationRow {
    pub pane: Option<String>,
    pub title: String,
    pub role: String,
    pub host: String,
    pub state: String,
    pub worker: bool,
}

/// The only lead/worker grouping seam. Ledger rows are workers; Herdr agents
/// without a ledger row are leads. Do not infer a worker's owner from row order.
/// The server's owner_pane/title/unseen fields will replace this join.
pub fn navigation_groups(board: &BoardView, agents: &[AgentView]) -> Vec<NavigationRepo> {
    let mut groups: Vec<NavigationRepo> = board
        .repos
        .iter()
        .map(|repo| NavigationRepo {
            repository: repo.repository.clone(),
            rows: repo
                .rows
                .iter()
                .map(|row| navigation_row(row, &repo.todos))
                .collect(),
            to_land: repo
                .rows
                .iter()
                .filter(|row| row.group == GroupView::ToLand)
                .count(),
        })
        .collect();

    for agent in agents {
        if groups
            .iter()
            .flat_map(|group| &group.rows)
            .any(|row| row.pane.as_deref() == Some(&agent.pane))
        {
            continue;
        }
        let repository = agent.cwd.as_deref().and_then(repository_from_cwd);
        let index = groups
            .iter()
            .position(|group| match (&group.repository, &repository) {
                (Some(known), Some(name)) => repository_name(known) == *name,
                (None, None) => true,
                _ => false,
            })
            .unwrap_or_else(|| {
                groups.push(NavigationRepo {
                    repository,
                    rows: Vec::new(),
                    to_land: 0,
                });
                groups.len() - 1
            });
        let title = board
            .repos
            .iter()
            .flat_map(|repo| &repo.todos)
            .find(|todo| todo.pane == agent.pane)
            .map(|todo| todo.current.as_str())
            .filter(|title| !title.trim().is_empty())
            .or(agent.name.as_deref())
            .map(task_title)
            .unwrap_or_else(|| "Agent task".to_owned());
        groups[index].rows.push(NavigationRow {
            pane: Some(agent.pane.clone()),
            title,
            role: "lead".to_owned(),
            host: String::new(),
            state: agent.state.clone(),
            worker: false,
        });
    }
    for group in &mut groups {
        group.rows.sort_by_key(|row| row.worker);
    }
    groups
}

fn navigation_row(row: &RowView, todos: &[TodoView]) -> NavigationRow {
    let title = todos
        .iter()
        .find(|todo| row.pane.as_deref() == Some(&todo.pane))
        .map(|todo| todo.current.as_str())
        .filter(|title| !title.trim().is_empty())
        .unwrap_or(&row.name);
    NavigationRow {
        pane: row.pane.clone(),
        title: task_title(title),
        role: row.kind.clone(),
        host: row.host.clone(),
        state: if row.kind == "job" {
            "job".to_owned()
        } else {
            row.state.clone()
        },
        worker: true,
    }
}

/// The repository folder, not the name of its linked worktree.
fn repository_from_cwd(cwd: &str) -> Option<String> {
    let root = cwd
        .split("/.worktrees/")
        .next()
        .unwrap_or(cwd)
        .trim_end_matches('/');
    root.rsplit('/')
        .next()
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
}

pub fn repository_name(repository: &str) -> &str {
    repository.rsplit('/').next().unwrap_or(repository)
}

fn task_title(title: &str) -> String {
    if title.starts_with("u-") {
        "Agent task".to_owned()
    } else {
        title.to_owned()
    }
}

#[component]
pub fn RepoGroup(group: NavigationRepo, inbox: Vec<InboxItem>) -> Element {
    let count = group.rows.len();
    let branches = group.to_land;
    rsx! {
        section { class: "repo",
            if let Some(repository) = group.repository {
                h2 { title: "{repository}",
                    "{repository_name(&repository)}"
                    span { "{count} " if count == 1 { "agent" } else { "agents" } }
                }
            }
            for (index, row) in group.rows.into_iter().enumerate() {
                NavigationAgent { key: "{index}:{row.pane:?}", row, inbox: inbox.clone() }
            }
            if branches > 0 {
                div { class: "land",
                    span { "To land" }
                    span { "{branches} " if branches == 1 { "branch" } else { "branches" } }
                }
            }
        }
    }
}

#[component]
fn NavigationAgent(row: NavigationRow, inbox: Vec<InboxItem>) -> Element {
    let mut selected = use_context::<Signal<Option<String>>>();
    let mut selection = use_context::<Signal<Option<InboxSelection>>>();
    let pending = inbox
        .iter()
        .find(|item| Some(&item.selection.pane) == row.pane.as_ref());
    let tag = pending.map(|item| item.selection.kind);
    let is_selected = row.pane.is_some() && selected() == row.pane;
    let class = match (row.worker, is_selected) {
        (true, true) => "nav-agent worker selected",
        (true, false) => "nav-agent worker",
        (false, true) => "nav-agent selected",
        (false, false) => "nav-agent",
    };
    let dot = if tag.is_some() {
        "status-dot blocked"
    } else {
        match row.state.as_str() {
            "working" | "running" => "status-dot working",
            "done" | "closed" => "status-dot done",
            _ => "status-dot",
        }
    };
    let body = rsx! {
        span { class: dot, aria_hidden: true }
        span { class: "nav-title", "{row.title}" }
        if row.worker {
            span { class: if tag == Some(InboxKind::Prompt) { "worker-status prompt" } else { "worker-status" },
                if tag == Some(InboxKind::Prompt) { "prompt" } else { "{row.state}" }
            }
        } else {
            span { class: "nav-meta", "{row.role}" if !row.host.is_empty() { " · {row.host}" } }
            if let Some(kind) = tag {
                span { class: "nav-tag {kind.css()}", "{kind.tag()}" }
            }
        }
    };
    match row.pane {
        Some(pane) => rsx! {
            button { r#type: "button", class, title: "{row.title}", aria_pressed: is_selected,
                onclick: move |_| {
                    selected.set(Some(pane.clone()));
                    selection.set(None);
                },
                {body}
            }
        },
        None => rsx! { div { class, title: "{row.title}", {body} } },
    }
}

#[cfg(test)]
mod shell_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn navigation_keeps_ledger_rows_and_uses_todo_titles() -> Result<(), serde_json::Error> {
        let board: BoardView = serde_json::from_value(json!({
            "hosts": [], "generated_ms": 0,
            "repos": [{ "repository": "github.com/owner/repo", "rows": [{
                "group": "to_land", "kind": "agent", "id": "unit", "name": "u-hidden",
                "what": "pi · writer", "host": "netcup", "pane": "worker", "state": "done", "activity": "", "age_ms": 0
            }], "todos": [{ "session": "s", "branch": null, "revision": 0, "current": "Ship portal shell", "pane": "worker", "sections": [] }] }]
        }))?;
        let agents: Vec<AgentView> = serde_json::from_value(json!([
            { "pane": "worker", "state": "done", "cwd": "/dev/repo" },
            { "pane": "lead", "name": "Plan portal", "state": "working", "cwd": "/dev/repo/.worktrees/portal" }
        ]))?;
        let groups = navigation_groups(&board, &agents);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].rows.len(), 2);
        assert!(!groups[0].rows[0].worker);
        assert_eq!(groups[0].rows[1].title, "Ship portal shell");
        assert!(groups[0].rows[1].worker);
        assert_eq!(groups[0].to_land, 1);
        let disconnected = navigation_groups(&board, &[]);
        assert_eq!(disconnected[0].rows[0].title, "Ship portal shell");
        assert_eq!(disconnected[0].rows[0].host, "netcup");
        assert_eq!(task_title("u-not-a-title"), "Agent task");
        Ok(())
    }
}
