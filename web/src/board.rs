//! Repository navigation, joined by the server's explicit owner relationships.

use dioxus::prelude::*;
use portal_types::agents::{AgentAddress, AgentView};
use portal_types::{BoardView, GroupView, RowView, TodoView};

use crate::app::{InboxItem, InboxKind, InboxSelection};
use crate::components::sidebar::{
    SidebarGroup, SidebarGroupContent, SidebarGroupLabel, SidebarMenu, SidebarMenuButton,
    SidebarMenuButtonSize, SidebarMenuItem,
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
        self.rows.iter().filter(|row| !row.worker).count()
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
    pub jobs: Vec<NavigationRow>,
    pub stale: bool,
    pub progress: Option<(usize, usize)>,
}

/// An agent with pending inbox items. It leaves its repository group and moves to the top of
/// the sidebar until the owner answers.
#[derive(Clone, PartialEq)]
pub struct WaitingAgent {
    pub row: NavigationRow,
    pub repository: Option<String>,
    /// Newest first, as the inbox orders them.
    pub items: Vec<InboxItem>,
}

/// Moves every agent with an inbox item out of its repository group, newest item first.
/// A worker nested under a lead is copied, so the lead keeps its full worker list. An item
/// whose agent has no navigation row still gets a row, so no waiting item is hidden.
pub fn take_waiting(navigation: &mut Navigation, inbox: &[InboxItem]) -> Vec<WaitingAgent> {
    let mut waiting: Vec<WaitingAgent> = Vec::new();
    for item in inbox {
        let (host, pane) = (&item.selection.host, &item.selection.pane);
        if let Some(agent) = waiting
            .iter_mut()
            .find(|agent| &agent.row.host == host && agent.row.pane.as_ref() == Some(pane))
        {
            agent.items.push(item.clone());
            continue;
        }
        let matches = |row: &NavigationRow| &row.host == host && row.pane.as_ref() == Some(pane);
        let listed = navigation.repos.iter_mut().find_map(|repo| {
            let index = repo.rows.iter().position(matches)?;
            Some((repo.rows.remove(index), repo.repository.clone()))
        });
        let nested = || {
            navigation.repos.iter().find_map(|repo| {
                repo.rows
                    .iter()
                    .flat_map(|lead| &lead.workers)
                    .find(|worker| matches(worker))
                    .map(|worker| (worker.clone(), repo.repository.clone()))
            })
        };
        let (row, repository) = listed.or_else(nested).unwrap_or_else(|| {
            (
                NavigationRow {
                    pane: Some(pane.clone()),
                    title: item
                        .agent_title
                        .as_deref()
                        .map_or_else(|| "Agent task".to_owned(), task_title),
                    role: "agent".to_owned(),
                    host: host.clone(),
                    state: "waiting".to_owned(),
                    worker: item.selection.kind == InboxKind::UnseenWorker,
                    unseen: false,
                    workers: Vec::new(),
                    jobs: Vec::new(),
                    stale: false,
                    progress: None,
                },
                item.repository.clone(),
            )
        });
        waiting.push(WaitingAgent {
            row,
            repository,
            items: vec![item.clone()],
        });
    }
    navigation
        .repos
        .retain(|repo| !repo.rows.is_empty() || repo.to_land > 0);
    waiting
}

/// Ledger rows are workers; Herdr agents without a ledger row are leads, unless a Dogsled
/// launch started them. Closed rows remain in their lead's Workers tab, but never become
/// sidebar entries.
pub fn navigation_groups(board: &BoardView, agents: &[AgentView]) -> Navigation {
    let mut navigation = Navigation::default();
    let rows: Vec<_> = board
        .repos
        .iter()
        .flat_map(|repo| &repo.rows)
        .filter(|row| is_navigation_row(row))
        .collect();
    let unlisted = |agent: &&AgentView| {
        agent.state != "closed"
            && !rows
                .iter()
                .any(|row| row.host == agent.host && row.pane.as_deref() == Some(&agent.pane))
    };
    for agent in agents
        .iter()
        .filter(unlisted)
        .filter(|agent| agent.launch.is_none() && agent.agent.as_deref() != Some("dogsled-job"))
    {
        let title = board
            .repos
            .iter()
            .flat_map(|repo| &repo.todos)
            .find(|todo| agent.host == "local" && todo.pane == agent.pane)
            .map(|todo| todo.current.as_str());
        let title = agent_title(title, agent);
        repository_group(&mut navigation.repos, agent.repository.clone())
            .rows
            .push(NavigationRow {
                pane: Some(agent.pane.clone()),
                title,
                role: "lead".to_owned(),
                host: agent.host.clone(),
                state: agent.state.clone(),
                worker: false,
                unseen: false,
                workers: Vec::new(),
                jobs: Vec::new(),
                stale: false,
                progress: todo_progress(
                    board
                        .repos
                        .iter()
                        .flat_map(|repo| &repo.todos)
                        .find(|todo| agent.host == "local" && todo.pane == agent.pane),
                ),
            });
    }
    let worker_addresses: std::collections::BTreeSet<_> = rows
        .iter()
        .filter(|r| r.kind == "agent" && r.state != "closed")
        .filter_map(|r| Some((r.host.as_str(), r.pane.as_deref()?)))
        .chain(
            agents
                .iter()
                .filter(|a| a.launch.is_some() && a.state != "closed")
                .map(|a| (a.host.as_str(), a.pane.as_str())),
        )
        .collect();
    for repo in &board.repos {
        for row in repo.rows.iter().filter(|row| is_navigation_row(row)) {
            let repository = row
                .repository
                .as_ref()
                .or(repo.repository.as_ref())
                .cloned();
            let live = row.state != "closed"
                && agents.iter().any(|agent| {
                    agent.state != "closed"
                        && row.host == agent.host
                        && row.pane.as_deref() == Some(&agent.pane)
                });
            let has_lead = navigation
                .repos
                .iter()
                .flat_map(|repo| &repo.rows)
                .any(|lead| !lead.worker && owns_worker(lead, row));
            if repository.is_none() && !live && !has_lead {
                navigation.finished_without_repository += 1;
                continue;
            }
            if row.group == GroupView::ToLand {
                repository_group(&mut navigation.repos, repository.clone()).to_land += 1;
            }
            let worker = navigation_row(row, &repo.todos);
            if let Some(lead) = navigation
                .repos
                .iter_mut()
                .flat_map(|repo| &mut repo.rows)
                .find(|lead| !lead.worker && owns_worker(lead, row))
            {
                if row.kind == "job" {
                    lead.jobs.push(worker);
                } else {
                    lead.workers.push(worker);
                }
            } else if row.state != "closed"
                && !row.owner_pane.as_deref().is_some_and(|pane| {
                    worker_addresses
                        .contains(&(row.owner_host.as_deref().unwrap_or(&row.host), pane))
                })
            {
                repository_group(&mut navigation.repos, repository)
                    .rows
                    .push(worker);
            }
        }
    }
    // A launched agent whose owner has no ledger row for it, such as a start that its owner
    // gave up on, is still a worker. It stays visible, so it can be opened and closed.
    for agent in agents
        .iter()
        .filter(unlisted)
        .filter(|agent| agent.launch.is_some() || agent.agent.as_deref() == Some("dogsled-job"))
    {
        let job = agent.agent.as_deref() == Some("dogsled-job");
        repository_group(&mut navigation.repos, agent.repository.clone())
            .rows
            .push(NavigationRow {
                pane: Some(agent.pane.clone()),
                title: if job {
                    "Job · owner unavailable".to_owned()
                } else {
                    agent_title(None, agent)
                },
                role: if job { "job" } else { "agent" }.to_owned(),
                host: agent.host.clone(),
                state: agent.state.clone(),
                worker: true,
                unseen: false,
                workers: Vec::new(),
                jobs: Vec::new(),
                stale: false,
                progress: None,
            });
    }
    navigation
        .repos
        .retain(|repo| !repo.rows.is_empty() || repo.to_land > 0);
    // Groups keep their places as agents change state, so sort them by the name the sidebar
    // shows. Agents without a repository come last.
    navigation.repos.sort_by_cached_key(|repo| {
        repo.repository
            .as_deref()
            .map_or((true, String::new(), String::new()), |repository| {
                (
                    false,
                    repository_name(repository).to_lowercase(),
                    repository.to_owned(),
                )
            })
    });
    navigation
}

/// Resolve the root through host-qualified direct owners, not the viewing or execution host.
pub(crate) fn lead_hostname<'a>(
    board: &'a BoardView,
    agents: &[AgentView],
    selected: &AgentAddress,
) -> Option<&'a str> {
    let mut address = (selected.host.as_str(), selected.pane.as_str());
    let mut seen = std::collections::BTreeSet::new();
    while seen.insert(address) {
        let mut matches = board
            .repos
            .iter()
            .flat_map(|repo| &repo.rows)
            .filter(|row| row.host == address.0 && row.pane.as_deref() == Some(address.1));
        if let Some(row) = matches.next() {
            if matches.next().is_some() || row.kind != "agent" || row.ownership_stale {
                return None;
            }
            address = (row.owner_host.as_deref()?, row.owner_pane.as_deref()?);
            continue;
        }
        let root = agents
            .iter()
            .find(|agent| agent.host == address.0 && agent.pane == address.1)?;
        if root.launch.is_some() || root.agent.as_deref() == Some("dogsled-job") {
            return None;
        }
        return board
            .hosts
            .iter()
            .find(|host| host.label == address.0)?
            .hostname
            .as_deref();
    }
    None
}

/// Decide whether a restored address can open a conversation before mounting AgentView.
pub(crate) fn status_only(
    board: &BoardView,
    agents: &[AgentView],
    address: &AgentAddress,
) -> Option<&'static str> {
    let rows = || {
        board
            .repos
            .iter()
            .flat_map(|repo| &repo.rows)
            .filter(|r| r.host == address.host && r.pane.as_ref() == Some(&address.pane))
    };
    if rows().any(|r| r.kind == "job")
        || agents.iter().any(|a| {
            a.host == address.host
                && a.pane == address.pane
                && a.agent.as_deref() == Some("dogsled-job")
        })
    {
        Some("Command job · status only")
    } else if rows().any(|r| r.ownership_stale) {
        Some("Ownership unavailable · last known status only")
    } else {
        None
    }
}

pub(crate) fn is_navigation_row(row: &RowView) -> bool {
    row.kind != "hold"
}

pub(crate) fn owns_worker(lead: &NavigationRow, row: &RowView) -> bool {
    lead.host == row.owner_host.as_deref().unwrap_or(&row.host)
        && row.owner_pane.is_some()
        && lead.pane == row.owner_pane
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
        state: row.state.clone(),
        worker: true,
        unseen: row.unseen,
        workers: Vec::new(),
        jobs: Vec::new(),
        stale: row.ownership_stale,
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
/// name, then its worktree folder, then the folder it runs in.
pub fn agent_title(todo: Option<&str>, agent: &AgentView) -> String {
    let readable = |title: &&str| !title.trim().is_empty() && !generated(title);
    todo.filter(readable)
        .or(agent.title.as_deref().filter(readable))
        .or(agent.name.as_deref().filter(readable))
        .or_else(|| worktree(agent.cwd.as_deref()?))
        .or_else(|| folder(agent.cwd.as_deref()?))
        .unwrap_or("Agent task")
        .to_owned()
}

/// A name Dogsled made from an id: a unit's `u-…`, or an older lead's `lead-r-…`.
fn generated(name: &str) -> bool {
    name.starts_with("u-") || name.starts_with("lead-r-")
}

/// The folder of a linked worktree, which names its branch.
fn worktree(cwd: &str) -> Option<&str> {
    let (_, folder) = cwd.trim_end_matches('/').rsplit_once("/.worktrees/")?;
    (!folder.is_empty() && !folder.contains('/')).then_some(folder)
}

/// The last folder of a path, which names the checkout an agent runs in.
fn folder(cwd: &str) -> Option<&str> {
    cwd.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|folder| !folder.is_empty())
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
    let land = if branches == 1 {
        "1 branch to land".to_owned()
    } else {
        format!("{branches} branches to land")
    };
    rsx! {
        SidebarGroup {
            if let Some(repository) = group.repository {
                SidebarGroupLabel { title: "{repository}",
                    span { "{repository_name(&repository)}" }
                    if branches > 0 { span { class: "repo-land", title: "{land}", "{branches} to land" } }
                    else { span { "{count}" } }
                }
            }
            SidebarGroupContent {
                SidebarMenu {
                    for (index, row) in group.rows.into_iter().enumerate() {
                        SidebarMenuItem { key: "{index}:{row.pane:?}",
                            NavigationAgent { row: row.clone() }
                            if !row.workers.is_empty() || !row.jobs.is_empty() {
                                WorkerSummary { lead: row, inbox: inbox.clone() }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Agents with inbox items have already moved to the top of the sidebar, so a row here
/// never carries a pending item of its own.
#[component]
fn NavigationAgent(row: NavigationRow) -> Element {
    let mut selected = use_context::<Signal<Option<AgentAddress>>>();
    let mut selection = use_context::<Signal<Option<InboxSelection>>>();
    let mut panel = use_context::<Signal<crate::app::LeadPanel>>();
    let needs_owner = row.unseen;
    let worker_status = worker_status(&row);
    let label = row.title.clone();
    let is_selected = selected().is_some_and(|address| {
        (address.host == row.host && Some(&address.pane) == row.pane.as_ref())
            || row.workers.iter().any(|worker| {
                worker.host == address.host && worker.pane.as_ref() == Some(&address.pane)
            })
    });
    let status = if needs_owner {
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
    let now = if needs_owner {
        "Waiting for you".to_owned()
    } else {
        state_phrase(&row.state)
    };
    let body = rsx! {
        span { class: "agent-dot", "data-status": status, aria_hidden: "true" }
        span { class: "agent-title", "{row.title}" }
        if row.worker { small { "{worker_status}" } }
        else {
            if let Some((done, total)) = row.progress { small { "{done}/{total}" } }
            span { class: "agent-now", "data-status": status, "{now}" }
        }
        if let Some(pane) = &row.pane {
            crate::unread::UnreadDot { host: row.host.clone(), pane: pane.clone() }
        }
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
                let host = row.host.clone();
                match row.pane.clone().filter(|_| row.role != "job" && !row.stale && row.state != "closed") {
                    Some(pane) => rsx! {
                        button {
                            r#type: "button",
                            aria_current: is_selected.then_some("true"),
                            onclick: move |_| {
                                selected.set(Some(AgentAddress { host: host.clone(), pane: pane.clone() }));
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

/// A lead's coarse state as the sidebar says it: "working" reads "Working".
fn state_phrase(state: &str) -> String {
    let mut letters = state.chars();
    letters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(letters).collect()
    })
}

#[component]
pub(crate) fn WorkerSummary(lead: NavigationRow, inbox: Vec<InboxItem>) -> Element {
    let mut selected = use_context::<Signal<Option<AgentAddress>>>();
    let mut selection = use_context::<Signal<Option<InboxSelection>>>();
    let mut panel = use_context::<Signal<crate::app::LeadPanel>>();
    let mut detail = use_context::<crate::app::LeadDetail>().0;
    let finished = lead
        .workers
        .iter()
        .filter(|worker| crate::workers::finished(&worker.state))
        .count();
    let count = lead.workers.len() - finished;
    let working = lead
        .workers
        .iter()
        .filter(|worker| matches!(worker.state.as_str(), "working" | "running"))
        .count();
    let mut summary = worker_summary(count, working, finished);
    if !lead.jobs.is_empty() {
        let active = lead
            .jobs
            .iter()
            .filter(|row| !crate::workers::job_finished(&row.state))
            .count();
        if !summary.is_empty() {
            summary.push_str(" · ");
        }
        let count = if active == 0 { lead.jobs.len() } else { active };
        let state = if active == 0 { "finished " } else { "" };
        let noun = if count == 1 { "job" } else { "jobs" };
        summary.push_str(&format!("{count} {state}{noun}"));
    }
    let states: Vec<_> = lead
        .workers
        .iter()
        .filter(|worker| !crate::workers::finished(&worker.state))
        .map(|worker| {
            let waiting = worker.unseen
                || inbox.iter().any(|item| {
                    item.selection.kind == InboxKind::Prompt
                        && item.selection.host == worker.host
                        && Some(&item.selection.pane) == worker.pane.as_ref()
                });
            crate::workers::WorkerGroup::from_state(&worker.state, waiting)
        })
        .collect();
    let waiting = states.contains(&crate::workers::WorkerGroup::Waiting);
    rsx! {
        crate::components::button::Button { class: "worker-summary", variant: crate::components::button::ButtonVariant::Ghost,
            r#type: "button", "data-waiting": waiting,
            aria_label: "Show workers for {lead.title}: {summary}",
            onclick: move |_| {
                selected.set(lead.pane.clone().map(|pane| AgentAddress { host: lead.host.clone(), pane }));
                selection.set(None);
                detail.set(crate::app::LeadPanel::Workers);
                panel.set(crate::app::LeadPanel::Workers);
            },
            span { class: "worker-dots", aria_hidden: "true",
                for (index, state) in states.into_iter().enumerate() {
                    span { key: "{index}", class: "worker-dot", "data-state": state.value() }
                }
            }
            span { "{summary}" }
        }
    }
}

/// Finished workers stay listed until their branches land, but only open workers count.
fn worker_summary(open: usize, working: usize, finished: usize) -> String {
    let mut summary = if open == 0 {
        String::new()
    } else {
        format!(
            "{open} {} · {working} working",
            if open == 1 { "worker" } else { "workers" }
        )
    };
    if finished > 0 {
        if !summary.is_empty() {
            summary.push_str(" · ");
        }
        summary.push_str(&format!("{finished} finished"));
    }
    summary
}

fn worker_status(row: &NavigationRow) -> &str {
    if row.unseen {
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
            { "host": "netcup", "pane": "lead-a", "name": "First lead", "state": "working", "repository": "github.com/owner/repo", "cwd": "/unrelated" },
            { "host": "netcup", "pane": "worker-b", "state": "done", "repository": "github.com/owner/repo" },
            { "host": "netcup", "pane": "lead-b", "name": "Second lead", "state": "idle", "repository": "github.com/owner/repo" }
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
    fn owned_work_toolbar_follows_the_root_not_the_worker_or_viewer()
    -> Result<(), serde_json::Error> {
        let mut worker = row("worker", Some("lead-a"), "working")?;
        worker.host = "local".into();
        worker.owner_host = Some("netcup".into());
        let mut child = row("child", Some("worker"), "working")?;
        child.owner_host = Some("local".into());
        let mut board = board(vec![worker, child]);
        board.hosts = serde_json::from_value(json!([
            {"label":"local","hostname":"mac","service":"up","reachable":true,"cores":1,"eligible":true},
            {"label":"netcup","hostname":"netcup-dev","service":"up","reachable":true,"cores":1,"eligible":true}
        ]))?;
        let target = AgentAddress {
            host: "netcup".into(),
            pane: "child".into(),
        };
        let agents = agents()?;
        assert_eq!(lead_hostname(&board, &agents, &target), Some("netcup-dev"));
        board.repos[0].rows[0].owner_pane = Some("child".into());
        assert_eq!(lead_hostname(&board, &agents, &target), None);
        Ok(())
    }

    #[test]
    fn equal_panes_on_different_hosts_remain_distinct_navigation_targets()
    -> Result<(), serde_json::Error> {
        let mut agents = agents()?;
        let mut local = agents[0].clone();
        local.host = "local".into();
        agents.push(local);
        let navigation = navigation_groups(&board(vec![row("lead-a", None, "working")?]), &agents);
        let rows = &navigation.repos[0].rows;
        let targets: Vec<_> = rows
            .iter()
            .filter(|row| row.pane.as_deref() == Some("lead-a"))
            .map(|row| (row.host.as_str(), row.worker))
            .collect();
        assert!(targets.contains(&("local", false)));
        assert!(targets.contains(&("netcup", true)));
        Ok(())
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
    fn repository_groups_keep_name_order_whatever_order_agents_arrive_in()
    -> Result<(), serde_json::Error> {
        let mut agents: Vec<AgentView> = serde_json::from_value(json!([
            { "host": "local", "pane": "r", "state": "working", "repository": "github.com/owner/rondo" },
            { "host": "local", "pane": "n", "state": "idle" },
            { "host": "local", "pane": "d", "state": "idle", "repository": "github.com/owner/Dogsled" },
            { "host": "local", "pane": "z", "state": "working", "repository": "github.com/other/zoomies" }
        ]))?;
        let empty = BoardView {
            hosts: vec![],
            generated_ms: 0,
            repos: vec![],
        };
        let order = |agents: &[AgentView]| {
            navigation_groups(&empty, agents)
                .repos
                .into_iter()
                .map(|repo| repo.repository)
                .collect::<Vec<_>>()
        };
        let expected = vec![
            Some("github.com/owner/Dogsled".to_owned()),
            Some("github.com/owner/rondo".to_owned()),
            Some("github.com/other/zoomies".to_owned()),
            None,
        ];
        assert_eq!(order(&agents), expected);
        agents.reverse();
        assert_eq!(order(&agents), expected);
        Ok(())
    }

    #[test]
    fn navigation_counts_only_leads_without_removing_jobs() -> Result<(), serde_json::Error> {
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
        assert_eq!(repo.agent_count(), 2);
        assert_eq!(repo.rows.len(), 3);
        assert_eq!(repo.rows[0].workers.len(), 1);
        assert_eq!(repo.rows[0].jobs.len(), 1);
        assert_eq!(repo.rows[0].jobs[0].state, "working");
        assert!(repo.rows.iter().any(|row| row.role == "job"));
        Ok(())
    }

    #[test]
    fn local_lead_owns_cross_host_workers_including_closed_rows() -> Result<(), serde_json::Error> {
        let mut agents = agents()?;
        agents[0].host = "local".into();
        let mut remote = row("remote", Some("lead-a"), "working")?;
        remote.owner_host = Some("local".into());
        let mut closed = remote.clone();
        closed.id = "closed".into();
        closed.state = "closed".into();
        let same_pane_foreign_owner = row("other", Some("lead-a"), "idle")?;
        let groups = navigation_groups(
            &board(vec![remote, closed, same_pane_foreign_owner]),
            &agents,
        );
        let lead = &groups.repos[0].rows[0];
        assert_eq!(lead.host, "local");
        assert_eq!(lead.workers.len(), 2);
        assert_eq!(lead.workers[0].host, "netcup");
        assert_eq!(lead.workers[1].state, "closed");
        assert!(
            groups.repos[0]
                .rows
                .iter()
                .any(|row| row.pane.as_deref() == Some("other"))
        );
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
            host: "local".into(),
            pane: "p".to_owned(),
            name: Some("u-0cc2".to_owned()),
            title: None,
            agent: None,
            state: "idle".to_owned(),
            cwd: Some("/r/.worktrees/fix-portal-titles".to_owned()),
            repository: None,
            blocked: None,
            reply: None,
            launch: None,
        };
        assert_eq!(agent_title(None, &agent), "fix-portal-titles");
        agent.name = Some("reviewer".to_owned());
        assert_eq!(agent_title(None, &agent), "reviewer");
        agent.title = Some("Ship the redesign".to_owned());
        assert_eq!(agent_title(Some(" "), &agent), "Ship the redesign");
        assert_eq!(agent_title(Some("Verify it"), &agent), "Verify it");
        agent.title = None;
        agent.name = Some("lead-r-mwai1wqf-1lln-0".to_owned());
        agent.cwd = Some("/home/dev/rondo".to_owned());
        assert_eq!(agent_title(None, &agent), "rondo");
        agent.cwd = None;
        assert_eq!(agent_title(None, &agent), "Agent task");
    }

    #[test]
    fn finished_workers_are_listed_apart_from_the_open_count() {
        assert_eq!(worker_summary(3, 1, 0), "3 workers · 1 working");
        assert_eq!(worker_summary(0, 0, 34), "34 finished");
        assert_eq!(
            worker_summary(2, 0, 34),
            "2 workers · 0 working · 34 finished"
        );
    }

    /// A worker whose start its owner gave up on has no ledger row, but its host still
    /// publishes the launch. It is a worker, never a lead of its own.
    #[test]
    fn a_launched_agent_without_a_ledger_row_is_a_worker_not_a_lead()
    -> Result<(), serde_json::Error> {
        let mut agents = agents()?;
        let mut launched = agents[0].clone();
        launched.pane = "timed-out".into();
        launched.name = None;
        launched.cwd = Some("/r/.worktrees/feat-account-deletion".into());
        launched.launch = Some("attempted".into());
        agents.push(launched);
        let groups = navigation_groups(&board(vec![]), &agents);
        let repo = &groups.repos[0];
        assert_eq!(
            repo.agent_count(),
            3,
            "only agents no launch started are leads"
        );
        let row = repo
            .rows
            .iter()
            .find(|row| row.pane.as_deref() == Some("timed-out"))
            .expect("the launched agent stays visible");
        assert!(row.worker);
        assert_eq!(row.title, "feat-account-deletion");
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
        assert_eq!(worker_status(&worker), "idle · unseen");
        row.unseen = false;
        row.state = "working".to_owned();
        assert_eq!(worker_status(&navigation_row(&row, &[])), "working");
        Ok(())
    }

    fn pending(host: &str, pane: &str, kind: InboxKind, id: &str) -> InboxItem {
        InboxItem {
            selection: InboxSelection {
                host: host.into(),
                kind,
                id: id.into(),
                pane: pane.into(),
            },
            revision: 1,
            title: id.into(),
            agent_title: Some("Asked elsewhere".into()),
            repository: Some("github.com/owner/other".into()),
            age_ms: 0,
        }
    }

    /// Agents that need the owner leave their repository, newest first, one row per agent.
    #[test]
    fn waiting_agents_move_to_the_top_and_keep_every_item() -> Result<(), serde_json::Error> {
        let board = board(vec![row("worker-a", Some("lead-a"), "working")?]);
        let mut navigation = navigation_groups(&board, &agents()?);
        let inbox = [
            pending("netcup", "lead-b", InboxKind::Question, "newest"),
            pending("netcup", "worker-a", InboxKind::Prompt, "prompt"),
            pending("netcup", "lead-b", InboxKind::Review, "older"),
            pending("local", "gone", InboxKind::Question, "unlisted"),
        ];
        let waiting = take_waiting(&mut navigation, &inbox);
        let order: Vec<_> = waiting
            .iter()
            .map(|agent| {
                (
                    agent.row.pane.as_deref().unwrap_or_default(),
                    agent.items.len(),
                )
            })
            .collect();
        assert_eq!(order, [("lead-b", 2), ("worker-a", 1), ("gone", 1)]);
        assert_eq!(waiting[0].row.title, "Second lead");
        assert_eq!(
            waiting[0].repository.as_deref(),
            Some("github.com/owner/repo")
        );
        assert_eq!(waiting[2].row.title, "Asked elsewhere");
        let rows = &navigation.repos[0].rows;
        assert!(rows.iter().all(|row| row.pane.as_deref() != Some("lead-b")));
        let lead = rows
            .iter()
            .find(|row| row.pane.as_deref() == Some("lead-a"));
        assert!(
            lead.is_some_and(|lead| lead.workers.len() == 1),
            "a waiting worker stays under its lead"
        );
        Ok(())
    }
}
