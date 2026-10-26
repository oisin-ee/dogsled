//! The selected agent's conversation and the controls that answer it.

use dioxus::prelude::*;
use portal_types::agents::{self as wire, BlockedPrompt, Role, Turn};

use crate::api;

const AGENT_CSS: Asset = asset!("/assets/agent_view.css");
const POLL_MS: u32 = 5000;

/// A message draft shared with the review view when its provider is present.
#[derive(Clone, Copy)]
pub struct Draft(pub Signal<String>);

/// These keys are a subset of the server's `ALLOWED_KEYS` in `web::agents`.
const PANEL_KEYS: &[(&str, &str)] = &[
    ("enter", "Enter"),
    ("esc", "Esc"),
    ("up", "Up"),
    ("down", "Down"),
    ("left", "Left"),
    ("right", "Right"),
    ("ctrl+c", "Ctrl+C"),
    ("y", "y"),
    ("n", "n"),
    ("1", "1"),
    ("2", "2"),
    ("3", "3"),
    ("4", "4"),
    ("5", "5"),
    ("6", "6"),
    ("7", "7"),
    ("8", "8"),
    ("9", "9"),
];

#[component]
pub fn AgentView(pane: String) -> Element {
    rsx! {
        // Selection owns this component's tasks and local draft. Switching
        // panes drops the old stream and cannot apply its replies to the new pane.
        AgentSession { key: "{pane}", pane }
        document::Stylesheet { href: AGENT_CSS }
    }
}

#[component]
fn AgentSession(pane: String) -> Element {
    let mut agent = use_signal(|| None::<wire::AgentView>);
    let mut turns = use_signal(Vec::<Turn>::new);
    let mut screen = use_signal(|| None::<String>);
    let mut loading = use_signal(|| true);
    let mut read_error = use_signal(|| None::<String>);
    let mut stream_error = use_signal(|| None::<String>);
    let mut action_error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let mut refresh = use_signal(|| 0_u64);
    let local_draft = use_signal(String::new);
    let mut draft = try_use_context::<Draft>().map_or(local_draft, |draft| draft.0);

    let read_pane = pane.clone();
    use_resource(move || {
        let _ = refresh();
        let pane = read_pane.clone();
        async move {
            let result = async {
                let Some(current) = api::fetch_agent(&pane).await? else {
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
                // A transcript failure must not hide the terminal needed to
                // answer a blocked agent. Refresh both reads independently.
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

    let event_pane = pane.clone();
    use_future(move || {
        let pane = event_pane.clone();
        async move {
            let result = api::watch_agent(&pane, |event| {
                // The list supplies the full blocked prompt; every event also
                // invalidates the conversation, even if the state is unchanged.
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

    let focus_pane = pane.clone();
    let send_pane = pane.clone();
    let mut send = move |_| {
        let pane = send_pane.clone();
        let text = draft();
        if busy() || text.trim().is_empty() {
            return;
        }
        busy.set(true);
        action_error.set(None);
        spawn(async move {
            // Recheck before posting: an agent may have become blocked since
            // the last event or poll. Keep the draft whenever no prompt is sent.
            let result = async {
                let current = api::fetch_agent(&pane)
                    .await?
                    .ok_or_else(|| "This agent is no longer available.".to_owned())?;
                let blocked = is_blocked(&current);
                agent.set(Some(current));
                if blocked {
                    return Err(
                        "The agent needs a key response. Use the blocked panel below.".to_owned(),
                    );
                }
                api::send_prompt(&pane, &text).await
            }
            .await;
            match result {
                Ok(()) => {
                    // A review quote or another edit can arrive while posting.
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
    let mut keyboard_send = send.clone();
    let current = agent();
    let blocked = current.as_ref().is_some_and(is_blocked);

    rsx! {
        section { class: "av", aria_label: "Selected agent",
            header { class: "av-header",
                div { class: "av-identity",
                    h2 { "{current.as_ref().and_then(|item| item.name.as_deref()).unwrap_or(&pane)}" }
                    div { class: "av-meta",
                        if let Some(kind) = current.as_ref().and_then(|item| item.agent.as_ref()) {
                            span { "{kind}" }
                        }
                        if let Some(current) = current.as_ref() {
                            span { class: if blocked { "pill av-state blocked" } else { "pill av-state" }, "{current.state}" }
                        }
                        span { "{pane}" }
                    }
                    if let Some(cwd) = current.as_ref().and_then(|item| item.cwd.as_ref()) {
                        p { class: "av-cwd", "{cwd}" }
                    }
                }
                button {
                    r#type: "button",
                    disabled: busy(),
                    onclick: move |_| {
                        let pane = focus_pane.clone();
                        async move {
                            if busy() {
                                return;
                            }
                            busy.set(true);
                            action_error.set(api::focus_agent(&pane).await.err());
                            busy.set(false);
                        }
                    },
                    "Open in Herdr"
                }
            }
            if let Some(message) = read_error() {
                p { class: "error", role: "alert", "{message}" }
            }
            if stream_error().is_some() {
                p { class: "av-notice", role: "status", "Live stream disconnected. Checking every 5 seconds." }
            }
            Conversation { turns: turns(), loading: loading() }
            if let Some(message) = action_error() {
                p { class: "error", role: "alert", "{message}" }
            }
            if blocked {
                BlockedPanel {
                    prompt: current.and_then(|item| item.blocked),
                    screen: screen(),
                    busy: busy(),
                    on_key: move |key: String| {
                        let pane = pane.clone();
                        if busy() {
                            return;
                        }
                        busy.set(true);
                        action_error.set(None);
                        spawn(async move {
                            action_error.set(api::send_key(&pane, &key).await.err());
                            refresh += 1;
                            busy.set(false);
                        });
                    },
                }
            } else {
                form {
                    class: "av-compose",
                    onsubmit: move |event: FormEvent| {
                        event.prevent_default();
                        send(());
                    },
                    label { r#for: "agent-message", "Message" }
                    textarea {
                        id: "agent-message",
                        name: "message",
                        rows: "3",
                        value: "{draft}",
                        disabled: loading() || current.is_none(),
                        placeholder: "Write to this agent…",
                        oninput: move |event: FormEvent| draft.set(event.value()),
                        onkeydown: move |event: KeyboardEvent| {
                            if event.key() == Key::Enter && !event.modifiers().shift() && !event.is_composing() {
                                event.prevent_default();
                                keyboard_send(());
                            }
                        },
                    }
                    div { class: "av-compose-footer",
                        span { class: "av-muted", "Enter to send · Shift+Enter for a new line" }
                        button {
                            r#type: "submit",
                            class: "av-send",
                            disabled: busy() || loading() || current.is_none() || draft().trim().is_empty(),
                            if busy() { "Sending…" } else { "Send message" }
                        }
                    }
                }
            }
        }
    }
}

fn is_blocked(agent: &wire::AgentView) -> bool {
    agent.state == "blocked" || agent.blocked.is_some()
}

#[component]
fn Conversation(turns: Vec<Turn>, loading: bool) -> Element {
    // The browser owns scroll position. Observe rendered turns, not merely
    // their count: a streamed assistant turn can change without adding a turn.
    rsx! {
        section {
            id: "agent-conversation",
            class: "av-conversation",
            aria_label: "Conversation",
            tabindex: "0",
            onmounted: move |_| async move {
                let _ = document::eval(
                    "const el = document.getElementById('agent-conversation');
                     if (el) {
                       el.dataset.follow = 'true';
                       el.scrollTop = el.scrollHeight;
                       el.addEventListener('scroll', () => {
                         el.dataset.follow = String(el.scrollHeight - el.scrollTop - el.clientHeight < 48);
                       });
                       new MutationObserver(() => {
                         if (el.dataset.follow !== 'false') el.scrollTop = el.scrollHeight;
                       }).observe(el, {childList: true, subtree: true, characterData: true});
                     }"
                ).await;
            },
            if loading {
                p { class: "av-muted", role: "status", "Loading conversation…" }
            } else if turns.is_empty() {
                p { class: "av-muted", "No conversation yet. Send a message to start." }
            }
            for (index , turn) in turns.iter().enumerate() {
                article {
                    key: "{index}",
                    class: match turn.role { Role::User => "av-turn av-user", Role::Assistant => "av-turn av-assistant" },
                    h3 { match turn.role { Role::User => "You", Role::Assistant => "Agent" } }
                    pre { "{turn.text}" }
                }
            }
        }
    }
}

#[component]
fn BlockedPanel(
    prompt: Option<BlockedPrompt>,
    screen: Option<String>,
    busy: bool,
    on_key: EventHandler<String>,
) -> Element {
    rsx! {
        section { class: "av-blocked", aria_label: "Blocked agent",
            h3 { "Agent needs your response" }
            if let Some(prompt) = prompt {
                p { "{prompt.question}" }
                div { class: "av-options",
                    for option in prompt.options {
                        button {
                            key: "{option.key}",
                            r#type: "button",
                            disabled: busy || !allowed_key(&option.key),
                            onclick: move |_| on_key.call(option.key.clone()),
                            "{option.label} ({option.key})"
                        }
                    }
                }
            }
            match screen {
                Some(text) => rsx! { pre { class: "av-screen", tabindex: "0", aria_label: "Agent screen", "{text}" } },
                None => rsx! { p { class: "av-muted", role: "status", "Loading agent screen…" } },
            }
            div { class: "av-keys", aria_label: "Terminal keys",
                for &(key , label) in PANEL_KEYS {
                    button {
                        key: "{key}",
                        r#type: "button",
                        disabled: busy,
                        onclick: move |_| on_key.call(key.to_owned()),
                        "{label}"
                    }
                }
            }
            p { class: "av-muted", "Messages stay in your draft while the agent waits for a key." }
        }
    }
}

fn allowed_key(key: &str) -> bool {
    PANEL_KEYS
        .iter()
        .any(|(allowed, _)| key.eq_ignore_ascii_case(allowed))
        || matches!(
            key.to_ascii_lowercase().as_str(),
            "tab" | "space" | "backspace" | "delete" | "home" | "end" | "pageup" | "pagedown" | "0"
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
}
