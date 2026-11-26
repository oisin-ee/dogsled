//! The selected agent's screen: its pending request, or its conversation.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use dioxus::html::{FileData, HasFileData, geometry::PixelsVector2D};
use dioxus::prelude::*;
use dioxus::web::WebEventExt;
use dioxus_icons::lucide::{ArrowUp, ExternalLink, Paperclip, Square, X};
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::scroll_area::ScrollDirection;
use dioxus_sdk_time::{sleep, use_interval};
use gloo_net::http::Request;
use js_sys::wasm_bindgen::JsCast;
use portal_types::agents::{self as wire, BlockedPrompt, Role, Turn, current_turn};
use portal_types::questions::{Pending, PendingList};
use portal_types::reviews::{ReviewList, ReviewSummary};
use portal_types::{TodoStatusView, TodoView};
use serde::de::DeserializeOwned;

use crate::activity::{Gap, Live, use_live_activity};
use crate::api;
use crate::components::{
    accordion::{Accordion, AccordionContent, AccordionItem, AccordionTrigger},
    badge::{Badge, BadgeVariant},
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

/// The selected agent's draft, shared with its review view to quote a passage.
#[derive(Clone, Copy)]
pub struct Draft(pub Signal<String>);

/// What one composer keeps while the page is open: the unsent draft and its images, and the
/// sent messages that the agent's transcript does not show yet. It outlives the agent's view,
/// so a switch to another agent neither carries this draft along nor drops a queued message.
#[derive(Clone, Copy)]
pub struct ComposerState {
    pub draft: Signal<String>,
    pub images: Signal<Vec<String>>,
    outbox: Signal<Vec<Outgoing>>,
    /// Messages waiting for the one before them, so the agent receives them in order.
    queue: Signal<std::collections::VecDeque<Delivering>>,
    delivering: Signal<bool>,
    /// Why the last message was not sent.
    error: Signal<Option<String>>,
    next: Signal<u64>,
    /// The scope that keeps this state, and runs its deliveries.
    owner: ScopeId,
}

impl ComposerState {
    fn new_in_scope(owner: ScopeId) -> Self {
        Self {
            draft: Signal::new_in_scope(String::new(), owner),
            images: Signal::new_in_scope(Vec::new(), owner),
            outbox: Signal::new_in_scope(Vec::new(), owner),
            queue: Signal::new_in_scope(std::collections::VecDeque::new(), owner),
            delivering: Signal::new_in_scope(false, owner),
            error: Signal::new_in_scope(None, owner),
            next: Signal::new_in_scope(0, owner),
            owner,
        }
    }

    /// Send one message after the ones before it. The send outlives the agent's view.
    fn deliver(self, pane: wire::AgentAddress, message: Delivering) {
        let Self {
            mut draft,
            mut images,
            mut outbox,
            mut queue,
            mut delivering,
            mut error,
            owner,
            ..
        } = self;
        queue.write().push_back(message);
        if *delivering.peek() {
            return;
        }
        delivering.set(true);
        dioxus::core::Runtime::current().spawn(owner, async move {
            loop {
                let next = queue.write().pop_front();
                let Some(sent) = next else { break };
                match api::send_prompt(&pane, &sent.message, sent.reply).await {
                    Ok(()) => {
                        if let Some(item) =
                            outbox.write().iter_mut().find(|item| item.id == sent.id)
                        {
                            item.delivery = if sent.queued {
                                Delivery::Queued
                            } else {
                                Delivery::Sent
                            };
                        }
                    }
                    Err(failure) => {
                        outbox.write().retain(|item| item.id != sent.id);
                        // The text comes back unless the owner has started a new message.
                        if draft.peek().is_empty() && images.peek().is_empty() {
                            draft.set(sent.text);
                            images.set(sent.images);
                        }
                        error.set(Some(failure));
                    }
                }
            }
            delivering.set(false);
        });
    }
}

/// Every composer's state, one per agent and one for the New conversation page.
#[derive(Clone)]
pub struct Composers {
    states: Rc<std::cell::RefCell<BTreeMap<String, ComposerState>>>,
    owner: ScopeId,
}

/// The composers live as long as the scope that creates them.
impl Default for Composers {
    fn default() -> Self {
        Self {
            states: Rc::default(),
            owner: dioxus::core::current_scope_id(),
        }
    }
}

impl Composers {
    /// The state of one composer, created empty on first use.
    pub fn state(&self, key: &str) -> ComposerState {
        *self
            .states
            .borrow_mut()
            .entry(key.to_owned())
            .or_insert_with(|| ComposerState::new_in_scope(self.owner))
    }
}

/// The state of one composer, kept by the page's `Composers` when it provides them.
pub fn use_composer(key: &str) -> ComposerState {
    let composers = try_use_context::<Composers>();
    use_hook(|| {
        composers.map_or_else(
            || ComposerState::new_in_scope(dioxus::core::current_scope_id()),
            |composers| composers.state(key),
        )
    })
}

/// Where a sent message is until the transcript records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Delivery {
    /// The request is on its way to the agent.
    Sending,
    /// The agent was working, so it reads the message when its current step ends.
    Queued,
    /// The agent has the message and has not recorded it yet.
    Sent,
}

impl Delivery {
    fn label(self) -> &'static str {
        match self {
            Self::Sending => "Sending…",
            Self::Queued => "Queued",
            Self::Sent => "Sent",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::Sending => "Sending the message to the agent.",
            Self::Queued => {
                "The agent is working. It reads this message when its current step ends."
            }
            Self::Sent => "The agent has this message. It shows here until the agent records it.",
        }
    }
}

/// A message the owner sent that the transcript does not show yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Outgoing {
    id: u64,
    text: String,
    delivery: Delivery,
    /// The newest transcript record when this message was sent. The history window
    /// slides, so a record id stays put where a count from the front would not.
    after: Option<String>,
}

/// The history entry that Up (`earlier`) or Down shows next, with its text. Down past the
/// newest entry returns to an empty message; Up at the oldest entry does nothing.
pub(crate) fn recall(
    history: &[String],
    current: Option<usize>,
    earlier: bool,
) -> Option<(Option<usize>, String)> {
    let shown = |index: usize| Some((Some(index), history.get(index)?.clone()));
    match (current, earlier) {
        (None, true) => shown(history.len().checked_sub(1)?),
        (Some(index), true) => shown(index.checked_sub(1)?),
        (Some(index), false) => shown(index + 1).or(Some((None, String::new()))),
        (None, false) => None,
    }
}

/// One message for the delivery queue. A prompt reply has no outbox entry.
struct Delivering {
    id: u64,
    reply: bool,
    message: String,
    text: String,
    images: Vec<String>,
    queued: bool,
}

/// Whether the owner typed this record. Dogsled and extension input are not the owner's.
fn owner_message(turn: &Turn) -> bool {
    turn.role == Role::User && matches!(turn.source, wire::Source::Harness { .. })
}

/// Messages the owner typed, oldest first.
fn owner_messages(turns: &[Turn]) -> impl Iterator<Item = &str> {
    turns
        .iter()
        .filter(|turn| owner_message(turn))
        .map(|turn| turn.text.trim())
}

/// The sent messages that no later owner message in the transcript accounts for.
/// A recorded message may carry more than was sent, such as its attached images.
pub(crate) fn undelivered(outbox: &[Outgoing], turns: &[Turn]) -> Vec<Outgoing> {
    let mut used = vec![false; turns.len()];
    outbox
        .iter()
        .filter(|item| {
            let sent = item.text.trim();
            // A record that has left the window is older than everything still in it.
            let start = item.after.as_ref().map_or(0, |id| {
                turns
                    .iter()
                    .position(|turn| &turn.id == id)
                    .map_or(0, |index| index + 1)
            });
            let found = turns.iter().enumerate().skip(start).find(|(index, turn)| {
                used.get(*index) == Some(&false) && owner_message(turn) && turn.text.contains(sent)
            });
            match found.and_then(|(index, _)| used.get_mut(index)) {
                Some(slot) => {
                    *slot = true;
                    false
                }
                None => true,
            }
        })
        .cloned()
        .collect()
}

/// A sent message, shown as the owner's record with where it is.
#[component]
pub(crate) fn PendingMessage(text: String, label: String, detail: String) -> Element {
    rsx! {
        article { class: "conversation-event conversation-owner conversation-pending", aria_label: "You",
            span { class: "conversation-time", aria_hidden: "true" }
            div { class: "conversation-body",
                div { class: "conversation-meta", strong { "You" }
                    span { class: "conversation-delivery", role: "status", title: "{detail}", "{label}" } }
                p { class: "conversation-pending-text", "{text}" }
            }
        }
    }
}

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
    lead: Option<crate::board::NavigationRow>,
    inbox: Vec<crate::app::InboxItem>,
) -> Element {
    // Every session hook reads its own agent, so another agent mounts a fresh session.
    crate::keyed(rsx! {
        AgentSession { key: "{host}:{pane}", host, pane, review, question, lead, inbox }
    })
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

fn close_disabled(pending: bool, metadata: &AgentMetadata) -> bool {
    pending || !matches!(metadata, AgentMetadata::Present(_))
}

/// A late close response may only clear selections for its own host and pane.
fn complete_close(
    address: &wire::AgentAddress,
    result: Result<(), String>,
    selected: &mut Option<wire::AgentAddress>,
    inbox: &mut Option<crate::app::InboxSelection>,
) -> Option<String> {
    if let Err(error) = result {
        return Some(error);
    }
    if selected.as_ref() == Some(address) {
        *selected = None;
    }
    if inbox
        .as_ref()
        .is_some_and(|item| item.host == address.host && item.pane == address.pane)
    {
        *inbox = None;
    }
    None
}

#[component]
fn AgentSession(
    host: String,
    pane: String,
    review: Option<String>,
    question: Option<String>,
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
    // A revisit renders the cached history at once; the first event confirms or replaces it.
    let mut detail_session = use_signal(|| {
        conversations
            .peek()
            .host(&address.host)
            .and_then(|cache| cache.session(&address.pane))
            .cloned()
    });
    let mut screen = use_signal(|| None::<wire::Screen>);
    let mut files = use_signal(|| None::<portal_types::files::FilesState>);
    let mut screen_error = use_signal(|| None::<String>);
    let mut pending_error = use_signal(|| None::<String>);
    let mut stream_error = use_signal(|| None::<String>);
    let mut action_error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let mut conversation_trigger = use_signal(|| 0_u64);
    let mut screen_trigger = use_signal(|| 0_u64);
    let mut files_trigger = use_signal(|| 0_u64);
    let mut settings_trigger = use_signal(|| 0_u64);
    let mut settings = use_signal(|| None::<portal_types::models::AgentSettings>);
    let mut settings_error = use_signal(|| None::<String>);
    let mut settings_load_error = use_signal(|| None::<String>);
    let mut settings_busy = use_signal(|| false);
    let settings_pane = address.clone();
    use_resource(move || {
        let trigger = settings_trigger();
        let is_pi = agent
            .read()
            .as_ref()
            .is_some_and(|agent| agent.agent.as_deref() == Some("pi"));
        let pane = settings_pane.clone();
        let session = detail_session.peek().clone();
        async move {
            if !is_pi || *settings_busy.peek() {
                return;
            }
            let result = api::agent_settings(&pane).await;
            if *detail_session.peek() != session
                || *settings_trigger.peek() != trigger
                || *settings_busy.peek()
            {
                return;
            }
            match result {
                Ok(value) => {
                    settings.set(value);
                    settings_load_error.set(None);
                }
                Err(error) => {
                    settings.set(None);
                    settings_load_error.set(Some(error));
                }
            }
        }
    });
    let settings_target = address.clone();
    let change_settings = move |change: portal_types::models::SettingsChange| {
        if settings_busy() {
            return;
        }
        settings_busy.set(true);
        settings_trigger += 1;
        settings_error.set(None);
        let pane = settings_target.clone();
        let session = detail_session.peek().clone();
        spawn(async move {
            let result = api::change_settings(&pane, &change).await;
            if *detail_session.peek() == session {
                match result {
                    Ok(value) => settings.set(Some(value)),
                    Err(error) => settings_error.set(Some(error)),
                }
            }
            settings_busy.set(false);
        });
    };
    // A choice belongs to the request that was visible when it was made.
    // A new request opens in front; the same request preserves the tab.
    let mut choice = use_signal(|| None::<(String, Tab)>);
    let uploading = use_signal(|| false);
    use_effect(move || {
        let _ = selection();
        choice.set(None);
    });
    let composer = use_composer(&format!("{host}:{pane}"));
    let ComposerState {
        mut draft,
        mut images,
        mut outbox,
        error: mut delivery_error,
        next: mut next_message,
        ..
    } = composer;
    use_context_provider(|| Draft(draft));

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

    let files_pane = address.clone();
    use_resource(move || {
        let trigger = files_trigger();
        let session = detail_session.peek().clone();
        let pane = files_pane.clone();
        async move {
            if trigger == 0 {
                return;
            }
            let result = api::fetch_files(&pane).await;
            if *detail_session.peek() != session {
                return;
            }
            use portal_types::files::FilesState;
            let state = match result {
                Ok(state) => state,
                Err(error) => match files.peek().as_ref() {
                    Some(FilesState::Current(files) | FilesState::Stale { files, .. }) => {
                        FilesState::Stale {
                            files: files.clone(),
                            error,
                        }
                    }
                    _ => FilesState::Unavailable(error),
                },
            };
            files.set(Some(state));
        }
    });

    let pending_host = host.clone();
    let pending_address = address.clone();
    let inbox_ids = use_memo(use_reactive(&inbox, move |inbox| {
        agent_inbox_ids(&inbox, &pending_address)
    }));
    use_resource(move || {
        let _ = inbox_ids();
        let host = pending_host.clone();
        async move {
            let (review_result, question_result) = futures_util::join!(
                pending_list::<ReviewList>(&host, "reviews"),
                pending_list::<PendingList>(&host, "questions")
            );
            let mut errors = Vec::new();
            match review_result {
                Ok(list) => reviews.set(Some(list.reviews)),
                Err(message) => errors.push(message),
            }
            match question_result {
                Ok(mut list) => {
                    for question in &mut list.pending {
                        question.host.clone_from(&host);
                    }
                    questions.set(Some(list.pending));
                }
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
                    let session_changed = detail_session.peek().as_ref() != Some(&event.session);
                    if session_changed {
                        settings.set(None);
                        settings_error.set(None);
                        settings_load_error.set(None);
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
                        files.set(None);
                        detail_session.set(Some(event.session.clone()));
                    }
                    revisions.clone_from(&event.revisions);
                    let refresh_settings = settings_due(
                        latest_event.peek().as_ref(),
                        &event,
                        &parts,
                        session_changed,
                    );
                    latest_event.set(Some(event));
                    if refresh_settings {
                        settings_trigger += 1;
                    }
                    if parts.conversation {
                        conversation_trigger += 1;
                    }
                    if parts.screen {
                        screen_trigger += 1;
                    }
                    if parts.files {
                        files_trigger += 1;
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
        let text = draft();
        let sent_images = if prompt_reply { Vec::new() } else { images() };
        let message = message_with_images(&text, &sent_images);
        if uploading() || !reachable() || message.trim().is_empty() {
            return;
        }
        // The box clears at once and the message shows where it is until the transcript has it.
        draft.set(String::new());
        if !prompt_reply {
            images.set(Vec::new());
        }
        action_error.set(None);
        delivery_error.set(None);
        let id = next_message();
        next_message += 1;
        let snapshot = match detail_session.peek().as_ref() {
            Some(wire::DetailSession::Available(session)) => conversations
                .peek()
                .host(&send_pane.host)
                .and_then(|cache| cache.get_session(&send_pane.pane, session)),
            _ => None,
        };
        let turns = snapshot
            .as_ref()
            .map_or(&[][..], |snapshot| snapshot.turns.as_slice());
        let mut pending = undelivered(&outbox.peek(), turns);
        let working = !pending.is_empty()
            || match &*live.peek() {
                Live::Detail(state) => state.phase != wire::LivePhase::Idle,
                _ => agent
                    .peek()
                    .as_ref()
                    .is_some_and(|agent| agent.state != "idle"),
            };
        if !prompt_reply {
            pending.push(Outgoing {
                id,
                text: message.clone(),
                delivery: Delivery::Sending,
                after: turns.last().map(|turn| turn.id.clone()),
            });
            outbox.set(pending);
        }
        composer.deliver(
            send_pane.clone(),
            Delivering {
                id,
                reply: prompt_reply,
                message,
                text,
                images: sent_images,
                queued: working,
            },
        );
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
            .and_then(|cache| cache.get_session(&pane, session)),
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
    let worker_rows = crate::workers::owned_rows(board_view.as_ref(), Some(&address));
    let workers: Vec<(String, String)> = worker_rows
        .iter()
        .filter(|row| row.kind == "agent")
        .map(|row| (row.id.clone(), format!("{} · {}", row.what, row.host)))
        .collect();
    let files_count = crate::files::path_count(files.read().as_ref())
        .map_or_else(String::new, |paths| paths.to_string());
    let worker_count = crate::workers::active_count(&worker_rows);
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
        reviews().as_deref(),
        questions().as_deref(),
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
    let close_pane = address.clone();
    let close_unavailable = close_disabled(busy(), &metadata.read());
    let mut selected = use_context::<Signal<Option<wire::AgentAddress>>>();
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
    let completed: usize = todos
        .iter()
        .flat_map(|todo| &todo.sections)
        .filter(|section| section.status == TodoStatusView::Completed)
        .map(|section| section.count)
        .sum();
    let role = if row.is_some() { "worker" } else { "lead" };
    let lead_hostname = board_view
        .as_ref()
        .and_then(|board| {
            crate::board::lead_hostname(
                board,
                portal.agents.read().as_deref().unwrap_or_default(),
                &address,
            )
        })
        .unwrap_or("unavailable");
    let assistant = current
        .as_ref()
        .and_then(|agent| agent.name.as_deref())
        .and_then(nonempty)
        .unwrap_or("Assistant")
        .to_owned();
    let coarse = current
        .as_ref()
        .map_or("Loading…", |agent| agent.state.as_str())
        .to_owned();
    let turns = retained
        .as_ref()
        .map_or(&[][..], |snapshot| snapshot.turns.as_slice());
    let unsent = undelivered(&outbox.read(), turns);
    // Up in an empty composer recalls the owner's messages, newest first.
    let history: Vec<String> = owner_messages(turns)
        .map(str::to_owned)
        .chain(unsent.iter().map(|item| item.text.clone()))
        .collect();
    let mut page = use_signal(|| None::<Rc<MountedData>>);
    let mut following = use_signal(|| true);
    let follow_error = use_signal(|| None::<String>);
    // Follow the newest turn until the owner scrolls up to read; scrolling back
    // to the end resumes it. The page region is the one that scrolls.
    use_follow_scroll(
        Follow {
            page,
            following,
            error: follow_error,
        },
        retained.clone(),
        transcript_loading,
        tab == Tab::Conversation,
        live.into(),
        unsent.len(),
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
    let picker = rsx! {
        if harness == Some("pi") {
            if let Some(value) = settings() {
                crate::model_picker::ModelPicker { choices: value.models, selected_model: value.model,
                    selected_level: Some(value.thinking), busy: settings_busy(), on_change: change_settings }
            }
        }
    };
    let wide = width() >= 1400;
    let now = rsx! {
        LiveActivity { live, snapshot: retained.clone(), coarse: coarse.clone(),
            connected: loading() || (reachable() && stream_error().is_none()),
            waiting: pending.as_ref().map(|pending| pending.state().to_owned()),
            step: todo_title.map(str::to_owned) }
    };
    let read_edge = current
        .as_ref()
        .filter(|_| reachable() && tab == Tab::Conversation)
        .and_then(|agent| crate::unread::matching_reply(agent, retained.as_deref()?));
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
                            live, coarse: coarse.clone(), unsent: unsent.clone(), read_edge: read_edge.clone() }
                    }
                }
            if tab == Tab::Conversation {
                footer { class: "conversation-footer",
                    // A wide console shows Now in its right column instead.
                    if !wide { {now.clone()} }
                    Composer { draft, images, uploading, disabled, busy: busy(), placeholder: placeholder.clone(), history: history.clone(), on_error: move |message| action_error.set(Some(message)),
                        model_picker: picker.clone(), on_send: { let mut send = conversation_send.clone(); move |_| send(false) } }
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
                        p { class: "lead-meta",
                            span { "{state}" }
                            span { "{role} on {address.host}" if let Some(harness) = harness { " · {harness}" } }
                            if let Some(branch) = branch { code { "{branch}" } }
                        }
                    }
                }
                if matches!(harness, Some("claude" | "codex")) {
                    crate::model_picker::ReadOnlyModel { model: retained.as_ref().and_then(|conversation| conversation.turns.iter().rev().find_map(|turn| match &turn.source {
                        wire::Source::Harness { model, .. } => model.clone(), _ => None,
                    })) }
                }
                if !plan_open && width() < 1400 {
                    div { class: "lead-tabs",
                        TabList { aria_label: "Lead view",
                            TabTrigger { value: "chat", index: 0usize, "Chat" }
                            TabTrigger { value: "todos", index: 1usize, "Todos {completed}/{todo_count}" }
                            TabTrigger { value: "workers", index: 2usize, "Workers {worker_count}" }
                            TabTrigger { value: "files", index: 3usize, "Files {files_count}" }
                        }
                    }
                }
                div { class: "lead-actions",
                    if worker_view { span { class: "lead-meta", "Lead host: {lead_hostname}" } }
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
                    Button { class: "icon-button", variant: ButtonVariant::Outline, r#type: "button", disabled: close_unavailable,
                        aria_label: "Close session", title: "Close session",
                        onclick: move |_| {
                            if close_disabled(busy(), &metadata.read()) { return; }
                            let pane = close_pane.clone();
                            busy.set(true);
                            action_error.set(None);
                            spawn(async move {
                                let result = api::close_agent(&pane).await;
                                let error = complete_close(&pane, result, &mut selected.write(), &mut selection.write());
                                action_error.set(error);
                                busy.set(false);
                            });
                        }, X { size: 16, attributes: attributes!(svg { "aria-hidden": "true" }) }
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
                if let Some(message) = settings_error().or(settings_load_error()) { Alert { "{message}" } }
                if let Some(message) = action_error().or(delivery_error()) { Alert { "{message}" } }
                if let Some(message) = follow_error() { Alert { "Could not follow the conversation: {message}" } }
            }
            if plan_open {
                div { class: "pending-review",
                    if let Some(RequestTab::Review(id)) = &pending {
                        ReviewView { host: address.host.clone(), review_id: id.clone(),
                            revision: inbox.iter().find(|item| item.selection.host == address.host && item.selection.pane == address.pane && item.selection.kind == crate::app::InboxKind::Review && item.selection.id == *id).map_or(0, |item| item.revision),
                            embedded: true,
                            lead: title.clone(), role: role.to_owned(),
                            repository: current.as_ref().and_then(|agent| agent.repository.clone()).unwrap_or_else(|| "Unknown repository".to_owned()),
                            on_close: move |_| choice.set(Some((review_identity.clone(), Tab::Conversation))),
                        }
                    }
                }
            } else {
                if wide {
                    {chat.clone()}
                    aside { class: "console-todos",
                        section { class: "now-panel", aria_label: "Now",
                            h2 { class: "detail-heading", "Now" }
                            {now}
                        }
                        Tabs { class: "lead-detail", horizontal: true, value: Some(detail().value().to_owned()),
                            on_value_change: move |value: String| {
                                let next = match value.as_str() {
                                    "workers" => crate::app::LeadPanel::Workers,
                                    "files" => crate::app::LeadPanel::Files,
                                    _ => crate::app::LeadPanel::Todos,
                                };
                                detail.set(next);
                                panel.set(next);
                            },
                            header { class: "console-column-header detail-header",
                                div { class: "lead-tabs detail-tabs", TabList { aria_label: "Lead detail",
                                    TabTrigger { value: "todos", index: 0usize, "Todos {completed}/{todo_count}" }
                                    TabTrigger { value: "workers", index: 1usize, "Workers {worker_count}" }
                                    TabTrigger { value: "files", index: 2usize, "Files {files_count}" }
                                } }
                                if detail() == crate::app::LeadPanel::Workers { span { "{working_count} working" } }
                            }
                            TabContent { class: "detail-content", value: "todos", index: 0usize, Todos { todos: todos.clone(), workers: workers.clone() } }
                            TabContent { class: "detail-content", value: "workers", index: 1usize,
                                crate::workers::Workers { rows: worker_rows.clone(), inbox: inbox.clone(), header: false }
                            }
                            TabContent { class: "detail-content", value: "files", index: 2usize,
                                crate::files::Files { state: files() }
                            }
                        }
                    }
                } else {
                    TabContent { class: "chat-tab", value: "chat", index: 0usize, {chat.clone()} }
                    TabContent { class: "todos-tab", value: "todos", index: 1usize,
                        div { class: "todo-tab-body", Todos { todos: todos.clone(), workers: workers.clone() } }
                        Composer { draft, images, uploading, disabled, busy: busy(), placeholder: placeholder.clone(), history, on_error: move |message| action_error.set(Some(message)),
                            model_picker: picker.clone(), on_send: move |_| conversation_send(false) }
                    }
                    TabContent { class: "workers-tab", value: "workers", index: 2usize,
                        crate::workers::Workers { rows: worker_rows.clone(), inbox: inbox.clone() }
                    }
                    TabContent { class: "files-tab", value: "files", index: 3usize,
                        crate::files::Files { state: files() }
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
    files: bool,
}

/// Pi settings change with the session, or when a model or thinking change lands in the
/// transcript. A working turn rewrites the transcript constantly, so those changes wait
/// for its state change instead of reading settings again on every streamed event.
fn settings_due(
    previous: Option<&wire::AgentEvent>,
    event: &wire::AgentEvent,
    parts: &ChangedParts,
    session_changed: bool,
) -> bool {
    session_changed
        || previous.is_none_or(|previous| previous.state != event.state)
        || (parts.conversation && event.state.as_deref() != Some("working"))
}

fn event_is_blocked(event: &wire::AgentEvent, was_blocked: bool) -> bool {
    event.blocked.is_some()
        || event
            .state
            .as_deref()
            .map_or(was_blocked, |state| state == "blocked")
}

/// Older peers lack revisions; their events must invalidate all three parts.
fn changed_parts(
    held: Option<&wire::DetailRevisions>,
    event: &wire::AgentEvent,
    was_blocked: bool,
) -> ChangedParts {
    let mut parts = if let Some(revisions) = &event.revisions {
        let blocked = event_is_blocked(event, was_blocked);
        ChangedParts {
            conversation: held.is_none_or(|held| held.conversation != revisions.conversation),
            files: held.is_none_or(|held| held.files != revisions.files),
            screen: blocked
                && (held.is_none_or(|held| held.screen != revisions.screen) || !was_blocked),
        }
    } else {
        ChangedParts {
            conversation: true,
            screen: true,
            files: true,
        }
    };
    if let Some(pending) = &event.pending {
        parts.conversation &= !pending.conversation;
        parts.screen &= !pending.screen;
        parts.files &= !pending.files;
    }
    parts
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

async fn pending_list<T: DeserializeOwned>(host: &str, kind: &str) -> Result<T, String> {
    let response = Request::get(&api::host_url(host, kind))
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

/// The scrolling region, whether it follows the newest record, and why it last failed to.
#[derive(Clone, Copy)]
pub(crate) struct Follow {
    pub page: Signal<Option<Rc<MountedData>>>,
    pub following: Signal<bool>,
    pub error: Signal<Option<String>>,
}

/// Reuse the same follow policy for transcript, outbox, and live-edge changes.
/// The effect reads `live` itself, so live frames never render the caller.
pub(crate) fn use_follow_scroll(
    follow: Follow,
    snapshot: Option<Rc<wire::Conversation>>,
    loading: bool,
    visible: bool,
    live: ReadSignal<Live>,
    unsent: usize,
) {
    use_effect(use_reactive(
        (&snapshot, &loading, &visible, &unsent),
        move |_| {
            let Follow {
                page,
                following,
                mut error,
            } = follow;
            live.read();
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
                error.set(scrolled.err().map(|error| error.to_string()));
            });
        },
    ));
}

/// The newest records that open with a conversation. Each request for earlier history adds
/// as many again, so a long transcript opens as fast as a short one.
const HISTORY_WINDOW: usize = 150;

/// The first group of the newest groups that fill `limit` records. A group is never split.
fn window_start(groups: &[&[Turn]], limit: usize) -> usize {
    let mut shown = 0;
    groups
        .iter()
        .rposition(|group| {
            shown += group.len();
            shown >= limit
        })
        .unwrap_or(0)
}

/// Live frames reach only the live edge. The history renders again only when its snapshot,
/// its window, or whether the agent is active changes.
#[component]
pub(crate) fn Conversation(
    snapshot: Option<Rc<wire::Conversation>>,
    loading: bool,
    assistant: String,
    live: ReadSignal<Live>,
    #[props(default = "idle".to_owned())] coarse: String,
    /// Receipt key for the verified final reply in this snapshot.
    read_edge: Option<String>,
    /// Sent messages the transcript does not show yet. They follow the live edge.
    #[props(default)]
    unsent: Vec<Outgoing>,
) -> Element {
    let mut limit = use_signal(|| HISTORY_WINDOW);
    let active = use_memo(use_reactive(&coarse, move |coarse| match &*live.read() {
        Live::Detail(state) => state.phase != wire::LivePhase::Idle,
        _ => !matches!(coarse.as_str(), "idle" | "done"),
    }));
    let active = active();
    let turns = snapshot
        .as_ref()
        .map_or(&[][..], |snapshot| snapshot.turns.as_slice());
    let groups = turns
        .chunk_by(|_, next| next.role != Role::User)
        .collect::<Vec<_>>();
    let first = window_start(&groups, limit());
    let earlier = groups.get(..first).map_or(0, |groups| {
        groups.iter().map(|group| group.len()).sum::<usize>()
    });
    let edge_turn = wire::reply_edge(turns);
    let record = |turn: &Turn| {
        let receipt = (!active && edge_turn.is_some_and(|edge| std::ptr::eq(edge, turn)))
            .then(|| read_edge.clone())
            .flatten();
        conversation_record(turn, &assistant, receipt)
    };
    let records = groups.iter().enumerate().skip(first).flat_map(|(index, group)| {
        // The newest turn stays open while the agent works. History without its opening
        // message, such as the start of a truncated window, has nothing to fold under.
        let live = active && index + 1 == groups.len();
        let head = &group[0];
        if live || !(head.role == Role::User || head.is_nudge()) {
            return group.iter().map(&record).collect::<Vec<_>>();
        }
        let mut records = vec![record(head)];
        let rest = &group[1..];
        let mut previous = head;
        let mut replied = false;
        let mut seen = 0;
        // Every message between the owner and the agent stays on the timeline. Each stretch
        // of thinking, tool calls, and check-ins between two messages folds on its own.
        let shown = shown_messages(rest);
        let positions = (0..rest.len()).collect::<Vec<_>>();
        for span in positions.chunk_by(|&a, &b| shown[a] == shown[b]) {
            let stretch = &rest[span[0]..=span[span.len() - 1]];
            seen += stretch.len();
            if shown[span[0]] {
                records.extend(stretch.iter().map(&record));
                previous = &stretch[stretch.len() - 1];
                replied |= stretch.iter().any(|turn| turn.pinned().is_some());
            } else if seen < rest.len() || replied {
                // Work after the last message is the agent's final state unless the agent
                // already ended its turn, so it folds only then.
                let label = fold_label(previous, stretch);
                records.push(rsx! {
                    ToolRecords { key: "work:{stretch[0].id}", turns: stretch.to_vec(), assistant: assistant.clone(), label }
                });
            } else {
                records.extend(stretch.iter().map(&record));
            }
        }
        records
    }).collect::<Vec<_>>();
    rsx! {
        div { class: "conversation-turns",
            div { class: "conversation-column",
                if loading { Skeleton { role: "status", aria_label: "Loading conversation" } }
                if let Some(snapshot) = &snapshot {
                    if earlier > 0 {
                        Button { class: "conversation-earlier-history", variant: ButtonVariant::Ghost, r#type: "button",
                            onclick: move |_| limit += HISTORY_WINDOW, "Show earlier history ({earlier})" }
                    } else if snapshot.truncated {
                        p { class: "conversation-history-notice", "Earlier history was not returned." }
                    }
                    if snapshot.turns.is_empty() && !snapshot.truncated {
                        Status { "No conversation yet. Send a message to start." }
                    }
                }
                {records.into_iter()}
                LiveEdge { live, last: turns.last().cloned() }
                for item in unsent {
                    PendingMessage { key: "outbox:{item.id}", text: item.text, label: item.delivery.label(),
                        detail: item.delivery.detail() }
                }
            }
        }
    }
}

/// A message between the owner and the agent: what the owner sent, what the agent wrote,
/// and how the agent ended its turn. Everything else is work that folds.
fn is_message(turn: &Turn) -> bool {
    matches!(turn.role, Role::User | Role::Assistant) || turn.pinned().is_some()
}

/// Which turns stay on the timeline. A successful `end_turn` message is the reply, so text
/// the agent writes after it, before anything new arrives, folds: it repeats the reply.
fn shown_messages(turns: &[Turn]) -> Vec<bool> {
    let mut ended = false;
    turns
        .iter()
        .map(|turn| {
            if turn.role == Role::User || turn.is_nudge() {
                ended = false;
            }
            let shown = is_message(turn) && (!ended || turn.pinned().is_some());
            ended |= turn.pinned().is_some();
            shown
        })
        .collect()
}

/// The label of one folded stretch: how long it ran from the message before it, and what it
/// holds.
fn fold_label(previous: &Turn, turns: &[Turn]) -> String {
    let (thoughts, calls) = work_counts(turns);
    let check_ins = match turns.iter().filter(|turn| turn.is_nudge()).count() {
        0 => None,
        1 => Some("1 check-in".to_owned()),
        count => Some(format!("{count} check-ins")),
    };
    let later = match turns.iter().filter(|turn| is_message(turn)).count() {
        0 => None,
        1 => Some("1 message after the reply".to_owned()),
        count => Some(format!("{count} messages after the reply")),
    };
    let parts = [
        (thoughts + calls > 0).then(|| worked_label(previous, turns)),
        check_ins,
        later,
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if parts.is_empty() {
        "Context".to_owned()
    } else {
        parts.join(" · ")
    }
}

fn conversation_record(turn: &Turn, assistant: &str, receipt: Option<String>) -> Element {
    if let Some(kind) = turn.pinned() {
        // The pinned message is the agent's reply, not a tool record.
        let reply = Turn {
            role: Role::Assistant,
            tool: None,
            ..turn.clone()
        };
        let kind = kind.label().map(str::to_owned);
        rsx! { MessageRecord { key: "{turn.id}", turn: reply, assistant: assistant.to_owned(), receipt, kind } }
    } else if turn.role == Role::Tool {
        rsx! { ToolRecord { key: "{turn.id}", turn: turn.clone() } }
    } else {
        rsx! { MessageRecord { key: "{turn.id}", turn: turn.clone(), assistant: assistant.to_owned(), receipt } }
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

pub(crate) fn elapsed_label(seconds: u64) -> String {
    if seconds >= 60 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

fn worked_label(owner: &Turn, turns: &[Turn]) -> String {
    let span = owner
        .timestamp
        .as_ref()
        .zip(wire::latest_record_time(turns))
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
fn MessageRecord(
    turn: Turn,
    assistant: String,
    receipt: Option<String>,
    /// How an `end_turn` message ends the turn, when it is not a plain reply.
    #[props(default)]
    kind: Option<String>,
) -> Element {
    let (author, source, owner) = source_label(&turn, &assistant);
    let event = crate::conversation_events::delivery(&turn.source)
        .map(|(tone, label)| (tone, label.to_owned()));
    // Polls, activity, and attribution changes must not reparse unchanged Markdown.
    let markdown = use_memo(use_reactive(&turn.text, |text| {
        crate::review_view::render_markdown(&text)
    }));
    rsx! {
        article { class: if owner { "conversation-event conversation-owner" } else if turn.role == Role::Thinking { "conversation-event conversation-thought" } else if event.is_some() { "conversation-event ds-delivery" } else { "conversation-event" },
            "data-tone": event.as_ref().map(|(tone, _)| tone.class()), "data-record-id": turn.id, aria_label: author.clone(),
            ConversationTime { timestamp: turn.timestamp, role: turn.role, source: turn.source }
            div { class: "conversation-body",
                if let Some((tone, label)) = &event { crate::conversation_events::DeliveryLabel { tone: *tone, label: label.clone() } }
                div { class: "conversation-meta", strong { "{author}" } span { "{source}" }
                    if let Some(kind) = kind { Badge { variant: BadgeVariant::Outline, "data-part": "end-turn-kind", "{kind}" } }
                }
                div { class: "markdown-body", dangerous_inner_html: markdown() }
                if let Some(receipt) = receipt {
                    dogsled-read-edge { key: "{receipt}", "data-receipt": receipt, "aria-hidden": "true" }
                }
            }
        }
    }
}

#[component]
fn ToolRecords(turns: Vec<Turn>, assistant: String, label: String) -> Element {
    // The fold sits on the timeline at the time its work started.
    let first = turns.first();
    rsx! {
        div { class: "conversation-event conversation-work",
            if let Some(first) = first {
                ConversationTime { timestamp: first.timestamp.clone(), role: first.role, source: first.source.clone() }
            }
            Collapsible { class: "conversation-body", keep_mounted: true,
                CollapsibleTrigger { "{label}" }
                CollapsibleContent { "data-part": "turn-work",
                    for turn in &turns { {conversation_record(turn, &assistant, None)} }
                }
            }
        }
    }
}

#[component]
fn ToolRecord(turn: Turn) -> Element {
    let tool = turn.tool.as_ref();
    if let Some(tool) = tool
        && let Some(method) = tool
            .name
            .as_deref()
            .and_then(crate::conversation_events::operation)
    {
        return rsx! {
            article { class: "conversation-event conversation-tool ds-call", "data-record-id": turn.id,
                ConversationTime { timestamp: turn.timestamp, role: turn.role, source: turn.source }
                crate::conversation_events::CallPanel { tool: tool.clone(), method: method.to_owned() }
            }
        };
    }
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
    use js_sys::{Date, Intl::DateTimeFormat, wasm_bindgen::JsValue};
    thread_local! {
        // Building an Intl formatter costs far more than formatting with one, and every
        // record shows a time, so the page builds the pair once.
        static FORMATS: Option<[DateTimeFormat; 2]> = time_formats();
    }
    let date = Date::new(&JsValue::from_f64(ms as f64));
    if !date.get_time().is_finite() {
        return None;
    }
    FORMATS.with(|formats| {
        let [short, full] = formats.as_ref()?;
        let text = |format: &DateTimeFormat| {
            format
                .format()
                .call1(&JsValue::UNDEFINED, &date)
                .ok()?
                .as_string()
        };
        Some((text(short)?, text(full)?, date.to_iso_string().into()))
    })
}

#[cfg(target_arch = "wasm32")]
fn time_formats() -> Option<[js_sys::Intl::DateTimeFormat; 2]> {
    use js_sys::{Array, Intl::DateTimeFormat, Object, Reflect, wasm_bindgen::JsValue};
    let format = |settings: &[(&str, JsValue)]| {
        let options = Object::new();
        for (key, value) in settings {
            Reflect::set(&options, &JsValue::from_str(key), value).ok()?;
        }
        Some(DateTimeFormat::new(&Array::new(), &options))
    };
    Some([
        format(&[
            ("hour", "2-digit".into()),
            ("minute", "2-digit".into()),
            ("hour12", false.into()),
        ])?,
        format(&[("dateStyle", "full".into()), ("timeStyle", "long".into())])?,
    ])
}

/// The live edge shares the history's scrolling region, not the pinned footer.
#[component]
pub(crate) fn LiveEdge(live: ReadSignal<Live>, #[props(default)] last: Option<Turn>) -> Element {
    let Live::Detail(state) = live() else {
        return rsx! {};
    };
    let reasoning = state.reasoning.filter(|text| !text.is_empty());
    rsx! {
        div { class: "conversation-live-edge", aria_live: "off",
            if let Some(text) = reasoning.filter(|text| !last.as_ref().is_some_and(|turn| turn.role == Role::Thinking && turn.text == *text)) {
                LiveThought { text }
            }
            for tool in state.active_tools {
                if let Some(method) = crate::conversation_events::operation(&tool.name) {
                    div { key: "{tool.call_id}", class: "ds-call-panel",
                        crate::conversation_events::CallHeading { method: method.to_owned(), name: tool.name.clone(), call: tool.call.clone() }
                        div { class: "ds-receipt", "Running call…" }
                    }
                } else {
                    div { key: "{tool.call_id}", class: "conversation-live-tool",
                        p { class: "conversation-meta", "Running · {tool.name}" }
                        pre { class: "conversation-call", "{tool.call.as_deref().unwrap_or(&tool.name)}" }
                    }
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

/// A timer updates only this panel; unchanged history Markdown stays memoized.
#[component]
fn LiveActivity(
    live: ReadSignal<Live>,
    snapshot: Option<Rc<wire::Conversation>>,
    coarse: String,
    connected: bool,
    waiting: Option<String>,
    #[props(default)] step: Option<String>,
) -> Element {
    let mut now = use_signal(now_ms);
    use_interval(Duration::from_secs(1), move |()| now.set(now_ms()));
    let live = live();
    let turn = snapshot
        .as_ref()
        .map_or(&[][..], |snapshot| current_turn(&snapshot.turns));
    let elapsed_seconds = match &live {
        Live::Detail(state) => {
            wire::TurnClock::from_history(turn, state.phase).and_then(|clock| clock.seconds(now()))
        }
        _ => None,
    };
    rsx! { ActivityRow { live, coarse, connected, waiting, turns: turn.to_vec(), elapsed_seconds, step } }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn now_ms() -> u64 {
    js_sys::Date::now() as u64
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn now_ms() -> u64 {
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
    /// The agent's current todo, the step this turn works on.
    #[props(default)]
    step: Option<String>,
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
    let state = if detail == "Needs you" {
        "blocked"
    } else if phase == "Disconnected" {
        "disconnected"
    } else {
        coarse.as_str()
    };
    rsx! {
        div { class: "conversation-activity", "data-state": state,
            span { class: "activity-dot", aria_hidden: "true" }
            div { class: "activity-what",
                p { class: "activity-phase", role: "status", aria_live: "polite", aria_atomic: "true", "{phase}" }
                if let Some(step) = step.filter(|step| !step.trim().is_empty()) {
                    p { class: "activity-step", "{step}" }
                }
                if detail == "Live" { p { class: "activity-counts", "{count_label(thoughts, calls)}" } }
            }
            div { class: "activity-clock",
                if detail == "Live" && let Some(seconds) = elapsed_seconds {
                    // Green means running, so a finished turn's time stays white.
                    p { class: "activity-elapsed", "data-running": phase != "Idle", "{elapsed_label(seconds)}" }
                    p { class: "activity-detail", if phase == "Idle" { "last turn" } else { "this turn" } }
                } else {
                    p { class: "activity-detail", "{detail}" }
                }
            }
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
            wire::LivePhase::Compacting => "Auto-compacting…".into(),
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

/// The browser files carried by a file input or drop.
pub(crate) fn attachment_files(files: Vec<FileData>) -> Vec<web_sys::File> {
    files
        .iter()
        .filter_map(|file| file.inner().downcast_ref::<web_sys::File>().cloned())
        .collect()
}

pub(crate) fn pasted_files(event: &ClipboardEvent) -> Vec<web_sys::File> {
    let list = event
        .data()
        .try_as_web_event()
        .and_then(|event| event.dyn_into::<web_sys::ClipboardEvent>().ok())
        .and_then(|event| event.clipboard_data())
        .and_then(|data| data.files());
    list.map_or_else(Vec::new, |list| {
        (0..list.length())
            .filter_map(|index| list.item(index))
            .collect()
    })
}

fn file_drag(event: &DragEvent) -> Option<web_sys::DataTransfer> {
    // The file list is protected until drop; only the types are visible during dragover.
    event
        .data()
        .try_as_web_event()
        .and_then(|event| event.data_transfer())
        .filter(|data| data.types().includes(&"Files".into(), 0))
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
    #[props(default = VNode::empty())] model_picker: Element,
    /// Controls before the model, such as the New conversation repository.
    #[props(default = VNode::empty())]
    controls: Element,
    /// The owner's earlier messages, oldest first. Up in an empty composer recalls them.
    #[props(default)]
    history: Vec<String>,
    /// Focus the message box when it mounts, so typing starts at once.
    #[props(default)]
    autofocus: bool,
) -> Element {
    let mut round = use_signal(|| 0_u32);
    // The history entry the composer shows, until the owner edits it.
    let mut recalled = use_signal(|| None::<usize>);
    let mut file_over = use_signal(|| false);
    let accepts_files = !disabled && !busy && !uploading();
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
        form { class: "console-composer message-composer", aria_label: "Message composer", aria_busy: uploading() || busy,
            "data-drop-target": accepts_files && file_over(),
            ondragover: move |event: DragEvent| {
                if let Some(data) = file_drag(&event) {
                    // Cancel even when disabled, so dropping a file cannot navigate away.
                    event.prevent_default();
                    data.set_drop_effect(if accepts_files { "copy" } else { "none" });
                    if *file_over.peek() != accepts_files { file_over.set(accepts_files); }
                }
            },
            ondragleave: move |event: DragEvent| {
                // Drag events bubble from the controls; crossing between them is not a leave.
                let still_inside = event.data().try_as_web_event()
                    .and_then(|event| event.related_target())
                    .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                    .is_some_and(|target| target.closest(".message-composer").ok().flatten().is_some());
                if !still_inside { file_over.set(false); }
            },
            ondrop: move |event: DragEvent| {
                file_over.set(false);
                let files = event.files();
                if !files.is_empty() || file_drag(&event).is_some() {
                    event.prevent_default();
                    upload.call(attachment_files(files));
                }
            },
            // Enter in the repository or model search submits the form too. Only the message's
            // Enter and the send button send, so choosing a control never sends the message.
            onsubmit: move |event| event.prevent_default(),
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
                Textarea { name: "message", aria_label: "Message this agent", placeholder, rows: "1", value: "{draft}", disabled: disabled || busy,
                    onmounted: move |event: MountedEvent| {
                        if autofocus {
                            spawn(async move { let _ = event.data().set_focus(true).await; });
                        }
                    },
                    oninput: move |event: FormEvent| {
                        recalled.set(None);
                        draft.set(event.value());
                    },
                    onpaste: move |event: ClipboardEvent| {
                        let files = pasted_files(&event);
                        if !files.is_empty() { event.prevent_default(); upload.call(files); }
                    },
                    onkeydown: move |event: KeyboardEvent| {
                        if event.is_composing() {
                            return;
                        }
                        let key = event.key();
                        if key == Key::Enter && !event.modifiers().shift() {
                            event.prevent_default();
                            recalled.set(None);
                            if !disabled && !busy && !uploading() { on_send.call(()); }
                        } else if (key == Key::ArrowUp || key == Key::ArrowDown) && event.modifiers().is_empty()
                            && (recalled().is_some() || draft.peek().is_empty())
                            && let Some((index, text)) = recall(&history, recalled(), key == Key::ArrowUp)
                        {
                            event.prevent_default();
                            recalled.set(index);
                            draft.set(text);
                        }
                    },
                }
            }
            div { class: "composer-toolbar",
                {controls}
                {model_picker}
                Button { class: "icon-button attach-button", variant: ButtonVariant::Ghost, r#type: "button",
                    aria_label: "Attach images", title: "Attach images", disabled: disabled || busy || uploading(),
                    onclick: move |_| { let _ = document::eval("document.getElementById('composer-images').click()"); },
                    Paperclip { size: 16, attributes: attributes!(svg { "aria-hidden": "true" }) }
                }
                // Enter sends, so Tab goes from the message to the controls, never to this button.
                Button { class: "icon-button send-button", r#type: "button", aria_label: "Send message", title: "Send message", tabindex: "-1",
                    onclick: move |_| if !disabled && !busy && !uploading() { on_send.call(()); },
                    disabled: disabled || busy || uploading() || (draft().trim().is_empty() && images().is_empty()),
                    ArrowUp { size: 16, attributes: attributes!(svg { "aria-hidden": "true" }) }
                }
                Input { key: "{round}", id: "composer-images", name: "images", r#type: "file", hidden: true,
                    accept: "image/png,image/jpeg,image/webp", multiple: true, disabled: disabled || busy || uploading(),
                    onchange: move |event: FormEvent| upload.call(attachment_files(event.files())),
                }
            }
            if accepts_files && file_over() {
                div { class: "composer-drop-label", "Drop images here" }
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
            model: None,
            name: None,
            title: None,
            agent: None,
            state: "working".into(),
            cwd: None,
            repository: None,
            blocked: None,
            reply: None,
            launch: None,
        }
    }

    #[test]
    fn close_is_enabled_for_working_and_blocked_agents_but_not_unavailable_or_pending() {
        for state in ["working", "blocked"] {
            let mut agent = metadata_agent();
            agent.state = state.into();
            let metadata = AgentMetadata::Present(agent);
            assert!(!close_disabled(false, &metadata));
            assert!(close_disabled(true, &metadata));
        }
        for metadata in [
            AgentMetadata::Loading,
            AgentMetadata::Missing,
            AgentMetadata::Unreachable {
                message: "offline".into(),
                agent: None,
            },
            AgentMetadata::Unreachable {
                message: "offline".into(),
                agent: Some(metadata_agent()),
            },
        ] {
            assert!(close_disabled(false, &metadata));
        }
    }

    fn close_inbox(address: &wire::AgentAddress) -> crate::app::InboxSelection {
        crate::app::InboxSelection {
            host: address.host.clone(),
            pane: address.pane.clone(),
            kind: crate::app::InboxKind::Prompt,
            id: "request".into(),
        }
    }

    #[test]
    fn close_success_clears_only_matching_host_and_pane_selections() {
        let requested = wire::AgentAddress::local("pane");
        let other_host = wire::AgentAddress {
            host: "remote".into(),
            pane: "pane".into(),
        };
        let other_pane = wire::AgentAddress::local("other-pane");
        for current in [
            None,
            Some(requested.clone()),
            Some(other_host),
            Some(other_pane),
        ] {
            for inbox_address in [None, Some(requested.clone()), current.clone()] {
                let mut selected = current.clone();
                let mut inbox = inbox_address.as_ref().map(close_inbox);
                let original_inbox = inbox.clone();
                assert_eq!(
                    complete_close(&requested, Ok(()), &mut selected, &mut inbox),
                    None
                );
                assert_eq!(
                    selected,
                    current.clone().filter(|address| address != &requested)
                );
                assert_eq!(
                    inbox,
                    original_inbox
                        .filter(|item| item.host != requested.host || item.pane != requested.pane)
                );
            }
        }
    }

    #[test]
    fn close_failure_retains_the_view_and_inbox_and_returns_the_action_error() {
        let requested = wire::AgentAddress::local("pane");
        let mut selected = Some(requested.clone());
        let mut inbox = Some(close_inbox(&requested));
        let original_inbox = inbox.clone();
        assert_eq!(
            complete_close(
                &requested,
                Err("confirm worktree group".into()),
                &mut selected,
                &mut inbox
            ),
            Some("confirm worktree group".into())
        );
        assert_eq!(selected, Some(requested));
        assert_eq!(inbox, original_inbox);
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
            pending: None,
            revisions: Some(wire::DetailRevisions {
                conversation,
                screen,
                files: 0,
            }),
            session: wire::DetailSession::Available("session".into()),
            kind: "pane.agent_status_changed".into(),
            pane: "pane".into(),
            state: Some(state.into()),
            blocked: None,
        }
    }

    fn parts(conversation: bool, screen: bool, files: bool) -> ChangedParts {
        ChangedParts {
            conversation,
            screen,
            files,
        }
    }

    #[test]
    fn settings_are_read_on_session_or_state_changes_not_every_working_event() {
        let working = event(4, 8, "working");
        let changed = parts(true, false, false);
        let unchanged = parts(false, false, false);
        assert!(
            settings_due(None, &working, &unchanged, false),
            "first event"
        );
        assert!(
            settings_due(Some(&working), &working, &unchanged, true),
            "new session"
        );
        assert!(
            !settings_due(Some(&working), &working, &changed, false),
            "a streamed transcript change while working"
        );
        let idle = event(5, 8, "idle");
        assert!(
            settings_due(Some(&working), &idle, &unchanged, false),
            "turn ended"
        );
        assert!(
            settings_due(Some(&idle), &idle, &changed, false),
            "a model change at the prompt lands in the transcript"
        );
        assert!(
            !settings_due(Some(&idle), &idle, &unchanged, false),
            "nothing changed"
        );
    }

    #[test]
    fn first_event_reads_the_conversation_and_only_a_blocked_screen() {
        assert_eq!(
            changed_parts(None, &event(4, 8, "working"), false),
            parts(true, false, true)
        );
        assert_eq!(
            changed_parts(None, &event(4, 8, "blocked"), true),
            parts(true, true, true)
        );
    }

    #[test]
    fn pending_parts_are_read_once_as_each_becomes_available() {
        let mut event = event(4, 8, "blocked");
        event.pending = Some(wire::PendingParts {
            conversation: true,
            screen: true,
            files: true,
        });
        assert_eq!(
            changed_parts(None, &event, false),
            parts(false, false, false)
        );
        let held = event.revisions.clone();
        if let Some(pending) = event.pending.as_mut() {
            pending.conversation = false;
        }
        if let Some(revisions) = event.revisions.as_mut() {
            revisions.conversation += 1;
        }
        assert_eq!(
            changed_parts(held.as_ref(), &event, true),
            parts(true, false, false)
        );
        let held = event.revisions.clone();
        assert_eq!(
            changed_parts(held.as_ref(), &event, true),
            parts(false, false, false)
        );
        if let Some(pending) = event.pending.as_mut() {
            pending.files = false;
        }
        if let Some(revisions) = event.revisions.as_mut() {
            revisions.files += 1;
        }
        assert_eq!(
            changed_parts(held.as_ref(), &event, true),
            parts(false, false, true)
        );
        let held = event.revisions.clone();
        if let Some(pending) = event.pending.as_mut() {
            pending.screen = false;
        }
        if let Some(revisions) = event.revisions.as_mut() {
            revisions.screen += 1;
        }
        assert_eq!(
            changed_parts(held.as_ref(), &event, true),
            parts(false, true, false)
        );
        assert_eq!(
            changed_parts(event.revisions.as_ref(), &event, true),
            parts(false, false, false)
        );
    }

    #[test]
    fn pending_screen_is_not_read_when_becoming_blocked() {
        let mut event = event(4, 8, "blocked");
        event.pending = Some(wire::PendingParts {
            conversation: false,
            screen: true,
            files: false,
        });
        assert_eq!(
            changed_parts(event.revisions.as_ref(), &event, false),
            parts(false, false, false)
        );
    }

    #[test]
    fn pending_parts_are_skipped_even_without_part_revisions() {
        let mut event = event(4, 8, "blocked");
        event.revisions = None;
        event.pending = Some(wire::PendingParts {
            conversation: true,
            screen: true,
            files: true,
        });
        assert_eq!(
            changed_parts(None, &event, true),
            parts(false, false, false)
        );
    }

    #[test]
    fn unchanged_revisions_make_no_requests() {
        let event = event(4, 8, "blocked");
        assert_eq!(
            changed_parts(event.revisions.as_ref(), &event, true),
            parts(false, false, false)
        );
    }

    #[test]
    fn screen_only_change_reads_only_a_blocked_screen() {
        let held = event(4, 8, "blocked").revisions;
        assert_eq!(
            changed_parts(held.as_ref(), &event(4, 9, "blocked"), true),
            parts(false, true, false)
        );
        assert_eq!(
            changed_parts(held.as_ref(), &event(4, 9, "working"), false),
            parts(false, false, false)
        );
        assert_eq!(
            changed_parts(held.as_ref(), &event(4, 9, "working"), true),
            parts(false, false, false)
        );
    }

    #[test]
    fn conversation_change_reads_only_the_conversation() {
        let held = event(4, 8, "blocked").revisions;
        assert_eq!(
            changed_parts(held.as_ref(), &event(5, 8, "blocked"), true),
            parts(true, false, false)
        );
        assert_eq!(
            changed_parts(held.as_ref(), &event(5, 8, "working"), false),
            parts(true, false, false)
        );
    }

    #[test]
    fn becoming_blocked_reads_the_screen_even_without_a_new_revision() {
        let mut event = event(4, 8, "blocked");
        assert_eq!(
            changed_parts(event.revisions.as_ref(), &event, false),
            parts(false, true, false)
        );
        event.state = Some("idle".into());
        event.blocked = Some(BlockedPrompt {
            question: "Allow?".into(),
            options: Vec::new(),
        });
        assert_eq!(
            changed_parts(event.revisions.as_ref(), &event, false),
            parts(false, true, false)
        );
        event.state = None;
        event.blocked = None;
        assert_eq!(
            changed_parts(event.revisions.as_ref(), &event, true),
            parts(false, false, false)
        );
    }

    #[test]
    fn a_files_only_revision_change_reads_only_files() {
        let mut event = event(4, 8, "blocked");
        let held = event.revisions.clone();
        if let Some(revisions) = event.revisions.as_mut() {
            revisions.files += 1;
        }
        assert_eq!(
            changed_parts(held.as_ref(), &event, true),
            parts(false, false, true)
        );
    }

    #[test]
    fn an_older_peer_invalidates_all_three_parts_on_every_event() {
        let mut event = event(4, 8, "working");
        let held = event.revisions.take();
        assert_eq!(
            changed_parts(held.as_ref(), &event, false),
            parts(true, true, true)
        );
        assert_eq!(changed_parts(None, &event, false), parts(true, true, true));
    }

    #[test]
    fn reconnect_clears_revisions_and_reads_each_eligible_part_again() {
        let event = event(4, 8, "blocked");
        assert_eq!(
            changed_parts(event.revisions.as_ref(), &event, true),
            parts(false, false, false)
        );
        assert_eq!(changed_parts(None, &event, true), parts(true, true, true));
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
            model: None,
            name: None,
            title: None,
            agent: None,
            state: "idle".to_owned(),
            cwd: None,
            repository: None,
            blocked: None,
            reply: None,
            launch: None,
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
            host: "local".into(),
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

    fn said(text: &str, source: wire::Source) -> Turn {
        Turn {
            id: text.into(),
            role: Role::User,
            text: text.into(),
            source,
            timestamp: None,
            tool: None,
            end_turn: None,
        }
    }

    #[test]
    fn a_sent_message_stays_in_the_outbox_until_the_transcript_records_it() {
        let pi = wire::Source::Harness {
            harness: "pi".into(),
            model: None,
            provider: None,
        };
        let outgoing = |id, text: &str| Outgoing {
            id,
            text: text.into(),
            delivery: Delivery::Queued,
            after: Some("earlier".into()),
        };
        // The owner said "ok" before; two new "ok" messages are both still on their way.
        let mut earlier = said("ok", pi.clone());
        earlier.id = "earlier".into();
        let outbox = vec![outgoing(0, "ok"), outgoing(1, "ok")];
        let mut turns = vec![earlier];
        assert_eq!(undelivered(&outbox, &turns).len(), 2);
        // A Dogsled delivery with the same words is not the owner's message.
        turns.push(said(
            "ok",
            wire::Source::Dogsled {
                sender: "worker".into(),
                delivery_kind: "message".into(),
            },
        ));
        assert_eq!(undelivered(&outbox, &turns).len(), 2);
        // Pi records the first one, with the images it was sent with.
        turns.push(said("ok\n\nAttached image: /tmp/a.png", pi.clone()));
        assert_eq!(undelivered(&outbox, &turns), vec![outgoing(1, "ok")]);
        turns.push(said("ok", pi));
        assert!(undelivered(&outbox, &turns).is_empty());
        // The window slides past the record the messages were sent after.
        turns.remove(0);
        assert!(undelivered(&outbox, &turns).is_empty());
    }
}
