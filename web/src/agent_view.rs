//! The selected agent's screen: its pending request, or its conversation.

use std::rc::Rc;
use std::time::Duration;

use dioxus::html::geometry::PixelsVector2D;
use dioxus::prelude::*;
use dioxus::web::WebEventExt;
use dioxus_icons::lucide::{ArrowUp, ExternalLink, Paperclip, Square, X};
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::scroll_area::ScrollDirection;
use dioxus_sdk_time::{sleep, use_interval};
use gloo_net::http::Request;
use js_sys::wasm_bindgen::JsCast;
use portal_types::agents::{self as wire, BlockedPrompt, Role, Turn};
use portal_types::questions::{Pending, PendingList};
use portal_types::reviews::{ReviewList, ReviewSummary};
use portal_types::{TodoStatusView, TodoView};
use serde::de::DeserializeOwned;

use crate::activity::{Gap, Live, use_live_activity};
use crate::api;
use crate::components::{
    accordion::{Accordion, AccordionContent, AccordionItem, AccordionTrigger},
    button::{Button, ButtonVariant},
    collapsible::{Collapsible, CollapsibleContent, CollapsibleTrigger},
    input::Input,
    item::{Item, ItemContent, ItemDescription, ItemGroup, ItemMedia, ItemSize, ItemTitle},
    scroll_area::ScrollArea,
    skeleton::Skeleton,
    tabs::{TabContent, TabList, TabTrigger, Tabs},
    textarea::Textarea,
};
use crate::conversations::{Conversations, use_conversation};
use crate::notice::{Alert, Status};
use crate::question_form::QuestionForm;
use crate::review_view::ReviewView;

const BASE: &str = match option_env!("DOGSLED_API") {
    Some(base) => base,
    None => "",
};

/// A message draft shared with the review view when its provider is present.
#[derive(Clone, Copy)]
pub struct Draft(pub Signal<String>);

/// The compact prompt controls are a subset of the server's allowed keys.
const PANEL_KEYS: &[(&str, &str)] = &[
    ("esc", "Esc"),
    ("enter", "Enter"),
    ("up", "↑"),
    ("down", "↓"),
];

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Pending,
    Conversation,
}

#[derive(Clone, PartialEq)]
enum RequestTab {
    Review(String),
    Questions(String, usize),
    Prompt,
}

impl RequestTab {
    fn identity(&self) -> String {
        match self {
            Self::Review(id) => format!("review:{id}"),
            Self::Questions(id, _) => format!("question:{id}"),
            Self::Prompt => "prompt".to_owned(),
        }
    }

    fn state(&self) -> &'static str {
        match self {
            Self::Review(_) => "Waiting for your plan review",
            Self::Questions(_, _) => "Waiting for your answers",
            Self::Prompt => "Blocked on a prompt",
        }
    }
}

#[component]
pub fn AgentView(
    host: String,
    pane: String,
    #[props(default)] review: Option<String>,
    #[props(default)] question: Option<String>,
    agent_count: usize,
    lead: Option<crate::board::NavigationRow>,
    inbox: Vec<crate::app::InboxItem>,
) -> Element {
    rsx! {
        AgentSession { key: "{host}:{pane}", host, pane, review, question, agent_count, lead, inbox }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum AgentMetadata {
    Loading,
    Missing,
    Unreachable {
        message: String,
        agent: Option<wire::AgentView>,
    },
    Present(wire::AgentView),
}

fn agent_metadata(
    agents: Option<&[wire::AgentView]>,
    error: Option<&str>,
    address: &wire::AgentAddress,
) -> AgentMetadata {
    let agent = agents.and_then(|agents| {
        agents
            .iter()
            .find(|agent| agent.host == address.host && agent.pane == address.pane)
            .cloned()
    });
    if let Some(message) = error {
        return AgentMetadata::Unreachable {
            message: message.to_owned(),
            agent,
        };
    }
    match (agents, agent) {
        (None, _) => AgentMetadata::Loading,
        (_, Some(agent)) => AgentMetadata::Present(agent),
        (Some(_), None) => AgentMetadata::Missing,
    }
}

#[component]
fn AgentSession(
    host: String,
    pane: String,
    review: Option<String>,
    question: Option<String>,
    agent_count: usize,
    lead: Option<crate::board::NavigationRow>,
    inbox: Vec<crate::app::InboxItem>,
) -> Element {
    let address = wire::AgentAddress {
        host: host.clone(),
        pane: pane.clone(),
    };
    let portal = use_context::<crate::app::AgentPortal>();
    let mut latest_event = use_signal(|| None::<wire::AgentEvent>);
    let agent_address = address.clone();
    let metadata = use_memo(move || {
        agent_metadata(
            portal.agents.read().as_deref(),
            portal.agent_error.read().as_deref(),
            &agent_address,
        )
    });
    let loading = use_memo(move || matches!(&*metadata.read(), AgentMetadata::Loading));
    let agent = use_memo(move || {
        let mut current = match &*metadata.read() {
            AgentMetadata::Present(agent) => Some(agent.clone()),
            AgentMetadata::Unreachable { agent, .. } => agent.clone(),
            AgentMetadata::Loading | AgentMetadata::Missing => None,
        };
        if let (Some(current), Some(event)) = (current.as_mut(), latest_event.read().as_ref()) {
            if let Some(state) = &event.state {
                current.state.clone_from(state);
                current.blocked.clone_from(&event.blocked);
            } else if event.blocked.is_some() {
                current.blocked.clone_from(&event.blocked);
            }
        }
        current
    });
    let reachable = use_memo(move || {
        matches!(
            &*metadata.read(),
            AgentMetadata::Present(_) | AgentMetadata::Missing
        )
    });
    let metadata_error = use_memo(move || match &*metadata.read() {
        AgentMetadata::Loading | AgentMetadata::Present(_) => None,
        AgentMetadata::Missing => {
            Some("This agent is no longer available. Select another agent.".to_owned())
        }
        AgentMetadata::Unreachable { message, .. } => Some(message.clone()),
    });
    let mut reviews = use_signal(|| None::<Vec<ReviewSummary>>);
    let mut questions = use_signal(|| None::<Vec<Pending>>);
    let mut selection = use_context::<Signal<Option<crate::app::InboxSelection>>>();
    let mut conversations = use_context::<Signal<Conversations>>();
    let mut detail_session = use_signal(|| None::<wire::DetailSession>);
    let mut screen = use_signal(|| None::<wire::Screen>);
    let mut screen_error = use_signal(|| None::<String>);
    let mut pending_error = use_signal(|| None::<String>);
    let mut stream_error = use_signal(|| None::<String>);
    let mut action_error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let mut conversation_trigger = use_signal(|| 0_u64);
    let mut screen_trigger = use_signal(|| 0_u64);
    // A choice belongs to the request that was visible when it was made.
    // A new request opens in front; the same request preserves the tab.
    let mut choice = use_signal(|| None::<(String, Tab)>);
    let mut images = use_signal(Vec::<String>::new);
    let uploading = use_signal(|| false);
    use_effect(move || {
        let _ = selection();
        choice.set(None);
    });
    let local_draft = use_signal(String::new);
    let mut draft = try_use_context::<Draft>().map_or(local_draft, |draft| draft.0);

    // Transcript I/O must start and render without waiting for the full agent list.
    let transcript = use_conversation(address.clone(), conversation_trigger, detail_session);
    let live = use_live_activity(address.clone(), detail_session);

    let screen_pane = address.clone();
    use_resource(move || {
        let trigger = screen_trigger();
        let blocked = latest_event.peek().as_ref().map_or_else(
            || agent.peek().as_ref().is_some_and(is_blocked),
            |event| event_is_blocked(event, agent.peek().as_ref().is_some_and(is_blocked)),
        );
        let pane = screen_pane.clone();
        async move {
            if trigger == 0 {
                return;
            }
            // A failed or pending transcript must not hide the prompt needed to reply.
            let result = if blocked {
                match api::fetch_screen(&pane).await {
                    Ok(value)
                        if detail_session.peek().as_ref().is_some_and(|session| {
                            session != &wire::DetailSession::Available(value.session.clone())
                        }) =>
                    {
                        return;
                    }
                    Ok(value) => Ok(Some(value)),
                    Err(error) => Err(error),
                }
            } else {
                Ok(None)
            };
            match result {
                Ok(value) => {
                    screen.set(value);
                    screen_error.set(None);
                }
                Err(message) => {
                    screen.set(None);
                    screen_error.set(Some(message));
                }
            }
        }
    });

    let pending_address = address.clone();
    let inbox_ids = use_memo(use_reactive(&inbox, move |inbox| {
        agent_inbox_ids(&inbox, &pending_address)
    }));
    use_resource(move || {
        let _ = inbox_ids();
        async move {
            let (review_result, question_result) = futures_util::join!(
                pending_list::<ReviewList>("reviews"),
                pending_list::<PendingList>("questions")
            );
            let mut errors = Vec::new();
            match review_result {
                Ok(list) => reviews.set(Some(list.reviews)),
                Err(message) => errors.push(message),
            }
            match question_result {
                Ok(list) => questions.set(Some(list.pending)),
                Err(message) => errors.push(message),
            }
            pending_error.set((!errors.is_empty()).then(|| errors.join(" · ")));
        }
    });

    let event_pane = address.clone();
    use_future(move || {
        let pane = event_pane.clone();
        async move {
            let mut backoff = Backoff::default();
            let mut revisions = None;
            loop {
                let mut was_blocked = agent.peek().as_ref().is_some_and(is_blocked);
                let result = api::watch_agent(&pane, |event| {
                    backoff.reset();
                    stream_error.set(None);
                    let parts = changed_parts(revisions.as_ref(), &event, was_blocked);
                    was_blocked = event_is_blocked(&event, was_blocked);
                    if detail_session.peek().as_ref() != Some(&event.session) {
                        match &event.session {
                            wire::DetailSession::Available(session) => conversations
                                .write()
                                .host_mut(&pane.host)
                                .select_session(&pane.pane, session),
                            wire::DetailSession::Unavailable => conversations
                                .write()
                                .host_mut(&pane.host)
                                .unavailable(&pane.pane),
                        }
                        screen.set(None);
                        detail_session.set(Some(event.session.clone()));
                    }
                    revisions.clone_from(&event.revisions);
                    latest_event.set(Some(event));
                    if parts.conversation {
                        conversation_trigger += 1;
                    }
                    if parts.screen {
                        screen_trigger += 1;
                    }
                    if !was_blocked {
                        screen.set(None);
                        screen_error.set(None);
                    }
                })
                .await;
                stream_error.set(Some(
                    result
                        .err()
                        .unwrap_or_else(|| "the agent stream ended".to_owned()),
                ));
                revisions = None;
                latest_event.set(None);
                sleep(backoff.next_delay()).await;
            }
        }
    });

    let send_pane = address.clone();
    let send = move |prompt_reply: bool| {
        let pane = send_pane.clone();
        let text = draft();
        let sent_images = if prompt_reply { Vec::new() } else { images() };
        let message = message_with_images(&text, &sent_images);
        if busy() || uploading() || !reachable() || message.trim().is_empty() {
            return;
        }
        busy.set(true);
        action_error.set(None);
        spawn(async move {
            let result = api::send_prompt(&pane, &message, prompt_reply).await;
            match result {
                Ok(()) => {
                    if draft() == text {
                        draft.set(String::new());
                    }
                    if !prompt_reply && images() == sent_images {
                        images.set(Vec::new());
                    }
                }
                Err(message) => action_error.set(Some(message)),
            }
            busy.set(false);
        });
    };

    let mut reply_send = send.clone();
    let mut conversation_send = send;
    let key_pane = address.clone();
    let send_key = move |key: String| {
        let pane = key_pane.clone();
        if busy() || !reachable() || !allowed_key(&key) {
            return;
        }
        busy.set(true);
        action_error.set(None);
        spawn(async move {
            action_error.set(api::send_key(&pane, &key).await.err());
            busy.set(false);
        });
    };
    let mut interrupt = send_key.clone();
    let current = agent();
    let retained = match detail_session.read().as_ref() {
        Some(wire::DetailSession::Available(session)) => conversations
            .read()
            .host(&host)
            .and_then(|cache| cache.get_session(&pane, session))
            .cloned(),
        _ => None,
    };
    let transcript_loading =
        retained.is_none() && (conversation_trigger() == 0 || transcript.read().is_none());
    let transcript_error = transcript
        .read()
        .as_ref()
        .and_then(|result| result.as_ref().err().cloned());
    let board_view = (portal.board)();
    let row = board_view.as_ref().and_then(|board| {
        board
            .repos
            .iter()
            .flat_map(|repo| &repo.rows)
            .find(|row| row.host == host && row.pane.as_deref() == Some(&pane))
    });
    let owner_host = lead
        .as_ref()
        .map_or(host.as_str(), |lead| lead.host.as_str());
    let owner_pane = lead
        .as_ref()
        .and_then(|lead| lead.pane.as_deref())
        .unwrap_or(&pane);
    let todos: Vec<TodoView> = board_view.as_ref().map_or_else(Vec::new, |board| {
        board
            .repos
            .iter()
            .flat_map(|repo| &repo.todos)
            .filter(|todo| owner_host == "local" && todo.pane == owner_pane)
            .cloned()
            .collect()
    });
    let worker_rows = crate::workers::owned_rows(board_view.as_ref(), lead.as_ref());
    let workers: Vec<(String, String)> = worker_rows
        .iter()
        .map(|row| (row.id.clone(), format!("{} · {}", row.what, row.host)))
        .collect();
    let worker_count = worker_rows.len();
    let working_count = crate::workers::working_count(&worker_rows);
    let todo_count: usize = todos
        .iter()
        .flat_map(|todo| &todo.sections)
        .map(|section| section.count)
        .sum();
    let todo_title = todos.iter().find_map(|todo| nonempty(&todo.current));
    let title = if let Some(row) = row {
        row.title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(&row.name)
            .to_owned()
    } else {
        match current.as_ref() {
            Some(agent) => crate::board::agent_title(todo_title, agent),
            None => todo_title.unwrap_or(&pane).to_owned(),
        }
    };
    let branch = todos.iter().find_map(|todo| todo.branch.as_deref());
    let harness = row.and_then(|row| nonempty(&row.what)).or_else(|| {
        current
            .as_ref()
            .and_then(|agent| agent.agent.as_deref())
            .and_then(nonempty)
    });
    let pending = pending_tab(
        &pane,
        review.as_deref(),
        question.as_deref(),
        reviews().filter(|_| host == "local").as_deref(),
        questions().filter(|_| host == "local").as_deref(),
        current.as_ref().is_some_and(is_blocked),
    );
    let identity = pending
        .as_ref()
        .map_or_else(String::new, RequestTab::identity);
    let tab = choice().filter(|(key, _)| key == &identity).map_or_else(
        || {
            if pending.is_some() {
                Tab::Pending
            } else {
                Tab::Conversation
            }
        },
        |(_, tab)| {
            if tab == Tab::Pending && pending.is_none() {
                Tab::Conversation
            } else {
                tab
            }
        },
    );
    let state = if loading() {
        "Loading…"
    } else {
        pending
            .as_ref()
            .map(RequestTab::state)
            .or_else(|| current.as_ref().map(|agent| agent.state.as_str()))
            .unwrap_or("Loading…")
    }
    .to_owned();
    let disabled = busy() || !reachable() || current.is_none();
    let plan_open = tab == Tab::Pending && matches!(pending, Some(RequestTab::Review(_)));
    let focus_pane = address.clone();
    let mut panel = use_context::<Signal<crate::app::LeadPanel>>();
    use_effect(use_reactive(&identity, move |identity| {
        if !identity.is_empty() {
            panel.set(crate::app::LeadPanel::Chat);
        }
    }));
    let width = use_context::<crate::app::ConsoleWidth>().0;
    let mut detail = use_context::<crate::app::LeadDetail>().0;
    let worker_view = row.is_some();
    let placeholder = worker_view.then(|| "Message the worker…".to_owned());
    let first_tab = usize::from(width() < 1100);
    let completed: usize = todos
        .iter()
        .flat_map(|todo| &todo.sections)
        .filter(|section| section.status == TodoStatusView::Completed)
        .map(|section| section.count)
        .sum();
    let role = if row.is_some() { "worker" } else { "lead" };
    let host = row.map(|row| row.host.as_str());
    let assistant = current
        .as_ref()
        .and_then(|agent| agent.name.as_deref())
        .and_then(nonempty)
        .unwrap_or("Assistant")
        .to_owned();
    let mut page = use_signal(|| None::<Rc<MountedData>>);
    let mut following = use_signal(|| true);
    let follow_error = use_signal(|| None::<String>);
    // Follow the newest turn until the owner scrolls up to read; scrolling back
    // to the end resumes it. The page region is the one that scrolls.
    use_follow_scroll(
        page,
        following,
        follow_error,
        retained.clone(),
        transcript_loading,
        tab == Tab::Conversation,
        live(),
    );
    let tracking = attributes!(div {
        onmounted: move |event| {
            page.set(Some(event.data()));
            following.set(true);
        },
        onscroll: move |event| {
            let remaining = f64::from(event.scroll_height())
                - event.scroll_top()
                - f64::from(event.client_height());
            following.set(remaining < 48.0);
        },
    });

    let review_identity = identity.clone();
    let chat = rsx! {
        div { class: "lead-chat",
                if tab == Tab::Pending {
                    match &pending {
                        Some(RequestTab::Questions(id, _)) => rsx! {
                            if let Some(question) = questions().and_then(|items| items.into_iter().find(|item| item.id == *id)) {
                                QuestionForm { question, on_done: move |id: String| {
                                    if let Some(pending) = questions.write().as_mut() { pending.retain(|item| item.id != id); }
                                    if selection.peek().as_ref().is_some_and(|item| item.kind == crate::app::InboxKind::Question && item.id == id) { selection.set(None); }
                                } }
                            } else {
                                Status { "This question is loading, answered, or no longer available." }
                            }
                        },
                        Some(RequestTab::Prompt) => rsx! {
                            BlockedPanel { prompt: current.as_ref().and_then(|agent| agent.blocked.clone()), screen: screen().filter(|value| detail_session.peek().as_ref().is_none_or(|session| session == &wire::DetailSession::Available(value.session.clone()))).map(|value| value.text),
                                disabled, on_key: send_key, draft, on_send: move |_| reply_send(true) }
                        },
                        _ => rsx! {},
                    }
                } else {
                    ScrollArea { class: "conversation-scroll", direction: ScrollDirection::Vertical, attributes: tracking,
                        Conversation { snapshot: retained.clone(), loading: transcript_loading, assistant,
                            live: live(), coarse: current.as_ref().map_or("Loading…", |agent| agent.state.as_str()).to_owned() }
                    }
                }
            if tab == Tab::Conversation {
                footer { class: "conversation-footer",
                    LiveActivity { live: live(), turns: retained.as_ref().map_or_else(Vec::new, |snapshot| snapshot.turns.clone()),
                        coarse: current.as_ref().map_or("Loading…", |agent| agent.state.as_str()).to_owned(),
                        connected: loading() || (reachable() && stream_error().is_none()),
                        waiting: pending.as_ref().map(|pending| pending.state().to_owned()) }
                    Composer { draft, images, uploading, disabled, busy: busy(), placeholder: placeholder.clone(), on_error: move |message| action_error.set(Some(message)),
                        on_send: { let mut send = conversation_send.clone(); move |_| send(false) } }
                }
            }
        }
    };

    rsx! {
        section { class: "lead-panel", "data-review-open": plan_open,
            if !plan_open {
            header { class: "lead-header",
                if worker_view && lead.is_some() {
                    if let Some(lead) = &lead {
                        crate::workers::WorkerHeading { title: title.clone(), lead: lead.clone(),
                            what: harness.unwrap_or("agent").to_owned(), host: address.host.clone() }
                    }
                } else {
                    div { class: "lead-heading", h1 { "{title}" }
                        p { class: "lead-meta", "{state} · {role}"
                            if let Some(harness) = harness { " · {harness}" }
                            if let Some(host) = host { " · {host}" }
                            if let Some(branch) = branch { " · {branch}" }
                        }
                    }
                }
                if !plan_open && width() < 1400 {
                    div { class: "lead-tabs",
                        TabList { aria_label: "Lead view",
                            if width() < 1100 { TabTrigger { value: "agents", index: 0usize, "Agents {agent_count}" } }
                            TabTrigger { value: "chat", index: first_tab, "Chat" }
                            TabTrigger { value: "todos", index: first_tab + 1, "Todos {completed}/{todo_count}" }
                            TabTrigger { value: "workers", index: first_tab + 2, "Workers {worker_count}" }
                        }
                    }
                }
                div { class: "lead-actions",
                    if tab == Tab::Pending {
                        Button { class: "icon-button", variant: ButtonVariant::Ghost, r#type: "button",
                            aria_label: if plan_open { "Close plan review" } else { "Show conversation" },
                            title: if plan_open { "Close plan review" } else { "Show conversation" },
                            onclick: move |_| choice.set(Some((identity.clone(), Tab::Conversation))), X { size: 16, attributes: attributes!(svg { "aria-hidden": "true" }) }
                        }
                    }
                    if !plan_open {
                        Button { class: "icon-button", variant: ButtonVariant::Outline, r#type: "button", disabled,
                            aria_label: "Interrupt", title: "Interrupt", onclick: move |_| interrupt("ctrl+c".to_owned()),
                            Square { size: 16, attributes: attributes!(svg { "aria-hidden": "true" }) }
                        }
                    }
                    Button { class: "icon-button", variant: ButtonVariant::Outline, r#type: "button", disabled,
                        aria_label: "Open in Herdr", title: "Open in Herdr",
                        onclick: move |_| {
                            let pane = focus_pane.clone();
                            async move {
                                if busy() || !reachable() { return; }
                                busy.set(true);
                                action_error.set(api::focus_agent(&pane).await.err());
                                busy.set(false);
                            }
                        }, ExternalLink { size: 16, attributes: attributes!(svg { "aria-hidden": "true" }) }
                    }
                }
            }
            }
            div { class: "lead-notices",
                if matches!(&*metadata.read(), AgentMetadata::Unreachable { .. }) {
                    Status { "Herdr is not reachable. The board still works; conversation and messages do not." }
                }
                if let Some(message) = metadata_error() { Alert { "{message}" } }
                if let Some(message) = screen_error() { Alert { "{message}" } }
                if let Some(message) = transcript_error { Alert { "{message}" } }
                if let Some(message) = pending_error() { Alert { "{message}" } }
                if let Some(message) = (portal.board_error)() { Alert { "{message}" } }
                if stream_error().is_some() { Status { "Live updates paused. Reconnecting." } }
                if let Some(message) = action_error() { Alert { "{message}" } }
                if let Some(message) = follow_error() { Alert { "Could not follow the conversation: {message}" } }
            }
            if plan_open {
                div { class: "pending-review",
                    if let Some(RequestTab::Review(id)) = &pending {
                        ReviewView { review_id: id.clone(),
                            revision: inbox.iter().find(|item| item.selection.host == address.host && item.selection.pane == address.pane && item.selection.kind == crate::app::InboxKind::Review && item.selection.id == *id).map_or(0, |item| item.revision),
                            embedded: true,
                            lead: title.clone(), role: role.to_owned(),
                            repository: current.as_ref().and_then(|agent| agent.repository.clone()).unwrap_or_else(|| "Unknown repository".to_owned()),
                            on_close: move |_| choice.set(Some((review_identity.clone(), Tab::Conversation))),
                        }
                    }
                }
            } else {
                if width() >= 1400 {
                    {chat.clone()}
                    aside { class: "console-todos",
                        Tabs { class: "lead-detail", horizontal: true, value: Some(detail().value().to_owned()),
                            on_value_change: move |value: String| detail.set(if value == "workers" { crate::app::LeadPanel::Workers } else { crate::app::LeadPanel::Todos }),
                            header { class: "console-column-header detail-header",
                                div { class: "lead-tabs detail-tabs", TabList { aria_label: "Lead detail",
                                    TabTrigger { value: "todos", index: 0usize, "Todos {completed}/{todo_count}" }
                                    TabTrigger { value: "workers", index: 1usize, "Workers {worker_count}" }
                                } }
                                if detail() == crate::app::LeadPanel::Workers { span { "{working_count} working" } }
                            }
                            TabContent { class: "detail-content", value: "todos", index: 0usize, Todos { todos: todos.clone(), workers: workers.clone() } }
                            TabContent { class: "detail-content", value: "workers", index: 1usize,
                                crate::workers::Workers { rows: worker_rows.clone(), inbox: inbox.clone(), header: false }
                            }
                        }
                    }
                } else {
                    TabContent { class: "chat-tab", value: "chat", index: first_tab, {chat.clone()} }
                    TabContent { class: "todos-tab", value: "todos", index: first_tab + 1,
                        div { class: "todo-tab-body", Todos { todos: todos.clone(), workers: workers.clone() } }
                        Composer { draft, images, uploading, disabled, busy: busy(), placeholder: placeholder.clone(), on_error: move |message| action_error.set(Some(message)),
                            on_send: move |_| conversation_send(false) }
                    }
                    TabContent { class: "workers-tab", value: "workers", index: first_tab + 2,
                        crate::workers::Workers { rows: worker_rows.clone(), inbox: inbox.clone() }
                    }
                }
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ChangedParts {
    conversation: bool,
    screen: bool,
}

fn event_is_blocked(event: &wire::AgentEvent, was_blocked: bool) -> bool {
    event.blocked.is_some()
        || event
            .state
            .as_deref()
            .map_or(was_blocked, |state| state == "blocked")
}

/// Older peers lack revisions; their events must invalidate both parts.
fn changed_parts(
    held: Option<&wire::DetailRevisions>,
    event: &wire::AgentEvent,
    was_blocked: bool,
) -> ChangedParts {
    let Some(revisions) = &event.revisions else {
        return ChangedParts {
            conversation: true,
            screen: true,
        };
    };
    let blocked = event_is_blocked(event, was_blocked);
    ChangedParts {
        conversation: held.is_none_or(|held| held.conversation != revisions.conversation),
        screen: blocked
            && (held.is_none_or(|held| held.screen != revisions.screen) || !was_blocked),
    }
}

fn agent_inbox_ids(
    inbox: &[crate::app::InboxItem],
    address: &wire::AgentAddress,
) -> std::collections::BTreeSet<(String, u64)> {
    inbox
        .iter()
        .filter(|item| item.selection.host == address.host && item.selection.pane == address.pane)
        .map(|item| (item.selection.id.clone(), item.revision))
        .collect()
}

struct Backoff(Duration);

impl Default for Backoff {
    fn default() -> Self {
        Self(Duration::from_secs(1))
    }
}

impl Backoff {
    fn next_delay(&mut self) -> Duration {
        let delay = self.0;
        self.0 = (delay * 2).min(Duration::from_secs(30));
        delay
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

fn nonempty(value: &str) -> Option<&str> {
    (!value.trim().is_empty()).then_some(value)
}

async fn pending_list<T: DeserializeOwned>(kind: &str) -> Result<T, String> {
    let response = Request::get(&format!("{BASE}/api/{kind}"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!(
            "Could not load {kind} ({}). Retrying shortly.",
            response.status()
        ));
    }
    response.json().await.map_err(|error| error.to_string())
}

fn pending_tab(
    pane: &str,
    review: Option<&str>,
    question: Option<&str>,
    reviews: Option<&[ReviewSummary]>,
    questions: Option<&[Pending]>,
    blocked: bool,
) -> Option<RequestTab> {
    let review_tab = |id: &str| RequestTab::Review(id.to_owned());
    let question_tab = |id: &str| {
        RequestTab::Questions(
            id.to_owned(),
            questions
                .and_then(|items| items.iter().find(|item| item.id == id))
                .map_or(0, |item| item.questions.questions.len()),
        )
    };
    review
        .filter(|id| {
            reviews.is_none_or(|items| {
                items
                    .iter()
                    .any(|item| item.id == *id && item.decision.is_none())
            })
        })
        .map(review_tab)
        .or_else(|| {
            question
                .filter(|id| questions.is_none_or(|items| items.iter().any(|item| item.id == *id)))
                .map(question_tab)
        })
        .or_else(|| {
            reviews
                .and_then(|items| {
                    items
                        .iter()
                        .find(|item| item.pane == pane && item.decision.is_none())
                })
                .map(|item| review_tab(&item.id))
        })
        .or_else(|| {
            questions
                .and_then(|items| items.iter().find(|item| item.pane == pane))
                .map(|item| question_tab(&item.id))
        })
        .or(blocked.then_some(RequestTab::Prompt))
}

fn is_blocked(agent: &wire::AgentView) -> bool {
    agent.state == "blocked" || agent.blocked.is_some()
}

/// Reuse the same follow policy for transcript and live-edge changes.
pub(crate) fn use_follow_scroll(
    page: Signal<Option<Rc<MountedData>>>,
    following: Signal<bool>,
    mut follow_error: Signal<Option<String>>,
    snapshot: Option<wire::Conversation>,
    loading: bool,
    visible: bool,
    live: Live,
) {
    use_effect(use_reactive(
        (&snapshot, &loading, &visible, &live),
        move |_| {
            if !visible || !following() {
                return;
            }
            let Some(page) = page() else {
                return;
            };
            spawn(async move {
                let scrolled = async {
                    let size = page.get_scroll_size().await?;
                    page.scroll(
                        PixelsVector2D::new(0.0, size.height),
                        ScrollBehavior::Instant,
                    )
                    .await
                }
                .await;
                follow_error.set(scrolled.err().map(|error| error.to_string()));
            });
        },
    ));
}

#[component]
pub(crate) fn Conversation(
    snapshot: Option<wire::Conversation>,
    loading: bool,
    assistant: String,
    #[props(default = Live::Pending)] live: Live,
    #[props(default = "idle".to_owned())] coarse: String,
) -> Element {
    let turns = snapshot
        .as_ref()
        .map_or(&[][..], |snapshot| snapshot.turns.as_slice());
    let active = match &live {
        Live::Detail(state) => state.phase != wire::LivePhase::Idle,
        _ => coarse != "idle",
    };
    let groups = turns
        .chunk_by(|_, next| next.role != Role::User)
        .collect::<Vec<_>>();
    let records = groups.iter().enumerate().flat_map(|(index, group)| {
        let final_reply = group.iter().rposition(|turn| turn.role == Role::Assistant);
        let finished = !(active && index + 1 == groups.len());
        let mut records = Vec::new();
        if let Some(final_reply) = final_reply.filter(|&reply| {
            finished && group[0].role == Role::User && group[1..reply].iter().any(is_work)
        }) {
            records.push(conversation_record(&group[0], &assistant));
            let work = &group[1..final_reply];
            let label = worked_label(&group[0], group);
            records.push(rsx! {
                ToolRecords { key: "work:{group[0].id}", turns: work.to_vec(), assistant: assistant.clone(), label }
            });
            records.extend(group[final_reply..].iter().map(|turn| conversation_record(turn, &assistant)));
        } else {
            records.extend(group.iter().map(|turn| conversation_record(turn, &assistant)));
        }
        records
    }).collect::<Vec<_>>();
    rsx! {
        div { class: "conversation-turns",
            div { class: "conversation-column",
                if loading { Skeleton { role: "status", aria_label: "Loading conversation" } }
                if let Some(snapshot) = &snapshot {
                    if snapshot.truncated {
                        p { class: "conversation-history-notice", "Earlier history was not returned." }
                    }
                    if snapshot.turns.is_empty() && !snapshot.truncated {
                        Status { "No conversation yet. Send a message to start." }
                    }
                }
                {records.into_iter()}
                LiveEdge { live, last: turns.last().cloned() }
            }
        }
    }
}

fn is_work(turn: &Turn) -> bool {
    matches!(turn.role, Role::Thinking | Role::Tool)
}

fn conversation_record(turn: &Turn, assistant: &str) -> Element {
    if turn.role == Role::Tool {
        rsx! { ToolRecord { key: "{turn.id}", turn: turn.clone() } }
    } else {
        rsx! { MessageRecord { key: "{turn.id}", turn: turn.clone(), assistant: assistant.to_owned() } }
    }
}

fn work_counts(turns: &[Turn]) -> (usize, usize) {
    (
        turns
            .iter()
            .filter(|turn| turn.role == Role::Thinking)
            .count(),
        turns.iter().filter(|turn| turn.role == Role::Tool).count(),
    )
}

fn count_label(thoughts: usize, calls: usize) -> String {
    format!(
        "{thoughts} {} · {calls} {}",
        if thoughts == 1 { "thought" } else { "thoughts" },
        if calls == 1 {
            "tool call"
        } else {
            "tool calls"
        }
    )
}

fn elapsed_label(seconds: u64) -> String {
    if seconds >= 60 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

fn latest_record_time(turns: &[Turn]) -> Option<u64> {
    turns
        .iter()
        .filter(|turn| turn.role != Role::User)
        .filter_map(|turn| turn.timestamp.as_ref().map(|time| time.ms))
        .max()
}

fn worked_label(owner: &Turn, turns: &[Turn]) -> String {
    let span = owner
        .timestamp
        .as_ref()
        .zip(latest_record_time(turns))
        .and_then(|(start, end)| end.checked_sub(start.ms))
        .map(|ms| format!(" for {}", elapsed_label(ms / 1000)))
        .unwrap_or_default();
    let (thoughts, calls) = work_counts(turns);
    format!("Worked{span} · {}", count_label(thoughts, calls))
}

/// Attribution comes from typed history, never from the body or today's model settings.
fn source_label(turn: &Turn, assistant: &str) -> (String, String, bool) {
    match &turn.source {
        wire::Source::Dogsled {
            sender,
            delivery_kind,
        } => (sender.clone(), format!("Dogsled · {delivery_kind}"), false),
        wire::Source::Custom { custom_type } => {
            (custom_type.clone(), "Extension context".into(), false)
        }
        wire::Source::Harness {
            harness,
            model,
            provider,
        } => {
            let author = match turn.role {
                Role::User => "You",
                Role::Assistant => assistant,
                Role::Thinking => "Thinking",
                Role::Context => "Context",
                Role::Tool => "Tool",
            };
            let mut metadata = vec![harness.as_str()];
            if turn.role == Role::Assistant {
                metadata.extend(model.as_deref());
                metadata.extend(provider.as_deref());
            }
            (author.into(), metadata.join(" · "), turn.role == Role::User)
        }
    }
}

#[component]
fn MessageRecord(turn: Turn, assistant: String) -> Element {
    let (author, source, owner) = source_label(&turn, &assistant);
    // Polls, activity, and attribution changes must not reparse unchanged Markdown.
    let markdown = use_memo(use_reactive(&turn.text, |text| {
        crate::review_view::render_markdown(&text)
    }));
    rsx! {
        article { class: if owner { "conversation-event conversation-owner" } else if turn.role == Role::Thinking { "conversation-event conversation-thought" } else { "conversation-event" },
            "data-record-id": turn.id, aria_label: author.clone(),
            ConversationTime { timestamp: turn.timestamp, role: turn.role, source: turn.source }
            div { class: "conversation-axis", aria_hidden: "true", i {} }
            div { class: "conversation-body",
                div { class: "conversation-meta", strong { "{author}" } span { "{source}" } }
                div { class: "markdown-body", dangerous_inner_html: markdown() }
            }
        }
    }
}

#[component]
fn ToolRecords(turns: Vec<Turn>, assistant: String, label: String) -> Element {
    rsx! {
        div { class: "conversation-work",
            Collapsible { keep_mounted: true,
                CollapsibleTrigger { "{label}" }
                CollapsibleContent { "data-part": "turn-work",
                    for turn in &turns { {conversation_record(turn, &assistant)} }
                }
            }
        }
    }
}

#[component]
fn ToolRecord(turn: Turn) -> Element {
    let tool = turn.tool.as_ref();
    let call = tool
        .and_then(|tool| tool.call.as_deref().or(tool.name.as_deref()))
        .unwrap_or("Unnamed tool");
    let output = tool.and_then(|tool| tool.output.as_deref());
    let shown = output.map_or(0, |output| output.lines().count() as u32);
    let earlier = tool
        .and_then(|tool| tool.output_lines)
        .unwrap_or(shown)
        .saturating_sub(shown);
    let error = tool.is_some_and(|tool| tool.status == wire::ToolStatus::Error);
    rsx! {
        article { class: "conversation-event conversation-tool", "data-record-id": turn.id,
            "data-error": error,
            ConversationTime { timestamp: turn.timestamp, role: turn.role, source: turn.source }
            div { class: "conversation-axis", aria_hidden: "true", i {} }
            div { class: "conversation-body",
                div { class: "conversation-tool-panel",
                    pre { class: "conversation-call", "{call}" }
                    p { class: "conversation-tool-outcome", "{tool_outcome(tool.map(|tool| tool.status))}" }
                    if earlier > 0 { p { class: "conversation-earlier", "{earlier} earlier lines" } }
                    if let Some(output) = output { pre { class: "conversation-output", "{output}" } }
                }
            }
        }
    }
}

fn tool_outcome(status: Option<wire::ToolStatus>) -> &'static str {
    match status {
        Some(wire::ToolStatus::Completed) => "Result recorded",
        Some(wire::ToolStatus::Error) => "Error recorded",
        Some(wire::ToolStatus::Unknown) | None => "No recorded outcome",
    }
}

fn time_meaning(role: Role, source: &wire::Source, meaning: wire::TimeMeaning) -> &'static str {
    match meaning {
        wire::TimeMeaning::Recorded => "Transcript entry recorded",
        wire::TimeMeaning::Created => "Dogsled delivery created",
        wire::TimeMeaning::Message
            if role == Role::Assistant
                && matches!(source, wire::Source::Harness { harness, .. } if harness.eq_ignore_ascii_case("pi")) =>
        {
            "Request started (not response completion)"
        }
        wire::TimeMeaning::Message => "Harness message time",
    }
}

#[component]
fn ConversationTime(
    timestamp: Option<wire::Timestamp>,
    role: Role,
    source: wire::Source,
) -> Element {
    let Some(timestamp) = timestamp else {
        return rsx! { span { class: "conversation-time", aria_label: "Time unavailable", "—" } };
    };
    let meaning = time_meaning(role, &source, timestamp.meaning);
    let formatted = platform_time(timestamp.ms);
    let label = formatted.as_ref().map_or_else(
        || format!("{meaning}; date and time formatting unavailable"),
        |(_, full, _)| format!("{meaning}: {full}"),
    );
    rsx! {
        time { class: "conversation-time", datetime: formatted.as_ref().map(|(_, _, iso)| iso.clone()),
            "data-timestamp-ms": "{timestamp.ms}", title: label.clone(), aria_label: label,
            "{formatted.as_ref().map_or(\"—\", |(short, _, _)| short.as_str())}"
        }
    }
}

/// This boundary deliberately has no native test calendar or synthetic dates.
#[cfg(not(target_arch = "wasm32"))]
fn platform_time(_ms: u64) -> Option<(String, String, String)> {
    None
}

#[cfg(target_arch = "wasm32")]
fn platform_time(ms: u64) -> Option<(String, String, String)> {
    use js_sys::{Array, Date, Intl::DateTimeFormat, Object, Reflect, wasm_bindgen::JsValue};
    let date = Date::new(&JsValue::from_f64(ms as f64));
    if !date.get_time().is_finite() {
        return None;
    }
    let format = |settings: &[(&str, JsValue)]| {
        let options = Object::new();
        for (key, value) in settings {
            Reflect::set(&options, &JsValue::from_str(key), value).ok()?;
        }
        DateTimeFormat::new(&Array::new(), &options)
            .format()
            .call1(&JsValue::UNDEFINED, &date)
            .ok()?
            .as_string()
    };
    Some((
        format(&[
            ("hour", "2-digit".into()),
            ("minute", "2-digit".into()),
            ("hour12", false.into()),
        ])?,
        format(&[("dateStyle", "full".into()), ("timeStyle", "long".into())])?,
        date.to_iso_string().into(),
    ))
}

/// The live edge shares the history's scrolling region, not the pinned footer.
#[component]
pub(crate) fn LiveEdge(live: Live, #[props(default)] last: Option<Turn>) -> Element {
    let Live::Detail(state) = live else {
        return rsx! {};
    };
    let reasoning = state.reasoning.filter(|text| !text.is_empty());
    rsx! {
        div { class: "conversation-live-edge", aria_live: "off",
            if let Some(text) = reasoning.filter(|text| !last.as_ref().is_some_and(|turn| turn.role == Role::Thinking && turn.text == *text)) {
                LiveThought { text }
            }
            for tool in state.active_tools {
                div { key: "{tool.call_id}", class: "conversation-live-tool",
                    p { class: "conversation-meta", "Running · {tool.name}" }
                    pre { class: "conversation-call", "{tool.call.as_deref().unwrap_or(&tool.name)}" }
                }
            }
        }
    }
}

#[component]
fn LiveThought(text: String) -> Element {
    let markdown = use_memo(use_reactive(&text, |text| {
        crate::review_view::render_markdown(&text)
    }));
    rsx! {
        article { class: "conversation-thought conversation-live-thought", aria_label: "Live thought",
            div { class: "markdown-body", dangerous_inner_html: markdown() }
        }
    }
}

/// A timer updates only the footer; unchanged history Markdown stays memoized.
#[component]
fn LiveActivity(
    live: Live,
    turns: Vec<Turn>,
    coarse: String,
    connected: bool,
    waiting: Option<String>,
) -> Element {
    let mut now = use_signal(now_ms);
    use_interval(Duration::from_secs(1), move |()| now.set(now_ms()));
    let turn = current_turn(&turns);
    let end = if matches!(&live, Live::Detail(state) if state.phase == wire::LivePhase::Idle) {
        latest_record_time(turn)
    } else {
        Some(now())
    };
    let elapsed_seconds = turn
        .first()
        .filter(|turn| turn.role == Role::User)
        .and_then(|turn| turn.timestamp.as_ref())
        .zip(end)
        .and_then(|(start, end)| end.checked_sub(start.ms))
        .map(|ms| ms / 1000);
    rsx! { ActivityRow { live, coarse, connected, waiting, turns, elapsed_seconds } }
}

fn current_turn(turns: &[Turn]) -> &[Turn] {
    &turns[turns
        .iter()
        .rposition(|turn| turn.role == Role::User)
        .unwrap_or(0)..]
}

#[cfg(target_arch = "wasm32")]
fn now_ms() -> u64 {
    js_sys::Date::now() as u64
}

#[cfg(not(target_arch = "wasm32"))]
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[component]
pub(crate) fn ActivityRow(
    live: Live,
    coarse: String,
    connected: bool,
    waiting: Option<String>,
    #[props(default)] turns: Vec<Turn>,
    #[props(default)] elapsed_seconds: Option<u64>,
) -> Element {
    let (phase, detail) = activity_label(&live, &coarse, connected, waiting.as_deref());
    let turns = current_turn(&turns);
    let (mut thoughts, mut calls) = work_counts(turns);
    if detail == "Live"
        && let Live::Detail(state) = &live
    {
        if state.reasoning.as_ref().is_some_and(|text| {
            !text.is_empty()
                && !turns
                    .last()
                    .is_some_and(|turn| turn.role == Role::Thinking && turn.text == *text)
        }) {
            thoughts += 1;
        }
        calls += state
            .active_tools
            .iter()
            .filter(|active| {
                !turns.iter().any(|turn| {
                    turn.tool
                        .as_ref()
                        .is_some_and(|tool| tool.call_id == active.call_id)
                })
            })
            .count();
    }
    rsx! {
        div { class: "conversation-activity", "data-state": if detail == "Needs you" { "blocked" } else if phase == "Disconnected" { "disconnected" } else { coarse.as_str() },
            p { role: "status", aria_live: "polite", aria_atomic: "true", "{phase}" }
            if detail == "Live" {
                if let Some(seconds) = elapsed_seconds { p { class: "activity-elapsed", "{elapsed_label(seconds)}" } }
                p { class: "activity-counts", "{count_label(thoughts, calls)}" }
            }
            p { class: "activity-detail", "{detail}" }
        }
    }
}

fn activity_label(
    live: &Live,
    coarse: &str,
    connected: bool,
    waiting: Option<&str>,
) -> (String, &'static str) {
    if let Some(waiting) = waiting {
        return (waiting.into(), "Needs you");
    }
    if !connected || coarse == "disconnected" {
        return ("Disconnected".into(), "Agent state unavailable");
    }
    if coarse == "blocked" {
        return ("Blocked".into(), "Needs you");
    }
    if let Live::Detail(state) = live {
        let phase = match state.phase {
            wire::LivePhase::Idle => "Idle".into(),
            wire::LivePhase::Working => "Working…".into(),
            wire::LivePhase::Thinking => "Thinking…".into(),
            wire::LivePhase::Responding => "Responding…".into(),
            wire::LivePhase::ToolRunning => {
                let names = state
                    .active_tools
                    .iter()
                    .map(|tool| tool.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                if names.is_empty() {
                    "Running tools…".into()
                } else {
                    format!("Running tools: {names}")
                }
            }
        };
        return (phase, "Live");
    }
    let phase = match coarse {
        "working" => "Working…",
        "idle" => "Idle",
        state => state,
    }
    .to_owned();
    let detail = match live {
        Live::Pending => "Activity pending",
        Live::Unavailable(Gap::Disconnected) => "Activity disconnected",
        Live::Unavailable(Gap::SessionMismatch) => "Activity session mismatch",
        Live::Unavailable(Gap::Invalid) => "Activity unavailable",
        Live::Unavailable(Gap::Server(wire::UnavailableReason::UnsupportedHarness)) => {
            "Detailed activity unsupported"
        }
        Live::Unavailable(Gap::Server(_)) => "Detailed activity unavailable",
        Live::Detail(_) => "Live",
    };
    (phase, detail)
}

pub(crate) fn message_with_images(text: &str, images: &[String]) -> String {
    if images.is_empty() {
        return text.to_owned();
    }
    format!(
        "{text}\n\nReference images:\n{}",
        images
            .iter()
            .map(|path| format!("- {path}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

#[component]
pub(crate) fn Composer(
    draft: Signal<String>,
    images: Signal<Vec<String>>,
    uploading: Signal<bool>,
    disabled: bool,
    busy: bool,
    on_error: EventHandler<String>,
    on_send: EventHandler<()>,
    #[props(default)] placeholder: Option<String>,
) -> Element {
    let mut round = use_signal(|| 0_u32);
    use_drop(move || uploading.set(false));
    let upload = use_callback(move |files: Vec<web_sys::File>| {
        if disabled || busy || uploading() || files.is_empty() {
            return;
        }
        uploading.set(true);
        spawn(async move {
            for file in files {
                match api::upload_image(&file).await {
                    Ok(path) => images.write().push(path),
                    Err(message) => {
                        on_error.call(message);
                        break;
                    }
                }
            }
            round += 1;
            uploading.set(false);
        });
    });
    rsx! {
        form { class: "console-composer", aria_label: "Message composer", aria_busy: uploading() || busy,
            onsubmit: move |event| { event.prevent_default(); if !disabled && !busy && !uploading() { on_send.call(()); } },
            if !images().is_empty() {
                div { class: "composer-images",
                    for (index, path) in images().iter().enumerate() {
                        {
                            let filename = path.rsplit('/').next().unwrap_or(path).to_owned();
                            rsx! {
                                div { class: "composer-image", key: "{index}:{path}",
                                    img { src: "/api/attachments/{filename}", alt: "Attached image {index + 1}" }
                                    Button { class: "icon-button", variant: ButtonVariant::Secondary, r#type: "button", disabled: disabled || busy || uploading(),
                                        aria_label: "Remove image {index + 1}", title: "Remove image {index + 1}",
                                        onclick: move |_| { images.write().remove(index); }, X { size: 16, attributes: attributes!(svg { "aria-hidden": "true" }) }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "composer-field",
                Button { class: "icon-button attach-button", variant: ButtonVariant::Ghost, r#type: "button",
                    aria_label: "Attach images", title: "Attach images", disabled: disabled || busy || uploading(),
                    onclick: move |_| { let _ = document::eval("document.getElementById('composer-images').click()"); },
                    Paperclip { size: 16, attributes: attributes!(svg { "aria-hidden": "true" }) }
                }
                Input { key: "{round}", id: "composer-images", name: "images", r#type: "file", hidden: true,
                    accept: "image/png,image/jpeg,image/webp", multiple: true, disabled: disabled || busy || uploading(),
                    onchange: move |event: FormEvent| upload.call(event.files().iter().filter_map(|file| file.inner().downcast_ref::<web_sys::File>().cloned()).collect()),
                }
                Textarea { name: "message", aria_label: "Message this agent", placeholder, rows: "1", value: "{draft}", disabled: disabled || busy,
                    oninput: move |event: FormEvent| draft.set(event.value()),
                    onpaste: move |event: ClipboardEvent| {
                        let files = event.data().try_as_web_event()
                            .and_then(|event| event.dyn_into::<web_sys::ClipboardEvent>().ok())
                            .and_then(|event| event.clipboard_data()).and_then(|data| data.files());
                        let files: Vec<_> = files.map_or_else(Vec::new, |files| (0..files.length()).filter_map(|index| files.item(index)).collect());
                        if !files.is_empty() { event.prevent_default(); upload.call(files); }
                    },
                    onkeydown: move |event: KeyboardEvent| {
                        if event.key() == Key::Enter && !event.modifiers().shift() && !event.is_composing() {
                            event.prevent_default();
                            if !disabled && !busy && !uploading() { on_send.call(()); }
                        }
                    },
                }
                Button { class: "icon-button send-button", r#type: "submit", aria_label: "Send message", title: "Send message",
                    disabled: disabled || busy || uploading() || (draft().trim().is_empty() && images().is_empty()),
                    ArrowUp { size: 16, attributes: attributes!(svg { "aria-hidden": "true" }) }
                }
            }
        }
    }
}

#[component]
fn BlockedPanel(
    prompt: Option<BlockedPrompt>,
    screen: Option<String>,
    disabled: bool,
    on_key: EventHandler<String>,
    draft: Signal<String>,
    on_send: EventHandler<()>,
) -> Element {
    let title = prompt
        .as_ref()
        .map_or("Agent needs your response", |prompt| {
            prompt.question.as_str()
        });
    // The prompt's own options first, then the keys every prompt accepts.
    let keys: Vec<(String, String, Option<&'static str>, bool)> = prompt
        .iter()
        .flat_map(|prompt| &prompt.options)
        .map(|option| {
            (
                option.key.clone(),
                format!("{} · {}", option.key, option.label),
                None,
                allowed_key(&option.key),
            )
        })
        .chain(PANEL_KEYS.iter().map(|&(key, label)| {
            (
                key.to_owned(),
                label.to_owned(),
                match key {
                    "up" => Some("Arrow up"),
                    "down" => Some("Arrow down"),
                    _ => None,
                },
                true,
            )
        }))
        .collect();
    rsx! {
        section { class: "prompt-panel", aria_label: "Blocked agent",
            div { class: "prompt-body console-scroll",
                div { class: "prompt-measure",
                h2 { "{title}" }
                match screen {
                    Some(text) => rsx! {
                        pre { tabindex: "0", aria_label: "Agent screen",
                            onkeydown: move |event: KeyboardEvent| {
                                if disabled || event.is_composing() { return; }
                                let key = match event.key() { Key::Escape => "esc", Key::Enter => "enter", Key::ArrowUp => "up", Key::ArrowDown => "down", _ => return };
                                event.prevent_default(); on_key.call(key.to_owned());
                            },
                            "{text}"
                        }
                    },
                    None => rsx! { Status { "Agent screen is not available yet." } },
                }
                div { class: "prompt-keys", role: "group", aria_label: "Prompt options and terminal keys",
                    for (index, (key, text, label, allowed)) in keys.into_iter().enumerate() {
                        Button { key: "{index}-{key}", variant: ButtonVariant::Outline, r#type: "button",
                            disabled: disabled || !allowed, aria_label: label,
                            onclick: move |_| on_key.call(key.clone()), "{text}"
                        }
                    }
                }
                }
            }
            form { class: "console-composer prompt-composer",
                onsubmit: move |event| { event.prevent_default(); on_send.call(()); },
                div { class: "composer-field",
                    Input { r#type: "text", name: "reply", aria_label: "Reply to the prompt",
                        value: "{draft}", disabled, oninput: move |event: FormEvent| draft.set(event.value()) }
                    Button { class: "icon-button send-button", r#type: "submit", aria_label: "Send reply",
                        disabled: disabled || draft().trim().is_empty(),
                        ArrowUp { size: 16, attributes: attributes!(svg { "aria-hidden": "true" }) }
                    }
                }
            }
        }
    }
}

#[component]
pub(crate) fn Todos(todos: Vec<TodoView>, workers: Vec<(String, String)>) -> Element {
    let total: usize = todos
        .iter()
        .flat_map(|todo| &todo.sections)
        .map(|section| section.count)
        .sum();
    let completed: usize = todos
        .iter()
        .flat_map(|todo| &todo.sections)
        .filter(|section| section.status == TodoStatusView::Completed)
        .map(|section| section.count)
        .sum();
    rsx! {
        section { class: "todo-panel", aria_label: "Agent todos",
            header { class: "console-column-header", h2 { "Todos" } span { "{completed} of {total}" } }
            ScrollArea { class: "console-scroll todo-list", direction: ScrollDirection::Vertical,
                if total == 0 { Status { "data-part": "column-empty", "No todos are recorded for this agent." } }
                for todo in todos {
                    {
                        let mut sections = todo.sections;
                        sections.sort_by_key(|section| match section.status {
                            TodoStatusView::InProgress => 0, TodoStatusView::Pending => 1,
                            TodoStatusView::Completed => 2, TodoStatusView::Abandoned => 3,
                        });
                        rsx! {
                            Accordion { key: "{todo.session}", allow_multiple_open: true, horizontal: false,
                                for (position, section) in sections.into_iter().filter(|section| !section.items.is_empty()).enumerate() {
                                    {
                                        let mut items = section.items;
                                        if section.status == TodoStatusView::Completed { items.reverse(); }
                                        rsx! {
                                            AccordionItem { key: "{position}", index: position, default_open: true,
                                                AccordionTrigger { "{status_label(section.status)} · {section.count}" }
                                                AccordionContent {
                                                    ItemGroup {
                                                        for item in items {
                                                            Item { class: "todo-item", "data-status": status_class(item.status), size: ItemSize::Sm,
                                                                ItemMedia { span { class: "todo-indicator", aria_label: status_label(item.status) } }
                                                                ItemContent {
                                                                    ItemTitle { class: "todo-title", "{item.text}" }
                                                                    if matches!(item.status, TodoStatusView::InProgress | TodoStatusView::Pending) {
                                                                        if let Some(worker) = item.unit.as_ref().and_then(|unit| workers.iter().find(|(id, _)| id == unit)) {
                                                                            ItemDescription { "{worker.1}" }
                                                                        }
                                                                        if let Some(description) = item.description { ItemDescription { "{description}" } }
                                                                    }
                                                                    ItemGroup {
                                                                        for child in item.children {
                                                                            Item { class: "todo-item", "data-status": status_class(child.status), size: ItemSize::Sm,
                                                                                ItemMedia { span { class: "todo-indicator", aria_label: status_label(child.status) } }
                                                                                ItemContent {
                                                                                    ItemTitle { class: "todo-title", "{child.text}" }
                                                                                    if matches!(child.status, TodoStatusView::InProgress | TodoStatusView::Pending) {
                                                                                        if let Some(worker) = child.unit.as_ref().and_then(|unit| workers.iter().find(|(id, _)| id == unit)) {
                                                                                            ItemDescription { "{worker.1}" }
                                                                                        }
                                                                                        if let Some(description) = child.description { ItemDescription { "{description}" } }
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
                                }
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
        TodoStatusView::InProgress => "in-progress",
        TodoStatusView::Pending => "pending",
        TodoStatusView::Completed => "completed",
        TodoStatusView::Abandoned => "abandoned",
    }
}

fn status_label(status: TodoStatusView) -> &'static str {
    match status {
        TodoStatusView::InProgress => "In progress",
        TodoStatusView::Pending => "Pending",
        TodoStatusView::Completed => "Completed",
        TodoStatusView::Abandoned => "Abandoned",
    }
}

pub(crate) fn allowed_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "enter"
            | "esc"
            | "up"
            | "down"
            | "left"
            | "right"
            | "ctrl+c"
            | "tab"
            | "space"
            | "backspace"
            | "delete"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
            | "y"
            | "n"
            | "0"
            | "1"
            | "2"
            | "3"
            | "4"
            | "5"
            | "6"
            | "7"
            | "8"
            | "9"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata_agent() -> wire::AgentView {
        wire::AgentView {
            host: "local".into(),
            pane: "pane".into(),
            name: None,
            title: None,
            agent: None,
            state: "working".into(),
            cwd: None,
            repository: None,
            blocked: None,
        }
    }

    #[test]
    fn metadata_is_loading_before_the_first_agent_list() {
        assert_eq!(
            agent_metadata(None, None, &wire::AgentAddress::local("pane")),
            AgentMetadata::Loading
        );
    }

    #[test]
    fn metadata_is_missing_only_after_a_list_arrives() {
        let agents = [metadata_agent()];
        for (host, pane) in [("other-host", "pane"), ("local", "other-pane")] {
            let address = wire::AgentAddress {
                host: host.into(),
                pane: pane.into(),
            };
            assert_eq!(
                agent_metadata(Some(&agents), None, &address),
                AgentMetadata::Missing
            );
        }
        assert_eq!(
            agent_metadata(Some(&[]), None, &wire::AgentAddress::local("pane")),
            AgentMetadata::Missing
        );
    }

    #[test]
    fn metadata_is_unreachable_with_the_last_known_agent() {
        let agent = metadata_agent();
        assert_eq!(
            agent_metadata(
                Some(std::slice::from_ref(&agent)),
                Some("stream disconnected"),
                &wire::AgentAddress::local("pane"),
            ),
            AgentMetadata::Unreachable {
                message: "stream disconnected".into(),
                agent: Some(agent),
            }
        );
    }

    #[test]
    fn metadata_is_unreachable_without_an_earlier_entry() {
        for agents in [None, Some([].as_slice())] {
            assert_eq!(
                agent_metadata(
                    agents,
                    Some("stream disconnected"),
                    &wire::AgentAddress::local("pane"),
                ),
                AgentMetadata::Unreachable {
                    message: "stream disconnected".into(),
                    agent: None,
                }
            );
        }
    }

    #[test]
    fn metadata_is_present_when_the_address_matches() {
        let agent = metadata_agent();
        assert_eq!(
            agent_metadata(
                Some(std::slice::from_ref(&agent)),
                None,
                &wire::AgentAddress::local("pane"),
            ),
            AgentMetadata::Present(agent)
        );
    }

    fn event(conversation: u64, screen: u64, state: &str) -> wire::AgentEvent {
        wire::AgentEvent {
            revisions: Some(wire::DetailRevisions {
                conversation,
                screen,
            }),
            session: wire::DetailSession::Available("session".into()),
            kind: "pane.agent_status_changed".into(),
            pane: "pane".into(),
            state: Some(state.into()),
            blocked: None,
        }
    }

    fn parts(conversation: bool, screen: bool) -> ChangedParts {
        ChangedParts {
            conversation,
            screen,
        }
    }

    #[test]
    fn first_event_reads_the_conversation_and_only_a_blocked_screen() {
        assert_eq!(
            changed_parts(None, &event(4, 8, "working"), false),
            parts(true, false)
        );
        assert_eq!(
            changed_parts(None, &event(4, 8, "blocked"), true),
            parts(true, true)
        );
    }

    #[test]
    fn unchanged_revisions_make_no_requests() {
        let event = event(4, 8, "blocked");
        assert_eq!(
            changed_parts(event.revisions.as_ref(), &event, true),
            parts(false, false)
        );
    }

    #[test]
    fn screen_only_change_reads_only_a_blocked_screen() {
        let held = event(4, 8, "blocked").revisions;
        assert_eq!(
            changed_parts(held.as_ref(), &event(4, 9, "blocked"), true),
            parts(false, true)
        );
        assert_eq!(
            changed_parts(held.as_ref(), &event(4, 9, "working"), false),
            parts(false, false)
        );
        assert_eq!(
            changed_parts(held.as_ref(), &event(4, 9, "working"), true),
            parts(false, false)
        );
    }

    #[test]
    fn conversation_change_reads_only_the_conversation() {
        let held = event(4, 8, "blocked").revisions;
        assert_eq!(
            changed_parts(held.as_ref(), &event(5, 8, "blocked"), true),
            parts(true, false)
        );
        assert_eq!(
            changed_parts(held.as_ref(), &event(5, 8, "working"), false),
            parts(true, false)
        );
    }

    #[test]
    fn becoming_blocked_reads_the_screen_even_without_a_new_revision() {
        let mut event = event(4, 8, "blocked");
        assert_eq!(
            changed_parts(event.revisions.as_ref(), &event, false),
            parts(false, true)
        );
        event.state = Some("idle".into());
        event.blocked = Some(BlockedPrompt {
            question: "Allow?".into(),
            options: Vec::new(),
        });
        assert_eq!(
            changed_parts(event.revisions.as_ref(), &event, false),
            parts(false, true)
        );
        event.state = None;
        event.blocked = None;
        assert_eq!(
            changed_parts(event.revisions.as_ref(), &event, true),
            parts(false, false)
        );
    }

    #[test]
    fn an_older_peer_invalidates_both_parts_on_every_event() {
        let mut event = event(4, 8, "working");
        let held = event.revisions.take();
        assert_eq!(
            changed_parts(held.as_ref(), &event, false),
            parts(true, true)
        );
        assert_eq!(changed_parts(None, &event, false), parts(true, true));
    }

    #[test]
    fn reconnect_clears_revisions_and_reads_each_eligible_part_again() {
        let event = event(4, 8, "blocked");
        assert_eq!(
            changed_parts(event.revisions.as_ref(), &event, true),
            parts(false, false)
        );
        assert_eq!(changed_parts(None, &event, true), parts(true, true));
    }

    #[test]
    fn reconnect_backoff_doubles_caps_and_resets_after_an_event() {
        let mut backoff = Backoff::default();
        let delays: Vec<_> = (0..8).map(|_| backoff.next_delay().as_secs()).collect();
        assert_eq!(delays, [1, 2, 4, 8, 16, 30, 30, 30]);
        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_secs(1));
        assert_eq!(backoff.next_delay(), Duration::from_secs(2));
    }

    #[test]
    fn pending_list_identity_uses_only_this_agents_inbox_ids_and_revisions() {
        use crate::app::{InboxItem, InboxKind, InboxSelection};
        let item = |host: &str, pane: &str, id: &str| InboxItem {
            selection: InboxSelection {
                host: host.into(),
                pane: pane.into(),
                id: id.into(),
                kind: InboxKind::Question,
            },
            revision: 0,
            title: "Question".into(),
            agent_title: None,
            repository: None,
            age_ms: 0,
        };
        let address = wire::AgentAddress::local("pane");
        let mut inbox = vec![
            item("local", "pane", "a"),
            item("local", "pane", "b"),
            item("other-host", "pane", "foreign-host"),
            item("local", "other-pane", "foreign-pane"),
        ];
        let ids = agent_inbox_ids(&inbox, &address);
        assert_eq!(
            ids,
            [("a".to_owned(), 0), ("b".to_owned(), 0)]
                .into_iter()
                .collect()
        );
        inbox[0].age_ms = 2_000;
        inbox[1].title = "Updated title".into();
        inbox.reverse();
        inbox.push(item("local", "pane", "a"));
        assert_eq!(agent_inbox_ids(&inbox, &address), ids);
        inbox[2].revision = 1;
        assert_ne!(agent_inbox_ids(&inbox, &address), ids);
        inbox[2].revision = 0;
        assert_eq!(agent_inbox_ids(&inbox, &address), ids);
        inbox[0].revision = 2;
        assert_eq!(agent_inbox_ids(&inbox, &address), ids);
        inbox.retain(|item| item.selection.id != "a");
        assert_ne!(agent_inbox_ids(&inbox, &address), ids);
    }

    #[test]
    fn panel_keys_and_options_follow_the_server_allowlist() {
        assert!(PANEL_KEYS.iter().all(|(key, _)| allowed_key(key)));
        assert!(allowed_key("Y"));
        assert!(allowed_key("tab"));
        assert!(!allowed_key("hello"));
        assert!(!allowed_key("a"));
        assert!(!allowed_key("ctrl+d"));
    }

    #[test]
    fn a_prompt_also_marks_an_agent_blocked() {
        let mut agent = wire::AgentView {
            host: "local".into(),
            pane: "test".to_owned(),
            name: None,
            title: None,
            agent: None,
            state: "idle".to_owned(),
            cwd: None,
            repository: None,
            blocked: None,
        };
        assert!(!is_blocked(&agent));
        agent.blocked = Some(BlockedPrompt {
            question: "Allow?".to_owned(),
            options: vec![],
        });
        assert!(is_blocked(&agent));
        agent.blocked = None;
        agent.state = "blocked".to_owned();
        assert!(is_blocked(&agent));
    }

    #[test]
    fn pending_items_follow_precedence_and_explicit_links() -> Result<(), serde_json::Error> {
        let reviews = vec![ReviewSummary {
            id: "plan".to_owned(),
            pane: "pane".to_owned(),
            session: String::new(),
            path: "plan.md".to_owned(),
            revisions: 1,
            annotations: 0,
            decision: None,
        }];
        let questions: PendingList = serde_json::from_value(serde_json::json!({"pending": [{
            "id": "form", "pane": "pane", "session": "", "cwd": "", "created_ms": 0,
            "questions": {"questions": [{"id": "q", "type": "text", "question": "Which?"}]}
        }]}))?;
        assert!(matches!(
            pending_tab(
                "pane",
                None,
                None,
                Some(&reviews),
                Some(&questions.pending),
                true
            ),
            Some(RequestTab::Review(_))
        ));
        assert!(matches!(
            pending_tab(
                "pane",
                None,
                Some("form"),
                Some(&reviews),
                Some(&questions.pending),
                true
            ),
            Some(RequestTab::Questions(_, 1))
        ));
        assert!(matches!(
            pending_tab(
                "pane",
                None,
                None,
                Some(&[]),
                Some(&questions.pending),
                true
            ),
            Some(RequestTab::Questions(_, 1))
        ));
        assert!(matches!(
            pending_tab("pane", Some("answered"), None, Some(&[]), Some(&[]), true),
            Some(RequestTab::Prompt)
        ));
        assert!(
            pending_tab(
                "other",
                None,
                None,
                Some(&reviews),
                Some(&questions.pending),
                false
            )
            .is_none()
        );
        assert!(pending_tab("pane", None, None, Some(&[]), Some(&[]), false).is_none());
        Ok(())
    }
}
