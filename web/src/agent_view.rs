//! The selected agent's screen: its pending request, or its conversation.

use std::rc::Rc;
use std::time::Duration;

use dioxus::html::geometry::PixelsVector2D;
use dioxus::prelude::*;
use dioxus::web::WebEventExt;
use dioxus_icons::lucide::{ArrowUp, ExternalLink, Paperclip, Square, X};
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::scroll_area::ScrollDirection;
use dioxus_sdk_time::use_interval;
use gloo_net::http::Request;
use js_sys::wasm_bindgen::JsCast;
use portal_types::agents::{self as wire, BlockedPrompt, Role, Turn};
use portal_types::questions::{Pending, PendingList};
use portal_types::reviews::{ReviewList, ReviewSummary};
use portal_types::{BoardView, TodoStatusView, TodoView};
use serde::de::DeserializeOwned;

use crate::api;
use crate::components::{
    accordion::{Accordion, AccordionContent, AccordionItem, AccordionTrigger},
    button::{Button, ButtonVariant},
    input::Input,
    item::{Item, ItemContent, ItemDescription, ItemGroup, ItemMedia, ItemSize, ItemTitle},
    scroll_area::ScrollArea,
    skeleton::Skeleton,
    tabs::{TabContent, TabList, TabTrigger},
    textarea::Textarea,
};
use crate::conversations::{Conversations, use_conversation};
use crate::notice::{Alert, Status};
use crate::question_form::QuestionForm;
use crate::review_view::ReviewView;

/// The time between two refreshes of an open agent.
const POLL: Duration = Duration::from_secs(5);
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
    pane: String,
    #[props(default)] review: Option<String>,
    #[props(default)] question: Option<String>,
    agent_count: usize,
) -> Element {
    rsx! {
        AgentSession { key: "{pane}", pane, review, question, agent_count }
    }
}

#[component]
fn AgentSession(
    pane: String,
    review: Option<String>,
    question: Option<String>,
    agent_count: usize,
) -> Element {
    let mut agent = use_signal(|| None::<wire::AgentView>);
    let mut board = use_signal(|| None::<BoardView>);
    let mut reviews = use_signal(|| None::<Vec<ReviewSummary>>);
    let mut questions = use_signal(|| None::<Vec<Pending>>);
    let mut selection = use_context::<Signal<Option<crate::app::InboxSelection>>>();
    let conversations = use_context::<Signal<Conversations>>();
    let mut screen = use_signal(|| None::<String>);
    let mut metadata_loading = use_signal(|| true);
    let mut reachable = use_signal(|| false);
    let mut metadata_error = use_signal(|| None::<String>);
    let mut screen_error = use_signal(|| None::<String>);
    let mut pending_error = use_signal(|| None::<String>);
    let mut board_error = use_signal(|| None::<String>);
    let mut stream_error = use_signal(|| None::<String>);
    let mut action_error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let mut refresh = use_signal(|| 0_u64);
    // A choice belongs to the request that was visible when it was made.
    // A new request opens in front; polling the same request preserves the tab.
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
    let transcript = use_conversation(pane.clone(), refresh);

    let read_pane = pane.clone();
    use_resource(move || {
        let _ = refresh();
        let pane = read_pane.clone();
        async move {
            match api::fetch_agent(&pane).await {
                Ok(current) => {
                    reachable.set(true);
                    metadata_error.set(current.is_none().then(|| {
                        "This agent is no longer available. Select another agent.".to_owned()
                    }));
                    agent.set(current);
                }
                Err(message) => {
                    reachable.set(false);
                    metadata_error.set(Some(message));
                }
            }
            metadata_loading.set(false);
        }
    });

    let screen_pane = pane.clone();
    use_resource(move || {
        let _ = refresh();
        let blocked = agent.read().as_ref().is_some_and(is_blocked);
        let pane = screen_pane.clone();
        async move {
            // A failed or pending transcript must not hide the prompt needed to reply.
            let result = if blocked {
                api::fetch_screen(&pane).await.map(|value| Some(value.text))
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

    use_resource(move || {
        let _ = refresh();
        async move {
            match api::fetch_board().await {
                Ok(value) => {
                    board.set(Some(value));
                    board_error.set(None);
                }
                Err(message) => board_error.set(Some(message)),
            }
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

    let event_pane = pane.clone();
    use_future(move || {
        let pane = event_pane.clone();
        async move {
            let result = api::watch_agent(&pane, |event| {
                if let Some(current) = agent.write().as_mut() {
                    if let Some(state) = event.state {
                        current.state = state;
                        current.blocked = event.blocked;
                    } else if event.blocked.is_some() {
                        current.blocked = event.blocked;
                    }
                }
                refresh += 1;
            })
            .await;
            stream_error.set(result.err());
        }
    });

    use_interval(POLL, move |()| refresh += 1);

    let send_pane = pane.clone();
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
            let result = async {
                let current = api::fetch_agent(&pane)
                    .await?
                    .ok_or_else(|| "This agent is no longer available.".to_owned())?;
                let blocked = is_blocked(&current);
                agent.set(Some(current));
                if blocked && !prompt_reply {
                    return Err(
                        "The agent needs a response. Select its prompt in Waiting on you to reply."
                            .to_owned(),
                    );
                }
                api::send_prompt(&pane, &message).await
            }
            .await;
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
            refresh += 1;
            busy.set(false);
        });
    };

    let mut reply_send = send.clone();
    let mut conversation_send = send;
    let key_pane = pane.clone();
    let send_key = move |key: String| {
        let pane = key_pane.clone();
        if busy() || !reachable() || !allowed_key(&key) {
            return;
        }
        busy.set(true);
        action_error.set(None);
        spawn(async move {
            action_error.set(api::send_key(&pane, &key).await.err());
            refresh += 1;
            busy.set(false);
        });
    };
    let mut interrupt = send_key.clone();
    let current = agent();
    let retained = conversations.read().get(&pane).cloned();
    let transcript_loading = retained.is_none() && transcript.read().is_none();
    let transcript_error = transcript
        .read()
        .as_ref()
        .and_then(|result| result.as_ref().err().cloned());
    let board_view = board();
    let row = board_view.as_ref().and_then(|board| {
        board
            .repos
            .iter()
            .flat_map(|repo| &repo.rows)
            .find(|row| row.pane.as_deref() == Some(&pane))
    });
    let todos: Vec<TodoView> = board_view.as_ref().map_or_else(Vec::new, |board| {
        board
            .repos
            .iter()
            .flat_map(|repo| &repo.todos)
            .filter(|todo| todo.pane == pane)
            .cloned()
            .collect()
    });
    let workers: Vec<(String, String)> = board_view.as_ref().map_or_else(Vec::new, |board| {
        board
            .repos
            .iter()
            .flat_map(|repo| &repo.rows)
            .map(|row| (row.id.clone(), format!("{} · {}", row.what, row.host)))
            .collect()
    });
    let todo_count: usize = todos
        .iter()
        .flat_map(|todo| &todo.sections)
        .map(|section| section.count)
        .sum();
    let todo_title = todos.iter().find_map(|todo| nonempty(&todo.current));
    let title = match current.as_ref() {
        Some(agent) => crate::board::agent_title(todo_title, agent),
        None => todo_title.unwrap_or(&pane).to_owned(),
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
    let state = pending
        .as_ref()
        .map(RequestTab::state)
        .or_else(|| current.as_ref().map(|agent| agent.state.as_str()))
        .unwrap_or("Loading…")
        .to_owned();
    let disabled = busy() || metadata_loading() || !reachable() || current.is_none();
    let plan_open = tab == Tab::Pending && matches!(pending, Some(RequestTab::Review(_)));
    let focus_pane = pane.clone();
    let mut panel = use_context::<Signal<crate::app::LeadPanel>>();
    use_effect(use_reactive(&identity, move |identity| {
        if !identity.is_empty() {
            panel.set(crate::app::LeadPanel::Chat);
        }
    }));
    let width = use_context::<crate::app::ConsoleWidth>().0;
    let first_tab = usize::from(width() < 1100);
    let completed: usize = todos
        .iter()
        .flat_map(|todo| &todo.sections)
        .filter(|section| section.status == TodoStatusView::Completed)
        .map(|section| section.count)
        .sum();
    let role = if row.is_some() { "worker" } else { "lead" };
    let host = row.map(|row| row.host.as_str());
    let working = current
        .as_ref()
        .is_some_and(|agent| agent.state == "working")
        && reachable();
    let mut page = use_signal(|| None::<Rc<MountedData>>);
    let mut following = use_signal(|| true);
    let mut follow_error = use_signal(|| None::<String>);
    // Follow the newest turn until the owner scrolls up to read; scrolling back
    // to the end resumes it. The page region is the one that scrolls.
    use_effect(use_reactive(
        (&retained, &transcript_loading, &working, &tab),
        move |_| {
            if tab != Tab::Conversation || !following() {
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
                            QuestionForm { requested: id.clone(), on_done: move |id: String| {
                                if let Some(pending) = questions.write().as_mut() { pending.retain(|item| item.id != id); }
                                if selection.peek().as_ref().is_some_and(|item| item.kind == crate::app::InboxKind::Question && item.id == id) { selection.set(None); }
                                refresh += 1;
                            } }
                        },
                        Some(RequestTab::Prompt) => rsx! {
                            BlockedPanel { prompt: current.as_ref().and_then(|agent| agent.blocked.clone()), screen: screen(),
                                disabled, on_key: send_key, draft, on_send: move |_| reply_send(true) }
                        },
                        _ => rsx! {},
                    }
                } else {
                    ScrollArea { class: "conversation-scroll", direction: ScrollDirection::Vertical, attributes: tracking,
                        Conversation { available: retained.is_some(), turns: retained.unwrap_or_default(), loading: transcript_loading, working }
                    }
                }
            if tab == Tab::Conversation {
                Composer { draft, images, uploading, disabled, busy: busy(), on_error: move |message| action_error.set(Some(message)),
                    on_send: { let mut send = conversation_send.clone(); move |_| send(false) } }
            }
        }
    };

    rsx! {
        section { class: "lead-panel", "data-review-open": plan_open,
            if !plan_open {
            header { class: "lead-header",
                div { class: "lead-heading", h1 { "{title}" }
                    p { class: "lead-meta", "{state} · {role}"
                        if let Some(harness) = harness { " · {harness}" }
                        if let Some(host) = host { " · {host}" }
                        if let Some(branch) = branch { " · {branch}" }
                    }
                }
                if !plan_open && width() < 1400 {
                    div { class: "lead-tabs",
                        TabList { aria_label: "Lead view",
                            if width() < 1100 { TabTrigger { value: "agents", index: 0usize, "Agents {agent_count}" } }
                            TabTrigger { value: "chat", index: first_tab, "Chat" }
                            TabTrigger { value: "todos", index: first_tab + 1, "Todos {completed}/{todo_count}" }
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
                if !metadata_loading() && !reachable() {
                    Status { "Herdr is not reachable. The board still works; conversation and messages do not." }
                }
                if let Some(message) = metadata_error() { Alert { "{message}" } }
                if let Some(message) = screen_error() { Alert { "{message}" } }
                if let Some(message) = transcript_error { Alert { "{message}" } }
                if let Some(message) = pending_error() { Alert { "{message}" } }
                if let Some(message) = board_error() { Alert { "{message}" } }
                if stream_error().is_some() { Status { "Live updates paused. Checking every 5 seconds." } }
                if let Some(message) = action_error() { Alert { "{message}" } }
                if let Some(message) = follow_error() { Alert { "Could not follow the conversation: {message}" } }
            }
            if plan_open {
                div { class: "pending-review",
                    if let Some(RequestTab::Review(id)) = &pending {
                        ReviewView { review_id: id.clone(), embedded: true,
                            lead: format!("{title} · {role} · {}", current.as_ref().and_then(|agent| agent.repository.as_deref()).unwrap_or("Unknown repository")),
                            on_close: move |_| choice.set(Some((review_identity.clone(), Tab::Conversation))),
                        }
                    }
                }
            } else {
                if width() >= 1400 {
                    {chat.clone()}
                    aside { class: "console-todos", Todos { todos: todos.clone(), workers: workers.clone() } }
                } else {
                    TabContent { class: "chat-tab", value: "chat", index: first_tab, {chat.clone()} }
                    TabContent { class: "todos-tab", value: "todos", index: first_tab + 1,
                        div { class: "todo-tab-body", Todos { todos: todos.clone(), workers: workers.clone() } }
                        Composer { draft, images, uploading, disabled, busy: busy(), on_error: move |message| action_error.set(Some(message)),
                            on_send: move |_| conversation_send(false) }
                    }
                }
            }
        }
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

#[component]
fn Conversation(turns: Vec<Turn>, available: bool, loading: bool, working: bool) -> Element {
    rsx! {
        div { class: "conversation-turns",
            if loading { Skeleton { role: "status", aria_label: "Loading conversation" } }
            else if available && turns.is_empty() { Status { "No conversation yet. Send a message to start." } }
            for (index, turn) in turns.iter().enumerate() {
                article { key: "{index}", class: if turn.role == Role::User { "conversation-owner" } else { "conversation-agent" },
                    aria_label: if turn.role == Role::User { "You" } else { "Agent" },
                    div { dangerous_inner_html: crate::review_view::render_markdown(&turn.text) }
                }
            }
            if working { Status { "Working…" } }
        }
    }
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
                Textarea { name: "message", aria_label: "Message this agent", rows: "1", value: "{draft}", disabled: disabled || busy,
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
fn Todos(todos: Vec<TodoView>, workers: Vec<(String, String)>) -> Element {
    rsx! {
        section { class: "todo-panel", aria_label: "Agent todos",
            header { class: "console-column-header", h2 { "Todos" } }
            ScrollArea { class: "console-scroll todo-list", direction: ScrollDirection::Vertical,
                if todos.is_empty() { Status { "No todos are recorded for this agent." } }
                for todo in todos {
                    {
                        let mut sections = todo.sections;
                        sections.sort_by_key(|section| match section.status {
                            TodoStatusView::InProgress => 0, TodoStatusView::Pending => 1,
                            TodoStatusView::Completed => 2, TodoStatusView::Abandoned => 3,
                        });
                        rsx! {
                            Accordion { key: "{todo.session}", allow_multiple_open: true, horizontal: false,
                                for (position, section) in sections.into_iter().enumerate() {
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
                                                                    if let Some(worker) = item.unit.as_ref().and_then(|unit| workers.iter().find(|(id, _)| id == unit)) {
                                                                        ItemDescription { "{worker.1}" }
                                                                    }
                                                                    if let Some(description) = item.description { ItemDescription { "{description}" } }
                                                                    ItemGroup {
                                                                        for child in item.children {
                                                                            Item { class: "todo-item", "data-status": status_class(child.status), size: ItemSize::Sm,
                                                                                ItemMedia { span { class: "todo-indicator", aria_label: status_label(child.status) } }
                                                                                ItemContent {
                                                                                    ItemTitle { class: "todo-title", "{child.text}" }
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
