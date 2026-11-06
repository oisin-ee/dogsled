//! The persistent portal shell, repository navigation, and cross-agent inbox.

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_primitives::scroll_area::ScrollDirection;
use dioxus_sdk_time::{sleep, use_interval};
use portal_types::agents::AgentView as Agent;
use portal_types::inbox::{InboxKind as ServerInboxKind, InboxView};
use portal_types::{BoardView, PortalEvent};

use crate::agent_view::{AgentView, Draft};
use crate::api;
use crate::board::{RepoGroup, navigation_groups, repository_name};
use crate::components::badge::{Badge, BadgeVariant};
use crate::components::button::{Button, ButtonVariant};
use crate::components::item::{Item, ItemContent, ItemGroup, ItemSize, ItemTitle, ItemVariant};
use crate::components::progress::Progress;
use crate::components::scroll_area::ScrollArea;
use crate::components::sidebar::{Sidebar, SidebarCollapsible, SidebarHeader, SidebarProvider};
use crate::components::skeleton::Skeleton;
use crate::components::tabs::{TabContent, TabList, TabTrigger, Tabs};
use crate::conversations::Conversations;
use crate::new_lead::{NewConversation, NewLead};
use dioxus_icons::lucide::{CircleQuestionMark, CornerDownLeft, FileText, Moon, Plus};

/// Portal stream projections shared with the selected agent view.
#[derive(Clone, Copy)]
pub struct AgentPortal {
    pub agents: Memo<Option<Vec<Agent>>>,
    pub agent_error: Memo<Option<String>>,
    pub board: Memo<Option<BoardView>>,
    pub board_error: Memo<Option<String>>,
}

#[derive(Clone, Copy)]
pub struct ConsoleWidth(pub Signal<u32>);

/// Last right-column choice, retained while navigating within the portal.
#[derive(Clone, Copy)]
pub struct LeadDetail(pub Signal<LeadPanel>);
use crate::notice::{Alert, Status};

/// The time between two reads of the board and the agents.
const RETRY: Duration = Duration::from_secs(2);

const COMPONENTS_CSS: Asset = asset!("/assets/dx-components-theme.css");
const CONSOLE_CSS: Asset = asset!("/assets/console.css");
const MARKDOWN_CSS: Asset = asset!("/assets/markdown.css");

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LeadPanel {
    Agents,
    Chat,
    Todos,
    Workers,
}

impl LeadPanel {
    pub fn value(self) -> &'static str {
        match self {
            Self::Agents => "agents",
            Self::Chat => "chat",
            Self::Todos => "todos",
            Self::Workers => "workers",
        }
    }
}

/// The pending item the owner picked from the inbox. The shell passes it to
/// `AgentView` as its `review` or `question` prop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxSelection {
    pub host: String,
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
    pub revision: u64,
    pub title: String,
    pub agent_title: Option<String>,
    pub repository: Option<String>,
    pub age_ms: u64,
}

#[derive(Clone, Default, PartialEq)]
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
                            host: "local".into(),
                            kind: item.kind.into(),
                            id: item.id.clone(),
                            pane: item.pane.clone(),
                        },
                        revision: item.revision,
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
    host: Option<String>,
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
            host: get("host"),
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
            host: self.host.clone().unwrap_or_else(|| "local".into()),
            kind,
            id: id.clone(),
            pane: self.pane.clone().unwrap_or_default(),
        })
    }
}

/// One reducer owns all browser projections and their connection status.
#[derive(Clone, Default)]
struct PortalState {
    board: Option<BoardView>,
    board_error: Option<String>,
    agents: Vec<Agent>,
    agents_received: bool,
    agent_error: Option<String>,
    inbox: InboxState,
    paused: bool,
}

impl PortalState {
    fn received_agents(&self) -> Option<&[Agent]> {
        self.agents_received.then_some(self.agents.as_slice())
    }

    fn update(&mut self, result: Result<PortalEvent, String>, now: u64) {
        let result = result.and_then(|event| match event {
            PortalEvent::Snapshot(snapshot) => Ok(snapshot),
            PortalEvent::Status(status) => Err(status.error),
        });
        match result {
            Ok(snapshot) => {
                let gaps = snapshot
                    .gaps
                    .iter()
                    .map(|gap| gap.detail.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                let missing = || {
                    if gaps.is_empty() {
                        "observation unavailable".to_owned()
                    } else {
                        gaps.clone()
                    }
                };
                match snapshot.board {
                    Some(view) => {
                        self.board = Some(view);
                        self.board_error = None;
                    }
                    None => self.board_error = Some(missing()),
                }
                match snapshot.agents {
                    Some(agents) => {
                        self.agents = agents;
                        self.agents_received = true;
                        self.agent_error = None;
                    }
                    None => self.agent_error = Some(missing()),
                }
                self.inbox.update(snapshot.inbox.ok_or_else(missing), now);
                self.paused = !snapshot.gaps.is_empty();
            }
            Err(message) => {
                self.paused = true;
                self.board_error = Some(message.clone());
                self.agent_error = Some(message.clone());
                self.inbox.update(Err(message), now);
            }
        }
    }
}

#[component]
pub fn App() -> Element {
    let mut publication = use_signal(PortalState::default);
    let board = use_memo(move || publication.read().board.clone());
    let board_error = use_memo(move || publication.read().board_error.clone());
    let stream_paused = use_memo(move || publication.read().paused);
    let mut panel = use_context_provider(|| Signal::new(LeadPanel::Chat));
    let mut detail = use_context_provider(|| LeadDetail(Signal::new(LeadPanel::Todos))).0;
    let mut width = use_context_provider(|| ConsoleWidth(Signal::new(1440))).0;
    use_effect(move || {
        spawn(async move {
            let mut eval = document::eval(
                r#"
                window.__consoleResizeHandler = () => dioxus.send(window.innerWidth);
                window.addEventListener('resize', window.__consoleResizeHandler);
                window.__consoleResizeHandler();
            "#,
            );
            while let Ok(value) = eval.recv::<u32>().await {
                width.set(value);
                if value >= 1100 && *panel.peek() == LeadPanel::Agents {
                    panel.set(LeadPanel::Chat);
                }
            }
        });
    });
    use_drop(|| {
        let _ = document::eval(
            "window.removeEventListener('resize', window.__consoleResizeHandler); delete window.__consoleResizeHandler;",
        );
    });
    let agents = use_memo(move || publication.read().agents.clone());
    let agent_error = use_memo(move || publication.read().agent_error.clone());
    let received_agents =
        use_memo(move || publication.read().received_agents().map(<[Agent]>::to_vec));
    use_context_provider(|| AgentPortal {
        agents: received_agents,
        agent_error,
        board,
        board_error,
    });
    let inbox_state = use_memo(move || publication.read().inbox.clone());
    let mut now = use_signal(|| 0_u64);
    let link = use_hook(DeepLink::read);
    let mut selected = use_context_provider(|| {
        Signal::new(
            link.pane
                .clone()
                .map(|pane| portal_types::agents::AgentAddress {
                    host: link.host.clone().unwrap_or_else(|| "local".into()),
                    pane,
                }),
        )
    });
    let mut selection = use_context_provider(|| Signal::new(link.selection()));
    use_context_provider(|| Draft(Signal::new(String::new())));
    let conversation = use_context_provider(|| NewConversation {
        open: Signal::new(false),
        repository: Signal::new(None),
    });
    let mut composing = conversation.open;
    let mut hint = conversation.repository;
    let mut start_new = move || {
        let current = selected.peek().clone();
        hint.set(current.and_then(|pane| {
            agents
                .peek()
                .iter()
                .find(|agent| agent.host == pane.host && agent.pane == pane.pane)
                .and_then(|agent| agent.repository.clone())
        }));
        selected.set(None);
        selection.set(None);
        composing.set(true);
        panel.set(LeadPanel::Chat);
    };
    use_effect(move || {
        if selected().is_some() {
            composing.set(false);
        }
    });
    use_context_provider(|| Signal::new(Conversations::default()));

    use_future(move || async move {
        loop {
            let result = api::watch(|event| publication.write().update(event, clock_ms())).await;
            if let Err(message) = result {
                publication.write().update(Err(message), clock_ms());
            }
            sleep(RETRY).await;
        }
    });
    use_interval(RETRY, move |()| now.set(clock_ms()));

    let inbox = use_memo(move || {
        let mut items = inbox_state.read().items();
        if let Some(board) = &publication.read().board {
            for item in &mut items {
                if item.selection.kind == InboxKind::UnseenWorker
                    && let Some(row) = board
                        .repos
                        .iter()
                        .flat_map(|repo| &repo.rows)
                        .find(|row| row.id == item.selection.id)
                {
                    item.selection.host.clone_from(&row.host);
                }
            }
        }
        items
    });
    use_effect(move || {
        if let Some(mut current) = selection()
            && current.pane.is_empty()
            && let Some(item) = inbox()
                .iter()
                .find(|item| item.selection.kind == current.kind && item.selection.id == current.id)
        {
            current.pane.clone_from(&item.selection.pane);
            current.host.clone_from(&item.selection.host);
            selected.set(Some(portal_types::agents::AgentAddress {
                host: current.host.clone(),
                pane: current.pane.clone(),
            }));
            selection.set(Some(current));
        }
    });

    let view = board();
    let items = inbox();
    let state = inbox_state();
    let elapsed_ms = now().saturating_sub(state.received_ms);
    let groups = view.as_ref().map(|view| navigation_groups(view, &agents()));
    let agent_count = groups.as_ref().map_or(0, |groups| {
        groups
            .repos
            .iter()
            .map(|repo| repo.agent_count())
            .sum::<usize>()
    });

    let lead = groups.as_ref().and_then(|groups| {
        let address = selected()?;
        groups
            .repos
            .iter()
            .flat_map(|repo| &repo.rows)
            .find(|row| {
                !row.worker
                    && ((row.host == address.host && row.pane.as_ref() == Some(&address.pane))
                        || row.workers.iter().any(|worker| {
                            worker.host == address.host
                                && worker.pane.as_ref() == Some(&address.pane)
                        }))
            })
            .cloned()
    });

    let navigation = rsx! {
            Sidebar { class: "console-agents", collapsible: SidebarCollapsible::None, aria_label: "Repositories and agents",
                SidebarHeader { class: "console-column-header", h2 { "Agents" } span { "{agent_count}" }
                    Button { class: "console-new", variant: ButtonVariant::Secondary, r#type: "button", aria_pressed: composing(),
                        onclick: move |_| start_new(),
                        Plus { size: 16, attributes: dioxus_primitives::dioxus_attributes::attributes!(svg { "aria-hidden": "true" }) }
                        "New"
                    }
                }
                ScrollArea { class: "console-scroll", direction: ScrollDirection::Vertical,
                    if let Some(message) = board_error() { Alert { "Board updates unavailable: {message}" } }
                    if let Some(message) = agent_error() { Status { title: "{message}", "Herdr is not reachable. The board still works." } }
                    match groups {
                        Some(groups) => rsx! {
                            for (index, group) in groups.repos.into_iter().enumerate() {
                                RepoGroup { key: "{index}:{group.repository:?}", group, inbox: items.clone() }
                            }
                            if groups.finished_without_repository > 0 {
                                Item { variant: ItemVariant::Muted, size: ItemSize::Sm, "data-part": "finished-branches",
                                    ItemContent { ItemTitle { "{groups.finished_without_repository} finished branches have no repository" } }
                                }
                            }
                        },
                        None => rsx! { Skeleton { role: "status", aria_label: "Loading agents" } },
                    }
                }
            }
    };

    rsx! {
        document::Stylesheet { href: COMPONENTS_CSS }
        document::Stylesheet { href: CONSOLE_CSS }
        document::Stylesheet { href: MARKDOWN_CSS }
        document::Stylesheet { href: asset!("/assets/review.css") }
        SidebarProvider { class: "console-provider",
        Tabs { class: "console-shell", "data-panel": panel().value(), horizontal: true, value: Some(panel().value().to_owned()),
            on_value_change: move |value: String| {
                let next = match value.as_str() { "agents" => LeadPanel::Agents, "todos" => LeadPanel::Todos, "workers" => LeadPanel::Workers, _ => LeadPanel::Chat };
                if matches!(next, LeadPanel::Todos | LeadPanel::Workers) { detail.set(next); }
                panel.set(next);
            },
            header { class: "console-topbar",
                a { href: "/", aria_label: "Homepage", "Dogsled" }
                if stream_paused() { Status { "Live updates paused, retrying" } }
                div { class: "console-hosts", aria_label: "Host load",
                    if let Some(view) = &view {
                        for host in &view.hosts {
                            {
                                let busy = host.busy.filter(|busy| busy.is_finite());
                                let load = busy.map_or(0.0, |busy| if host.cores == 0 { 0.0 } else { (busy / f64::from(host.cores) * 100.0).clamp(0.0, 100.0) });
                                let value = busy.map_or_else(|| format!("—/{}", host.cores), |busy| format!("{busy:.0}/{}", host.cores));
                                rsx! {
                                    div { class: "console-host", key: "{host.label}", title: "{host.label}: {host.service}",
                                        span { "{host.label}" }
                                        Progress { value: Some(load), max: 100.0, aria_label: "{host.label} CPU load" }
                                        span { "{value}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            InboxSidebar { count: state.count(), loading: state.snapshot.is_none(), error: state.error.clone(),
                items: items.clone(), elapsed_ms, agents: agents(), reachable: agent_error().is_none() }
            if width() >= 1100 { {navigation.clone()} } else {
                TabContent { class: "console-agents-tab", value: "agents", index: 0usize, {navigation.clone()} }
            }
            main { class: "console-main",
                match selected() {
                    Some(pane) => {
                        let picked = selection().filter(|item| item.host == pane.host && item.pane == pane.pane);
                        let pick = |kind| picked.as_ref().filter(|item| item.kind == kind).map(|item| item.id.clone());
                        let review = pick(InboxKind::Review);
                        let question = pick(InboxKind::Question);
                        rsx! { AgentView { key: "{pane.host}:{pane.pane}", host: pane.host, pane: pane.pane, review, question, agent_count, lead: lead.clone(), inbox: items.clone() } }
                    }
                    None if composing() => rsx! { NewLead {} },
                    None => rsx! { div { class: "empty-lead",
                        if width() < 1100 {
                            header { class: "lead-header", TabList { aria_label: "Lead view", TabTrigger { value: "agents", index: 0usize, "Agents {agent_count}" } TabTrigger { value: "chat", index: 1usize, "Chat" } }
                                Button { class: "console-new", variant: ButtonVariant::Secondary, r#type: "button", onclick: move |_| start_new(),
                                    Plus { size: 16, attributes: dioxus_primitives::dioxus_attributes::attributes!(svg { "aria-hidden": "true" }) }
                                    "New"
                                }
                            }
                        }
                        TabContent { class: "empty-chat", value: "chat", index: 1usize,
                        div { class: "console-empty",
                        if view.is_none() { Skeleton { role: "status", aria_label: "Loading agents" } }
                        else if view.as_ref().is_some_and(|view| view.repos.iter().all(|repo| repo.rows.is_empty())) && agents().is_empty() {
                            h1 { "No agents are running" }
                            Button { class: "console-new", variant: ButtonVariant::Secondary, r#type: "button", onclick: move |_| start_new(),
                                Plus { size: 16, attributes: dioxus_primitives::dioxus_attributes::attributes!(svg { "aria-hidden": "true" }) }
                                "New"
                            }
                        } else { h1 { "Select an agent" } p { "Choose an agent or a waiting item to open its work." } }
                    } } } },
                }
            }
        }
        }
    }
}

#[component]
pub(crate) fn InboxSidebar(
    count: Option<usize>,
    loading: bool,
    error: Option<String>,
    items: Vec<InboxItem>,
    elapsed_ms: u64,
    agents: Vec<Agent>,
    reachable: bool,
) -> Element {
    let label = count.map_or_else(|| "—".to_owned(), |count| count.to_string());
    let waiting = count.is_some_and(|count| count > 0);
    rsx! {
        Sidebar { class: "console-inbox", collapsible: SidebarCollapsible::None, aria_label: "Waiting on you",
            SidebarHeader { class: "console-column-header", h2 { "Waiting on you" }
                Badge { "data-part": "inbox-count", variant: BadgeVariant::Secondary, "data-waiting": waiting, "{label}" }
            }
            ScrollArea { class: "console-scroll", direction: ScrollDirection::Vertical,
                if let Some(message) = &error { Alert { "Inbox unavailable: {message}. Reload to retry." } }
                if loading && error.is_none() { Skeleton { role: "status", aria_label: "Loading inbox" } }
                else if error.is_none() && count == Some(0) { Status { "data-part": "column-empty", "Nothing is waiting on you." } }
                else { InboxList { items, elapsed_ms, agents, reachable } }
            }
        }
    }
}

#[component]
pub(crate) fn InboxList(
    items: Vec<InboxItem>,
    elapsed_ms: u64,
    agents: Vec<Agent>,
    reachable: bool,
) -> Element {
    let mut selected = use_context::<Signal<Option<portal_types::agents::AgentAddress>>>();
    let mut selection = use_context::<Signal<Option<InboxSelection>>>();
    let mut panel = use_context::<Signal<LeadPanel>>();
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    rsx! {
        if let Some(message) = error() { Alert { "{message}" } }
        ItemGroup { class: "inbox-list",
            for item in items {
                {
                    let kind = item.selection.kind;
                    let title = item.agent_title.as_deref().filter(|title| !title.starts_with("u-"))
                        .unwrap_or("Agent task");
                    let repository = item.repository.as_deref().map(repository_name);
                    let age = age(item.age_ms.saturating_add(elapsed_ms));
                    let open = selection().as_ref() == Some(&item.selection);
                    let options = agents.iter().find(|agent| agent.host == item.selection.host && agent.pane == item.selection.pane)
                        .and_then(|agent| agent.blocked.as_ref()).map(|prompt| prompt.options.clone()).unwrap_or_default();
                    rsx! {
                        div { class: "inbox-row", key: "{kind:?}:{item.selection.id}", "data-open": open,
                            Button { class: "inbox-item", variant: ButtonVariant::Ghost, r#type: "button",
                                aria_current: open.then_some("true"),
                                onclick: {
                                    let item = item.clone();
                                    move |_| {
                                        selected.set(Some(portal_types::agents::AgentAddress { host: item.selection.host.clone(), pane: item.selection.pane.clone() }));
                                        selection.set(Some(item.selection.clone()));
                                        panel.set(LeadPanel::Chat);
                                    }
                                },
                                span { class: "inbox-kind",
                                    match kind {
                                        InboxKind::Review => rsx! { FileText { size: 16, attributes: dioxus_primitives::dioxus_attributes::attributes!(svg { "aria-hidden": "true" }) } },
                                        InboxKind::Question => rsx! { CircleQuestionMark { size: 16, attributes: dioxus_primitives::dioxus_attributes::attributes!(svg { "aria-hidden": "true" }) } },
                                        InboxKind::Prompt => rsx! { CornerDownLeft { size: 16, attributes: dioxus_primitives::dioxus_attributes::attributes!(svg { "aria-hidden": "true" }) } },
                                        InboxKind::UnseenWorker => rsx! { Moon { size: 16, attributes: dioxus_primitives::dioxus_attributes::attributes!(svg { "aria-hidden": "true" }) } },
                                    }
                                    "{kind.label()}" span { class: "inbox-age", "{age}" }
                                }
                                span { class: "inbox-title", "{item.title}" }
                                span { class: "inbox-meta", "{title}" if let Some(repository) = repository { " · {repository}" } }
                            }
                            if kind == InboxKind::Prompt && !options.is_empty() {
                                div { class: "inbox-keys",
                                    for option in options {
                                        {
                                            let pane = portal_types::agents::AgentAddress { host: item.selection.host.clone(), pane: item.selection.pane.clone() };
                                            let send_key = option.key.clone();
                                            rsx! {
                                                Button { key: "{option.key}", variant: ButtonVariant::Secondary, r#type: "button",
                                                    disabled: !reachable || busy() || !crate::agent_view::allowed_key(&option.key),
                                                    title: "{option.label}", aria_label: "{option.label}",
                                                    onclick: move |_| {
                                                        let pane = pane.clone(); let key = send_key.clone();
                                                        async move {
                                                            if busy() || !reachable { return; }
                                                            busy.set(true);
                                                            error.set(api::send_key(&pane, &key).await.err());
                                                            busy.set(false);
                                                        }
                                                    }, "{option.key}"
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
    fn portal_agents_wait_for_a_snapshot_and_survive_a_stream_error()
    -> Result<(), serde_json::Error> {
        let mut state = PortalState::default();
        assert_eq!(state.received_agents(), None);
        let agents: Vec<Agent> = serde_json::from_value(json!([{
            "host": "local", "pane": "pane", "state": "working"
        }]))?;
        let snapshot = portal_types::PortalSnapshot {
            version: 1,
            epoch: "serve".into(),
            revision: 1,
            published_ms: 1,
            board: None,
            agents: Some(agents.clone()),
            inbox: None,
            gaps: vec![],
        };
        state.update(Ok(PortalEvent::Snapshot(Box::new(snapshot))), 1);
        assert_eq!(state.received_agents(), Some(agents.as_slice()));
        assert!(state.agent_error.is_none());
        state.update(Err("stream disconnected".into()), 2);
        assert_eq!(state.received_agents(), Some(agents.as_slice()));
        assert_eq!(state.agent_error.as_deref(), Some("stream disconnected"));
        Ok(())
    }

    #[test]
    fn portal_publications_and_status_keep_stream_order() -> Result<(), serde_json::Error> {
        use portal_types::{PortalAvailability, PortalSnapshot, PortalStatus};

        let snapshot = |revision| {
            PortalEvent::Snapshot(Box::new(PortalSnapshot {
                version: 1,
                epoch: "serve".to_owned(),
                revision,
                published_ms: revision,
                board: Some(BoardView {
                    hosts: vec![],
                    repos: vec![],
                    generated_ms: revision,
                }),
                agents: Some(vec![]),
                inbox: Some(InboxView::default()),
                gaps: vec![],
            }))
        };
        let disconnected = || {
            PortalEvent::Status(PortalStatus {
                observation: PortalAvailability::Disconnected,
                error: "observation publisher disconnected".to_owned(),
            })
        };
        let mut state = PortalState::default();
        // Exercise the same decoder and reducer as the one-name browser stream.
        let apply = |state: &mut PortalState, event: PortalEvent, now| {
            let bytes = serde_json::to_vec(&event)?;
            state.update(Ok(serde_json::from_slice(&bytes)?), now);
            Ok::<(), serde_json::Error>(())
        };
        apply(&mut state, disconnected(), 1)?;
        assert!(state.paused);
        assert!(state.board.is_none());
        apply(&mut state, snapshot(2), 2)?;
        assert!(!state.paused);
        assert!(state.board_error.is_none());
        assert!(state.agent_error.is_none());
        assert_eq!(state.inbox.count(), Some(0));
        assert_eq!(
            state.board.as_ref().map(|board| board.generated_ms),
            Some(2)
        );
        let retained = state.clone();
        apply(&mut state, disconnected(), 3)?;
        assert!(state.paused);
        assert!(state.board_error.is_some());
        assert!(state.agent_error.is_some());
        assert_eq!(state.board, retained.board);
        assert_eq!(state.agents, retained.agents);
        assert_eq!(state.inbox.snapshot, retained.inbox.snapshot);
        assert_eq!(state.inbox.received_ms, 2);
        assert_eq!(state.inbox.count(), None);
        apply(&mut state, snapshot(4), 4)?;
        assert!(!state.paused);
        assert!(state.board_error.is_none());
        assert!(state.agent_error.is_none());
        assert_eq!(state.inbox.count(), Some(0));
        assert_eq!(
            state.board.as_ref().map(|board| board.generated_ms),
            Some(4)
        );
        Ok(())
    }

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
            host: Some("local".into()),
            review: Some("plan".to_owned()),
            question: None,
            pane: Some("pane".to_owned()),
        };
        assert_eq!(
            link.selection(),
            Some(InboxSelection {
                kind: InboxKind::Review,
                id: "plan".to_owned(),
                host: "local".into(),
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
