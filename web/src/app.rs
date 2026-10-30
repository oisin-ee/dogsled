//! The persistent portal shell, repository navigation, and cross-agent inbox.

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_primitives::scroll_area::ScrollDirection;
use dioxus_sdk_time::{sleep, use_interval};
use portal_types::BoardView;
use portal_types::agents::AgentView as Agent;
use portal_types::inbox::{InboxKind as ServerInboxKind, InboxView};

use crate::agent_view::{AgentView, Draft};
use crate::api;
use crate::board::{RepoGroup, navigation_groups, repository_name};
use crate::components::badge::{Badge, BadgeVariant};
use crate::components::button::{Button, ButtonVariant};
use crate::components::card::{Card, CardContent, CardHeader, CardTitle};
use crate::components::item::{
    Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemSize, ItemTitle, ItemVariant,
};
use crate::components::progress::Progress;
use crate::components::scroll_area::ScrollArea;
use crate::components::sheet::{
    Sheet, SheetContentClose, SheetDescription, SheetFooter, SheetHeader, SheetTitle,
};
use crate::components::sidebar::{
    Sidebar, SidebarCollapsible, SidebarContent, SidebarFooter, SidebarGroup, SidebarGroupContent,
    SidebarGroupLabel, SidebarHeader, SidebarInset, SidebarProvider,
};
use crate::components::skeleton::Skeleton;
use crate::conversations::Conversations;
use crate::notice::{Alert, Status};

/// The time between two reads of the board and the agents.
const RETRY: Duration = Duration::from_secs(2);

const COMPONENTS_CSS: Asset = asset!("/assets/dx-components-theme.css");
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
    UnseenWorker,
}

impl InboxKind {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Prompt => "Prompt",
            Self::Question => "Question",
            Self::Review => "Plan",
            Self::UnseenWorker => "Idle",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Prompt => "Prompt",
            Self::Question => "Questions",
            Self::Review => "Plan review",
            Self::UnseenWorker => "Idle",
        }
    }
}

impl From<ServerInboxKind> for InboxKind {
    fn from(kind: ServerInboxKind) -> Self {
        match kind {
            ServerInboxKind::Prompt => Self::Prompt,
            ServerInboxKind::Question => Self::Question,
            ServerInboxKind::Review => Self::Review,
            ServerInboxKind::UnseenWorker => Self::UnseenWorker,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct InboxItem {
    pub selection: InboxSelection,
    pub title: String,
    pub agent_title: Option<String>,
    pub repository: Option<String>,
    pub age_ms: u64,
}

#[derive(Clone, Default)]
struct InboxState {
    snapshot: Option<InboxView>,
    error: Option<String>,
    received_ms: u64,
}

impl InboxState {
    fn update(&mut self, result: Result<InboxView, String>, now: u64) {
        match result {
            Ok(view) => {
                self.snapshot = Some(view);
                self.received_ms = now;
                self.error = None;
            }
            Err(message) => self.error = Some(message),
        }
    }

    fn count(&self) -> Option<usize> {
        self.snapshot
            .as_ref()
            .filter(|_| self.error.is_none())
            .map(|view| view.count)
    }

    fn items(&self) -> Vec<InboxItem> {
        let mut items: Vec<_> = self
            .snapshot
            .as_ref()
            .map(|view| {
                view.items
                    .iter()
                    .map(|item| InboxItem {
                        selection: InboxSelection {
                            kind: item.kind.into(),
                            id: item.id.clone(),
                            pane: item.pane.clone(),
                        },
                        title: item.title.clone(),
                        agent_title: item.agent_title.clone(),
                        repository: item.repository.clone(),
                        age_ms: item.age_ms,
                    })
                    .collect()
            })
            .unwrap_or_default();
        items.sort_by_key(|item| item.age_ms);
        items
    }
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
    let mut agent_error = use_signal(|| None::<String>);
    let mut inbox_state = use_signal(InboxState::default);
    let mut now = use_signal(|| 0_u64);
    let link = use_hook(DeepLink::read);
    let mut selected = use_context_provider(|| Signal::new(link.pane.clone()));
    let mut selection = use_context_provider(|| Signal::new(link.selection()));
    use_context_provider(|| Draft(Signal::new(String::new())));
    use_context_provider(|| Signal::new(Conversations::default()));

    use_future(move || async move {
        let (board_result, inbox_result) =
            futures_util::future::join(api::fetch_board(), api::fetch_inbox()).await;
        match board_result {
            Ok(view) => board.set(Some(view)),
            Err(message) => board_error.set(Some(message)),
        }
        inbox_state.write().update(inbox_result, clock_ms());
        loop {
            let result = api::watch(
                |view| {
                    board.set(Some(view));
                    board_error.set(None);
                },
                |result| inbox_state.write().update(result, clock_ms()),
                |paused| stream_paused.set(paused),
            )
            .await;
            stream_paused.set(true);
            if let Err(message) = result {
                board_error.set(Some(message));
            }
            sleep(RETRY).await;
        }
    });

    // Herdr supplies live leads only. Pending items come exclusively from inbox.
    use_future(move || async move {
        loop {
            match api::fetch_agents().await {
                Ok(value) => {
                    agents.set(value);
                    agent_error.set(None);
                }
                Err(message) => agent_error.set(Some(message)),
            }
            sleep(RETRY).await;
        }
    });
    use_interval(RETRY, move |()| now.set(clock_ms()));

    let inbox = use_memo(move || inbox_state.read().items());
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
    let state = inbox_state();
    // Red only when something waits; an empty or unread inbox stays quiet.
    let waiting = state.count().is_some_and(|count| count > 0);
    let count = state
        .count()
        .map_or_else(|| "—".to_owned(), |count| count.to_string());
    let elapsed_ms = now().saturating_sub(state.received_ms);
    let groups = view.as_ref().map(|view| navigation_groups(view, &agents()));

    rsx! {
        document::Stylesheet { href: COMPONENTS_CSS }
        document::Stylesheet { href: MAIN_CSS }
        document::Link { rel: "stylesheet", href: "https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600&display=swap" }
        SidebarProvider {
            Sidebar { collapsible: SidebarCollapsible::None, aria_label: "Repositories and agents",
                SidebarHeader {
                    Item {
                        ItemContent { ItemTitle { "Dogsled" } }
                        ItemActions {
                            Button { variant: ButtonVariant::Outline, r#type: "button",
                                aria_expanded: inbox_open(), aria_controls: "portal-inbox",
                                onclick: move |_| inbox_open.toggle(),
                                "Inbox"
                                Badge { variant: if waiting { BadgeVariant::Destructive } else { BadgeVariant::Secondary }, "{count}" }
                            }
                        }
                    }
                }
                SidebarContent {
                    if let Some(message) = board_error() {
                        Alert { "Board updates unavailable: {message}" }
                    }
                    match groups {
                        Some(groups) => rsx! {
                            for (index, group) in groups.repos.into_iter().enumerate() {
                                RepoGroup { key: "{index}:{group.repository:?}", group, inbox: items.clone() }
                            }
                            if groups.finished_without_repository > 0 {
                                Status { "{groups.finished_without_repository} finished branches have no repository — see Land and clean up" }
                            }
                        },
                        None => rsx! { Skeleton { role: "status", aria_label: "Loading agents" } },
                    }
                }
                SidebarFooter {
                    if stream_paused() {
                        Status { "Live updates paused, retrying" }
                    }
                    if let Some(message) = &state.error {
                        Alert { title: "{message}", "Inbox unavailable: {message}. Reload to retry." }
                    }
                    if let Some(message) = agent_error() {
                        Status { title: "{message}", "Herdr is not reachable. The board still works." }
                    }
                    if let Some(view) = &view {
                        SidebarGroup { aria_label: "Host load",
                            SidebarGroupLabel { "Host load" }
                            SidebarGroupContent {
                                ItemGroup {
                                    for host in &view.hosts {
                                        {
                                            let busy = host.busy.filter(|busy| busy.is_finite());
                                            let load = busy.map_or(0.0, |busy| if host.cores == 0 { 0.0 } else { (busy / f64::from(host.cores) * 100.0).clamp(0.0, 100.0) });
                                            let value = busy.map_or_else(|| format!("—/{}", host.cores), |busy| format!("{busy:.0}/{}", host.cores));
                                            rsx! {
                                                Item { key: "{host.label}", size: ItemSize::Sm, title: "{host.label}: {host.service}",
                                                    ItemContent {
                                                        ItemTitle { "{host.label}" Badge { variant: BadgeVariant::Outline, "{value}" } }
                                                        Progress { value: Some(load), max: 100.0 }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            SidebarInset {
                ScrollArea { direction: ScrollDirection::Vertical,
                    match selected() {
                        Some(pane) => {
                            let picked = selection().filter(|item| item.pane == pane);
                            let pick = |kind| picked.as_ref().filter(|item| item.kind == kind).map(|item| item.id.clone());
                            let review = pick(InboxKind::Review);
                            let question = pick(InboxKind::Question);
                            rsx! { AgentView { key: "{pane}", pane, review, question } }
                        }
                        None => rsx! {
                            if !items.is_empty() {
                                Card {
                                    CardHeader { CardTitle { "Waiting on you" } }
                                    CardContent { InboxList { items: items.clone(), elapsed_ms, inbox_open } }
                                }
                            } else if state.error.is_some() {
                                Alert { "Inbox unavailable. Reload to retry." }
                            } else if state.snapshot.is_none() || view.is_none() {
                                Skeleton { role: "status", aria_label: "Loading agents" }
                            } else if view.as_ref().is_some_and(|view| view.repos.iter().all(|repo| repo.rows.is_empty())) && agents().is_empty() {
                                Item { variant: ItemVariant::Muted,
                                    ItemContent {
                                        ItemTitle { "No agents are running" }
                                        ItemDescription { "Start an agent with `dogsled spawn`." }
                                    }
                                }
                            } else {
                                Item { variant: ItemVariant::Muted,
                                    ItemContent {
                                        ItemTitle { "Select an agent" }
                                        ItemDescription { "Choose an agent in the navigation to open its work." }
                                    }
                                }
                            }
                        },
                    }
                }
            }
            Sheet {
                open: Some(inbox_open()),
                on_open_change: move |open: bool| inbox_open.set(open),
                id: "portal-inbox",
                SheetContentClose {}
                SheetHeader {
                    SheetTitle { "Inbox" }
                    SheetDescription { "Everything waiting on you" }
                }
                if let Some(message) = &state.error {
                    Alert { "Inbox unavailable: {message}. Reload to retry." }
                }
                if state.snapshot.is_none() && state.error.is_none() {
                    Skeleton { role: "status", aria_label: "Loading inbox" }
                } else if state.count() == Some(0) && state.error.is_none() {
                    Status { "Nothing is waiting on you." }
                } else {
                    InboxList { items, elapsed_ms, inbox_open }
                }
                SheetFooter { "Selecting an item opens that agent with the item in front." }
            }
        }
    }
}

#[component]
fn InboxList(items: Vec<InboxItem>, elapsed_ms: u64, mut inbox_open: Signal<bool>) -> Element {
    let mut selected = use_context::<Signal<Option<String>>>();
    let mut selection = use_context::<Signal<Option<InboxSelection>>>();
    rsx! {
        ItemGroup {
            for item in items {
                {
                    let kind = item.selection.kind;
                    let title = item.agent_title.as_deref().filter(|title| !title.starts_with("u-"))
                        .unwrap_or("Agent task");
                    let repository = item.repository.as_deref().map(repository_name);
                    let age = age(item.age_ms.saturating_add(elapsed_ms));
                    rsx! {
                        Button {
                            key: "{kind:?}:{item.selection.id}",
                            variant: ButtonVariant::Ghost,
                            r#type: "button",
                            onclick: {
                                let item = item.clone();
                                move |_| {
                                    selected.set(Some(item.selection.pane.clone()));
                                    selection.set(Some(item.selection.clone()));
                                    inbox_open.set(false);
                                }
                            },
                            Badge { variant: BadgeVariant::Outline, "{kind.label()}" }
                            "{item.title}"
                            Badge { variant: BadgeVariant::Secondary,
                                "{title}"
                                if let Some(repository) = repository { " · {repository}" }
                            }
                            Badge { variant: BadgeVariant::Secondary, "{age}" }
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn clock_ms() -> u64 {
    js_sys::Date::now().max(0.0) as u64
}

fn age(ms: u64) -> String {
    match ms / 1000 {
        0..=59 => "now".to_owned(),
        seconds @ 60..=3599 => format!("{} m", seconds / 60),
        seconds @ 3600..=86399 => format!("{} h", seconds / 3600),
        seconds => format!("{} d", seconds / 86400),
    }
}

#[cfg(test)]
mod shell_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inbox_uses_server_count_metadata_ages_and_worker_selection() -> Result<(), serde_json::Error>
    {
        let view: InboxView = serde_json::from_value(json!({
            "count": 7, "items": [{
                "kind": "unseen_worker", "id": "unit", "pane": "worker-pane",
                "title": "Implement inbox", "agent_title": "Worker task",
                "repository": "github.com/owner/repo", "age_ms": 120000
            }]
        }))?;
        let mut state = InboxState::default();
        assert_eq!(state.count(), None);
        state.update(Ok(view.clone()), 100);
        assert_eq!(state.count(), Some(7));
        let items = state.items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].selection.kind, InboxKind::UnseenWorker);
        assert_eq!(items[0].selection.pane, "worker-pane");
        assert_eq!(items[0].selection.kind.tag(), "Idle");
        assert_eq!(items[0].agent_title.as_deref(), Some("Worker task"));
        assert_eq!(age(items[0].age_ms), "2 m");
        state.update(Err("reviews read failed".to_owned()), 200);
        assert_eq!(state.snapshot, Some(view));
        assert_eq!(state.items(), items);
        assert_eq!(state.count(), None);
        assert_eq!(state.received_ms, 100);
        assert_eq!(state.error.as_deref(), Some("reviews read failed"));
        state.update(Ok(InboxView::default()), 300);
        assert_eq!(state.count(), Some(0));
        assert!(state.items().is_empty());
        assert!(state.error.is_none());
        Ok(())
    }

    #[test]
    fn inbox_sorts_newest_first_and_does_not_report_zero_on_initial_failure()
    -> Result<(), serde_json::Error> {
        let mut state = InboxState::default();
        state.update(Err("questions read failed".to_owned()), 0);
        assert_eq!(state.count(), None);
        assert!(state.snapshot.is_none());
        let view: InboxView = serde_json::from_value(json!({
            "count": 2, "items": [
                { "kind": "review", "id": "old", "pane": "lead", "title": "Old plan", "age_ms": 120000 },
                { "kind": "question", "id": "new", "pane": "lead", "title": "New question", "age_ms": 1000 }
            ]
        }))?;
        state.update(Ok(view), 10);
        assert_eq!(
            state
                .items()
                .iter()
                .map(|item| item.selection.id.as_str())
                .collect::<Vec<_>>(),
            ["new", "old"]
        );
        assert_eq!(state.count(), Some(2));
        Ok(())
    }

    #[test]
    fn server_kinds_map_without_client_inference() {
        for (server, shell) in [
            (ServerInboxKind::Review, InboxKind::Review),
            (ServerInboxKind::Question, InboxKind::Question),
            (ServerInboxKind::Prompt, InboxKind::Prompt),
            (ServerInboxKind::UnseenWorker, InboxKind::UnseenWorker),
        ] {
            assert_eq!(InboxKind::from(server), shell);
        }
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
