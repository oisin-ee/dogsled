//! A blank conversation: pick a registered repository, and the first message starts its lead.

use dioxus::prelude::*;

use crate::agent_view::{Composer, Draft, message_with_images};
use crate::api;
use crate::board::repository_name;
use crate::components::select::{Select, SelectOption};
use crate::components::skeleton::Skeleton;
use crate::notice::{Alert, Status};

/// Whether the main panel shows a new conversation, and the repository of the agent that
/// was selected when it opened.
#[derive(Clone, Copy)]
pub struct NewConversation {
    pub open: Signal<bool>,
    pub repository: Signal<Option<String>>,
}

/// An origin as `host/owner/repo`, the form agent rows carry.
fn repository(origin: &str) -> String {
    let origin = origin.trim_end_matches('/').trim_end_matches(".git");
    let origin = origin.split_once("://").map_or(origin, |(_, rest)| rest);
    let origin = origin.rsplit_once('@').map_or(origin, |(_, rest)| rest);
    origin.replacen(':', "/", 1).to_lowercase()
}

/// The short name of a registered origin.
fn label(origin: &str) -> String {
    repository_name(&repository(origin)).to_owned()
}

#[component]
pub(crate) fn NewLead() -> Element {
    let mut selected = use_context::<Signal<Option<portal_types::agents::AgentAddress>>>();
    let conversation = use_context::<NewConversation>();
    let mut composing = conversation.open;
    let hint = conversation.repository;
    let mut draft = use_context::<Draft>().0;
    let mut images = use_signal(Vec::<String>::new);
    let uploading = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut repo = use_signal(|| None::<String>);
    let repos = use_resource(api::fetch_repos);
    use_effect(move || {
        if let Some(Ok(list)) = &*repos.read()
            && repo.peek().is_none()
            && let Some(first) = list.first()
        {
            let wanted = hint.peek().as_deref().map(str::to_lowercase);
            let preferred = list
                .iter()
                .find(|choice| wanted.as_deref() == Some(repository(&choice.origin).as_str()))
                .unwrap_or(first);
            repo.set(Some(preferred.origin.clone()));
        }
    });
    let send = move |()| {
        let Some(target) = repo() else { return };
        let text = draft();
        let sent = images();
        let message = message_with_images(&text, &sent);
        if busy() || uploading() || message.trim().is_empty() {
            return;
        }
        busy.set(true);
        error.set(None);
        spawn(async move {
            match api::create_lead(&target, &message).await {
                Ok(pane) => {
                    if draft() == text {
                        draft.set(String::new());
                    }
                    images.set(Vec::new());
                    composing.set(false);
                    selected.set(Some(pane));
                }
                Err(message) => error.set(Some(message)),
            }
            busy.set(false);
        });
    };
    let list = repos.read().clone();
    rsx! {
        section { class: "new-lead", aria_label: "New conversation", aria_busy: busy(),
            header { class: "lead-header new-lead-header",
                h1 { "New conversation" }
                match list {
                    Some(Ok(list)) if !list.is_empty() => rsx! {
                        Select::<String> { class: "new-lead-repo", aria_label: "Project", disabled: busy(),
                            value: Some(ReadSignal::from(repo)),
                            on_value_change: move |value: Option<String>| repo.set(value),
                            for (index, choice) in list.into_iter().enumerate() {
                                SelectOption::<String> { key: "{choice.origin}", index, value: choice.origin.clone(),
                                    text_value: "{label(&choice.origin)}", title: "{choice.path}",
                                    "{label(&choice.origin)}"
                                }
                            }
                        }
                    },
                    Some(Ok(_)) => rsx! { Status { "No repositories are registered. Add one with " code { "dogsled repo add" } "." } },
                    Some(Err(message)) => rsx! { Alert { "Projects unavailable: {message}" } },
                    None => rsx! { Skeleton { role: "status", aria_label: "Loading projects" } },
                }
            }
            div { class: "new-lead-body" }
            if let Some(message) = error() { Alert { "{message}" } }
            Composer { draft, images, uploading, disabled: repo().is_none(), busy: busy(),
                on_error: move |message| error.set(Some(message)), on_send: send }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{label, repository};

    #[test]
    fn origins_match_agent_repositories() {
        for origin in [
            "https://github.com/Oisin-ee/dogsled.git",
            "git@github.com:oisin-ee/dogsled.git",
            "ssh://git@github.com/oisin-ee/dogsled",
        ] {
            assert_eq!(
                repository(origin),
                "github.com/oisin-ee/dogsled",
                "{origin}"
            );
            assert_eq!(label(origin), "dogsled");
        }
    }
}
