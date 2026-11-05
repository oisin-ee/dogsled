//! A blank conversation: pick a GitHub repository, and the first message starts its lead.

use dioxus::prelude::*;
use dioxus_primitives::combobox::{
    Combobox, ComboboxEmpty, ComboboxInput, ComboboxList, ComboboxOption, default_combobox_filter,
};
use portal_types::agents::RepoChoice;

use crate::agent_view::{Composer, Draft, message_with_images};
use crate::api;
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

fn matches(choice: &RepoChoice, query: &str) -> bool {
    default_combobox_filter(
        query,
        &format!(
            "{} {}",
            choice.slug,
            choice.description.as_deref().unwrap_or("")
        ),
    )
}

/// Official autocomplete semantics and keyboard navigation, with catalogue order unchanged.
#[component]
fn RepositoryPicker(
    choices: Vec<RepoChoice>,
    mut selected: Signal<Option<String>>,
    busy: bool,
) -> Element {
    let searchable = choices.clone();
    rsx! {
        Combobox::<String> { class: "new-lead-repo", disabled: busy,
            value: Some(ReadSignal::from(selected)),
            on_value_change: move |value: Option<String>| selected.set(value),
            filter: move |(query, slug): (String, String)| {
                searchable.iter().find(|choice| choice.slug == slug)
                    .is_some_and(|choice| matches(choice, &query))
            },
            ComboboxInput { class: "repo-search", aria_label: "GitHub repository", "name": "repository" }
            ComboboxList { class: "repo-options", aria_label: "GitHub repositories",
                for (index, choice) in choices.into_iter().enumerate() {
                    ComboboxOption::<String> { key: "{choice.slug}", index,
                        value: choice.slug.clone(), text_value: choice.slug.clone(),
                        div { class: "repo-slug", "{choice.slug}" }
                        if let Some(description) = choice.description {
                            div { class: "repo-description", "{description}" }
                        }
                    }
                }
                ComboboxEmpty { div { class: "repo-empty", "No matching repositories" } }
            }
        }
    }
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
            && let Some(first) = list.repos.first()
        {
            let wanted = hint.peek().as_deref().map(repository);
            let preferred = list
                .repos
                .iter()
                .find(|choice| {
                    wanted.as_deref()
                        == Some(format!("github.com/{}", choice.slug.to_lowercase()).as_str())
                })
                .unwrap_or(first);
            repo.set(Some(preferred.slug.clone()));
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
                    Some(Ok(list)) if !list.repos.is_empty() => rsx! {
                        RepositoryPicker { choices: list.repos, selected: repo, busy: busy() }
                        if let Some(message) = list.error { Alert { "GitHub catalogue: {message}" } }
                    },
                    Some(Ok(list)) => rsx! {
                        Status { "No GitHub repositories found" }
                        if let Some(message) = list.error { Alert { "{message}" } }
                    },
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
    use super::{RepositoryPicker, matches, repository};
    use dioxus::prelude::*;
    use portal_types::agents::RepoChoice;

    fn picker_root() -> Element {
        let selected = use_signal(|| Some("owner/repo-0".to_owned()));
        let choices = (0..70)
            .map(|index| RepoChoice {
                slug: format!("owner/repo-{index}"),
                description: Some(format!("Repository {index}")),
                pushed_at: None,
                private: false,
            })
            .collect();
        rsx! { RepositoryPicker { choices, selected, busy: false } }
    }

    #[test]
    fn repository_picker_composes_official_search_for_seventy_choices() {
        let html = dioxus_ssr::render(&VirtualDom::prebuilt(picker_root));
        assert!(html.contains(r#"role="combobox""#));
        assert!(html.contains(r#"aria-label="GitHub repository""#));
        assert!(html.contains(r#"name="repository""#));
        assert!(html.contains(r#"aria-autocomplete="list""#));
    }

    #[test]
    fn repository_search_matches_slug_and_description_case_insensitively() {
        let repo = RepoChoice {
            slug: "owner/dogsled".into(),
            description: Some("Fleet orchestration".into()),
            pushed_at: None,
            private: false,
        };
        assert!(matches(&repo, "DOGSLED"));
        assert!(matches(&repo, "fleet"));
        assert!(matches(&repo, ""));
        assert!(!matches(&repo, "absent"));
    }

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
        }
    }
}
