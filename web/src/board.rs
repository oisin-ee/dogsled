//! Repository navigation, joined by the server's explicit owner relationships.

use dioxus::prelude::*;
use portal_types::agents::AgentView;
use portal_types::{BoardView, GroupView, RowView, TodoView};

use crate::app::{InboxItem, InboxKind, InboxSelection};

#[derive(Clone, Default, PartialEq)]
pub struct Navigation {
    pub repos: Vec<NavigationRepo>,
    pub finished_without_repository: usize,
}

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
    pub unseen: bool,
    pub workers: Vec<NavigationRow>,
}

/// Ledger rows are workers; Herdr agents without a ledger row are leads.
/// Closed rows contribute cleanup counts, never agent entries.
pub fn navigation_groups(board: &BoardView, agents: &[AgentView]) -> Navigation {
    let mut navigation = Navigation::default();
    let rows: Vec<_> = board.repos.iter().flat_map(|repo| &repo.rows).collect();
    for agent in agents.iter().filter(|agent| {
        agent.state != "closed"
            && !rows
                .iter()
                .any(|row| row.pane.as_deref() == Some(&agent.pane))
    }) {
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
        repository_group(&mut navigation.repos, agent.repository.clone())
            .rows
            .push(NavigationRow {
                pane: Some(agent.pane.clone()),
                title,
                role: "lead".to_owned(),
                host: String::new(),
                state: agent.state.clone(),
                worker: false,
                unseen: false,
                workers: Vec::new(),
            });
    }
    for repo in &board.repos {
        for row in &repo.rows {
            let repository = row
                .repository
                .as_ref()
                .or(repo.repository.as_ref())
                .cloned();
            let live = row.state != "closed"
                && agents.iter().any(|agent| {
                    agent.state != "closed" && row.pane.as_deref() == Some(&agent.pane)
                });
            if repository.is_none() && !live {
                navigation.finished_without_repository += 1;
                continue;
            }
            if row.group == GroupView::ToLand {
                repository_group(&mut navigation.repos, repository.clone()).to_land += 1;
            }
            if row.state == "closed" {
                continue;
            }
            let worker = navigation_row(row, &repo.todos);
            if let Some(lead) = navigation
                .repos
                .iter_mut()
                .flat_map(|repo| &mut repo.rows)
                .find(|lead| {
                    !lead.worker && row.owner_pane.is_some() && lead.pane == row.owner_pane
                })
            {
                lead.workers.push(worker);
            } else {
                repository_group(&mut navigation.repos, repository)
                    .rows
                    .push(worker);
            }
        }
    }
    navigation
        .repos
        .retain(|repo| !repo.rows.is_empty() || repo.to_land > 0);
    navigation
}

fn repository_group(
    groups: &mut Vec<NavigationRepo>,
    repository: Option<String>,
) -> &mut NavigationRepo {
    let index = groups
        .iter()
        .position(|group| group.repository == repository)
        .unwrap_or_else(|| {
            groups.push(NavigationRepo {
                repository,
                rows: Vec::new(),
                to_land: 0,
            });
            groups.len() - 1
        });
    &mut groups[index]
}

fn navigation_row(row: &RowView, todos: &[TodoView]) -> NavigationRow {
    let title = row
        .title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
        .or_else(|| {
            todos
                .iter()
                .find(|todo| row.pane.as_deref() == Some(&todo.pane))
                .map(|todo| todo.current.as_str())
                .filter(|title| !title.trim().is_empty())
        })
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
        unseen: row.unseen,
        workers: Vec::new(),
    }
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
    let count = group
        .rows
        .iter()
        .map(|row| 1 + row.workers.len())
        .sum::<usize>();
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
                div { class: "nav-lead", key: "{index}:{row.pane:?}",
                    NavigationAgent { row: row.clone(), inbox: inbox.clone() }
                    if !row.workers.is_empty() {
                        div { class: "nav-workers",
                            for (index, worker) in row.workers.into_iter().enumerate() {
                                NavigationAgent { key: "{index}:{worker.pane:?}", row: worker, inbox: inbox.clone() }
                            }
                        }
                    }
                }
            }
            if branches > 0 {
                div { class: "land",
                    span { "To land ·" }
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
    let pending = inbox.iter().find(|item| {
        Some(&item.selection.pane) == row.pane.as_ref()
            && (!row.worker || item.selection.kind == InboxKind::Prompt)
    });
    let tag = pending.map(|item| item.selection.kind);
    let needs_owner = tag == Some(InboxKind::Prompt) || row.unseen;
    let worker_status = worker_status(&row, tag);
    let is_selected = row.pane.is_some() && selected() == row.pane;
    let class = match (row.worker, is_selected) {
        (true, true) => "nav-agent worker selected",
        (true, false) => "nav-agent worker",
        (false, true) => "nav-agent selected",
        (false, false) => "nav-agent",
    };
    let dot = if row.worker {
        if needs_owner {
            "status-dot blocked"
        } else {
            "status-dot"
        }
    } else if tag.is_some() {
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
            span { class: if needs_owner { "worker-status prompt" } else { "worker-status" },
                "{worker_status}"
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

fn worker_status(row: &NavigationRow, tag: Option<InboxKind>) -> &str {
    if tag == Some(InboxKind::Prompt) {
        "prompt"
    } else if row.unseen {
        "idle · unseen"
    } else {
        &row.state
    }
}

#[cfg(test)]
mod shell_tests {
    use super::*;
    use serde_json::json;

    fn row(id: &str, owner: Option<&str>, state: &str) -> Result<RowView, serde_json::Error> {
        serde_json::from_value(json!({
            "group": "to_land", "kind": "agent", "id": id, "name": id,
            "what": "pi · writer", "host": "netcup", "pane": id, "owner_pane": owner,
            "state": state, "activity": "", "age_ms": 0
        }))
    }

    fn agents() -> Result<Vec<AgentView>, serde_json::Error> {
        serde_json::from_value(json!([
            { "pane": "lead-a", "name": "First lead", "state": "working", "repository": "github.com/owner/repo", "cwd": "/unrelated" },
            { "pane": "worker-b", "state": "done", "repository": "github.com/owner/repo" },
            { "pane": "lead-b", "name": "Second lead", "state": "idle", "repository": "github.com/owner/repo" }
        ]))
    }

    fn board(rows: Vec<RowView>) -> BoardView {
        BoardView {
            hosts: vec![],
            generated_ms: 0,
            repos: vec![portal_types::RepoGroup {
                repository: Some("github.com/owner/repo".to_owned()),
                rows,
                todos: vec![],
            }],
        }
    }

    #[test]
    fn workers_nest_under_matching_lead_and_orphans_follow_leads() -> Result<(), serde_json::Error>
    {
        let board = board(vec![
            row("worker-b", Some("lead-b"), "done")?,
            row("orphan", Some("missing"), "working")?,
            row("worker-a", Some("lead-a"), "working")?,
        ]);
        let groups = navigation_groups(&board, &agents()?);
        assert_eq!(groups.repos.len(), 1);
        let rows = &groups.repos[0].rows;
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].pane.as_deref(), Some("lead-a"));
        assert_eq!(rows[0].workers[0].pane.as_deref(), Some("worker-a"));
        assert_eq!(rows[1].pane.as_deref(), Some("lead-b"));
        assert_eq!(rows[1].workers[0].pane.as_deref(), Some("worker-b"));
        assert!(rows[1].workers[0].worker);
        assert_eq!(rows[2].pane.as_deref(), Some("orphan"));
        assert!(rows[2].worker);
        Ok(())
    }

    #[test]
    fn closed_rows_only_count_toward_repository_cleanup() -> Result<(), serde_json::Error> {
        let mut closed = row("closed", Some("lead-a"), "closed")?;
        closed.unseen = true;
        let mut not_to_land = closed.clone();
        not_to_land.id = "old".to_owned();
        not_to_land.group = GroupView::Attention;
        let groups = navigation_groups(&board(vec![closed, not_to_land]), &[]);
        assert_eq!(groups.repos.len(), 1);
        assert!(groups.repos[0].rows.is_empty());
        assert_eq!(groups.repos[0].to_land, 1);
        Ok(())
    }

    #[test]
    fn unknown_repository_without_live_pane_is_one_cleanup_count() -> Result<(), serde_json::Error>
    {
        let mut board = board(vec![
            row("closed", None, "closed")?,
            row("stale-pane", None, "done")?,
            row("worker-b", None, "idle")?,
        ]);
        board.repos[0].repository = None;
        let groups = navigation_groups(&board, &agents()?);
        assert_eq!(groups.finished_without_repository, 2);
        let unknown = groups.repos.iter().find(|repo| repo.repository.is_none());
        assert!(unknown.is_some_and(
            |repo| repo.rows.len() == 1 && repo.rows[0].pane.as_deref() == Some("worker-b")
        ));
        Ok(())
    }

    #[test]
    fn title_precedence_and_worker_status_are_server_driven() -> Result<(), serde_json::Error> {
        let mut row = row("u-hidden", None, "idle")?;
        row.title = Some("Server task title".to_owned());
        let todos = vec![TodoView {
            session: "s".to_owned(),
            branch: None,
            revision: 0,
            current: "Todo title".to_owned(),
            pane: "u-hidden".to_owned(),
            sections: vec![],
        }];
        assert_eq!(navigation_row(&row, &todos).title, "Server task title");
        row.title = None;
        assert_eq!(navigation_row(&row, &todos).title, "Todo title");
        assert_eq!(navigation_row(&row, &[]).title, "Agent task");
        row.unseen = true;
        let worker = navigation_row(&row, &[]);
        assert_eq!(worker_status(&worker, None), "idle · unseen");
        assert_eq!(worker_status(&worker, Some(InboxKind::Prompt)), "prompt");
        row.unseen = false;
        row.state = "working".to_owned();
        assert_eq!(
            worker_status(&navigation_row(&row, &[]), Some(InboxKind::Review)),
            "working"
        );
        Ok(())
    }
}
