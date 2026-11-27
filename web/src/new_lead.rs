//! A blank conversation: pick a GitHub repository, and the first message starts its lead.

use dioxus::prelude::*;
use dioxus_icons::lucide::Server;
use dioxus_primitives::combobox::{
    Combobox, ComboboxEmpty, ComboboxInput, ComboboxList, ComboboxOption, default_combobox_filter,
};
use portal_types::HostView;
use portal_types::agents::{
    AgentAddress, CreateLeadBody, LeadStartState, LeadStartView, RepoChoice,
};
use portal_types::models::{ModelCatalogue, ModelChoice, ModelRef, SettingsChange, ThinkingLevel};

use crate::agent_view::{Composer, PendingMessage, message_with_images, use_composer};
use crate::api;
use crate::board::{NavigationRepo, NavigationRow, repository_name};
use crate::components::skeleton::Skeleton;
use crate::notice::{Alert, Status};

/// The New composer and its submitted thread share the shell's lifetime.
#[derive(Clone, Copy)]
pub(crate) struct NewConversation {
    pub open: Signal<bool>,
    pub repository: Signal<Option<String>>,
    pub thread: Signal<Option<LeadThread>>,
    owner: ScopeId,
}

#[derive(Clone, PartialEq)]
pub(crate) struct LeadThread {
    key: String,
    prompt: String,
    receipt: Option<Result<LeadStartView, String>>,
}

impl LeadThread {
    fn view(&self) -> Option<&LeadStartView> {
        self.receipt.as_ref()?.as_ref().ok()
    }
}

pub(crate) struct ReadyLead {
    pub address: AgentAddress,
    pub prompt: String,
    pub name: Option<String>,
}

impl NewConversation {
    pub(crate) fn new() -> Self {
        Self {
            open: Signal::new(false),
            repository: Signal::new(None),
            thread: Signal::new(None),
            owner: dioxus::core::current_scope_id(),
        }
    }

    fn open_thread(&mut self, key: String, prompt: String) {
        self.thread.set(Some(LeadThread {
            key,
            prompt,
            receipt: None,
        }));
        self.open.set(false);
    }

    fn complete(&mut self, key: &str, result: Result<LeadStartView, String>) {
        if let Some(thread) = self
            .thread
            .write()
            .as_mut()
            .filter(|thread| thread.key == key)
        {
            thread.receipt = Some(result);
        }
    }

    /// Follow only the receipt returned for this Send. Consume it once the pane is ready.
    pub(crate) fn follow(&mut self, starts: &[LeadStartView]) -> Option<ReadyLead> {
        let mut thread = (self.thread)()?;
        let saved = thread.view()?;
        let live = starts
            .iter()
            .find(|view| view.id == saved.id && view.revision >= saved.revision);
        let current = live.unwrap_or(saved);
        if current.state == LeadStartState::Ready
            && let Some(address) = &current.address
        {
            let ready = ReadyLead {
                address: address.clone(),
                name: current.name.clone(),
                prompt: thread.prompt,
            };
            self.thread.set(None);
            return Some(ready);
        }
        if let Some(live) = live.filter(|view| *view != saved) {
            thread.receipt = Some(Ok(live.clone()));
            self.thread.set(Some(thread));
        }
        None
    }

    fn send(self, key: String, body: CreateLeadBody) {
        let prompt = body.prompt.clone();
        self.submit(key, prompt, |key| async move {
            api::create_lead(&key, &body).await
        });
    }

    fn submit<F>(mut self, key: String, prompt: String, accept: impl FnOnce(String) -> F)
    where
        F: std::future::Future<Output = Result<LeadStartView, String>> + 'static,
    {
        self.open_thread(key.clone(), prompt);
        let accepted = accept(key.clone());
        dioxus::core::Runtime::current().spawn(self.owner, async move {
            self.complete(&key, accepted.await);
        });
    }
}

#[component]
pub(crate) fn LeadOpening(thread: LeadThread) -> Element {
    let view = thread.view();
    let title = view
        .as_ref()
        .and_then(|view| view.name.clone())
        .unwrap_or_else(|| {
            let words = thread.prompt.split_whitespace().take(8).collect::<Vec<_>>();
            words.join(" ").chars().take(72).collect::<String>()
        });
    let state = view.as_ref().map(|view| view.state);
    let refusal = thread
        .receipt
        .as_ref()
        .and_then(|result| result.as_ref().err());
    let (label, detail) = match (state, refusal) {
        (None, None) => ("Saving", "Saving the lead request."),
        (None, Some(_)) => (
            "Start not confirmed",
            "The portal did not receive a receipt.",
        ),
        (Some(LeadStartState::Accepted | LeadStartState::Dispatching), _) => {
            ("Starting", "The service will start the lead.")
        }
        (Some(LeadStartState::Ready), _) => ("Ready", "Opening the lead's transcript."),
        (Some(LeadStartState::Failed), _) => {
            ("Start failed", "The service could not start the lead.")
        }
        (Some(LeadStartState::Uncertain), _) => (
            "Start uncertain",
            "The lead may have started. Dogsled did not retry it.",
        ),
    };
    rsx! {
        section { class: "lead-panel", aria_label: "Lead conversation",
            header { class: "lead-header", div { class: "lead-heading", h1 { "{title}" } } }
            div { class: "lead-chat", div { class: "conversation-scroll", div { class: "conversation-turns",
            div { class: "conversation-column",
                PendingMessage { text: thread.prompt.clone(), label: label.to_owned(), detail: detail.to_owned() }
                if let Some(error) = refusal { Alert { "{error}" } }
                if let Some(view) = view {
                    if view.state == LeadStartState::Failed {
                        if let Some(error) = &view.error { Alert { "{error}" } }
                    }
                    if view.state == LeadStartState::Uncertain { Alert { "{detail}" } }
                    if let Some(phase) = view.phase.as_deref().filter(|_| view.state != LeadStartState::Ready) {
                        Status { "Launch phase: {phase}" }
                    }
                    if matches!(view.state, LeadStartState::Failed | LeadStartState::Uncertain) {
                        if let Some(address) = &view.address { Status { "Pane: {address.pane}" } }
                    }
                }
            } } } }
        }
    }
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

/// The host a new lead would start on now: the eligible host with the most free cores, as
/// the server chooses, with ties to the lexically smaller label.
fn roomiest(hosts: &[HostView]) -> Option<(&HostView, f64)> {
    hosts
        .iter()
        .filter(|host| host.reachable && host.eligible)
        .filter_map(|host| {
            let busy = host.busy.filter(|busy| busy.is_finite())?;
            Some((host, busy))
        })
        .min_by(|(a, a_busy), (b, b_busy)| {
            (f64::from(b.cores) - b_busy)
                .total_cmp(&(f64::from(a.cores) - a_busy))
                .then(a.label.cmp(&b.label))
        })
}

#[component]
pub(crate) fn NewLead(
    /// The fleet's hosts, for where the lead starts.
    hosts: Vec<HostView>,
    /// Every repository group, for what already runs in the chosen repository.
    running: Vec<NavigationRepo>,
) -> Element {
    let catalogue = use_resource(api::models);
    rsx! { NewLeadForm { model_catalogue: catalogue(), hosts, running } }
}

#[component]
pub(crate) fn NewLeadForm(
    #[props(default)] model_catalogue: Option<Result<ModelCatalogue, String>>,
    #[props(default)] hosts: Vec<HostView>,
    #[props(default)] running: Vec<NavigationRepo>,
) -> Element {
    let mut settings = use_signal(LaunchSettings::default);
    let conversation = use_context::<NewConversation>();
    let hint = conversation.repository;
    let crate::agent_view::ComposerState {
        mut draft,
        mut images,
        ..
    } = use_composer("new");
    let uploading = use_signal(|| false);
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
        let text = draft();
        let sent = images();
        let message = message_with_images(&text, &sent);
        if !*conversation.open.peek() || uploading() || message.trim().is_empty() {
            return;
        }
        let Some(target) = repo().filter(|_| valid()) else {
            error.set(Some("Select a repository before sending.".to_owned()));
            return;
        };
        let key = match api::lead_key() {
            Ok(key) => key,
            Err(message) => {
                error.set(Some(message));
                return;
            }
        };
        let launch = settings();
        let body = api::create_lead_body(&target, &message, launch.model, launch.thinking);
        draft.set(String::new());
        images.set(Vec::new());
        conversation.send(key, body);
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
    let start = roomiest(&hosts);
    let chosen = repo().map(|slug| format!("github.com/{}", slug.to_lowercase()));
    let leads = running
        .iter()
        .filter(|group| group.repository.as_deref().map(str::to_lowercase) == chosen)
        .flat_map(|group| group.rows.iter().filter(|row| !row.worker))
        .cloned()
        .collect::<Vec<_>>();
    // Message, repository, model, effort: the composer holds them in that keyboard order.
    rsx! {
        section { class: "new-lead", aria_label: "New conversation",
          div { class: "new-lead-center",
            h1 { class: "new-lead-title", "New conversation" }
            p { class: "new-lead-lede", "Describe the task. A lead starts on the host with the most room." }
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
            div { class: "new-lead-composer",
                Composer { draft, images, uploading, disabled: false, busy: false, autofocus: true,
                    placeholder: "Send a message or start a task…".to_owned(),
                    controls: rsx! { RepositoryPicker { choices, selected: repo, busy: false,
                        on_select: move |()| {
                            initialized.set(true);
                            selection_removed.set(false);
                        }
                    } },
                    model_picker: rsx! { crate::model_picker::ModelPicker {
                        choices: model_choices.clone(), selected_model: launch.model,
                        selected_level: launch.thinking, busy: false,
                        on_change: move |change| settings.write().change(change, &model_choices),
                    } },
                    on_error: move |message| error.set(Some(message)), on_send: send }
            }
            if let Some((host, busy)) = start {
                div { class: "new-lead-fact",
                    span { class: "fact-icon", Server { size: 16, attributes: dioxus_primitives::dioxus_attributes::attributes!(svg { "aria-hidden": "true" }) } }
                    div {
                        p { class: "fact-label", "Starts on" }
                        p { class: "fact-value", "{host.label}" }
                        p { class: "fact-note", "Most room now: {busy:.0} of {host.cores} cores busy" }
                    }
                }
            }
            if let Some(slug) = repo().filter(|_| !leads.is_empty()) {
                section { class: "new-lead-running", aria_label: "Already running",
                    h2 { "Already running in {repository_name(&slug)}" }
                    ul {
                        for row in leads {
                            li { key: "{row.host}:{row.pane:?}",
                                span { class: "agent-dot", "data-status": lead_status(&row), aria_hidden: "true" }
                                span { class: "running-title", "{row.title}" }
                                span { class: "running-state", "data-status": lead_status(&row), if row.unseen { "Waits on you" } else { "{row.state}" } }
                                code { "{row.host}" }
                            }
                        }
                    }
                }
            }
            p { class: "new-lead-keys",
                span { kbd { "Enter" } " start" }
                span { kbd { "Tab" } " repository, model, effort" }
                span { kbd { "Shift Enter" } " new line" }
            }
          }
        }
    }
}

fn lead_status(row: &NavigationRow) -> &'static str {
    if row.unseen {
        "waiting"
    } else if matches!(row.state.as_str(), "working" | "running") {
        "running"
    } else {
        "muted"
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
            scoped: false,
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

    fn lead_opening_root() -> Element {
        let start = use_context_provider(super::NewConversation::new);
        rsx! {
            if let Some(thread) = (start.thread)() { super::LeadOpening { thread } }
        }
    }

    fn receipt(state: super::LeadStartState, revision: u64) -> super::LeadStartView {
        super::LeadStartView {
            id: "receipt".into(),
            revision,
            state,
            name: None,
            address: None,
            phase: None,
            error: None,
            created_ms: 0,
        }
    }

    #[test]
    fn send_draws_the_opening_turn_before_post_and_ready_binds_the_same_prompt() {
        use super::LeadStartState::{Accepted, Dispatching, Ready};
        let mut dom = VirtualDom::new(lead_opening_root);
        dom.rebuild_in_place();
        let mut start = dom.in_runtime(|| {
            dioxus::core::Runtime::current()
                .consume_context::<super::NewConversation>(ScopeId::APP)
                .unwrap()
        });
        start.open.set(true);
        let posts = std::cell::Cell::new(0);
        let prompt = "Plan the release and keep notes. Include a second line.";
        dom.in_runtime(|| {
            start.submit("send-key".into(), prompt.into(), |key| {
                assert_eq!(key, "send-key");
                posts.set(posts.get() + 1);
                std::future::pending()
            })
        });
        dom.render_immediate_to_vec();
        let html = dioxus_ssr::render(&dom);
        assert!(!*start.open.peek());
        assert!(html.contains("Lead conversation"), "{html}");
        assert!(
            html.contains("<h1>Plan the release and keep notes. Include a</h1>"),
            "{html}"
        );
        assert!(html.contains(prompt) && html.contains("Saving"), "{html}");
        start.complete("send-key", Ok(receipt(Accepted, 1)));
        let named = super::LeadStartView {
            name: Some("release-checklist".into()),
            phase: Some("starting".into()),
            ..receipt(Dispatching, 2)
        };
        assert!(start.follow(std::slice::from_ref(&named)).is_none());
        dom.render_immediate_to_vec();
        let html = dioxus_ssr::render(&dom);
        assert!(
            html.contains("Starting") && html.contains("release-checklist"),
            "{html}"
        );
        let address = super::AgentAddress::local("w2:p1");
        let ready = super::LeadStartView {
            state: Ready,
            address: Some(address.clone()),
            revision: 3,
            ..named
        };
        let bound = start.follow(std::slice::from_ref(&ready)).unwrap();
        assert_eq!(bound.address, address);
        assert_eq!(bound.prompt, prompt);
        assert_eq!(bound.name.as_deref(), Some("release-checklist"));
        assert!(start.follow(&[ready]).is_none());
        assert_eq!(posts.get(), 1);
    }

    #[test]
    fn failed_and_uncertain_starts_stay_in_the_thread_without_retry() {
        use super::LeadStartState::{Accepted, Failed, Uncertain};
        for (state, expected) in [
            (Failed, "Could not send the first prompt"),
            (
                Uncertain,
                "The lead may have started. Dogsled did not retry it.",
            ),
        ] {
            let mut dom = VirtualDom::new(lead_opening_root);
            dom.rebuild_in_place();
            let mut start = dom.in_runtime(|| {
                dioxus::core::Runtime::current()
                    .consume_context::<super::NewConversation>(ScopeId::APP)
                    .unwrap()
            });
            start.open_thread("send-key".into(), "First message".into());
            start.complete("send-key", Ok(receipt(Accepted, 1)));
            assert!(
                start
                    .follow(&[super::LeadStartView {
                        phase: Some("prompting".into()),
                        address: Some(super::AgentAddress {
                            host: "other-machine".into(),
                            pane: "w2:p1".into()
                        }),
                        error: Some("Could not send the first prompt".into()),
                        ..receipt(state, 3)
                    }])
                    .is_none()
            );
            dom.render_immediate_to_vec();
            let html = dioxus_ssr::render(&dom);
            for text in [
                expected,
                "Launch phase: prompting",
                "Pane: w2:p1",
                "First message",
            ] {
                assert!(html.contains(text), "{text}: {html}");
            }
            assert!(
                !html.contains("Retry") && !html.contains("other-machine"),
                "{html}"
            );
        }
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

    fn new_conversation_root() -> Element {
        let publication = use_signal(crate::app::PortalState::default);
        let catalogue = use_memo(move || publication.read().catalogue.clone());
        use_context_provider(|| crate::app::CataloguePortal(catalogue));
        use_context_provider(|| Signal::new(None::<portal_types::agents::AgentAddress>));
        use_context_provider(super::NewConversation::new);
        rsx! { super::NewLeadForm {} }
    }

    #[test]
    fn new_conversation_orders_message_repository_model_and_effort_in_one_composer() {
        let html = dioxus_ssr::render(&VirtualDom::prebuilt(new_conversation_root));
        assert!(!html.contains("<header"), "{html}");
        let composer = html.split_once("new-lead-composer").unwrap().1;
        let place = |needle: &str| {
            composer
                .find(needle)
                .unwrap_or_else(|| panic!("{needle} missing: {html}"))
        };
        let order = [
            place(r#"name="message""#),
            place(r#"name="repository""#),
            place("model-trigger"),
            place(r#"aria-label="Effort""#),
            place(r#"aria-label="Attach images""#),
        ];
        assert!(order.is_sorted(), "{order:?}: {html}");
        // Enter sends, so the send button is not a Tab stop between message and repository.
        assert!(composer.contains(r#"tabindex="-1""#), "{html}");
        // The message box takes typing before the catalogue loads.
        let textarea = composer.split_once("<textarea").unwrap().1;
        let textarea = textarea.split_once('>').unwrap().0;
        assert!(!textarea.contains("disabled"), "{textarea}");
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
