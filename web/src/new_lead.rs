//! A blank conversation: pick a GitHub repository, and the first message starts its lead.

use dioxus::prelude::*;
use dioxus_primitives::combobox::{
    Combobox, ComboboxEmpty, ComboboxInput, ComboboxList, ComboboxOption, default_combobox_filter,
};
use portal_types::agents::RepoChoice;
use portal_types::models::{ModelCatalogue, ModelChoice, ModelRef, SettingsChange, ThinkingLevel};

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
    #[props(default)] on_select: EventHandler<()>,
) -> Element {
    let searchable = choices.clone();
    rsx! {
        Combobox::<String> { class: "new-lead-repo", disabled: busy,
            value: Some(ReadSignal::from(selected)),
            on_value_change: move |value: Option<String>| {
                selected.set(value);
                on_select.call(());
            },
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

#[derive(Clone, Debug, Default, PartialEq)]
struct LaunchSettings {
    model: Option<ModelRef>,
    thinking: Option<ThinkingLevel>,
}

impl LaunchSettings {
    fn change(&mut self, change: SettingsChange, choices: &[ModelChoice]) {
        if let Some(model) = change.model {
            if !choices.iter().any(|choice| {
                choice.model == model
                    && self
                        .thinking
                        .is_some_and(|level| choice.thinking.contains(&level))
            }) {
                self.thinking = None;
            }
            self.model = Some(model);
        }
        if let Some(level) = change.thinking {
            self.thinking = Some(level);
        }
    }
}

#[component]
pub(crate) fn NewLead() -> Element {
    let catalogue = use_resource(api::models);
    rsx! { NewLeadForm { model_catalogue: catalogue() } }
}

#[component]
pub(crate) fn NewLeadForm(
    #[props(default)] model_catalogue: Option<Result<ModelCatalogue, String>>,
) -> Element {
    let mut settings = use_signal(LaunchSettings::default);
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
    let mut initialized = use_signal(|| false);
    let mut selection_removed = use_signal(|| false);
    let repos = use_context::<crate::app::CataloguePortal>().0;
    use_effect(move || {
        if let Some(list) = &repos.read().list
            && !*initialized.peek()
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
            initialized.set(true);
        }
    });
    use_effect(move || {
        if let Some(list) = &repos.read().list
            && list.error.is_none()
            && let Some(slug) = repo()
            && !list.repos.iter().any(|choice| choice.slug == slug)
            && !*selection_removed.peek()
        {
            selection_removed.set(true);
        }
    });
    let valid = use_memo(move || {
        !selection_removed()
            && repo().is_some_and(|slug| {
                repos
                    .read()
                    .list
                    .as_ref()
                    .is_some_and(|list| list.repos.iter().any(|choice| choice.slug == slug))
            })
    });
    let send = move |()| {
        let Some(target) = repo().filter(|_| valid()) else {
            return;
        };
        let text = draft();
        let sent = images();
        let message = message_with_images(&text, &sent);
        if busy() || uploading() || message.trim().is_empty() {
            return;
        }
        let launch = settings();
        busy.set(true);
        error.set(None);
        spawn(async move {
            match api::create_lead(&target, &message, launch.model, launch.thinking).await {
                Ok(pane) => {
                    if draft() == text {
                        draft.set(String::new());
                    }
                    if images() == sent {
                        images.set(Vec::new());
                    }
                    composing.set(false);
                    selected.set(Some(pane));
                }
                Err(message) => error.set(Some(message)),
            }
            busy.set(false);
        });
    };
    let catalogue = repos();
    let choices = catalogue
        .list
        .as_ref()
        .map(|list| list.repos.clone())
        .unwrap_or_default();
    let model_choices = model_catalogue
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map(|catalogue| catalogue.models.clone())
        .unwrap_or_default();
    let model_error = model_catalogue.as_ref().and_then(|result| match result {
        Ok(catalogue) => catalogue.error.as_deref(),
        Err(message) => Some(message.as_str()),
    });
    let launch = settings();
    rsx! {
        section { class: "new-lead", aria_label: "New conversation", aria_busy: busy(),
            header { class: "lead-header new-lead-header",
                h1 { "New conversation" }
                RepositoryPicker { choices, selected: repo, busy: busy(),
                    on_select: move |()| {
                        initialized.set(true);
                        selection_removed.set(false);
                    }
                }
                crate::model_picker::ModelPicker {
                    choices: model_choices.clone(), selected_model: launch.model,
                    selected_level: launch.thinking, busy: busy(),
                    on_change: move |change| settings.write().change(change, &model_choices),
                }
            }
            div { class: "new-lead-body",
                if let Some(message) = model_error {
                    Alert { "Model list unavailable: {message}. The lead starts with Pi's default model." }
                }
                if let Some(message) = catalogue.observation_error {
                    if catalogue.list.is_some() {
                        Alert { "Last-known repository choices: {message}" }
                    } else {
                        Alert { "Projects unavailable: {message}" }
                    }
                } else if catalogue.list.is_none() {
                    Skeleton { role: "status", aria_label: "Loading projects" }
                }
                if let Some(list) = catalogue.list {
                    if let Some(message) = list.error {
                        Alert { "{message}" }
                    } else if list.repos.is_empty() {
                        Status { "No GitHub repositories found" }
                    }
                }
                if let Some(slug) = repo().filter(|_| !valid()) {
                    Status { "{slug} was removed from the catalogue. Select a repository before sending." }
                }
            }
            if let Some(message) = error() { Alert { "{message}" } }
            Composer { draft, images, uploading, disabled: !valid(), busy: busy(),
                on_error: move |message| error.set(Some(message)), on_send: send }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LaunchSettings, RepositoryPicker, matches, repository};
    use dioxus::prelude::*;
    use portal_types::agents::RepoChoice;
    use portal_types::models::{ModelChoice, ModelRef, SettingsChange, ThinkingLevel};

    fn model_choice(id: &str, thinking: Vec<ThinkingLevel>) -> ModelChoice {
        ModelChoice {
            model: ModelRef {
                provider: "provider".into(),
                id: id.into(),
            },
            name: None,
            thinking,
        }
    }

    #[test]
    fn chosen_model_and_level_reach_the_lead_request_body() {
        let choice = model_choice("reasoning", vec![ThinkingLevel::Off, ThinkingLevel::High]);
        let mut settings = LaunchSettings::default();
        settings.change(
            SettingsChange {
                model: Some(choice.model.clone()),
                thinking: None,
            },
            std::slice::from_ref(&choice),
        );
        settings.change(
            SettingsChange {
                model: None,
                thinking: Some(ThinkingLevel::High),
            },
            std::slice::from_ref(&choice),
        );
        let body = crate::api::create_lead_body(
            "owner/repo",
            "First message",
            settings.model,
            settings.thinking,
        );
        assert_eq!(
            serde_json::to_value(body).unwrap(),
            serde_json::json!({
                "repo": "owner/repo", "prompt": "First message",
                "model": {"provider": "provider", "id": "reasoning"}, "thinking": "high"
            })
        );
    }

    #[test]
    fn unset_launch_settings_omit_both_fields_for_pi_defaults() {
        let settings = LaunchSettings::default();
        let body = crate::api::create_lead_body(
            "owner/repo",
            "First message",
            settings.model,
            settings.thinking,
        );
        assert_eq!(
            serde_json::to_value(body).unwrap(),
            serde_json::json!({
                "repo": "owner/repo", "prompt": "First message"
            })
        );
    }

    #[test]
    fn model_change_clears_unsupported_thinking_but_keeps_a_supported_level() {
        let choices = vec![
            model_choice("reasoning", vec![ThinkingLevel::Off, ThinkingLevel::High]),
            model_choice("basic", vec![ThinkingLevel::Off]),
        ];
        let mut settings = LaunchSettings {
            model: None,
            thinking: Some(ThinkingLevel::High),
        };
        settings.change(
            SettingsChange {
                model: Some(choices[0].model.clone()),
                thinking: None,
            },
            &choices,
        );
        assert_eq!(settings.thinking, Some(ThinkingLevel::High));
        settings.change(
            SettingsChange {
                model: Some(choices[1].model.clone()),
                thinking: None,
            },
            &choices,
        );
        assert_eq!(settings.model, Some(choices[1].model.clone()));
        assert_eq!(settings.thinking, None);
        let body = crate::api::create_lead_body(
            "owner/repo",
            "First message",
            settings.model,
            settings.thinking,
        );
        assert!(
            serde_json::to_value(body)
                .unwrap()
                .get("thinking")
                .is_none()
        );
    }

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
