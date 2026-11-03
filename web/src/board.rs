//! Repository navigation, joined by the server's explicit owner relationships.

use dioxus::prelude::*;
use portal_types::agents::AgentView;
use portal_types::{BoardView, GroupView, RowView, TodoView};

use crate::app::{InboxItem, InboxKind, InboxSelection};
use crate::components::item::{Item, ItemContent, ItemSize, ItemTitle, ItemVariant};
use crate::components::sidebar::{
    SidebarGroup, SidebarGroupContent, SidebarGroupLabel, SidebarMenu, SidebarMenuButton,
    SidebarMenuButtonSize, SidebarMenuItem, SidebarMenuSub, SidebarMenuSubItem,
};

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

impl NavigationRepo {
    pub fn agent_count(&self) -> usize {
        self.rows
            .iter()
            .flat_map(|row| std::iter::once(row).chain(&row.workers))
            .filter(|row| row.role != "job")
            .count()
    }
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
    pub progress: Option<(usize, usize)>,
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
            .map(|todo| todo.current.as_str());
        let title = agent_title(title, agent);
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
                progress: todo_progress(
                    board
                        .repos
                        .iter()
                        .flat_map(|repo| &repo.todos)
                        .find(|todo| todo.pane == agent.pane),
                ),
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

fn todo_progress(todo: Option<&TodoView>) -> Option<(usize, usize)> {
    let todo = todo?;
    let total = todo.sections.iter().map(|section| section.count).sum();
    let completed = todo
        .sections
        .iter()
        .filter(|section| section.status == portal_types::TodoStatusView::Completed)
        .map(|section| section.count)
        .sum();
    Some((completed, total))
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
        progress: todo_progress(
            todos
                .iter()
                .find(|todo| row.pane.as_deref() == Some(&todo.pane)),
        ),
    }
}

pub fn repository_name(repository: &str) -> &str {
    repository.rsplit('/').next().unwrap_or(repository)
}

/// The title of an agent: its todo, then its first message, then its Herdr
/// name, then its worktree folder.
pub fn agent_title(todo: Option<&str>, agent: &AgentView) -> String {
    let readable = |title: &&str| !title.trim().is_empty() && !title.starts_with("u-");
    todo.filter(readable)
        .or(agent.title.as_deref().filter(readable))
        .or(agent.name.as_deref().filter(readable))
        .or_else(|| worktree(agent.cwd.as_deref()?))
        .unwrap_or("Agent task")
        .to_owned()
}

/// The folder of a linked worktree, which names its branch.
fn worktree(cwd: &str) -> Option<&str> {
    let (_, folder) = cwd.trim_end_matches('/').rsplit_once("/.worktrees/")?;
    (!folder.is_empty() && !folder.contains('/')).then_some(folder)
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
    let count = group.agent_count();
    let branches = group.to_land;
    rsx! {
        SidebarGroup {
            if let Some(repository) = group.repository {
                SidebarGroupLabel { title: "{repository}",
                    span { "{repository_name(&repository)}" }
                    span { "{count}" }
                }
            }
            SidebarGroupContent {
                SidebarMenu {
                    for (index, row) in group.rows.into_iter().enumerate() {
                        SidebarMenuItem { key: "{index}:{row.pane:?}",
                            NavigationAgent { row: row.clone(), inbox: inbox.clone() }
                            if !row.workers.is_empty() {
                                SidebarMenuSub {
                                    for (index, worker) in row.workers.into_iter().enumerate() {
                                        SidebarMenuSubItem { key: "{index}:{worker.pane:?}",
                                            NavigationAgent { row: worker, inbox: inbox.clone() }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if branches > 0 {
                        SidebarMenuItem {
                            Item { variant: ItemVariant::Muted, size: ItemSize::Sm,
                                ItemContent {
                                    ItemTitle { "{branches} " if branches == 1 { "branch to land" } else { "branches to land" } }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn NavigationAgent(row: NavigationRow, inbox: Vec<InboxItem>) -> Element {
    let mut selected = use_context::<Signal<Option<String>>>();
    let mut selection = use_context::<Signal<Option<InboxSelection>>>();
    let mut panel = use_context::<Signal<crate::app::LeadPanel>>();
    let pending = inbox.iter().find(|item| {
        Some(&item.selection.pane) == row.pane.as_ref()
            && (!row.worker || item.selection.kind == InboxKind::Prompt)
    });
    let tag = pending.map(|item| item.selection.kind);
    let needs_owner = tag == Some(InboxKind::Prompt) || row.unseen;
    let worker_status = worker_status(&row, tag);
    let label = tag.map_or_else(
        || row.title.clone(),
        |kind| format!("{} · {}", row.title, kind.tag()),
    );
    let is_selected = row.pane.is_some() && selected() == row.pane;
    let status = if needs_owner || (!row.worker && tag.is_some()) {
        "waiting"
    } else if !row.worker && matches!(row.state.as_str(), "working" | "running") {
        "running"
    } else {
        "muted"
    };
    let size = if row.worker {
        SidebarMenuButtonSize::Sm
    } else {
        SidebarMenuButtonSize::Default
    };
    let body = rsx! {
        span { class: "agent-dot", "data-status": status, aria_hidden: "true" }
        span { class: "agent-title", "{row.title}" }
        if row.worker { small { "{worker_status}" } }
        else if let Some((done, total)) = row.progress { small { "{done}/{total}" } }
    };
    rsx! {
        SidebarMenuButton {
            is_active: is_selected,
            "data-worker": row.worker,
            "data-needs-owner": needs_owner,
            size,
            title: "{label}",
            r#as: move |attributes: Vec<Attribute>| {
                let body = body.clone();
                match row.pane.clone() {
                    Some(pane) => rsx! {
                        button {
                            r#type: "button",
                            aria_current: is_selected.then_some("true"),
                            onclick: move |_| {
                                selected.set(Some(pane.clone()));
                                selection.set(None);
                                panel.set(crate::app::LeadPanel::Chat);
                            },
                            ..attributes,
                            {body}
                        }
                    },
                    // A queued task or a job has no pane to open, so its row only shows state.
                    None => rsx! { div { ..attributes, {body} } },
                }
            },
        }
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
    fn navigation_counts_agents_without_removing_jobs() -> Result<(), serde_json::Error> {
        let mut nested_job = row("sh", Some("lead-a"), "working")?;
        nested_job.kind = "job".to_owned();
        let mut standalone_job = row("pnpm", None, "working")?;
        standalone_job.kind = "job".to_owned();
        let groups = navigation_groups(
            &board(vec![
                row("worker-b", Some("lead-a"), "working")?,
                nested_job,
                standalone_job,
            ]),
            &agents()?,
        );
        let repo = &groups.repos[0];
        assert_eq!(repo.agent_count(), 3);
        assert_eq!(repo.rows.len(), 3);
        assert_eq!(repo.rows[0].workers.len(), 2);
        assert!(repo.rows[0].workers.iter().any(|row| row.role == "job"));
        assert!(repo.rows.iter().any(|row| row.role == "job"));
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
    fn a_lead_title_falls_back_from_todo_to_first_message_to_name_to_worktree() {
        let mut agent = AgentView {
            pane: "p".to_owned(),
            name: Some("u-0cc2".to_owned()),
            title: None,
            agent: None,
            state: "idle".to_owned(),
            cwd: Some("/r/.worktrees/fix-portal-titles".to_owned()),
            repository: None,
            blocked: None,
        };
        assert_eq!(agent_title(None, &agent), "fix-portal-titles");
        agent.name = Some("reviewer".to_owned());
        assert_eq!(agent_title(None, &agent), "reviewer");
        agent.title = Some("Ship the redesign".to_owned());
        assert_eq!(agent_title(Some(" "), &agent), "Ship the redesign");
        assert_eq!(agent_title(Some("Verify it"), &agent), "Verify it");
        agent.title = None;
        agent.name = None;
        agent.cwd = Some("/r".to_owned());
        assert_eq!(agent_title(None, &agent), "Agent task");
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
