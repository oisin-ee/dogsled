//! The persistent portal shell and its one sidebar: agents waiting on the owner first, then
//! repository navigation.

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_primitives::scroll_area::ScrollDirection;
use dioxus_sdk_time::{sleep, use_interval};
use portal_types::agents::{ActivityView, AgentAddress, AgentEvent, AgentView as Agent};
use portal_types::inbox::{InboxKind as ServerInboxKind, InboxView};
use portal_types::live::{ClientFrame, ServerFrame};
use portal_types::{BoardView, PortalEvent};

use crate::agent_view::{AgentView, Composers};
use crate::api;
use crate::board::{
    RepoGroup, WaitingAgent, WorkerSummary, navigation_groups, repository_name, take_waiting,
};
use crate::components::badge::{Badge, BadgeVariant};
use crate::components::button::{Button, ButtonVariant};
use crate::components::progress::Progress;
use crate::components::scroll_area::ScrollArea;
use crate::components::sidebar::{
    Sidebar, SidebarCollapsible, SidebarGroup, SidebarGroupContent, SidebarGroupLabel, SidebarMenu,
    SidebarMenuButton, SidebarMenuButtonSize, SidebarMenuItem, SidebarProvider,
};
use crate::components::skeleton::Skeleton;
use crate::components::tabs::{TabContent, Tabs};
use crate::conversations::Conversations;
use crate::new_lead::{NewConversation, NewLead};
use dioxus_icons::lucide::Plus;

/// Portal stream projections shared with the selected agent view.
#[derive(Clone, Copy)]
pub struct AgentPortal {
    pub agents: Memo<Option<Vec<Agent>>>,
    pub agent_error: Memo<Option<String>>,
    pub board: Memo<Option<BoardView>>,
    pub board_error: Memo<Option<String>>,
}

/// Latest data from the current selection on the tab's socket.
#[derive(Clone, Default)]
struct LiveSelection {
    id: u64,
    address: Option<AgentAddress>,
    agent: Option<Result<AgentEvent, String>>,
    activity: Option<Result<ActivityView, crate::activity::Gap>>,
}

impl LiveSelection {
    fn select(&mut self, address: Option<AgentAddress>) {
        *self = Self {
            id: self.id + 1,
            address,
            ..Self::default()
        };
    }

    fn command(&self) -> ClientFrame {
        ClientFrame::Open {
            id: self.id,
            address: self.address.clone(),
        }
    }

    fn receive(&mut self, frame: ServerFrame) {
        match frame {
            ServerFrame::Agent { id, event } if id == self.id => self.agent = Some(Ok(*event)),
            ServerFrame::AgentUnavailable { id, error } if id == self.id => {
                self.agent = Some(Err(error))
            }
            ServerFrame::Activity { id, view } if id == self.id => self.activity = Some(Ok(*view)),
            ServerFrame::ActivityUnavailable { id } if id == self.id => {
                self.activity = Some(Err(crate::activity::Gap::Disconnected));
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct LivePortal(Signal<LiveSelection>);

impl LivePortal {
    pub(crate) fn agent(self, address: &AgentAddress) -> Option<Result<AgentEvent, String>> {
        let current = self.0.read();
        (current.address.as_ref() == Some(address))
            .then(|| current.agent.clone())
            .flatten()
    }

    pub(crate) fn activity(
        self,
        address: &AgentAddress,
    ) -> Option<Result<ActivityView, crate::activity::Gap>> {
        let current = self.0.read();
        (current.address.as_ref() == Some(address))
            .then(|| current.activity.clone())
            .flatten()
    }
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
// The faces ship with the portal (OFL 1.1, licences beside them), so it never waits on a
// font service. Bundled assets get hashed names, so the faces are declared here.
const PLEX_SANS: Asset = asset!("/assets/fonts/ibm-plex-sans-latin.woff2");
const PLEX_SANS_ITALIC: Asset = asset!("/assets/fonts/ibm-plex-sans-italic-latin.woff2");
const JETBRAINS_MONO: Asset = asset!("/assets/fonts/jetbrains-mono-latin.woff2");
const JETBRAINS_MONO_ITALIC: Asset = asset!("/assets/fonts/jetbrains-mono-italic-latin.woff2");

fn font_faces() -> String {
    [
        ("IBM Plex Sans", "normal", "100 700", PLEX_SANS),
        ("IBM Plex Sans", "italic", "100 700", PLEX_SANS_ITALIC),
        ("JetBrains Mono", "normal", "100 800", JETBRAINS_MONO),
        ("JetBrains Mono", "italic", "100 800", JETBRAINS_MONO_ITALIC),
    ]
    .into_iter()
    .map(|(family, style, weight, source)| {
        format!(
            "@font-face{{font-family:\"{family}\";font-style:{style};font-weight:{weight};font-display:swap;src:url(\"{source}\") format(\"woff2\")}}"
        )
    })
    .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LeadPanel {
    Chat,
    Todos,
    Workers,
    Files,
}

impl LeadPanel {
    pub fn value(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Todos => "todos",
            Self::Workers => "workers",
            Self::Files => "files",
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

/// Pane ids can collide across hosts. Only an unambiguous item id resolves a Pi link.
fn resolve_link(
    requested: &InboxSelection,
    items: &[InboxItem],
) -> Result<Option<InboxSelection>, String> {
    let mut matches = items
        .iter()
        .filter(|item| item.selection.kind == requested.kind && item.selection.id == requested.id);
    let found = matches.next().map(|item| item.selection.clone());
    if matches.next().is_some() {
        return Err("This item id exists on more than one host. Select the item from its host's inbox instead.".into());
    }
    Ok(found)
}

impl InboxState {
    fn update(&mut self, result: Result<InboxView, String>, now: u64) {
        match result {
            Ok(view) => {
                self.snapshot = Some(view);
                self.received_ms = now;
                self.error = self.snapshot.as_ref().and_then(|view| {
                    (!view.gaps.is_empty()).then(|| {
                        view.gaps
                            .iter()
                            .map(|gap| format!("{}: {}", gap.host, gap.error))
                            .collect::<Vec<_>>()
                            .join(" · ")
                    })
                });
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
                            host: item.host.clone(),
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

/// The catalogue and its observation health share one equality-filtered projection.
#[derive(Clone, Default, PartialEq)]
pub(crate) struct CatalogueState {
    pub list: Option<portal_types::agents::RepoList>,
    pub observation_error: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) struct CataloguePortal(pub Memo<CatalogueState>);

/// One reducer owns all browser projections and their connection status.
#[derive(Clone, Default)]
pub(crate) struct PortalState {
    board: Option<BoardView>,
    board_error: Option<String>,
    agents: Vec<Agent>,
    agents_received: bool,
    agent_error: Option<String>,
    inbox: InboxState,
    pub(crate) catalogue: CatalogueState,
    paused: bool,
    /// The Dogsled release of the service that serves this portal.
    dogsled: Option<String>,
}

impl PortalState {
    fn received_agents(&self) -> Option<&[Agent]> {
        self.agents_received.then_some(self.agents.as_slice())
    }

    fn receive(&mut self, frame: ServerFrame, now: u64) {
        match frame {
            ServerFrame::Portal { event } => self.update(Ok(event), now),
            ServerFrame::Inbox { result } => self.inbox.update(result, now),
            _ => unreachable!("detail frames go to the selected pane"),
        }
    }

    pub(crate) fn update(&mut self, result: Result<PortalEvent, String>, now: u64) {
        // Only transport loss invalidates both lanes. Observer status and the
        // separately composed inbox publication each update their own projection.
        if let Err(message) = &result {
            self.inbox.update(Err(message.clone()), now);
        }
        let result = result.and_then(|event| match event {
            PortalEvent::Snapshot(snapshot) => Ok(snapshot),
            PortalEvent::Status(status) => Err(status.error),
        });
        match result {
            Ok(snapshot) => {
                if !snapshot.dogsled.is_empty() {
                    self.dogsled = Some(snapshot.dogsled.clone());
                }
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
                        let missing = snapshot
                            .gaps
                            .iter()
                            .filter(|gap| gap.domain.starts_with("ownership:"))
                            .map(|gap| gap.detail.as_str())
                            .collect::<Vec<_>>()
                            .join("; ");
                        self.board_error = (!missing.is_empty()).then_some(missing);
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
                match snapshot.repos {
                    Some(list) => {
                        // A failed fetch without repositories (serve restarted during a GitHub
                        // outage) carries no catalogue facts: keep the retained choices.
                        let list = match self.catalogue.list.take() {
                            Some(retained) if list.error.is_some() && list.repos.is_empty() => {
                                portal_types::agents::RepoList {
                                    repos: retained.repos,
                                    error: list.error,
                                }
                            }
                            _ => list,
                        };
                        self.catalogue.list = Some(list);
                        self.catalogue.observation_error = None;
                    }
                    None => {
                        let detail = snapshot
                            .gaps
                            .iter()
                            .filter(|gap| gap.domain == "repos")
                            .map(|gap| gap.detail.as_str())
                            .collect::<Vec<_>>()
                            .join("; ");
                        self.catalogue.observation_error = Some(if detail.is_empty() {
                            "catalogue observation unavailable".to_owned()
                        } else {
                            detail
                        });
                    }
                }
                // A delivered snapshot proves the stream is live. Its domain gaps are reported
                // where each domain is shown, never as a paused stream.
                self.paused = false;
            }
            Err(message) => {
                self.paused = true;
                self.board_error = Some(message.clone());
                self.agent_error = Some(message.clone());
                self.catalogue.observation_error = Some(message);
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
    let dogsled = use_memo(move || publication.read().dogsled.clone());
    let catalogue = use_memo(move || publication.read().catalogue.clone());
    use_context_provider(|| CataloguePortal(catalogue));
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
    let mut now = use_signal(|| 0_u64);
    let link = use_hook(DeepLink::read);
    let mut selected = use_context_provider(|| {
        Signal::new(
            link.pane
                .clone()
                .filter(|_| {
                    link.host.is_some() || (link.review.is_none() && link.question.is_none())
                })
                .map(|pane| portal_types::agents::AgentAddress {
                    host: link.host.clone().unwrap_or_else(|| "local".into()),
                    pane,
                }),
        )
    });
    let mut selection =
        use_context_provider(|| Signal::new(link.selection().filter(|_| link.host.is_some())));
    let mut linked = use_signal(|| link.host.is_some());
    let mut link_error = use_signal(|| None::<String>);
    use_context_provider(Composers::default);
    let conversation = use_context_provider(|| NewConversation {
        open: Signal::new(false),
        repository: Signal::new(None),
    });
    let mut composing = conversation.open;
    let mut metrics_open = use_signal(|| false);
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
        metrics_open.set(false);
        panel.set(LeadPanel::Chat);
    };
    use_effect(move || {
        if selected().is_some() {
            composing.set(false);
            metrics_open.set(false);
        }
    });
    use_context_provider(|| Signal::new(Conversations::default()));

    let mut live = use_context_provider(|| {
        LivePortal(Signal::new(LiveSelection {
            address: selected.peek().clone(),
            ..LiveSelection::default()
        }))
    })
    .0;
    let socket = use_coroutine(move |mut changes| async move {
        loop {
            let mut details = live;
            let result = api::watch(
                &mut changes,
                || live.peek().command(),
                |frame| match frame {
                    frame @ (ServerFrame::Portal { .. } | ServerFrame::Inbox { .. }) => {
                        publication.write().receive(frame, clock_ms())
                    }
                    detail => details.write().receive(detail),
                },
            )
            .await;
            if let Err(message) = result {
                publication.write().update(Err(message.clone()), clock_ms());
                let mut details = live.write();
                details.agent = Some(Err(message));
                details.activity = Some(Err(crate::activity::Gap::Disconnected));
            }
            sleep(RETRY).await;
            let address = live.peek().address.clone();
            live.write().select(address);
        }
    });
    use_effect(move || {
        let address = selected();
        if address != live.peek().address {
            live.write().select(address);
            socket.send(());
        }
    });
    use_interval(RETRY, move |()| now.set(clock_ms()));

    let inbox = use_memo(move || {
        let mut items = publication.read().inbox.items();
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
        if linked() {
            return;
        }
        let Some(requested) = link.selection() else {
            linked.set(true);
            return;
        };
        let current = publication.read();
        let state = &current.inbox;
        if state.snapshot.is_none() {
            return;
        }
        match resolve_link(&requested, &inbox()) {
            Ok(Some(current)) => {
                selected.set(Some(portal_types::agents::AgentAddress {
                    host: current.host.clone(),
                    pane: current.pane.clone(),
                }));
                selection.set(Some(current));
                link_error.set(None);
                linked.set(true);
            }
            Ok(None) => link_error.set(Some(state.error.as_ref().map_or_else(
                || "This question or review is no longer pending.".to_owned(),
                |error| format!("Cannot find this item while a host is unavailable: {error}"),
            ))),
            Err(error) => {
                selected.set(None);
                selection.set(None);
                link_error.set(Some(error));
            }
        }
    });

    let view = board();
    let items = inbox();
    let state = publication.read().inbox.clone();
    let elapsed_ms = now().saturating_sub(state.received_ms);
    let mut groups = view.as_ref().map(|view| navigation_groups(view, &agents()));
    let agent_count = groups.as_ref().map_or(0, |groups| {
        groups
            .repos
            .iter()
            .map(|repo| repo.agent_count())
            .sum::<usize>()
    });
    let to_land = groups.as_ref().map_or(0, |groups| {
        groups
            .repos
            .iter()
            .map(|repo| repo.to_land.len())
            .sum::<usize>()
    });
    let working = agents()
        .iter()
        .filter(|agent| matches!(agent.state.as_str(), "working" | "running"))
        .count();
    let idle = agents()
        .iter()
        .filter(|agent| agent.state == "idle")
        .count();

    let lead = groups.as_ref().and_then(|groups| {
        let address = selected()?;
        let roots = || groups.repos.iter().flat_map(|repo| &repo.rows);
        let owner = view
            .as_ref()?
            .repos
            .iter()
            .flat_map(|repo| &repo.rows)
            .find(|row| row.host == address.host && row.pane.as_ref() == Some(&address.pane))
            .and_then(|row| {
                Some((
                    row.owner_host.as_deref().unwrap_or(&row.host),
                    row.owner_pane.as_ref()?,
                ))
            });
        if let Some((host, pane)) = owner {
            roots()
                .flat_map(|row| std::iter::once(row).chain(&row.workers))
                .find(|row| row.host == host && row.pane.as_ref() == Some(pane))
                .cloned()
        } else {
            roots()
                .find(|row| {
                    !row.worker
                        && row.host == address.host
                        && row.pane.as_ref() == Some(&address.pane)
                })
                .cloned()
        }
    });
    // The New page lists every lead of its repository, waiting ones too.
    let running = groups
        .as_ref()
        .filter(|_| composing())
        .map(|groups| groups.repos.clone())
        .unwrap_or_default();
    // Leads are counted before waiting agents leave their repository groups.
    let waiting = groups
        .as_mut()
        .zip(view.as_ref())
        .map(|(groups, board)| take_waiting(groups, &items, &board.hosts, &agents()))
        .unwrap_or_default();

    let unavailable =
        selected().and_then(
            |address| match (&view, publication.read().received_agents()) {
                (Some(board), Some(agents)) => crate::board::status_only(board, agents, &address),
                _ => Some("Loading ownership…"),
            },
        );
    let navigation = rsx! {
            Sidebar { class: "console-agents", collapsible: SidebarCollapsible::None, aria_label: "Repositories and agents",
                ScrollArea { class: "console-scroll", direction: ScrollDirection::Vertical,
                    NeedsYou { count: state.count(), error: link_error().or(state.error.clone()), waiting,
                        inbox: items.clone(), elapsed_ms, agents: agents(), reachable: agent_error().is_none() }
                    h2 { class: "sidebar-section", "Agents" span { "{agent_count}" } }
                    if let Some(message) = board_error() { Alert { "Board updates unavailable: {message}" } }
                    // The agent list comes from the observation, not from Herdr. Show its own
                    // error, once when the board has the same one.
                    if let Some(message) = agent_error().filter(|message| board_error().as_ref() != Some(message)) { Alert { "Agents unavailable: {message}" } }
                    match groups {
                        Some(groups) => rsx! {
                            for (index, group) in groups.repos.into_iter().enumerate() {
                                RepoGroup { key: "{index}:{group.repository:?}", group, inbox: items.clone() }
                            }
                        },
                        None => rsx! { Skeleton { role: "status", aria_label: "Loading agents" } },
                    }
                }
            }
    };

    rsx! {
        document::Style { "{font_faces()}" }
        document::Script { src: asset!("/assets/unread.js") }
        document::Stylesheet { href: COMPONENTS_CSS }
        document::Stylesheet { href: CONSOLE_CSS }
        document::Stylesheet { href: MARKDOWN_CSS }
        document::Stylesheet { href: asset!("/assets/metrics.css") }
        document::Stylesheet { href: asset!("/assets/review.css") }
        SidebarProvider { class: "console-provider",
        Tabs { class: "console-shell", "data-panel": panel().value(), horizontal: true, value: Some(panel().value().to_owned()),
            on_value_change: move |value: String| {
                let next = match value.as_str() { "todos" => LeadPanel::Todos, "workers" => LeadPanel::Workers, "files" => LeadPanel::Files, _ => LeadPanel::Chat };
                if matches!(next, LeadPanel::Todos | LeadPanel::Workers | LeadPanel::Files) { detail.set(next); }
                panel.set(next);
            },
            header { class: "console-topbar",
                div { class: "console-brand",
                    a { href: "/", aria_label: "Homepage", "Dogsled" }
                    if let Some(version) = dogsled() {
                        Badge { class: "console-version", variant: BadgeVariant::Outline, title: "Dogsled {version}", "v{version}" }
                    }
                }
                div { class: "console-actions",
                    Button { class: "console-new", variant: ButtonVariant::Secondary, r#type: "button", aria_pressed: composing(),
                        onclick: move |_| start_new(),
                        Plus { size: 16, attributes: dioxus_primitives::dioxus_attributes::attributes!(svg { "aria-hidden": "true" }) }
                        "New"
                    }
                    Button { class: "console-metrics", variant: ButtonVariant::Secondary, r#type: "button", aria_pressed: metrics_open(),
                        onclick: move |_| {
                            selected.set(None);
                            selection.set(None);
                            composing.set(false);
                            metrics_open.set(true);
                            panel.set(LeadPanel::Chat);
                        },
                        "Metrics"
                    }
                }
                p { class: "console-tally", aria_label: "Fleet",
                    if let Some(count) = state.count().filter(|count| *count > 0) {
                        span { class: "tally-waiting", b { "{count}" } " need you" }
                    }
                    span { class: "tally-working", b { "{working}" } " working" }
                    span { b { "{idle}" } " idle" }
                    if to_land > 0 { span { b { "{to_land}" } " to land" } }
                }
                if stream_paused() { Status { "Live updates paused, retrying" } }
                div { class: "console-hosts", aria_label: "Host load",
                    if let Some(view) = &view {
                        for host in &view.hosts {
                            {
                                let busy = host.busy.filter(|busy| busy.is_finite());
                                let load = busy.map_or(0.0, |busy| if host.cores == 0 { 0.0 } else { (busy / f64::from(host.cores) * 100.0).clamp(0.0, 100.0) });
                                let value = busy.map_or_else(|| format!("—/{}", host.cores), |busy| format!("{busy:.0}/{}", host.cores));
                                rsx! {
                                    div { class: "console-host", key: "{host.label}", title: "{host.label}: {host.service}, CPU {value}",
                                        span { class: "host-label", "{host.label}" }
                                        Progress { value: Some(load), max: 100.0, aria_label: "{host.label} CPU load" }
                                        span { class: "host-value", "{value}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            {navigation}
            main { class: "console-main",
                match selected() {
                    Some(_) if unavailable.is_some() => rsx! { Status { "{unavailable.unwrap_or_default()}" } },
                    Some(pane) => {
                        let picked = selection().filter(|item| item.host == pane.host && item.pane == pane.pane);
                        let pick = |kind| picked.as_ref().filter(|item| item.kind == kind).map(|item| item.id.clone());
                        let review = pick(InboxKind::Review);
                        let question = pick(InboxKind::Question);
                        rsx! { AgentView { key: "{pane.host}:{pane.pane}", host: pane.host, pane: pane.pane, review, question, lead: lead.clone(), inbox: items.clone() } }
                    }
                    None if metrics_open() => rsx! { crate::metrics::Metrics {} },
                    None if composing() => rsx! { NewLead { hosts: view.as_ref().map(|view| view.hosts.clone()).unwrap_or_default(), running } },
                    None => rsx! { div { class: "empty-lead",
                        TabContent { class: "empty-chat", value: "chat", index: 0usize,
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

/// Agents waiting on the owner, at the top of the one sidebar. The section is shown while
/// anything waits or the inbox cannot be read, so a host gap is never an empty list.
#[component]
pub(crate) fn NeedsYou(
    count: Option<usize>,
    error: Option<String>,
    waiting: Vec<WaitingAgent>,
    inbox: Vec<InboxItem>,
    elapsed_ms: u64,
    agents: Vec<Agent>,
    reachable: bool,
) -> Element {
    let mut selected = use_context::<Signal<Option<portal_types::agents::AgentAddress>>>();
    let mut selection = use_context::<Signal<Option<InboxSelection>>>();
    let mut panel = use_context::<Signal<LeadPanel>>();
    let mut busy = use_signal(|| false);
    let mut key_error = use_signal(|| None::<String>);
    if waiting.is_empty() && error.is_none() {
        return rsx! {};
    }
    let label = count.map_or_else(|| "—".to_owned(), |count| count.to_string());
    rsx! {
        SidebarGroup { class: "needs-you", aria_label: "Needs you",
            SidebarGroupLabel { span { "Needs you" }
                Badge { "data-part": "inbox-count", variant: BadgeVariant::Secondary, "data-waiting": !waiting.is_empty(), "{label}" }
            }
            if let Some(message) = &error { Alert { "Inbox unavailable: {message}. Reload to retry." } }
            if let Some(message) = key_error() { Alert { "{message}" } }
            SidebarGroupContent {
                SidebarMenu {
                    for agent in waiting {
                        {
                            let item = agent.items[0].clone();
                            let kind = item.selection.kind;
                            let more = agent.items.len() - 1;
                            let address = portal_types::agents::AgentAddress { host: item.selection.host.clone(), pane: item.selection.pane.clone() };
                            let is_selected = selected().as_ref() == Some(&address);
                            let open = selection().as_ref().is_some_and(|picked| agent.items.iter().any(|item| &item.selection == picked));
                            let age = age(item.age_ms.saturating_add(elapsed_ms));
                            let repository = agent.repository.as_deref().map(repository_name);
                            let summary = format!("{}{} · {}: {}", agent.row.title,
                                repository.map(|name| format!(" · {name}")).unwrap_or_default(), kind.label(), item.title);
                            let place = repository.map_or_else(|| agent.row.title.clone(), str::to_owned);
                            let options = agents.iter().find(|agent| agent.host == address.host && agent.pane == address.pane)
                                .and_then(|agent| agent.blocked.as_ref()).map(|prompt| prompt.options.clone()).unwrap_or_default();
                            let workers = !agent.row.worker && !agent.row.workers.is_empty();
                            let lead = agent.row.clone();
                            let body = rsx! {
                                span { class: "inbox-kind",
                                    span { class: "inbox-place", "{kind.label()} · {place}" }
                                    if more > 0 { span { class: "inbox-more", "+{more}" } }
                                    span { class: "inbox-age", "{age}" }
                                    crate::unread::UnreadDot { host: address.host.clone(), pane: address.pane.clone() }
                                }
                                span { class: "inbox-title", "{item.title}" }
                                if repository.is_some() { span { class: "agent-title", "{agent.row.title}" } }
                                crate::board::RuntimeChips { runtime: agent.row.runtime.clone() }
                            };
                            rsx! {
                                SidebarMenuItem { key: "{address.host}:{address.pane}", class: "waiting-agent", "data-open": open,
                                    SidebarMenuButton { is_active: is_selected, size: SidebarMenuButtonSize::Lg, title: "{summary}",
                                        r#as: move |attributes: Vec<Attribute>| {
                                            let body = body.clone();
                                            let address = address.clone();
                                            let picked = item.selection.clone();
                                            rsx! {
                                                button { r#type: "button", aria_current: is_selected.then_some("true"),
                                                    onclick: move |_| {
                                                        selected.set(Some(address.clone()));
                                                        selection.set(Some(picked.clone()));
                                                        panel.set(LeadPanel::Chat);
                                                    },
                                                    ..attributes,
                                                    {body}
                                                }
                                            }
                                        },
                                    }
                                    if kind == InboxKind::Prompt && !options.is_empty() {
                                        div { class: "inbox-keys",
                                            for option in options {
                                                {
                                                    let pane = portal_types::agents::AgentAddress { host: lead.host.clone(), pane: lead.pane.clone().unwrap_or_default() };
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
                                                                    key_error.set(api::send_key(&pane, &key).await.err());
                                                                    busy.set(false);
                                                                }
                                                            }, "{option.key}"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    if workers { WorkerSummary { lead, inbox: inbox.clone() } }
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
    fn catalogue_keeps_refresh_and_observation_failures_separate_until_recovery() {
        let mut state = PortalState::default();
        let event = |repos, gaps| {
            serde_json::from_value(json!({
                "version":portal_types::PORTAL_VERSION,"epoch":"serve","revision":1,
                "published_ms":1,"board":null,"agents":null,"inbox":null,"repos":repos,"gaps":gaps
            }))
            .unwrap()
        };
        let failed = json!({"repos":[{"slug":"owner/repo","description":null,"pushed_at":null,"private":false}],"error":"GitHub catalogue: failed"});
        state.update(Ok(event(failed.clone(), json!([]))), 1);
        assert_eq!(state.catalogue.observation_error, None);
        let list = state.catalogue.list.clone();
        state.update(
            Ok(event(
                serde_json::Value::Null,
                json!([{
                    "domain":"repos","availability":"failed","detail":"malformed catalogue"
                }]),
            )),
            2,
        );
        assert_eq!(state.catalogue.list, list);
        assert_eq!(
            state.catalogue.observation_error.as_deref(),
            Some("malformed catalogue")
        );
        state.update(Ok(event(serde_json::Value::Null, json!([]))), 3);
        assert_eq!(state.catalogue.list, list);
        assert_eq!(
            state.catalogue.observation_error.as_deref(),
            Some("catalogue observation unavailable")
        );
        state.update(Err("stream lost".into()), 4);
        assert_eq!(state.catalogue.list, list);
        assert_eq!(
            state.catalogue.observation_error.as_deref(),
            Some("stream lost")
        );
        let mut good = failed;
        good["error"] = serde_json::Value::Null;
        state.update(
            Ok(event(
                good,
                json!([{"domain":"board","availability":"failed","detail":"unrelated"}]),
            )),
            5,
        );
        assert_eq!(state.catalogue.observation_error, None);
        assert_eq!(
            state
                .catalogue
                .list
                .as_ref()
                .and_then(|list| list.error.as_ref()),
            None
        );
        assert_eq!(
            state.catalogue.list.as_ref().map(|list| &list.repos),
            list.as_ref().map(|list| &list.repos)
        );
    }

    #[test]
    fn portal_agents_wait_for_a_snapshot_and_survive_a_stream_error()
    -> Result<(), serde_json::Error> {
        let mut state = PortalState::default();
        assert_eq!(state.received_agents(), None);
        let agents: Vec<Agent> = serde_json::from_value(json!([{
            "host": "local", "pane": "pane", "state": "working"
        }]))?;
        let snapshot = portal_types::PortalSnapshot {
            version: portal_types::PORTAL_VERSION,
            dogsled: "0.42.0".into(),
            epoch: "serve".into(),
            revision: 1,
            published_ms: 1,
            board: None,
            agents: Some(agents.clone()),
            inbox: None,
            repos: None,
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
    fn late_detail_frames_are_ignored_even_when_reopening_the_same_pane() {
        let address = AgentAddress::local("pane");
        let mut selection = LiveSelection::default();
        selection.select(Some(address.clone()));
        let first = selection.id;
        selection.select(None);
        selection.select(Some(address));
        selection.receive(ServerFrame::AgentUnavailable {
            id: first,
            error: "old selection".into(),
        });
        selection.receive(ServerFrame::ActivityUnavailable { id: first });
        assert!(selection.agent.is_none());
        assert!(selection.activity.is_none());
        selection.receive(ServerFrame::ActivityUnavailable { id: selection.id });
        assert_eq!(
            selection.activity,
            Some(Err(crate::activity::Gap::Disconnected))
        );
    }

    #[test]
    fn portal_publications_and_status_keep_stream_order() -> Result<(), String> {
        use portal_types::{PortalAvailability, PortalSnapshot, PortalStatus};

        let gapped = |revision, gaps| {
            PortalEvent::Snapshot(Box::new(PortalSnapshot {
                version: portal_types::PORTAL_VERSION,
                dogsled: "0.42.0".to_owned(),
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
                repos: Some(portal_types::agents::RepoList::default()),
                gaps,
            }))
        };
        let snapshot = |revision| gapped(revision, vec![]);
        let disconnected = || {
            PortalEvent::Status(PortalStatus {
                observation: PortalAvailability::Disconnected,
                error: "observation publisher disconnected".to_owned(),
            })
        };
        let mut state = PortalState::default();
        // Exercise the same decoder and reducer as the browser socket.
        let mut stamp = None;
        let mut apply = |state: &mut PortalState, frame: ServerFrame, now| {
            let text = serde_json::to_string(&frame).map_err(|error| error.to_string())?;
            state.receive(api::decode_live(&text, &mut stamp)?, now);
            Ok::<(), String>(())
        };
        apply(
            &mut state,
            ServerFrame::Portal {
                event: disconnected(),
            },
            1,
        )?;
        assert!(state.paused);
        assert!(state.board.is_none());
        apply(&mut state, ServerFrame::Portal { event: snapshot(2) }, 2)?;
        apply(
            &mut state,
            ServerFrame::Inbox {
                result: Ok(InboxView::default()),
            },
            2,
        )?;
        assert!(!state.paused);
        assert!(state.board_error.is_none());
        assert!(state.agent_error.is_none());
        assert_eq!(state.inbox.count(), Some(0));
        assert_eq!(
            state.board.as_ref().map(|board| board.generated_ms),
            Some(2)
        );
        let retained = state.clone();
        apply(
            &mut state,
            ServerFrame::Portal {
                event: disconnected(),
            },
            3,
        )?;
        assert!(state.paused);
        assert!(state.board_error.is_some());
        assert!(state.agent_error.is_some());
        assert_eq!(state.board, retained.board);
        assert_eq!(state.agents, retained.agents);
        apply(
            &mut state,
            ServerFrame::Inbox {
                result: Err("observation publisher disconnected".into()),
            },
            3,
        )?;
        assert_eq!(state.inbox.snapshot, retained.inbox.snapshot);
        assert_eq!(state.inbox.received_ms, 2);
        assert_eq!(state.inbox.count(), None);
        // A domain gap is not a transport pause: the stream still delivered this snapshot.
        let gap = portal_types::ObservationGap {
            domain: "holds/job-validity/lead".into(),
            availability: portal_types::ObservationAvailability::Failed,
            detail: "resource job-validity/lead has no recorded owner pane".into(),
        };
        apply(
            &mut state,
            ServerFrame::Portal {
                event: gapped(4, vec![gap]),
            },
            4,
        )?;
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
    fn an_unreachable_host_keeps_a_visible_gap_instead_of_an_empty_inbox() {
        let mut state = InboxState::default();
        state.update(
            Ok(InboxView {
                count: 0,
                items: Vec::new(),
                gaps: vec![portal_types::inbox::HostGap {
                    host: "netcup".into(),
                    error: "unreachable".into(),
                }],
            }),
            1,
        );
        assert_eq!(state.count(), None);
        assert!(
            state
                .error
                .as_deref()
                .is_some_and(|error| error.contains("netcup: unreachable"))
        );
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
    fn deep_links_resolve_by_item_id_and_reject_cross_host_collisions() {
        let requested = InboxSelection {
            host: "local".into(),
            kind: InboxKind::Review,
            id: "review".into(),
            pane: "same-pane".into(),
        };
        let item = |host: &str| InboxItem {
            selection: InboxSelection {
                host: host.into(),
                kind: InboxKind::Review,
                id: "review".into(),
                pane: "same-pane".into(),
            },
            revision: 1,
            title: "Plan".into(),
            agent_title: None,
            repository: None,
            age_ms: 0,
        };
        let peer = item("netcup");
        assert_eq!(
            resolve_link(&requested, std::slice::from_ref(&peer)),
            Ok(Some(peer.selection.clone()))
        );
        assert!(resolve_link(&requested, &[peer, item("local")]).is_err());
        assert_eq!(resolve_link(&requested, &[]), Ok(None));
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
