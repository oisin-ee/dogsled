//! The selected agent's screen: its pending request, or its conversation.

use dioxus::prelude::*;
use gloo_net::http::Request;
use portal_types::agents::{self as wire, BlockedPrompt, Role, Turn};
use portal_types::questions::{Pending, PendingList};
use portal_types::reviews::{ReviewList, ReviewSummary};
use portal_types::{BoardView, TodoStatusView, TodoView};
use serde::de::DeserializeOwned;

use crate::api;
use crate::question_form::QuestionForm;
use crate::review_view::ReviewView;

const AGENT_CSS: Asset = asset!("/assets/agent_view.css");
const POLL_MS: u32 = 5000;
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
    Todos,
    Branch,
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

    fn label(&self) -> String {
        match self {
            Self::Review(_) => "Plan review".to_owned(),
            Self::Questions(_, count) => format!("Questions {count}"),
            Self::Prompt => "Prompt".to_owned(),
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
) -> Element {
    rsx! {
        AgentSession { key: "{pane}", pane, review, question }
        document::Stylesheet { href: AGENT_CSS }
    }
}

#[component]
fn AgentSession(pane: String, review: Option<String>, question: Option<String>) -> Element {
    let mut agent = use_signal(|| None::<wire::AgentView>);
    let mut board = use_signal(|| None::<BoardView>);
    let mut reviews = use_signal(|| None::<Vec<ReviewSummary>>);
    let mut questions = use_signal(|| None::<Vec<Pending>>);
    let mut turns = use_signal(Vec::<Turn>::new);
    let mut screen = use_signal(|| None::<String>);
    let mut loading = use_signal(|| true);
    let mut reachable = use_signal(|| false);
    let mut read_error = use_signal(|| None::<String>);
    let mut pending_error = use_signal(|| None::<String>);
    let mut board_error = use_signal(|| None::<String>);
    let mut stream_error = use_signal(|| None::<String>);
    let mut action_error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let mut refresh = use_signal(|| 0_u64);
    // A choice belongs to the request that was visible when it was made.
    // A new request opens in front; polling the same request preserves the tab.
    let mut choice = use_signal(|| None::<(String, Tab)>);
    let local_draft = use_signal(String::new);
    let mut draft = try_use_context::<Draft>().map_or(local_draft, |draft| draft.0);

    let read_pane = pane.clone();
    use_resource(move || {
        let _ = refresh();
        let pane = read_pane.clone();
        async move {
            let result = async {
                let current = match api::fetch_agent(&pane).await {
                    Ok(current) => {
                        reachable.set(true);
                        current
                    }
                    Err(message) => {
                        reachable.set(false);
                        return Err(message);
                    }
                };
                let Some(current) = current else {
                    agent.set(None);
                    turns.set(Vec::new());
                    screen.set(None);
                    return Err(
                        "This agent is no longer available. Select another agent.".to_owned()
                    );
                };
                let blocked = is_blocked(&current);
                agent.set(Some(current));
                let conversation = api::fetch_conversation(&pane).await;
                // A failed transcript must not hide the prompt needed to reply.
                let screen_result = if blocked {
                    api::fetch_screen(&pane).await.map(|value| Some(value.text))
                } else {
                    Ok(None)
                };
                match screen_result.as_ref() {
                    Ok(value) => screen.set(value.clone()),
                    Err(_) => screen.set(None),
                }
                let conversation = conversation?;
                if *turns.peek() != conversation.turns {
                    turns.set(conversation.turns);
                }
                screen_result.map(|_| ())
            }
            .await;
            read_error.set(result.err());
            loading.set(false);
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

    use_future(move || async move {
        loop {
            let mut timer = document::eval(&format!(
                "await new Promise(resolve => setTimeout(resolve, {POLL_MS})); dioxus.send(true);"
            ));
            if timer.recv::<bool>().await.is_err() {
                read_error.set(Some(
                    "Live refresh stopped. Reload the page to reconnect.".to_owned(),
                ));
                break;
            }
            refresh += 1;
        }
    });

    let send_pane = pane.clone();
    let send = move |prompt_reply: bool| {
        let pane = send_pane.clone();
        let text = draft();
        if busy() || !reachable() || text.trim().is_empty() {
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
                        "The agent needs a response. Open the Prompt tab to reply.".to_owned()
                    );
                }
                api::send_prompt(&pane, &text).await
            }
            .await;
            match result {
                Ok(()) => {
                    if draft() == text {
                        draft.set(String::new());
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
    let todo_count: usize = todos
        .iter()
        .flat_map(|todo| &todo.sections)
        .map(|section| section.count)
        .sum();
    let title = todos
        .iter()
        .find_map(|todo| nonempty(&todo.current))
        .or_else(|| {
            current
                .as_ref()
                .and_then(|agent| agent.name.as_deref())
                .and_then(nonempty)
        })
        .unwrap_or(&pane);
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
    let disabled = busy() || loading() || !reachable() || current.is_none();
    let waiting = pending.is_some();
    let plan_open = tab == Tab::Pending && matches!(pending, Some(RequestTab::Review(_)));
    let focus_pane = pane.clone();
    let pending_identity = identity.clone();
    let conversation_identity = identity.clone();
    let todos_identity = identity.clone();
    let branch_identity = identity.clone();

    rsx! {
        section { class: "av", aria_label: "Selected agent",
            header { class: "av-header",
                div { class: "av-identity",
                    h2 { "{title}" }
                    div { class: "av-meta",
                        span { class: if waiting { "av-state waiting" } else { "av-state" }, "{state}" }
                        span { if row.is_some() { "worker" } else { "lead" } }
                        if let Some(harness) = harness { span { "{harness}" } }
                        if let Some(row) = row { span { "{row.host}" } }
                        if let Some(branch) = branch { span { class: "av-mono", "{branch}" } }
                    }
                }
                div { class: "av-actions",
                    if !plan_open {
                        button { r#type: "button", disabled,
                            onclick: move |_| interrupt("ctrl+c".to_owned()), "Interrupt" }
                    }
                    button {
                        r#type: "button", disabled,
                        onclick: move |_| {
                            let pane = focus_pane.clone();
                            async move {
                                if busy() || !reachable() { return; }
                                busy.set(true);
                                action_error.set(api::focus_agent(&pane).await.err());
                                busy.set(false);
                            }
                        },
                        "Open in Herdr"
                    }
                }
                nav { class: "av-tabs", aria_label: "Agent tabs",
                    if let Some(pending) = &pending {
                        button { r#type: "button", class: if tab == Tab::Pending { "active" } else { "" },
                            aria_current: if tab == Tab::Pending { "page" } else { "false" },
                            onclick: move |_| choice.set(Some((pending_identity.clone(), Tab::Pending))),
                            span { class: "av-pending-dot", aria_hidden: "true" }
                            "{pending.label()}"
                        }
                    }
                    button { r#type: "button", class: if tab == Tab::Conversation { "active" } else { "" },
                        aria_current: if tab == Tab::Conversation { "page" } else { "false" },
                        onclick: move |_| choice.set(Some((conversation_identity.clone(), Tab::Conversation))), "Conversation" }
                    button { r#type: "button", class: if tab == Tab::Todos { "active" } else { "" },
                        aria_current: if tab == Tab::Todos { "page" } else { "false" },
                        onclick: move |_| choice.set(Some((todos_identity.clone(), Tab::Todos))), "Todos {todo_count}" }
                    button { r#type: "button", class: if tab == Tab::Branch { "active" } else { "" },
                        aria_current: if tab == Tab::Branch { "page" } else { "false" },
                        onclick: move |_| choice.set(Some((branch_identity.clone(), Tab::Branch))), "Branch" }
                }
            }
            if !loading() && !reachable() {
                p { class: "av-notice", role: "status", "Herdr is not reachable. The board still works; conversation and messages do not." }
            }
            if let Some(message) = read_error() { p { class: "error av-notice", role: "alert", "{message}" } }
            if let Some(message) = pending_error() { p { class: "error av-notice", role: "alert", "{message}" } }
            if let Some(message) = board_error() { p { class: "error av-notice", role: "alert", "{message}" } }
            if stream_error().is_some() {
                p { class: "av-notice", role: "status", "Live updates paused. Checking every 5 seconds." }
            }
            if let Some(message) = action_error() { p { class: "error av-notice", role: "alert", "{message}" } }
            match tab {
                Tab::Pending => match pending {
                    Some(RequestTab::Review(id)) => rsx! {
                        div { class: "av-pending av-review", ReviewView { review_id: id, embedded: true } }
                    },
                    Some(RequestTab::Questions(id, _)) => rsx! {
                        div { class: "av-pending", QuestionForm { requested: id } }
                    },
                    Some(RequestTab::Prompt) => rsx! {
                        BlockedPanel {
                            prompt: current.as_ref().and_then(|agent| agent.blocked.clone()), screen: screen(),
                            disabled, on_key: send_key, draft,
                            on_send: move |_| reply_send(true),
                        }
                    },
                    None => rsx! {},
                },
                Tab::Conversation => rsx! {
                    div { class: "av-chat",
                        Conversation { turns: turns(), loading: loading(), working: current.as_ref().is_some_and(|agent| agent.state == "working") && reachable() }
                        Composer { draft, disabled, busy: busy(), on_send: move |_| conversation_send(false) }
                    }
                },
                Tab::Todos => rsx! { Todos { todos } },
                Tab::Branch => rsx! {
                    section { class: "av-details", aria_label: "Branch",
                        h3 { "Branch" }
                        if let Some(branch) = branch { p { class: "av-mono", "{branch}" } }
                        else { p { class: "av-muted", "No branch is recorded for this agent." } }
                        if let Some(row) = row {
                            if !row.activity.is_empty() { p { "{row.activity}" } }
                            if let Some(ahead) = row.ahead { p { "{ahead} commits ahead" } }
                            if row.dirty { p { class: "av-notice", "The worktree has uncommitted changes." } }
                        }
                    }
                },
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
fn Conversation(turns: Vec<Turn>, loading: bool, working: bool) -> Element {
    rsx! {
        section {
            id: "agent-conversation", class: "av-conversation", aria_label: "Conversation", tabindex: "0",
            onmounted: move |_| async move {
                let _ = document::eval(
                    "const el = document.getElementById('agent-conversation');
                     if (el) {
                       el.dataset.follow = 'true';
                       el.scrollTop = el.scrollHeight;
                       el.addEventListener('scroll', () => {
                         el.dataset.follow = String(el.scrollHeight - el.scrollTop - el.clientHeight < 48);
                       });
                       const observer = new MutationObserver(() => {
                         if (!el.isConnected) { observer.disconnect(); return; }
                         if (el.dataset.follow !== 'false') el.scrollTop = el.scrollHeight;
                       });
                       observer.observe(el, {childList: true, subtree: true, characterData: true});
                     }"
                ).await;
            },
            div { class: "av-turns",
                if loading { p { class: "av-muted", role: "status", "Loading conversation…" } }
                else if turns.is_empty() { p { class: "av-muted", "No conversation yet. Send a message to start." } }
                for (index, turn) in turns.iter().enumerate() {
                    article { key: "{index}", class: match turn.role { Role::User => "av-turn av-user", Role::Assistant => "av-turn av-assistant" },
                        h3 { match turn.role { Role::User => "You", Role::Assistant => "Agent" } }
                        div { class: "av-text", dangerous_inner_html: crate::review_view::render_markdown(&turn.text) }
                    }
                }
                if working { p { class: "av-live", role: "status", "Working…" } }
            }
        }
    }
}

#[component]
fn Composer(
    draft: Signal<String>,
    disabled: bool,
    busy: bool,
    on_send: EventHandler<()>,
) -> Element {
    rsx! {
        form { class: "av-compose", onsubmit: move |event| { event.prevent_default(); on_send.call(()); },
            div { class: "av-compose-box",
                textarea {
                    name: "message", aria_label: "Message this agent", rows: "3", value: "{draft}", disabled,
                    placeholder: "Message this agent",
                    oninput: move |event: FormEvent| draft.set(event.value()),
                    onkeydown: move |event: KeyboardEvent| {
                        if event.key() == Key::Enter && !event.modifiers().shift() && !event.is_composing() {
                            event.prevent_default(); on_send.call(());
                        }
                    },
                }
                div { class: "av-compose-footer",
                    span { class: "av-muted", "Enter sends · Shift+Enter adds a line · while it works, the message waits for the next turn" }
                    button { r#type: "submit", class: "av-send", disabled: disabled || draft().trim().is_empty(),
                        if busy { "Sending…" } else { "Send" }
                    }
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
    rsx! {
        section { class: "av-blocked", aria_label: "Blocked agent",
            div { class: "av-blocked-box",
                h3 { "{title}" }
                match screen {
                    Some(text) => rsx! {
                        pre { class: "av-screen", tabindex: "0", aria_label: "Agent screen",
                            onkeydown: move |event: KeyboardEvent| {
                                if disabled || event.is_composing() { return; }
                                let key = match event.key() { Key::Escape => "esc", Key::Enter => "enter", Key::ArrowUp => "up", Key::ArrowDown => "down", _ => return };
                                event.prevent_default(); on_key.call(key.to_owned());
                            },
                            "{text}"
                        }
                    },
                    None => rsx! { p { class: "av-muted", role: "status", "Agent screen is not available yet." } },
                }
                div { class: "av-keys", aria_label: "Prompt options and terminal keys",
                    if let Some(prompt) = prompt {
                        for option in prompt.options {
                            button { key: "{option.key}", r#type: "button", disabled: disabled || !allowed_key(&option.key),
                                onclick: move |_| on_key.call(option.key.clone()), "{option.key} · {option.label}" }
                        }
                    }
                    for &(key, label) in PANEL_KEYS {
                        button { key: "terminal-{key}", r#type: "button", disabled,
                            aria_label: match key { "up" => "Arrow up", "down" => "Arrow down", _ => label },
                            onclick: move |_| on_key.call(key.to_owned()), "{label}" }
                    }
                }
                form { class: "av-reply", onsubmit: move |event| { event.prevent_default(); on_send.call(()); },
                    input { r#type: "text", name: "reply", aria_label: "Reply to the prompt", placeholder: "Type a reply and press Enter",
                        value: "{draft}", disabled, oninput: move |event: FormEvent| draft.set(event.value()) }
                    button { r#type: "submit", disabled: disabled || draft().trim().is_empty(), "Send" }
                }
                p { class: "av-muted", "Buttons are read from the agent's screen. Open in Herdr when none fits." }
            }
        }
    }
}

#[component]
fn Todos(todos: Vec<TodoView>) -> Element {
    rsx! {
        section { class: "av-details", aria_label: "Agent todos",
            h3 { "Todos" }
            if todos.is_empty() { p { class: "av-muted", "No todos are recorded for this agent." } }
            for todo in todos {
                div { key: "{todo.session}", class: "av-todo",
                    h4 { "{todo.current}" }
                    for section in todo.sections {
                        section { class: "av-todo-section",
                            h5 { "{status_label(section.status)} · {section.count}" }
                            ul { role: "list",
                                for item in section.items {
                                    li { class: status_class(item.status),
                                        "{item.text}"
                                        if let Some(description) = item.description { p { class: "av-muted", "{description}" } }
                                        if !item.children.is_empty() {
                                            ul { role: "list",
                                                for child in item.children {
                                                    li { class: status_class(child.status), "{child.text}"
                                                        if let Some(description) = child.description { p { class: "av-muted", "{description}" } }
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
        TodoStatusView::InProgress => "av-todo-active",
        TodoStatusView::Completed => "av-todo-completed",
        TodoStatusView::Pending | TodoStatusView::Abandoned => "av-muted",
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

fn allowed_key(key: &str) -> bool {
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
