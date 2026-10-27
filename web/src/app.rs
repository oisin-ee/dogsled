//! The persistent portal shell, repository navigation, and cross-agent inbox.

use dioxus::prelude::*;
use portal_types::BoardView;
use portal_types::agents::AgentView as Agent;
use portal_types::questions::Pending;
use portal_types::reviews::ReviewSummary;

use crate::agent_view::{AgentView, Draft};
use crate::api;
use crate::board::{RepoGroup, navigation_groups, repository_name};

const MAIN_CSS: Asset = asset!("/assets/main.css");

/// The pending item the owner picked from the inbox. The shell passes it to
/// `AgentView` as its `review` or `question` prop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxSelection {
    pub kind: InboxKind,
    pub id: String,
    pub pane: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum InboxKind {
    Prompt,
    Question,
    Review,
}

impl InboxKind {
    pub fn css(self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::Question => "question",
            Self::Review => "plan",
        }
    }
    pub fn tag(self) -> &'static str {
        match self {
            Self::Prompt => "Prompt",
            Self::Question => "Question",
            Self::Review => "Plan",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Prompt => "Prompt",
            Self::Question => "Questions",
            Self::Review => "Plan review",
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct InboxItem {
    pub selection: InboxSelection,
    pub title: String,
    pub created_ms: Option<u64>,
}

#[derive(Clone, Default)]
struct DeepLink {
    review: Option<String>,
    question: Option<String>,
    pane: Option<String>,
}

impl DeepLink {
    fn read() -> Self {
        let Some(query) = web_sys::window().and_then(|window| window.location().search().ok())
        else {
            return Self::default();
        };
        let Ok(params) = web_sys::UrlSearchParams::new_with_str(&query) else {
            return Self::default();
        };
        let get = |name| params.get(name).filter(|value| !value.is_empty());
        Self {
            review: get("review"),
            question: get("question"),
            pane: get("pane"),
        }
    }

    fn selection(&self) -> Option<InboxSelection> {
        let (kind, id) = self
            .review
            .as_ref()
            .map(|id| (InboxKind::Review, id))
            .or_else(|| self.question.as_ref().map(|id| (InboxKind::Question, id)))?;
        Some(InboxSelection {
            kind,
            id: id.clone(),
            pane: self.pane.clone().unwrap_or_default(),
        })
    }
}

#[component]
pub fn App() -> Element {
    let mut board = use_signal(|| None::<BoardView>);
    let mut board_error = use_signal(|| None::<String>);
    let mut stream_paused = use_signal(|| false);
    let mut inbox_open = use_signal(|| false);
    let mut agents = use_signal(Vec::<Agent>::new);
    let mut questions = use_signal(Vec::<Pending>::new);
    let mut reviews = use_signal(Vec::<ReviewSummary>::new);
    let mut agent_error = use_signal(|| None::<String>);
    let mut inbox_errors = use_signal(Vec::<String>::new);
    let mut inbox_loaded = use_signal(|| false);
    let mut now = use_signal(|| 0_u64);
    let link = use_hook(DeepLink::read);
    let mut selected = use_context_provider(|| Signal::new(link.pane.clone()));
    let mut selection = use_context_provider(|| Signal::new(link.selection()));
    use_context_provider(|| Draft(Signal::new(String::new())));

    use_future(move || async move {
        match api::fetch_board().await {
            Ok(view) => board.set(Some(view)),
            Err(message) => board_error.set(Some(message)),
        }
        loop {
            let result = api::watch(
                |view| {
                    board.set(Some(view));
                    board_error.set(None);
                },
                |paused| stream_paused.set(paused),
            )
            .await;
            stream_paused.set(true);
            if let Err(message) = result {
                board_error.set(Some(message));
            }
            if pause().await.is_err() {
                break;
            }
        }
    });

    // Pending-item APIs are independent: a Herdr outage must not hide ledger
    // rows, plan reviews, or interviews. Retain each last successful snapshot.
    use_future(move || async move {
        loop {
            let (agent_result, question_result, review_result) = futures_util::future::join3(
                api::fetch_agents(),
                api::fetch_questions(),
                api::fetch_reviews(),
            )
            .await;
            let mut errors = Vec::new();
            match agent_result {
                Ok(value) => {
                    agents.set(value);
                    agent_error.set(None);
                }
                Err(message) => agent_error.set(Some(message)),
            }
            match question_result {
                Ok(value) => questions.set(value),
                Err(message) => errors.push(format!("Questions unavailable: {message}")),
            }
            match review_result {
                Ok(value) => reviews.set(value),
                Err(message) => errors.push(format!("Reviews unavailable: {message}")),
            }
            inbox_errors.set(errors);
            inbox_loaded.set(true);
            now.set(js_sys::Date::now().max(0.0) as u64);
            if let Err(message) = pause().await {
                inbox_errors.write().push(format!(
                    "Inbox refresh stopped: {message}. Reload the page."
                ));
                break;
            }
        }
    });

    let inbox = use_memo(move || inbox_items(&agents(), &questions(), &reviews()));
    use_effect(move || {
        if let Some(mut current) = selection()
            && current.pane.is_empty()
            && let Some(item) = inbox()
                .iter()
                .find(|item| item.selection.kind == current.kind && item.selection.id == current.id)
        {
            current.pane.clone_from(&item.selection.pane);
            selected.set(Some(current.pane.clone()));
            selection.set(Some(current));
        }
    });

    let view = board();
    let items = inbox();
    let count = items.len();
    let groups = view.as_ref().map(|view| navigation_groups(view, &agents()));

    rsx! {
        document::Stylesheet { href: MAIN_CSS }
        document::Link { rel: "stylesheet", href: "https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600&family=JetBrains+Mono:wght@400&display=swap" }
        div { class: "app", onkeydown: move |event| {
                if event.key() == Key::Escape { inbox_open.set(false); }
            },
            header { class: "top-bar",
                h1 { a { href: "/", aria_label: "Homepage", "Dogsled" } }
                button { r#type: "button", class: if inbox_open() { "shell-button inbox-toggle on" } else { "shell-button inbox-toggle" },
                    aria_expanded: inbox_open(), aria_controls: "portal-inbox",
                    onclick: move |_| inbox_open.toggle(),
                    "Inbox" b { "{count}" }
                }
                div { class: "shell-notices", role: "status",
                    if stream_paused() { span { "Live updates paused, retrying" } }
                    if let Some(message) = agent_error() {
                        span { title: "{message}", "Herdr is not reachable. The board still works." }
                    }
                }
                div { class: "hosts", aria_label: "Host load",
                    if let Some(view) = &view {
                        for host in &view.hosts {
                            {
                                let busy = host.busy.filter(|busy| busy.is_finite());
                                let load = busy.map_or(0.0, |busy| if host.cores == 0 { 0.0 } else { (busy / f64::from(host.cores) * 100.0).clamp(0.0, 100.0) });
                                let value = busy.map_or_else(|| format!("—/{}", host.cores), |busy| format!("{busy:.0}/{}", host.cores));
                                rsx! {
                                    span { key: "{host.label}", class: "host-load", title: "{host.label}: {host.service}",
                                        "{host.label}"
                                        i { aria_hidden: true, style: "--host-load: {load}%", b {} }
                                        span { "{value}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            nav { class: "board", aria_label: "Repositories and agents",
                if let Some(message) = board_error() { p { class: "error", "Board updates unavailable: {message}" } }
                match groups {
                    Some(groups) => rsx! {
                        for (index, group) in groups.into_iter().enumerate() {
                            RepoGroup { key: "{index}:{group.repository:?}", group, inbox: items.clone() }
                        }
                    },
                    None => rsx! { p { class: "empty", "Loading agents…" } },
                }
            }
            main { class: "centre",
                match selected() {
                    Some(pane) => {
                        let picked = selection().filter(|item| item.pane == pane);
                        let pick = |kind| picked.as_ref().filter(|item| item.kind == kind).map(|item| item.id.clone());
                        let review = pick(InboxKind::Review);
                        let question = pick(InboxKind::Question);
                        rsx! { AgentView { key: "{pane}", pane, review, question } }
                    }
                    None => rsx! {
                        section { class: "welcome",
                            if !items.is_empty() {
                                h2 { "Waiting on you" }
                                InboxList { items: items.clone(), board: view.clone(), agents: agents(), now: now(), inbox_open }
                            } else if !inbox_loaded() || view.is_none() {
                                p { class: "empty", "Loading agents…" }
                            } else if view.as_ref().is_some_and(|view| view.repos.iter().all(|repo| repo.rows.is_empty())) && agents().is_empty() {
                                h2 { "No agents are running" }
                                p { class: "empty", "Start an agent with " code { "dogsled spawn" } "." }
                            } else {
                                h2 { "Select an agent" }
                                p { class: "empty", "Choose an agent in the navigation to open its work." }
                            }
                            for message in inbox_errors() { p { class: "error", "{message}" } }
                        }
                    },
                }
            }
            if inbox_open() {
                section { class: "inbox-panel", id: "portal-inbox", aria_label: "Inbox",
                    h2 { "Inbox" span { "Everything waiting on you" } }
                    for message in inbox_errors() { p { class: "error", "{message}" } }
                    if let Some(message) = agent_error() { p { class: "empty", "Prompts unavailable: {message}" } }
                    if !inbox_loaded() {
                        p { class: "empty", "Loading inbox…" }
                    } else if items.is_empty() {
                        p { class: "inbox-empty", "Nothing is waiting on you." }
                    } else {
                        InboxList { items, board: view, agents: agents(), now: now(), inbox_open }
                    }
                    p { class: "inbox-foot", "Selecting an item opens that agent with the item in front." }
                }
            }
        }
    }
}

#[component]
fn InboxList(
    items: Vec<InboxItem>,
    board: Option<BoardView>,
    agents: Vec<Agent>,
    now: u64,
    mut inbox_open: Signal<bool>,
) -> Element {
    let mut selected = use_context::<Signal<Option<String>>>();
    let mut selection = use_context::<Signal<Option<InboxSelection>>>();
    rsx! {
        div { class: "inbox-list",
            for item in items {
                {
                    let kind = item.selection.kind;
                    let (title, repository) = agent_identity(&item.selection.pane, board.as_ref(), &agents);
                    let age = item.created_ms.map(|created| age(now.saturating_sub(created)));
                    rsx! {
                        button { key: "{kind:?}:{item.selection.id}", class: "inbox-item", r#type: "button",
                            onclick: move |_| {
                                selected.set(Some(item.selection.pane.clone()));
                                selection.set(Some(item.selection.clone()));
                                inbox_open.set(false);
                            },
                            span { class: "inbox-kind {kind.css()}", "{kind.label()}" }
                            div { class: "inbox-description",
                                div { class: "inbox-title", "{item.title}" }
                                div { class: "inbox-agent", "{title}" if let Some(repository) = repository { " · {repository}" } }
                            }
                            if let Some(age) = age { span { class: "inbox-age", "{age}" } }
                        }
                    }
                }
            }
        }
    }
}

fn inbox_items(
    agents: &[Agent],
    questions: &[Pending],
    reviews: &[ReviewSummary],
) -> Vec<InboxItem> {
    let mut items = Vec::new();
    for agent in agents {
        if let Some(prompt) = &agent.blocked {
            items.push(InboxItem {
                selection: InboxSelection {
                    kind: InboxKind::Prompt,
                    id: agent.pane.clone(),
                    pane: agent.pane.clone(),
                },
                title: prompt.question.clone(),
                created_ms: None,
            });
        }
    }
    for question in questions {
        items.push(InboxItem {
            selection: InboxSelection {
                kind: InboxKind::Question,
                id: question.id.clone(),
                pane: question.pane.clone(),
            },
            title: question
                .questions
                .title
                .clone()
                .unwrap_or_else(|| "Questions".to_owned()),
            created_ms: Some(question.created_ms),
        });
    }
    for review in reviews.iter().filter(|review| review.decision.is_none()) {
        items.push(InboxItem {
            selection: InboxSelection {
                kind: InboxKind::Review,
                id: review.id.clone(),
                pane: review.pane.clone(),
            },
            title: format!("{} · revision {}", review.path, review.revisions),
            created_ms: None,
        });
    }
    // Stable ordering preserves payload order when timestamps are unavailable.
    items.sort_by(|left, right| {
        left.selection
            .kind
            .cmp(&right.selection.kind)
            .then_with(|| right.created_ms.cmp(&left.created_ms))
    });
    items
}

fn agent_identity(
    pane: &str,
    board: Option<&BoardView>,
    agents: &[Agent],
) -> (String, Option<String>) {
    if let Some(board) = board {
        for group in navigation_groups(board, agents) {
            if let Some(row) = group
                .rows
                .iter()
                .find(|row| row.pane.as_deref() == Some(pane))
            {
                return (
                    row.title.clone(),
                    group
                        .repository
                        .map(|repository| repository_name(&repository).to_owned()),
                );
            }
        }
    }
    let name = agents
        .iter()
        .find(|agent| agent.pane == pane)
        .and_then(|agent| agent.name.clone());
    (
        name.filter(|name| !name.starts_with("u-"))
            .unwrap_or_else(|| "Agent task".to_owned()),
        None,
    )
}

fn age(ms: u64) -> String {
    match ms / 1000 {
        0..=59 => "now".to_owned(),
        seconds @ 60..=3599 => format!("{} m", seconds / 60),
        seconds @ 3600..=86399 => format!("{} h", seconds / 3600),
        seconds => format!("{} d", seconds / 86400),
    }
}

async fn pause() -> Result<(), String> {
    let mut timer = document::eval(
        "await new Promise(resolve => setTimeout(resolve, 2000)); dioxus.send(true);",
    );
    timer
        .recv::<bool>()
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod shell_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inbox_prioritizes_prompts_then_newest_questions_then_reviews()
    -> Result<(), serde_json::Error> {
        let agents: Vec<Agent> = serde_json::from_value(json!([
            { "pane": "first", "state": "blocked", "blocked": { "question": "First prompt", "options": [] } },
            { "pane": "second", "state": "blocked", "blocked": { "question": "Second prompt", "options": [] } }
        ]))?;
        let questions: Vec<Pending> = serde_json::from_value(json!([
            { "id": "old", "pane": "first", "session": "s", "cwd": "/repo", "created_ms": 10, "questions": { "title": "Old", "questions": [] } },
            { "id": "new", "pane": "second", "session": "s", "cwd": "/repo", "created_ms": 20, "questions": { "title": "New", "questions": [] } }
        ]))?;
        let reviews: Vec<ReviewSummary> = serde_json::from_value(json!([
            { "id": "plan", "pane": "first", "session": "s", "path": "plan.md", "revisions": 2, "annotations": 0 }
        ]))?;
        let items = inbox_items(&agents, &questions, &reviews);
        assert_eq!(
            items
                .iter()
                .map(|item| item.selection.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second", "new", "old", "plan"]
        );
        assert_eq!(items[0].created_ms, None);
        assert_eq!(items[4].created_ms, None);
        assert_eq!(items[4].title, "plan.md · revision 2");
        assert!(inbox_items(&[], &[], &[]).is_empty());
        Ok(())
    }

    #[test]
    fn deep_link_keeps_pending_item_and_pane() {
        let link = DeepLink {
            review: Some("plan".to_owned()),
            question: None,
            pane: Some("pane".to_owned()),
        };
        assert_eq!(
            link.selection(),
            Some(InboxSelection {
                kind: InboxKind::Review,
                id: "plan".to_owned(),
                pane: "pane".to_owned(),
            })
        );
        let question = DeepLink {
            question: Some("interview".to_owned()),
            ..DeepLink::default()
        };
        assert_eq!(
            question
                .selection()
                .map(|selection| (selection.kind, selection.pane)),
            Some((InboxKind::Question, String::new()))
        );
    }
}
