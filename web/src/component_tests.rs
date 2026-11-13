//! Composition smoke tests for the official controls and console surfaces.

use dioxus::core::DynamicNode;
use dioxus::prelude::*;

use crate::components::{button::Button, input::Input, label::Label, textarea::Textarea};

fn model_picker_root() -> Element {
    use portal_types::models::{ModelChoice, ModelRef, ThinkingLevel};
    let model = ModelRef {
        provider: "provider".into(),
        id: "test-model".into(),
    };
    rsx! { crate::model_picker::ModelPicker {
        choices: vec![ModelChoice { model: model.clone(), name: Some("Test model".into()), thinking: vec![ThinkingLevel::Off, ThinkingLevel::High] }],
        selected_model: Some(model), selected_level: Some(ThinkingLevel::High), busy: false, on_change: move |_| {},
    } }
}

#[test]
fn model_picker_composes_searchable_model_and_supported_thinking_controls() {
    let mut dom = VirtualDom::prebuilt(model_picker_root);
    settle_catalogue(&mut dom);
    let html = dioxus_ssr::render(&dom);
    assert!(html.contains(r#"aria-label="Model""#), "{html}");
    assert!(html.contains(r#"role="combobox""#), "{html}");
    assert!(html.contains(r#"name="model""#), "{html}");
    assert!(html.contains(r#"aria-label="Thinking level""#), "{html}");
    assert!(html.contains("high"), "{html}");
    assert!(!html.contains("xhigh"), "{html}");
}

type ModelChangeHandle =
    std::rc::Rc<std::cell::Cell<Option<Signal<Option<portal_types::models::SettingsChange>>>>>;

fn changing_model_picker_root(handle: ModelChangeHandle) -> Element {
    use portal_types::models::{ModelChoice, ModelRef, ThinkingLevel};
    let mut requested = use_signal(|| None::<portal_types::models::SettingsChange>);
    handle.set(Some(requested));
    let choices: Vec<_> = (0..70)
        .map(|index| ModelChoice {
            model: ModelRef {
                provider: "provider".into(),
                id: format!("model-{index}"),
            },
            name: Some(format!("Choice {index}")),
            thinking: vec![ThinkingLevel::Off, ThinkingLevel::High],
        })
        .collect();
    let selected_model = Some(choices[0].model.clone());
    rsx! { crate::model_picker::ModelPicker { choices, selected_model, selected_level: Some(ThinkingLevel::High), busy: false,
        on_change: move |change| requested.set(Some(change)),
    } }
}

#[test]
fn model_picker_search_requests_a_model_without_optimistically_changing_it() {
    let handle = ModelChangeHandle::default();
    let mut dom = VirtualDom::new_with_props(changing_model_picker_root, handle.clone());
    dom.rebuild_in_place();
    settle_catalogue(&mut dom);
    catalogue_query(&mut dom, "Choice 69");
    let html = dioxus_ssr::render(&dom);
    assert!(html.contains("Choice 69"), "{html}");
    assert!(!html.contains("Choice 68"), "{html}");
    catalogue_key(&mut dom, Key::ArrowDown);
    catalogue_key(&mut dom, Key::Enter);
    let change = handle.get().unwrap().peek().clone().unwrap();
    assert_eq!(change.model.unwrap().pattern(), "provider/model-69");
    assert_eq!(change.thinking, None);
    assert!(dioxus_ssr::render(&dom).contains(r#"value="provider/model-0""#));
}

#[test]
fn model_picker_readonly_note_preserves_the_recorded_model() {
    fn root() -> Element {
        rsx! { crate::model_picker::ReadOnlyModel { model: Some("recorded-model".to_owned()) } }
    }
    let html = dioxus_ssr::render(&VirtualDom::prebuilt(root));
    assert!(html.contains("Recorded model: recorded-model"), "{html}");
    assert!(
        html.contains("Model changes work for Pi sessions only"),
        "{html}"
    );
    assert!(!html.contains("combobox"), "{html}");
}

type PortalHandle = std::rc::Rc<std::cell::Cell<Option<Signal<crate::app::PortalState>>>>;
type DraftHandle = std::rc::Rc<std::cell::Cell<Option<Signal<String>>>>;

fn catalogue_root(props: (PortalHandle, DraftHandle)) -> Element {
    let publication = use_signal(crate::app::PortalState::default);
    props.0.set(Some(publication));
    let catalogue = use_memo(move || publication.read().catalogue.clone());
    use_context_provider(|| crate::app::CataloguePortal(catalogue));
    use_context_provider(|| Signal::new(None::<portal_types::agents::AgentAddress>));
    use_context_provider(|| crate::new_lead::NewConversation {
        open: Signal::new(true),
        repository: Signal::new(Some("github.com/owner/beta".into())),
    });
    let draft = use_context_provider(|| crate::agent_view::Draft(Signal::new(String::new()))).0;
    props.1.set(Some(draft));
    rsx! { crate::new_lead::NewLeadForm {} }
}

fn new_model_catalogue_root(
    model_catalogue: Result<portal_types::models::ModelCatalogue, String>,
) -> Element {
    let publication = use_signal(|| {
        let mut state = crate::app::PortalState::default();
        state.update(Ok(catalogue_snapshot(&["owner/repo"], None, 1)), 1);
        state
    });
    let catalogue = use_memo(move || publication.read().catalogue.clone());
    use_context_provider(|| crate::app::CataloguePortal(catalogue));
    use_context_provider(|| Signal::new(None::<portal_types::agents::AgentAddress>));
    use_context_provider(|| crate::new_lead::NewConversation {
        open: Signal::new(true),
        repository: Signal::new(None),
    });
    use_context_provider(|| crate::agent_view::Draft(Signal::new("First message".into())));
    rsx! { crate::new_lead::NewLeadForm { model_catalogue: Some(model_catalogue) } }
}

#[test]
fn new_conversation_renders_catalogue_choices_and_pi_defaults() {
    use portal_types::models::{ModelCatalogue, ModelChoice, ModelRef, ThinkingLevel};
    let mut dom = VirtualDom::new_with_props(
        new_model_catalogue_root,
        Ok(ModelCatalogue {
            models: vec![ModelChoice {
                model: ModelRef {
                    provider: "provider".into(),
                    id: "reasoning".into(),
                },
                name: Some("Reasoning model".into()),
                thinking: vec![ThinkingLevel::High],
            }],
            error: None,
        }),
    );
    dom.rebuild_in_place();
    settle_catalogue(&mut dom);
    let html = dioxus_ssr::render(&dom);
    let header = html.split_once("</header>").unwrap().0;
    assert!(header.contains("New conversation"), "{html}");
    assert!(header.contains(r#"name="model""#), "{html}");
    assert!(header.contains(r#"placeholder="Pi default""#), "{html}");
    assert!(header.contains(">Pi default</span>"), "{html}");
    catalogue_input_event_for(&dom, "model", "oninput", || {
        FormData::from(CatalogueInput("reasoning".into()))
    });
    settle_catalogue(&mut dom);
    let html = dioxus_ssr::render(&dom);
    assert!(html.contains("Reasoning model"), "{html}");
    assert!(html.contains("provider/reasoning"), "{html}");
}

#[test]
fn new_conversation_model_catalogue_failures_alert_without_disabling_send() {
    use portal_types::models::ModelCatalogue;
    for result in [
        Err("observation unavailable".into()),
        Ok(ModelCatalogue {
            models: vec![],
            error: Some("Pi catalogue refresh failed".into()),
        }),
    ] {
        let mut dom = VirtualDom::new_with_props(new_model_catalogue_root, result);
        dom.rebuild_in_place();
        settle_catalogue(&mut dom);
        let html = dioxus_ssr::render(&dom);
        assert!(html.contains(r#"role="alert""#), "{html}");
        assert!(html.contains("Model list unavailable:"), "{html}");
        assert!(
            html.contains("The lead starts with Pi&#39;s default model."),
            "{html}"
        );
        let send = html
            .split("<button")
            .find(|button| button.contains(r#"aria-label="Send message""#))
            .unwrap();
        let attributes = send.split('>').next().unwrap();
        assert!(
            !attributes.contains("disabled=true") && !attributes.contains(r#"disabled="true""#),
            "{attributes}"
        );
    }
}

fn catalogue_snapshot(
    slugs: &[&str],
    error: Option<&str>,
    revision: u64,
) -> portal_types::PortalEvent {
    let repos: Vec<_> = slugs.iter().map(|slug| serde_json::json!({
        "slug":slug,"description":format!("Description of {slug}"),"pushed_at":null,"private":false
    })).collect();
    serde_json::from_value(serde_json::json!({
        "version":portal_types::PORTAL_VERSION,"epoch":"serve","revision":revision,
        "published_ms":revision,"board":null,"agents":null,"inbox":null,
        "repos":{"repos":repos,"error":error},"gaps":[]
    }))
    .unwrap()
}

fn catalogue_component_scopes(
    dom: &VirtualDom,
    node: &VNode,
    scopes: &mut std::collections::BTreeMap<String, ScopeId>,
) {
    for (index, dynamic) in node.dynamic_nodes.iter().enumerate() {
        match dynamic {
            DynamicNode::Component(component) => {
                if let Some(scope) = component.mounted_scope_id(index, node, dom) {
                    if component.name.ends_with("::NewLeadForm")
                        || component.name.ends_with("::Composer")
                        || component.name.ends_with("::RepositoryPicker")
                        || component.name.ends_with("::ComboboxInput")
                        || component.name.ends_with("::Input")
                        || component.name.contains("::ComboboxOption")
                    {
                        scopes.insert(
                            format!("{}:{:?}:{scope:?}", component.name, node.key),
                            scope,
                        );
                    }
                    if let Some(state) = dom.get_scope(scope) {
                        catalogue_component_scopes(dom, state.root_node(), scopes);
                    }
                }
            }
            DynamicNode::Fragment(children) => {
                for child in children {
                    catalogue_component_scopes(dom, child, scopes);
                }
            }
            _ => {}
        }
    }
}

fn catalogue_scopes(dom: &VirtualDom) -> std::collections::BTreeMap<String, ScopeId> {
    let mut scopes = std::collections::BTreeMap::new();
    catalogue_component_scopes(dom, dom.base_scope().root_node(), &mut scopes);
    scopes
}

struct CatalogueEventConverter;

macro_rules! catalogue_event_converter {
    (enum Event { $(
        #[convert = $converter:ident]
        #[events = [$($(#[$attr:meta])* $name:ident => $raw:ident,)*]]
        $(#[raw = [$($raw_only:ident),* $(,)?]])?
        $group:ident($data:ident),
    )* }) => {
        impl dioxus::html::HtmlEventConverter for CatalogueEventConverter {
            $(fn $converter(&self, event: &dioxus::html::PlatformEventData) -> dioxus::html::$data {
                event.downcast::<Box<dyn Fn() -> dioxus::html::$data>>().expect("native event factory")()
            })*
        }
    };
}

dioxus::html::with_html_event_groups!(catalogue_event_converter);

fn catalogue_input_event<T: 'static>(dom: &VirtualDom, name: &str, data: impl Fn() -> T + 'static) {
    catalogue_input_event_for(dom, "repository", name, data);
}

fn catalogue_input_event_for<T: 'static>(
    dom: &VirtualDom,
    field: &str,
    name: &str,
    data: impl Fn() -> T + 'static,
) {
    let inputs: Vec<_> = catalogue_scopes(dom)
        .into_iter()
        .filter(|(name, _)| name.contains("::ComboboxInput"))
        .map(|(_, scope)| scope)
        .collect();
    let scope = inputs.iter().copied().find(|scope| {
        dom.get_scope(*scope).unwrap().root_node().dynamic_attrs.iter().flatten().any(|attr| {
            attr.name == "name" && matches!(&attr.value, dioxus::core::AttributeValue::Text(value) if value == field)
        })
    }).or_else(|| inputs.first().copied()).expect("mounted combobox input");
    let listener = dom
        .get_scope(scope)
        .unwrap()
        .root_node()
        .dynamic_attrs
        .iter()
        .flatten()
        .find_map(|attr| match &attr.value {
            dioxus::core::AttributeValue::Listener(listener) if attr.name == name => {
                Some(listener.clone())
            }
            _ => None,
        })
        .expect("combobox input event");
    let factory: Box<dyn Fn() -> T> = Box::new(data);
    let platform = dioxus::html::PlatformEventData::new(Box::new(factory));
    dioxus::html::set_event_converter(Box::new(CatalogueEventConverter));
    dom.in_runtime(|| listener.call(dioxus::core::Event::new(std::rc::Rc::new(platform), true)));
}

struct CatalogueInput(String);

impl dioxus::html::HasFileData for CatalogueInput {
    fn files(&self) -> Vec<dioxus::html::FileData> {
        vec![]
    }
}

impl dioxus::html::HasFormData for CatalogueInput {
    fn value(&self) -> String {
        self.0.clone()
    }
    fn valid(&self) -> bool {
        true
    }
    fn values(&self) -> Vec<(String, dioxus::html::FormValue)> {
        vec![]
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

struct CatalogueKey(Key);

impl dioxus::html::ModifiersInteraction for CatalogueKey {
    fn modifiers(&self) -> Modifiers {
        Modifiers::empty()
    }
}

impl dioxus::html::HasKeyboardData for CatalogueKey {
    fn key(&self) -> Key {
        self.0.clone()
    }
    fn code(&self) -> dioxus::html::Code {
        dioxus::html::Code::Unidentified
    }
    fn location(&self) -> dioxus::html::Location {
        dioxus::html::Location::Standard
    }
    fn is_auto_repeating(&self) -> bool {
        false
    }
    fn is_composing(&self) -> bool {
        false
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn settle_catalogue(dom: &mut VirtualDom) -> usize {
    use futures_util::FutureExt as _;
    let mut edits = 0;
    // Options register independently. Allow the bounded 1,000-model catalogue to settle.
    for _ in 0..1024 {
        edits += dom.render_immediate_to_vec().edits.len();
        if dom.wait_for_work().now_or_never().is_none() {
            return edits;
        }
    }
    panic!("catalogue did not settle");
}

fn catalogue_query(dom: &mut VirtualDom, value: &str) {
    let value = value.to_owned();
    catalogue_input_event(dom, "oninput", move || {
        FormData::from(CatalogueInput(value.clone()))
    });
    settle_catalogue(dom);
}

fn catalogue_key(dom: &mut VirtualDom, key: Key) {
    catalogue_input_event(dom, "onkeydown", move || {
        KeyboardData::from(CatalogueKey(key.clone()))
    });
    settle_catalogue(dom);
}

#[test]
fn catalogue_recovers_in_one_mounted_new_lead_and_preserves_draft_search_and_selection() {
    let handle = PortalHandle::default();
    let draft_handle = DraftHandle::default();
    let mut dom =
        VirtualDom::new_with_props(catalogue_root, (handle.clone(), draft_handle.clone()));
    dom.rebuild_in_place();
    settle_catalogue(&mut dom);
    let initial_scopes = catalogue_scopes(&dom);
    assert!(
        initial_scopes
            .keys()
            .any(|name| name.contains("::Composer"))
    );
    let mut publication = handle.get().unwrap();
    let mut draft = draft_handle.get().unwrap();
    assert!(dioxus_ssr::render(&dom).contains("Loading projects"));
    dom.in_runtime(|| {
        draft.set("Keep this draft".into());
        publication.write().update(
            Ok(catalogue_snapshot(
                &[],
                Some("GitHub catalogue: connection failed"),
                1,
            )),
            1,
        );
    });
    settle_catalogue(&mut dom);
    let failed = dioxus_ssr::render(&dom);
    assert!(
        failed.contains("GitHub catalogue: connection failed"),
        "{failed}"
    );
    assert_eq!(failed.matches("GitHub catalogue:").count(), 1);
    assert!(!failed.contains("No GitHub repositories found"));
    dom.in_runtime(|| {
        publication.write().update(
            Ok(catalogue_snapshot(&["owner/alpha", "owner/beta"], None, 2)),
            2,
        )
    });
    settle_catalogue(&mut dom);
    let recovered = dioxus_ssr::render(&dom);
    assert!(!recovered.contains("connection failed"), "{recovered}");
    assert!(
        recovered.contains("owner/beta"),
        "initial hint not selected: {recovered}"
    );
    assert!(recovered.contains("Keep this draft"));
    for (name, scope) in &initial_scopes {
        assert_eq!(
            catalogue_scopes(&dom).get(name),
            Some(scope),
            "remounted {name}"
        );
    }
    catalogue_query(&mut dom, "alpha");
    assert!(dioxus_ssr::render(&dom).contains("value=\"alpha\""));
    let before = catalogue_scopes(&dom);
    // Reordering and a refresh failure must not reset the open query or selected slug.
    dom.in_runtime(|| {
        publication.write().update(
            Ok(catalogue_snapshot(
                &["owner/beta", "owner/alpha"],
                Some("GitHub catalogue: retry next hour"),
                3,
            )),
            3,
        )
    });
    settle_catalogue(&mut dom);
    let failed = dioxus_ssr::render(&dom);
    assert!(failed.contains("value=\"alpha\""), "{failed}");
    assert!(failed.contains("retry next hour"));
    assert_eq!(catalogue_scopes(&dom), before);
    catalogue_key(&mut dom, Key::ArrowDown);
    catalogue_key(&mut dom, Key::Enter);
    let selected = dioxus_ssr::render(&dom);
    assert!(selected.contains("value=\"owner/alpha\""), "{selected}");
    dom.in_runtime(|| {
        publication
            .write()
            .update(Err("stream disconnected".into()), 4)
    });
    settle_catalogue(&mut dom);
    assert!(
        dioxus_ssr::render(&dom).contains("Last-known repository choices: stream disconnected")
    );
    dom.in_runtime(|| {
        publication.write().update(
            Ok(catalogue_snapshot(&["owner/beta", "owner/alpha"], None, 5)),
            5,
        )
    });
    settle_catalogue(&mut dom);
    let recovered = dioxus_ssr::render(&dom);
    assert!(recovered.contains("value=\"owner/alpha\""), "{recovered}");
    assert!(!recovered.contains("retry next hour") && !recovered.contains("disconnected"));
    assert_eq!(draft(), "Keep this draft");
    let before = catalogue_scopes(&dom);
    dom.in_runtime(|| {
        publication.write().update(
            Ok(catalogue_snapshot(&["owner/beta", "owner/alpha"], None, 6)),
            6,
        )
    });
    assert_eq!(
        settle_catalogue(&mut dom),
        0,
        "unrelated snapshot invalidated the picker"
    );
    assert_eq!(catalogue_scopes(&dom), before);
    dom.in_runtime(|| {
        publication
            .write()
            .update(Ok(catalogue_snapshot(&["owner/beta"], None, 7)), 7)
    });
    settle_catalogue(&mut dom);
    let removed = dioxus_ssr::render(&dom);
    assert!(
        removed.contains("owner/alpha was removed from the catalogue"),
        "{removed}"
    );
    assert!(removed.contains("disabled=true"));
    assert_eq!(draft(), "Keep this draft");
    // Returning to the list cannot silently re-enable a removed target.
    dom.in_runtime(|| {
        publication.write().update(
            Ok(catalogue_snapshot(&["owner/alpha", "owner/beta"], None, 8)),
            8,
        )
    });
    settle_catalogue(&mut dom);
    assert!(dioxus_ssr::render(&dom).contains("owner/alpha was removed from the catalogue"));
    catalogue_query(&mut dom, "beta");
    catalogue_key(&mut dom, Key::ArrowDown);
    catalogue_key(&mut dom, Key::Enter);
    let reselected = dioxus_ssr::render(&dom);
    assert!(reselected.contains("value=\"owner/beta\""), "{reselected}");
    assert!(!reselected.contains("was removed from the catalogue"));
    dom.in_runtime(|| {
        publication
            .write()
            .update(Ok(catalogue_snapshot(&[], None, 9)), 9)
    });
    settle_catalogue(&mut dom);
    let empty = dioxus_ssr::render(&dom);
    assert!(empty.contains("No GitHub repositories found"));
    assert!(empty.contains("owner/beta was removed from the catalogue"));
}

#[test]
fn catalogue_notices_render_in_the_body_instead_of_crowding_the_header() {
    let handle = PortalHandle::default();
    let mut dom =
        VirtualDom::new_with_props(catalogue_root, (handle.clone(), DraftHandle::default()));
    dom.rebuild_in_place();
    settle_catalogue(&mut dom);
    let assert_notices = |dom: &VirtualDom, notices: &[&str]| {
        let html = dioxus_ssr::render(dom);
        let (header, body) = html
            .split_once("</header>")
            .expect("New conversation header");
        assert!(header.contains("New conversation"), "{html}");
        assert!(header.contains(r#"role="combobox""#), "{html}");
        assert!(!header.contains(r#"role="alert""#), "{html}");
        assert!(!header.contains(r#"role="status""#), "{html}");
        assert!(body.starts_with(r#"<div class="new-lead-body">"#), "{html}");
        for notice in notices {
            assert!(body.contains(notice), "{notice}: {html}");
            assert!(!header.contains(notice), "{notice}: {html}");
        }
    };
    assert_notices(&dom, &["Loading projects"]);
    let mut publication = handle.get().unwrap();
    dom.in_runtime(|| publication.write().update(Err("offline".into()), 1));
    settle_catalogue(&mut dom);
    assert_notices(&dom, &["Projects unavailable: offline"]);
    dom.in_runtime(|| {
        publication.write().update(
            Ok(catalogue_snapshot(
                &["owner/beta"],
                Some("GitHub catalogue: gh exited 1"),
                2,
            )),
            2,
        );
    });
    settle_catalogue(&mut dom);
    assert_notices(&dom, &["GitHub catalogue: gh exited 1"]);
    dom.in_runtime(|| publication.write().update(Err("offline".into()), 3));
    settle_catalogue(&mut dom);
    assert_notices(
        &dom,
        &[
            "Last-known repository choices: offline",
            "GitHub catalogue: gh exited 1",
        ],
    );
    dom.in_runtime(|| {
        publication
            .write()
            .update(Ok(catalogue_snapshot(&[], None, 4)), 4)
    });
    settle_catalogue(&mut dom);
    assert_notices(
        &dom,
        &[
            "No GitHub repositories found",
            "owner/beta was removed from the catalogue",
        ],
    );
}

#[test]
fn a_failed_fetch_without_repositories_keeps_the_retained_catalogue_and_selection() {
    let handle = PortalHandle::default();
    let draft_handle = DraftHandle::default();
    let mut dom =
        VirtualDom::new_with_props(catalogue_root, (handle.clone(), draft_handle.clone()));
    dom.rebuild_in_place();
    settle_catalogue(&mut dom);
    let mut publication = handle.get().unwrap();
    let mut draft = draft_handle.get().unwrap();
    dom.in_runtime(|| {
        draft.set("Keep this draft".into());
        publication.write().update(
            Ok(catalogue_snapshot(&["owner/alpha", "owner/beta"], None, 1)),
            1,
        );
    });
    settle_catalogue(&mut dom);
    let healthy = dioxus_ssr::render(&dom);
    assert!(healthy.contains("value=\"owner/beta\""), "{healthy}");
    assert!(
        !healthy
            .split_once("<form")
            .unwrap()
            .1
            .contains("disabled=true"),
        "{healthy}"
    );
    let before = catalogue_scopes(&dom);
    // A serve restarted during a GitHub outage publishes its failure with no repositories.
    dom.in_runtime(|| {
        publication.write().update(
            Ok(catalogue_snapshot(
                &[],
                Some("GitHub catalogue: connection failed"),
                2,
            )),
            2,
        );
    });
    settle_catalogue(&mut dom);
    let failed = dioxus_ssr::render(&dom);
    assert_eq!(failed.matches("GitHub catalogue:").count(), 1, "{failed}");
    assert!(failed.contains("value=\"owner/beta\""), "{failed}");
    assert!(
        !failed.contains("was removed from the catalogue"),
        "{failed}"
    );
    assert!(!failed.contains("No GitHub repositories found"), "{failed}");
    assert!(
        !failed
            .split_once("<form")
            .unwrap()
            .1
            .contains("disabled=true"),
        "{failed}"
    );
    assert_eq!(catalogue_scopes(&dom), before);
    assert_eq!(draft(), "Keep this draft");
}

type SessionHandle =
    std::rc::Rc<std::cell::Cell<Option<Signal<Option<portal_types::agents::DetailSession>>>>>;

fn session_notification_root(
    props: (SessionHandle, std::rc::Rc<std::cell::Cell<usize>>),
) -> Element {
    let session = use_signal(|| None::<portal_types::agents::DetailSession>);
    props.0.set(Some(session));
    props.1.set(props.1.get() + 1);
    rsx! { "{session():?}" }
}

#[test]
fn unchanged_conversation_keys_do_not_notify_session_subscribers()
-> Result<(), Box<dyn std::error::Error>> {
    let handle = SessionHandle::default();
    let renders = std::rc::Rc::new(std::cell::Cell::new(0));
    let mut dom =
        VirtualDom::new_with_props(session_notification_root, (handle.clone(), renders.clone()));
    dom.rebuild_in_place();
    let session = handle.get().ok_or("Missing session signal")?;
    dom.in_runtime(|| crate::conversations::accept_session(session, "native".into()));
    dom.render_immediate_to_vec();
    let first = renders.get();
    assert!(first > 1, "first accepted key must notify subscribers");
    dom.in_runtime(|| crate::conversations::accept_session(session, "native".into()));
    dom.render_immediate_to_vec();
    assert_eq!(renders.get(), first, "unchanged key notified subscribers");
    dom.in_runtime(|| crate::conversations::accept_session(session, "replacement".into()));
    dom.render_immediate_to_vec();
    assert!(renders.get() > first, "a new key must notify subscribers");
    Ok(())
}

fn unknown_session_activity_root() -> Element {
    let session = use_signal(|| None::<portal_types::agents::DetailSession>);
    let live = crate::activity::use_live_activity(
        portal_types::agents::AgentAddress::local("pane"),
        session,
    );
    rsx! { "{live():?}" }
}

#[test]
fn activity_without_a_known_session_stays_pending_without_browser_requests() {
    // Poll the real hook on the native test target: browser network calls would panic.
    let mut dom = VirtualDom::new(unknown_session_activity_root);
    dom.rebuild_in_place();
    dom.render_immediate_to_vec();
    assert_eq!(dioxus_ssr::render(&dom), "Pending");
}

fn files_root(state: Option<portal_types::files::FilesState>) -> Element {
    rsx! { crate::files::Files { state } }
}

fn render_files(state: Option<portal_types::files::FilesState>) -> String {
    let mut dom = VirtualDom::new_with_props(files_root, state);
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

fn assert_accordion_has_no_inline_width(html: &str) {
    let root = html
        .split('<')
        .find(|tag| tag.starts_with("div ") && tag.contains("class=\"dx-accordion-"))
        .and_then(|tag| tag.split('>').next());
    assert!(
        root.is_some_and(|tag| {
            let style = tag
                .split("style=\"")
                .nth(1)
                .and_then(|value| value.split('"').next());
            style.is_none_or(|style| {
                style.split(';').all(|declaration| {
                    declaration
                        .split_once(':')
                        .is_none_or(|(property, _)| property.trim() != "width")
                })
            })
        }),
        "missing accordion root or unexpected inline width: {html}"
    );
}

fn checkout(entries: Vec<portal_types::files::Entry>) -> portal_types::files::Files {
    portal_types::files::Files {
        root: "/repo".into(),
        head: portal_types::files::Head::Branch("feat/files".into()),
        entries,
        captured_ms: 1234,
    }
}

fn file_entry(path: &[u8], change: portal_types::files::Change) -> portal_types::files::Entry {
    portal_types::files::Entry {
        path: portal_types::files::GitPath::new(path),
        change,
    }
}

#[test]
fn files_render_loading_clean_and_all_unavailable_states() {
    use portal_types::files::FilesState;
    for state in [None, Some(FilesState::Pending)] {
        let html = render_files(state.clone());
        assert!(
            html.contains(r#"aria-label="Loading file status""#),
            "{html}"
        );
        // Skeleton class names carry a build-specific asset hash; count the placeholders.
        assert_eq!(html.matches("skeleton").count(), 3, "{html}");
        assert!(!html.contains("Working tree clean"), "{html}");
        assert_eq!(crate::files::path_count(state.as_ref()), None);
    }
    for (state, message) in [
        (
            FilesState::Current(checkout(Vec::new())),
            "Working tree clean",
        ),
        (FilesState::NotRepository, "Not a Git checkout"),
        (FilesState::TooMany, "Too many changes to list"),
        (
            FilesState::Unavailable("host files unsupported".into()),
            "host files unsupported",
        ),
    ] {
        let count = crate::files::path_count(Some(&state));
        let html = render_files(Some(state));
        assert!(html.contains(message), "{html}");
        assert!(html.contains(r#"role="status""#), "{html}");
        assert_eq!(count, (message == "Working tree clean").then_some(0));
        assert!(!html.contains("files-group"), "{html}");
    }
}

#[test]
fn files_order_nonempty_groups_and_count_shared_paths_once() {
    use portal_types::files::{Change, FilesState};
    let shared = file_entry(
        b"both.rs",
        Change::Ordinary {
            index: 'M',
            worktree: 'M',
        },
    );
    let entries = vec![
        file_entry(b"new.rs", Change::Untracked),
        shared.clone(),
        file_entry(b"conflict.rs", Change::Conflict { code: ['U', 'U'] }),
    ];
    let state = FilesState::Current(checkout(entries));
    assert_eq!(crate::files::path_count(Some(&state)), Some(3));
    let html = render_files(Some(state));
    assert_accordion_has_no_inline_width(&html);
    let mut previous = 0;
    for heading in [
        "Conflicts · 1",
        "Staged · 1",
        "Unstaged · 1",
        "Untracked · 1",
    ] {
        let position = html.find(heading);
        assert!(
            position.is_some_and(|position| position >= previous),
            "missing/out of order {heading}: {html}"
        );
        previous = position.unwrap_or(previous);
    }
    let html = render_files(Some(FilesState::Current(checkout(vec![shared]))));
    assert!(
        html.contains("Staged · 1") && html.contains("Unstaged · 1"),
        "{html}"
    );
    assert!(
        !html.contains("Conflicts") && !html.contains("Untracked"),
        "{html}"
    );
}

#[test]
fn stale_files_keep_the_last_groups_count_time_and_error() {
    use portal_types::files::{Change, FilesState};
    let state = FilesState::Stale {
        files: checkout(vec![file_entry(b"last.rs", Change::Untracked)]),
        error: "Git read failed".into(),
    };
    assert_eq!(crate::files::path_count(Some(&state)), Some(1));
    let html = render_files(Some(state));
    for text in [
        "Last read at 1234 ms since Unix epoch: Git read failed",
        "Untracked · 1",
    ] {
        assert!(html.contains(text), "missing {text}: {html}");
    }
    assert!(!html.contains("Working tree clean"), "{html}");
}

fn file_row_root(props: (portal_types::files::Entry, usize)) -> Element {
    rsx! { crate::files::FileRow { entry: props.0, stage: props.1 } }
}

#[test]
fn file_rows_show_stage_codes_and_rename_paths_without_actions() {
    use portal_types::files::{Change, GitPath};
    for (change, stage, code, previous) in [
        (
            Change::Renamed {
                index: 'R',
                worktree: '.',
                from: GitPath::new(b"old/<name>.rs"),
            },
            1,
            "R",
            true,
        ),
        (
            Change::Ordinary {
                index: 'A',
                worktree: 'M',
            },
            1,
            "A",
            false,
        ),
        (
            Change::Ordinary {
                index: 'A',
                worktree: 'M',
            },
            2,
            "M",
            false,
        ),
        (Change::Conflict { code: ['U', 'U'] }, 0, "UU", false),
        (Change::Untracked, 3, "?", false),
    ] {
        let mut dom = VirtualDom::new_with_props(
            file_row_root,
            (file_entry(b"long/path/<new>.rs", change), stage),
        );
        dom.rebuild_in_place();
        let html = dioxus_ssr::render(&dom);
        // The renderer may spell the escape as a named or numeric entity; neither is markup.
        assert!(
            html.contains("long/path/") && !html.contains("<new>"),
            "{html}"
        );
        assert!(
            html.contains(&format!(r#"class="files-code">{code}</span>"#)),
            "{html}"
        );
        assert_eq!(
            html.contains("from old/") && !html.contains("<name>"),
            previous,
            "{html}"
        );
        for action in [
            "<button",
            "<a ",
            "href=",
            "onclick",
            "onkeydown",
            "tabindex=",
        ] {
            assert!(!html.contains(action), "unexpected {action}: {html}");
        }
    }
}

#[test]
fn files_count_exact_paths_even_when_display_text_matches() {
    use portal_types::files::{Change, FilesState};
    let state = FilesState::Current(checkout(vec![
        file_entry(b"\xff", Change::Untracked),
        file_entry(b"\xfe", Change::Untracked),
    ]));
    assert_eq!(crate::files::path_count(Some(&state)), Some(2));
}

fn worker_lead() -> crate::board::NavigationRow {
    crate::board::NavigationRow {
        pane: Some("lead".into()),
        title: "Ship the portal".into(),
        role: "lead".into(),
        host: "local".into(),
        state: "working".into(),
        worker: false,
        unseen: false,
        workers: Vec::new(),
        progress: Some((2, 6)),
    }
}

fn worker_row(id: &str, state: &str) -> Result<portal_types::RowView, serde_json::Error> {
    serde_json::from_value(serde_json::json!({
        "group": "running", "kind": "agent", "id": id, "name": id, "title": format!("Task {id}"),
        "what": "pi · model", "host": "netcup", "pane": id, "owner_pane": "lead", "owner_host": "local",
        "state": state, "activity": format!("Latest activity for {id}"), "age_ms": 120_000, "quiet_ms": 13_000
    }))
}

fn hold_row(id: &str, owner: Option<&str>) -> Result<portal_types::RowView, serde_json::Error> {
    serde_json::from_value(serde_json::json!({
        "group": "attention", "kind": "hold", "id": id, "name": id,
        "what": "resource hold", "host": "local", "pane": null, "owner_pane": owner,
        "state": "held", "activity": "", "age_ms": 0
    }))
}

fn worker_context(selected: Option<portal_types::agents::AgentAddress>) {
    use_context_provider(|| Signal::new(selected));
    use_context_provider(|| Signal::new(None::<crate::app::InboxSelection>));
    use_context_provider(|| Signal::new(crate::app::LeadPanel::Chat));
    use_context_provider(|| crate::app::LeadDetail(Signal::new(crate::app::LeadPanel::Workers)));
}

fn workers_root(props: (Vec<portal_types::RowView>, Vec<crate::app::InboxItem>)) -> Element {
    worker_context(Some(portal_types::agents::AgentAddress {
        host: "netcup".into(),
        pane: "working".into(),
    }));
    rsx! { crate::workers::Workers { rows: props.0, inbox: props.1 } }
}

#[test]
fn workers_render_the_four_groups_in_order_and_mark_the_open_worker()
-> Result<(), serde_json::Error> {
    let mut unseen = worker_row("unseen", "done")?;
    unseen.unseen = true;
    let prompt = crate::app::InboxItem {
        selection: crate::app::InboxSelection {
            host: "netcup".into(),
            pane: "prompt".into(),
            kind: crate::app::InboxKind::Prompt,
            id: "prompt".into(),
        },
        revision: 0,
        title: "Key prompt".into(),
        agent_title: None,
        repository: None,
        age_ms: 0,
    };
    let rows = vec![
        worker_row("finished", "closed")?,
        worker_row("idle", "idle")?,
        worker_row("working", "working")?,
        unseen,
        worker_row("prompt", "blocked")?,
    ];
    let mut dom = VirtualDom::new_with_props(workers_root, (rows, vec![prompt]));
    dom.rebuild_in_place();
    let html = dioxus_ssr::render(&dom);
    let headings = [
        "Waiting on you · 2",
        "Working · 1",
        "Idle · 1",
        "Finished · 1",
    ];
    let mut previous = 0;
    for heading in headings {
        let position = html.find(&format!("<h3>{heading}</h3>"));
        assert!(
            position.is_some_and(|position| position >= previous),
            "missing/out of order {heading}: {html}"
        );
        previous = position.unwrap_or(previous);
    }
    for expected in [
        "1 working",
        "Task working",
        "working · 13s · netcup",
        "Latest activity for working",
        "Finished · 13s · netcup",
        r#"aria-current="true""#,
    ] {
        assert!(html.contains(expected), "missing {expected}: {html}");
    }
    assert!(!html.contains("Held by the lead"));
    Ok(())
}

#[test]
fn worker_rows_filter_by_owner_host_and_pane_including_closed() -> Result<(), serde_json::Error> {
    let mut foreign_host = worker_row("foreign-host", "working")?;
    foreign_host.owner_host = Some("netcup".into());
    let mut foreign_pane = worker_row("foreign-pane", "idle")?;
    foreign_pane.owner_pane = Some("other-lead".into());
    let board = portal_types::BoardView {
        hosts: Vec::new(),
        generated_ms: 0,
        repos: vec![portal_types::RepoGroup {
            repository: None,
            todos: Vec::new(),
            rows: vec![
                worker_row("working", "working")?,
                foreign_host,
                foreign_pane,
                worker_row("closed", "closed")?,
            ],
        }],
    };
    let rows = crate::workers::owned_rows(Some(&board), Some(&worker_lead()));
    assert_eq!(
        rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        ["working", "closed"]
    );
    assert!(crate::workers::owned_rows(Some(&board), None).is_empty());
    Ok(())
}

fn worker_sidebar_root(
    props: (crate::board::NavigationRepo, Vec<crate::app::InboxItem>),
) -> Element {
    worker_context(Some(portal_types::agents::AgentAddress {
        host: "netcup".into(),
        pane: "worker".into(),
    }));
    rsx! { crate::components::sidebar::SidebarProvider { crate::board::RepoGroup { group: props.0, inbox: props.1 } } }
}

#[test]
fn resource_holds_are_excluded_from_owned_workers_and_sidebar_summary()
-> Result<(), serde_json::Error> {
    let mut job = worker_row("job", "done")?;
    job.kind = "job".into();
    job.pane = None;
    job.group = portal_types::GroupView::ToLand;
    let board = portal_types::BoardView {
        hosts: Vec::new(),
        generated_ms: 0,
        repos: vec![portal_types::RepoGroup {
            repository: Some("owner/repo".into()),
            todos: Vec::new(),
            rows: vec![
                worker_row("worker", "working")?,
                job,
                hold_row("resource", Some("lead"))?,
            ],
        }],
    };
    let agents =
        serde_json::from_value::<Vec<portal_types::agents::AgentView>>(serde_json::json!([{
            "host": "local", "pane": "lead", "name": "Ship the portal",
            "state": "working", "repository": "owner/repo"
        }]))?;
    let navigation = crate::board::navigation_groups(&board, &agents);
    assert_eq!(navigation.repos.len(), 1);
    let repo = &navigation.repos[0];
    assert_eq!(repo.agent_count(), 1);
    assert_eq!(repo.rows.len(), 1);
    assert_eq!(repo.to_land, 1);
    let lead = &repo.rows[0];
    assert_eq!(
        lead.workers
            .iter()
            .map(|row| row.role.as_str())
            .collect::<Vec<_>>(),
        ["agent", "job"]
    );
    let owned = crate::workers::owned_rows(Some(&board), Some(lead));
    assert_eq!(
        owned.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        ["worker", "job"]
    );
    let mut dom = VirtualDom::new_with_props(worker_sidebar_root, (repo.clone(), Vec::new()));
    dom.rebuild_in_place();
    let html = dioxus_ssr::render(&dom);
    assert!(html.contains("2 workers · 1 working"), "{html}");
    assert!(
        html.contains("Show 2 workers for Ship the portal"),
        "{html}"
    );
    assert_eq!(html.matches("class=\"worker-dot\"").count(), 2, "{html}");
    assert!(!html.contains("resource"), "{html}");
    Ok(())
}

#[test]
fn unowned_resource_holds_never_become_sidebar_rows() -> Result<(), serde_json::Error> {
    for repository in [None, Some("owner/repo".to_owned())] {
        let board = portal_types::BoardView {
            hosts: Vec::new(),
            generated_ms: 0,
            repos: vec![portal_types::RepoGroup {
                repository,
                todos: Vec::new(),
                rows: vec![
                    hold_row("unowned", None)?,
                    hold_row("orphan", Some("missing"))?,
                ],
            }],
        };
        let navigation = crate::board::navigation_groups(&board, &[]);
        assert!(navigation.repos.is_empty());
        assert_eq!(navigation.finished_without_repository, 0);
    }
    Ok(())
}

#[test]
fn sidebar_worker_summary_rolls_up_attention_and_keeps_the_lead_active() {
    let mut lead = worker_lead();
    let mut worker = worker_lead();
    worker.worker = true;
    worker.host = "netcup".into();
    worker.pane = Some("worker".into());
    worker.title = "Not a sidebar entry".into();
    worker.unseen = true;
    lead.workers.push(worker);
    let repo = crate::board::NavigationRepo {
        repository: Some("owner/repo".into()),
        rows: vec![lead],
        to_land: 0,
    };
    assert_eq!(repo.agent_count(), 1);
    let mut dom = VirtualDom::new_with_props(worker_sidebar_root, (repo, Vec::new()));
    dom.rebuild_in_place();
    let html = dioxus_ssr::render(&dom);
    for expected in [
        "1 workers · 1 working",
        "Show 1 workers for Ship the portal",
        "data-waiting=true",
        r#"data-state="waiting""#,
        r#"aria-current="true""#,
    ] {
        assert!(html.contains(expected), "missing {expected}: {html}");
    }
    assert!(!html.contains("Not a sidebar entry"));
    assert!(!html.contains("sidebar-menu-sub"));
}

fn worker_heading_root() -> Element {
    worker_context(None);
    let draft = use_signal(String::new);
    let images = use_signal(Vec::<String>::new);
    let uploading = use_signal(|| false);
    rsx! {
        crate::workers::WorkerHeading { title: "Worker task", lead: worker_lead(), what: "pi · model", host: "netcup" }
        crate::agent_view::Composer { draft, images, uploading, disabled: false, busy: false,
            placeholder: Some("Message the worker…".to_owned()), on_error: |_| {}, on_send: |_| {} }
    }
}

#[test]
fn worker_header_links_back_to_lead_and_composer_targets_the_worker() {
    let html = dioxus_ssr::render(&VirtualDom::prebuilt(worker_heading_root));
    for expected in [
        "Back to lead",
        "Worker task",
        "Worker of ",
        "Ship the portal",
        " · pi · model · netcup",
        r#"placeholder="Message the worker…""#,
    ] {
        assert!(html.contains(expected), "missing {expected}: {html}");
    }
}

fn root() -> Element {
    let subject = String::from("Foundation check");
    let message = String::from("Keep the existing portal layout");
    rsx! {
        form {
            Label { html_for: "smoke-subject", "Subject" }
            Input {
                id: "smoke-subject",
                name: "subject",
                r#type: "text",
                value: subject,
                required: true,
            }
            Label { html_for: "smoke-message", "Message" }
            Textarea {
                id: "smoke-message",
                name: "message",
                value: message,
                aria_describedby: "smoke-help",
            }
            p { id: "smoke-help", "Describe the requested change" }
            Button {
                r#type: "submit",
                name: "action",
                value: "send",
                disabled: true,
                aria_label: "Send message",
                "Send"
            }
        }
    }
}

fn composer_root() -> Element {
    let draft = use_signal(String::new);
    let images = use_signal(Vec::<String>::new);
    let uploading = use_signal(|| false);
    rsx! {
        crate::agent_view::Composer { draft, images, uploading, disabled: false, busy: false,
            on_error: |_| {}, on_send: |_| {} }
    }
}

fn attached_composer_root() -> Element {
    let draft = use_signal(String::new);
    let images = use_signal(|| vec!["/tmp/dogsled-attachments/example.png".to_owned()]);
    let uploading = use_signal(|| false);
    rsx! {
        crate::agent_view::Composer { draft, images, uploading, disabled: true, busy: false,
            on_error: |_| {}, on_send: |_| {} }
    }
}

fn empty_inbox_root() -> Element {
    rsx! {
        crate::components::sidebar::SidebarProvider {
            crate::app::InboxSidebar { count: Some(0), loading: false, error: None, items: Vec::new(),
                elapsed_ms: 0, agents: Vec::new(), reachable: true }
        }
    }
}

#[test]
fn composer_has_only_accessible_icon_actions_and_an_unprompted_field() {
    let html = dioxus_ssr::render(&VirtualDom::prebuilt(composer_root));
    assert!(html.contains(r#"aria-label="Message this agent""#));
    assert!(html.contains(r#"aria-label="Attach images""#));
    assert!(html.contains(r#"aria-label="Send message""#));
    assert!(html.contains(r#"accept="image/png,image/jpeg,image/webp""#));
    assert!(!html.contains("placeholder="));
    let text: String = html
        .split('<')
        .skip(1)
        .filter_map(|element| element.split_once('>').map(|(_, text)| text))
        .collect();
    assert!(text.trim().is_empty(), "unexpected composer text: {text}");
}

#[test]
fn attached_composer_shows_preview_removal_and_disabled_controls() {
    let html = dioxus_ssr::render(&VirtualDom::prebuilt(attached_composer_root));
    assert!(html.contains(r#"src="/api/attachments/example.png""#));
    assert!(html.contains(r#"aria-label="Remove image 1""#));
    for button in html.split("<button").skip(1) {
        let attributes = button.split('>').next().unwrap_or_default();
        assert!(
            attributes.contains("disabled"),
            "enabled action: {attributes}"
        );
    }
    assert!(!html.contains("placeholder="));
}

#[test]
fn empty_inbox_keeps_its_heading_and_zero_count() {
    let html = dioxus_ssr::render(&VirtualDom::prebuilt(empty_inbox_root));
    assert!(html.contains("Waiting on you"));
    assert!(html.contains(">0</span>"));
    assert!(html.contains("Nothing is waiting on you."));
    assert!(!html.contains("portal-inbox"));
    assert!(html.contains(r#"data-part="column-empty""#), "{html}");
    assert!(html.contains(r#"role="status""#), "{html}");
    assert!(html.contains(r#"data-slot="item""#), "{html}");
    assert_badge_class(&html, "inbox-count");
}

#[test]
fn message_keeps_text_and_appends_only_attached_references() {
    use crate::agent_view::message_with_images;
    assert_eq!(message_with_images("Review this.", &[]), "Review this.");
    let images = vec!["/tmp/a.png".to_owned(), "/tmp/b.webp".to_owned()];
    assert_eq!(
        message_with_images("Review this.", &images),
        "Review this.\n\nReference images:\n- /tmp/a.png\n- /tmp/b.webp"
    );
    assert_eq!(
        message_with_images("", &images),
        "\n\nReference images:\n- /tmp/a.png\n- /tmp/b.webp"
    );
}

fn review_root(props: (u32, u32, bool)) -> Element {
    use portal_types::reviews::{Annotation, Review, Revision};
    let width = use_signal(|| props.1);
    use_context_provider(|| crate::app::ConsoleWidth(width));
    let initial = Review {
        id: "review-fixture".to_owned(),
        pane: "pane".to_owned(),
        session: "session".to_owned(),
        cwd: "/repo/dogsled".to_owned(),
        path: "plans/fallback.md".to_owned(),
        revisions: (1..=props.0)
            .map(|n| Revision {
                n,
                content: "# Typed **SQLite** state store\n\nKeep the ledger.".to_owned(),
                submitted_ms: 0,
            })
            .collect(),
        annotations: if props.2 {
            vec![Annotation {
                id: "note".to_owned(),
                revision: props.0,
                start_line: 3,
                end_line: 3,
                quote: "Keep the ledger.".to_owned(),
                comment: "Keep it typed.".to_owned(),
                images: Vec::new(),
            }]
        } else {
            Vec::new()
        },
        general: String::new(),
        decision: None,
    };
    rsx! { crate::review_view::ReviewEditor { initial, embedded: true,
    lead: "Move state into typed SQLite with a very long task title", role: "lead",
    repository: "github.com/oisin-ee/dogsled", on_close: |_| {} } }
}

fn question_root() -> Element {
    use portal_types::questions::{Pending, QuestionSet};
    let questions: QuestionSet = match serde_json::from_value(serde_json::json!({
        "title": "Choose the renderer", "questions": [
            {"id": "ui", "type": "single", "question": "Which renderer?", "recommended": "Dioxus",
             "content": {"source": "Use **Rust** and `WASM`.", "lang": "md"},
             "options": [{"label": "Dioxus", "content": {"source": "Keep **one** language.", "lang": "markdown"}}]},
            {"id": "notes", "type": "text", "question": "Anything else?"}
        ]
    })) {
        Ok(questions) => questions,
        Err(error) => return rsx! { "Invalid question fixture: {error}" },
    };
    let pending = Pending {
        id: "question-fixture".to_owned(),
        pane: "pane".to_owned(),
        session: "session".to_owned(),
        cwd: "/repo/dogsled".to_owned(),
        created_ms: 0,
        questions,
    };
    rsx! { crate::question_form::AskForm { pending, elapsed_ms: 120_000, on_done: |_| {} } }
}

#[test]
fn embedded_review_has_one_header_and_revision_aware_tabs() -> Result<(), Box<dyn std::error::Error>>
{
    for (revisions, width) in [(1, 1440), (2, 1440), (1, 1024), (2, 1024)] {
        let mut dom = VirtualDom::new_with_props(review_root, (revisions, width, false));
        dom.rebuild_in_place();
        let html = dioxus_ssr::render(&dom);
        assert!(html.contains("Typed SQLite state store"), "{html}");
        assert!(html.contains(r#"class="review-lead-title""#), "{html}");
        assert!(
            html.contains(">Move state into typed SQLite with a very long task title</span>"),
            "{html}"
        );
        assert!(html.contains(">· lead</span>"), "{html}");
        assert!(html.contains(">· dogsled</span>"), "{html}");
        assert!(!html.contains("github.com/oisin-ee/dogsled"), "{html}");
        assert!(
            html.contains(r#"class="review-prose markdown-body""#),
            "{html}"
        );
        assert!(html.contains(r#"aria-label="Close plan review""#), "{html}");
        assert!(html.contains(">Plan</button>"), "{html}");
        assert_eq!(html.contains(">Changes</button>"), revisions > 1, "{html}");
        assert_eq!(html.contains(">Notes 0</button>"), width < 1100, "{html}");
        assert_eq!(html.matches("Approve plan").count(), 1, "{html}");
        if width >= 1100 {
            assert!(
                html.contains("Select text in the plan to add a note."),
                "{html}"
            );
        }
        let field = html
            .split("<textarea")
            .skip(1)
            .find(|field| field.contains("review-general"))
            .ok_or("Missing general note")?;
        let attributes = field
            .split('>')
            .next()
            .ok_or("Missing textarea attributes")?;
        assert!(
            attributes.contains(r#"aria-label="Note for the whole plan""#),
            "{html}"
        );
        assert!(!attributes.contains("placeholder="), "{html}");
    }
    Ok(())
}

#[test]
fn review_notes_use_accessible_icon_controls() {
    let mut dom = VirtualDom::new_with_props(review_root, (1, 1440, true));
    dom.rebuild_in_place();
    let html = dioxus_ssr::render(&dom);
    for expected in [
        "<blockquote>Keep the ledger.</blockquote>",
        "Keep it typed.",
        r#"aria-label="Edit note""#,
        r#"aria-label="Delete note""#,
    ] {
        assert!(html.contains(expected), "missing {expected}: {html}");
    }
}

#[test]
fn question_form_counts_answers_and_renders_markdown_content() {
    let html = dioxus_ssr::render(&VirtualDom::prebuilt(question_root));
    for expected in [
        "1 of 2 answered",
        "Send answers",
        "2m",
        "Recommended",
        "Other",
        "<strong>Rust</strong>",
        "<code>WASM</code>",
        "<strong>one</strong>",
        "question-decision",
    ] {
        assert!(html.contains(expected), "missing {expected}: {html}");
    }
    assert!(!html.contains("**Rust**"), "{html}");
    assert!(!html.contains("**one**"), "{html}");
    assert_badge_class(&html, "recommended-badge");
}

fn badge_root() -> Element {
    rsx! { crate::components::badge::Badge { "Component class baseline" } }
}

fn assert_badge_class(html: &str, part: &str) {
    let baseline = dioxus_ssr::render(&VirtualDom::prebuilt(badge_root));
    let class = baseline
        .split("class=\"")
        .nth(1)
        .and_then(|value| value.split('"').next());
    let tag = html.split('<').find(|tag| {
        tag.starts_with("span ")
            && tag
                .split('>')
                .next()
                .is_some_and(|attributes| attributes.contains(&format!("data-part=\"{part}\"")))
    });
    assert!(class.is_some_and(|class| tag.is_some_and(|tag| tag.contains(&format!("class=\"{class}\"")))), "missing official badge class for {part}: {html}");
}

fn todos_root() -> Element {
    use portal_types::{TodoChildView, TodoItemView, TodoSectionView, TodoStatusView, TodoView};
    let item = |status, text: &str| TodoItemView {
        status,
        text: text.to_owned(),
        description: Some(format!("{text} description")),
        review: false,
        unit: Some("worker".to_owned()),
        children: vec![TodoChildView {
            status,
            text: format!("{text} child"),
            description: Some(format!("{text} child description")),
            unit: Some("worker".to_owned()),
        }],
    };
    let todos = vec![TodoView {
        session: "session".to_owned(),
        branch: None,
        revision: 1,
        current: "Work".to_owned(),
        pane: "pane".to_owned(),
        sections: vec![
            TodoSectionView {
                status: TodoStatusView::Pending,
                count: 0,
                items: vec![],
            },
            TodoSectionView {
                status: TodoStatusView::InProgress,
                count: 1,
                items: vec![item(TodoStatusView::InProgress, "Active")],
            },
            TodoSectionView {
                status: TodoStatusView::Completed,
                count: 1,
                items: vec![item(TodoStatusView::Completed, "Done")],
            },
            TodoSectionView {
                status: TodoStatusView::Abandoned,
                count: 1,
                items: vec![item(TodoStatusView::Abandoned, "Dropped")],
            },
        ],
    }];
    rsx! { crate::agent_view::Todos { todos, workers: vec![("worker".to_owned(), "Assigned worker".to_owned())] } }
}

#[test]
fn todos_count_all_items_and_hide_empty_sections() {
    let html = dioxus_ssr::render(&VirtualDom::prebuilt(todos_root));
    assert_accordion_has_no_inline_width(&html);
    assert!(html.contains(">1 of 3</span>"), "{html}");
    assert!(!html.contains("Pending · 0"), "{html}");
    // AccordionContent mounts its body in the browser; SSR covers the section triggers.
    for text in ["In progress · 1", "Completed · 1", "Abandoned · 1"] {
        assert!(html.contains(text), "missing {text}: {html}");
    }
}

fn empty_todos_root() -> Element {
    rsx! { crate::agent_view::Todos { todos: Vec::new(), workers: Vec::new() } }
}

#[test]
fn empty_todos_use_muted_column_copy_without_sections() {
    let html = dioxus_ssr::render(&VirtualDom::prebuilt(empty_todos_root));
    assert!(html.contains(">0 of 0</span>"), "{html}");
    assert!(
        html.contains("No todos are recorded for this agent."),
        "{html}"
    );
    assert!(html.contains(r#"data-part="column-empty""#), "{html}");
    assert!(html.contains(r#"role="status""#), "{html}");
    assert!(html.contains(r#"data-slot="item""#), "{html}");
    assert!(!html.contains("Pending"), "{html}");
}

fn history_turn(
    id: &str,
    role: portal_types::agents::Role,
    text: &str,
) -> portal_types::agents::Turn {
    use portal_types::agents::{Source, Turn};
    Turn {
        id: id.into(),
        role,
        text: text.into(),
        source: Source::Harness {
            harness: "pi".into(),
            model: None,
            provider: None,
        },
        timestamp: None,
        tool: None,
    }
}

fn history(turns: Vec<portal_types::agents::Turn>) -> portal_types::agents::Conversation {
    portal_types::agents::Conversation {
        pane: "pane".into(),
        session: "session".into(),
        turns,
        truncated: false,
    }
}

fn timeline_root(snapshot: portal_types::agents::Conversation) -> Element {
    rsx! { crate::agent_view::Conversation { snapshot: Some(snapshot), loading: false, assistant: "Selected assistant" } }
}

fn render_timeline(snapshot: portal_types::agents::Conversation) -> String {
    let mut dom = VirtualDom::new_with_props(timeline_root, snapshot);
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

#[test]
fn timeline_attributes_native_input_assistant_deliveries_and_extension_context() {
    use portal_types::agents::{Role, Source};
    let owner = history_turn("owner", Role::User, "Owner input.");
    let mut assistant = history_turn("reply", Role::Assistant, "**Actual** reply.");
    assistant.source = Source::Harness {
        harness: "pi".into(),
        model: Some("recorded-model".into()),
        provider: Some("recorded-provider".into()),
    };
    // Even if a source arrives with a user role, attribution must not turn it into You.
    let mut delivery = history_turn("delivery", Role::User, "Worker report.");
    delivery.source = Source::Dogsled {
        sender: "Layout reviewer with a very long name".into(),
        delivery_kind: "task".into(),
    };
    let mut context = history_turn("context", Role::User, "Gate context.");
    context.source = Source::Custom {
        custom_type: "dogsled-gate".into(),
    };
    let html = render_timeline(history(vec![owner, assistant, delivery, context]));
    for text in [
        ">You</strong>",
        ">Selected assistant</strong>",
        "pi · recorded-model · recorded-provider",
        ">Layout reviewer with a very long name</strong>",
        "Dogsled · task",
        ">dogsled-gate</strong>",
        "Extension context",
        "<strong>Actual</strong>",
    ] {
        assert!(html.contains(text), "missing {text}: {html}");
    }
    assert_eq!(
        html.matches("conversation-event conversation-owner")
            .count(),
        1,
        "{html}"
    );
    assert_eq!(html.matches(r#"aria-label="You""#).count(), 1, "{html}");
}

#[test]
fn missing_metadata_stays_missing_and_markdown_keeps_scrollable_code_and_tables() {
    use portal_types::agents::Role;
    let html = render_timeline(history(vec![history_turn(
        "missing",
        Role::Assistant,
        "```rust\nlet answer = 42;\n```\n\n| Column |\n| --- |\n| Value |",
    )]));
    assert!(html.contains(">Selected assistant</strong>"), "{html}");
    assert!(html.contains(">pi</span>"), "{html}");
    assert!(html.contains(r#"aria-label="Time unavailable""#), "{html}");
    assert!(html.contains("—"), "{html}");
    assert!(!html.contains("datetime="), "{html}");
    assert!(!html.contains("Unknown model"), "{html}");
    assert!(!html.contains("recorded-model"), "{html}");
    assert!(
        html.contains("<pre ") && html.contains("<code class=\"language-rust\""),
        "{html}"
    );
    assert!(html.contains("<table "), "{html}");
    assert!(html.contains("markdown-body"), "{html}");
}

#[test]
fn timestamp_semantics_preserve_the_platform_boundary_and_do_not_claim_response_completion() {
    use portal_types::agents::{Role, Source, TimeMeaning, Timestamp};
    let cases = [
        (
            "request",
            Role::Assistant,
            TimeMeaning::Message,
            "Request started (not response completion)",
        ),
        (
            "message",
            Role::User,
            TimeMeaning::Message,
            "Harness message time",
        ),
        (
            "recorded",
            Role::Context,
            TimeMeaning::Recorded,
            "Transcript entry recorded",
        ),
        (
            "created",
            Role::Context,
            TimeMeaning::Created,
            "Dogsled delivery created",
        ),
    ];
    for (id, role, meaning, expected) in cases {
        let mut turn = history_turn(id, role, "Recorded text.");
        turn.timestamp = Some(Timestamp {
            ms: 1_799_999_999_123,
            meaning,
        });
        let html = render_timeline(history(vec![turn]));
        assert!(html.contains("<time "), "{html}");
        assert!(
            html.contains(r#"data-timestamp-ms="1799999999123""#),
            "{html}"
        );
        assert!(html.contains(expected), "{html}");
        // Native SSR must not invent a calendar or a test timezone for a browser-only UI.
        assert!(!html.contains("datetime="), "{html}");
        assert!(
            html.contains("date and time formatting unavailable"),
            "{html}"
        );
    }
    let mut other = history_turn("other-harness", Role::Assistant, "Reply.");
    other.source = Source::Harness {
        harness: "claude".into(),
        model: None,
        provider: None,
    };
    other.timestamp = Some(Timestamp {
        ms: 1000,
        meaning: TimeMeaning::Message,
    });
    let html = render_timeline(history(vec![other]));
    assert!(html.contains("Harness message time"), "{html}");
    assert!(!html.contains("Request started"), "{html}");
}

fn tool_turn(
    id: &str,
    name: Option<&str>,
    status: portal_types::agents::ToolStatus,
) -> portal_types::agents::Turn {
    use portal_types::agents::{Role, ToolSummary};
    let mut turn = history_turn(id, Role::Tool, "Do not render raw tool text or reasoning.");
    turn.tool = Some(ToolSummary {
        call: None,
        output: None,
        output_lines: None,
        call_id: format!("call-{id}"),
        name: name.map(str::to_owned),
        status,
    });
    turn
}

#[test]
fn tools_render_individually_in_recorded_order_with_name_fallbacks_and_outcomes() {
    use portal_types::agents::{Role, ToolStatus};
    let html = render_timeline(history(vec![
        tool_turn("read", Some("read"), ToolStatus::Completed),
        tool_turn("bash", Some("bash"), ToolStatus::Error),
        tool_turn("unknown", None, ToolStatus::Unknown),
        history_turn("reply", Role::Assistant, "Separate reply."),
        tool_turn("search", Some("search"), ToolStatus::Unknown),
    ]));
    assert_eq!(
        html.matches("conversation-event conversation-tool\"")
            .count(),
        4,
        "{html}"
    );
    assert!(!html.contains("aria-expanded=false"), "{html}");
    for expected in [
        ">read</pre>",
        ">bash</pre>",
        ">Unnamed tool</pre>",
        ">search</pre>",
        "Result recorded",
        "Error recorded",
        "No recorded outcome",
        "data-error=true",
        r#"data-record-id="unknown""#,
    ] {
        assert!(html.contains(expected), "missing {expected}: {html}");
    }
    assert!(!html.contains("Do not render raw tool text"), "{html}");
    assert!(!html.contains("Running"), "{html}");
    assert!(!html.contains("Success"), "{html}");
    assert!(!html.contains("<details"), "{html}");
    let positions = ["read", "bash", "unknown", "reply", "search"]
        .map(|id| html.find(&format!("data-record-id=\"{id}\"")));
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{html}");
}

#[test]
fn thinking_and_tool_payloads_keep_markdown_calls_output_tails_and_errors() {
    use portal_types::agents::{Role, ToolStatus, ToolSummary};
    let mut tool = tool_turn("call", Some("bash"), ToolStatus::Error);
    tool.tool = Some(ToolSummary {
        call_id: "call".into(),
        name: Some("bash".into()),
        status: ToolStatus::Error,
        call: Some("$ git status --short".into()),
        output: Some("line <one>\nline two".into()),
        output_lines: Some(25),
    });
    let html = render_timeline(history(vec![
        history_turn("thought", Role::Thinking, "Check **the cause** first."),
        tool,
        history_turn("reply", Role::Assistant, "Final reply."),
    ]));
    for text in [
        "conversation-thought",
        "Check <strong>the cause</strong> first.",
        "$ git status --short",
        "23 earlier lines",
        "line &#60;one&#62;\nline two",
        "data-error=true",
        "Error recorded",
    ] {
        assert!(html.contains(text), "missing {text}: {html}");
    }
    assert!(
        html.find("Check <strong>") < html.find("$ git status"),
        "{html}"
    );
    assert!(
        html.find("line &#60;one&#62;") < html.find("Final reply."),
        "{html}"
    );
    let css = include_str!("../assets/console.css");
    assert!(css.contains("font-style: italic"));
    assert!(css.contains("color: var(--primary-error-color)"));
    assert!(css.contains("white-space: pre-wrap"));
    assert!(css.contains("overflow-wrap: anywhere"));
}

fn worked_history(timed: bool) -> portal_types::agents::Conversation {
    use portal_types::agents::{Role, TimeMeaning, Timestamp, ToolStatus};
    let mut turns = vec![
        history_turn("input", Role::User, "Please check."),
        history_turn("thought", Role::Thinking, "Checking."),
        tool_turn("call", Some("read"), ToolStatus::Completed),
        history_turn("commentary", Role::Assistant, "One more check."),
        history_turn("thought-two", Role::Thinking, "Confirming."),
        tool_turn("call-two", Some("bash"), ToolStatus::Completed),
        history_turn("final", Role::Assistant, "Finished reply."),
    ];
    if timed {
        for (index, turn) in turns.iter_mut().enumerate() {
            turn.timestamp = Some(Timestamp {
                ms: 1000 + index as u64 * 1000,
                meaning: TimeMeaning::Recorded,
            });
        }
    }
    history(turns)
}

#[test]
fn finished_turn_work_folds_with_counts_and_only_recorded_time_spans() {
    for (timed, expected) in [
        (true, "Worked for 6s · 2 thoughts · 2 tool calls"),
        (false, "Worked · 2 thoughts · 2 tool calls"),
    ] {
        let html = render_timeline(worked_history(timed));
        assert!(html.contains(expected), "{html}");
        assert_eq!(html.matches("aria-expanded=false").count(), 1, "{html}");
        assert!(html.contains("data-open=false"), "{html}");
        assert!(html.contains("data-part=\"turn-work\""), "{html}");
        assert!(html.find("Please check.") < html.find(expected), "{html}");
        assert!(
            html.find("Confirming.") < html.find("Finished reply."),
            "{html}"
        );
        if !timed {
            assert!(!html.contains("Worked for"), "{html}");
        }
    }
    // Neither a missing owner time nor missing record times supplies a span.
    let mut missing_owner = worked_history(true);
    missing_owner.turns[0].timestamp = None;
    assert!(render_timeline(missing_owner).contains("Worked · 2 thoughts · 2 tool calls"));
    let mut missing_records = worked_history(true);
    for turn in missing_records.turns.iter_mut().skip(1) {
        turn.timestamp = None;
    }
    assert!(render_timeline(missing_records).contains("Worked · 2 thoughts · 2 tool calls"));
}

#[test]
fn worked_span_uses_the_latest_record_time_even_when_the_final_reply_time_is_earlier() {
    use portal_types::agents::{TimeMeaning, Timestamp};
    let mut snapshot = worked_history(true);
    snapshot.turns[2].timestamp = Some(Timestamp {
        ms: 12_000,
        meaning: TimeMeaning::Recorded,
    });
    snapshot.turns[4].timestamp = None;
    let html = render_timeline(snapshot);
    assert!(
        html.contains("Worked for 11s · 2 thoughts · 2 tool calls"),
        "{html}"
    );
    assert!(!html.contains("Worked for 6s"), "{html}");
}

fn live_timeline_root(
    props: (portal_types::agents::Conversation, crate::activity::Live),
) -> Element {
    rsx! { crate::agent_view::Conversation { snapshot: Some(props.0), loading: false, assistant: "Assistant", live: props.1, coarse: "working" } }
}

fn render_live_timeline(
    snapshot: portal_types::agents::Conversation,
    live: crate::activity::Live,
) -> String {
    let mut dom = VirtualDom::new_with_props(live_timeline_root, (snapshot, live));
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

#[test]
fn newest_active_turn_stays_open_and_live_thought_and_calls_are_at_the_edge() {
    use crate::activity::Live;
    use portal_types::agents::{ActiveTool, LivePhase, LiveState};
    let state = Live::Detail(LiveState {
        phase: LivePhase::ToolRunning,
        reasoning: Some("Newest **live** thought.".into()),
        active_tools: vec![
            ActiveTool {
                call_id: "running".into(),
                name: "bash".into(),
                parent_call_id: None,
                call: Some("$ cargo test -p web".into()),
            },
            ActiveTool {
                call_id: "fallback".into(),
                name: "read".into(),
                parent_call_id: None,
                call: None,
            },
        ],
    });
    let html = render_live_timeline(worked_history(true), state);
    assert!(!html.contains("Worked for"), "{html}");
    assert!(
        html.contains("Newest <strong>live</strong> thought."),
        "{html}"
    );
    assert!(
        html.find("Finished reply.") < html.find("Newest <strong>"),
        "{html}"
    );
    assert!(
        html.find("Newest <strong>") < html.find("$ cargo test -p web"),
        "{html}"
    );
    assert!(html.contains(">read</pre>"), "{html}");
    assert!(html.contains("conversation-live-thought"), "{html}");
    let mut two_turns = worked_history(true);
    two_turns
        .turns
        .extend(worked_history(false).turns.into_iter().map(|mut turn| {
            turn.id = format!("new-{}", turn.id);
            turn
        }));
    let html = render_live_timeline(two_turns, Live::Pending);
    assert_eq!(html.matches("Worked for 6s").count(), 1, "{html}");
    assert!(!html.contains("Worked ·"), "{html}");
}

type StreamHandle = std::rc::Rc<
    std::cell::Cell<Option<Signal<(portal_types::agents::Conversation, crate::activity::Live)>>>,
>;

fn changing_stream_root(handle: StreamHandle) -> Element {
    use crate::activity::Live;
    use portal_types::agents::{LivePhase, LiveState};
    let stream = use_signal(|| {
        (
            worked_history(true),
            Live::Detail(LiveState {
                phase: LivePhase::Thinking,
                reasoning: Some("Thought at the edge.".into()),
                active_tools: Vec::new(),
            }),
        )
    });
    handle.set(Some(stream));
    let (snapshot, live) = stream();
    rsx! { crate::agent_view::Conversation { snapshot: Some(snapshot), loading: false, assistant: "Assistant", live, coarse: "working" } }
}

#[test]
fn message_end_removes_live_copy_and_transcript_supplies_the_finished_thought()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::activity::Live;
    use portal_types::agents::{LivePhase, LiveState, Role};
    let handle = StreamHandle::default();
    let mut dom = VirtualDom::new_with_props(changing_stream_root, handle.clone());
    dom.rebuild_in_place();
    assert_eq!(
        dioxus_ssr::render(&dom)
            .matches("Thought at the edge.")
            .count(),
        1
    );
    let mut stream = handle.get().ok_or("Missing stream signal")?;
    dom.in_runtime(|| {
        let mut state = stream.write();
        state.0.turns.push(history_turn(
            "finished-thought",
            Role::Thinking,
            "Thought at the edge.",
        ));
        state.1 = Live::Detail(LiveState {
            phase: LivePhase::Responding,
            reasoning: None,
            active_tools: Vec::new(),
        });
    });
    dom.render_immediate_to_vec();
    let html = dioxus_ssr::render(&dom);
    assert_eq!(html.matches("Thought at the edge.").count(), 1, "{html}");
    assert!(!html.contains("conversation-live-thought"), "{html}");
    assert!(
        html.contains("data-record-id=\"finished-thought\""),
        "{html}"
    );
    Ok(())
}

#[derive(Clone, Default, PartialEq)]
struct FollowProbe {
    height: std::rc::Rc<std::cell::Cell<f64>>,
    positions: std::rc::Rc<std::cell::RefCell<Vec<f64>>>,
}

impl dioxus::html::RenderedElementBacking for FollowProbe {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn get_scroll_size(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = dioxus::html::MountedResult<dioxus::html::geometry::PixelsSize>,
                >,
        >,
    > {
        let height = self.height.get();
        Box::pin(async move { Ok(dioxus::html::geometry::PixelsSize::new(100.0, height)) })
    }
    fn scroll(
        &self,
        position: dioxus::html::geometry::PixelsVector2D,
        behavior: ScrollBehavior,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = dioxus::html::MountedResult<()>>>> {
        assert_eq!(behavior, ScrollBehavior::Instant);
        self.positions.borrow_mut().push(position.y);
        Box::pin(async { Ok(()) })
    }
}

type FollowHandle =
    std::rc::Rc<std::cell::Cell<Option<(Signal<crate::activity::Live>, Signal<bool>)>>>;

fn follow_root(props: (FollowProbe, FollowHandle)) -> Element {
    let page = use_signal(|| Some(std::rc::Rc::new(MountedData::new(props.0))));
    let following = use_signal(|| true);
    let error = use_signal(|| None::<String>);
    let live = use_signal(|| crate::activity::Live::Pending);
    props.1.set(Some((live, following)));
    crate::agent_view::use_follow_scroll(
        page,
        following,
        error,
        Some(worked_history(true)),
        false,
        true,
        live(),
    );
    rsx! { p { "Following" } }
}

#[test]
fn live_updates_follow_the_growing_edge_but_do_not_pull_the_owner_out_of_history()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::activity::Live;
    use portal_types::agents::{LivePhase, LiveState};
    let handle = FollowHandle::default();
    let probe = FollowProbe::default();
    probe.height.set(100.0);
    let mut dom = VirtualDom::new_with_props(follow_root, (probe.clone(), handle.clone()));
    dom.rebuild_in_place();
    let (mut live, mut following) = handle.get().ok_or("Missing follow signals")?;
    dom.render_immediate_to_vec();
    for (text, height) in [
        ("First live words", 200.0),
        ("First live words, then more", 350.0),
    ] {
        probe.height.set(height);
        dom.in_runtime(|| {
            live.set(Live::Detail(LiveState {
                phase: LivePhase::Thinking,
                reasoning: Some(text.into()),
                active_tools: Vec::new(),
            }))
        });
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        assert_eq!(probe.positions.borrow().last(), Some(&height));
    }
    dom.in_runtime(|| following.set(false));
    dom.render_immediate_to_vec();
    let previous = probe.positions.borrow().len();
    probe.height.set(500.0);
    dom.in_runtime(|| {
        live.set(Live::Detail(LiveState {
            phase: LivePhase::Thinking,
            reasoning: Some("Do not move history".into()),
            active_tools: Vec::new(),
        }))
    });
    dom.render_immediate_to_vec();
    dom.render_immediate_to_vec();
    assert_eq!(probe.positions.borrow().len(), previous);
    dom.in_runtime(|| following.set(true));
    dom.render_immediate_to_vec();
    dom.render_immediate_to_vec();
    assert_eq!(probe.positions.borrow().last(), Some(&500.0));
    Ok(())
}

#[test]
fn truncated_history_never_claims_a_complete_or_empty_start() {
    let mut snapshot = history(Vec::new());
    snapshot.truncated = true;
    let html = render_timeline(snapshot);
    assert!(html.contains("Earlier history was not returned."), "{html}");
    assert!(!html.contains("No conversation yet"), "{html}");
    let html = render_timeline(history(Vec::new()));
    assert!(html.contains("No conversation yet"), "{html}");
}

fn pending_history_root() -> Element {
    rsx! { crate::agent_view::Conversation { snapshot: None, loading: true, assistant: "Assistant" } }
}

#[test]
fn loading_history_does_not_wait_for_agent_metadata_or_claim_an_empty_conversation() {
    let html = dioxus_ssr::render(&VirtualDom::prebuilt(pending_history_root));
    assert!(
        html.contains(r#"aria-label="Loading conversation""#),
        "{html}"
    );
    assert!(!html.contains("No conversation yet"), "{html}");
}

type HistoryHandle =
    std::rc::Rc<std::cell::Cell<Option<Signal<portal_types::agents::Conversation>>>>;

fn changing_history_root(props: (portal_types::agents::Conversation, HistoryHandle)) -> Element {
    let snapshot = use_signal(|| props.0);
    props.1.set(Some(snapshot));
    rsx! { crate::agent_view::Conversation { snapshot: Some(snapshot()), loading: false, assistant: "Assistant" } }
}

fn record_scopes(
    dom: &VirtualDom,
    node: &VNode,
    records: &mut std::collections::BTreeMap<String, ScopeId>,
) {
    for (index, dynamic) in node.dynamic_nodes.iter().enumerate() {
        match dynamic {
            DynamicNode::Component(component) => {
                if let Some(key) = &node.key
                    && (component.name.ends_with("::MessageRecord")
                        || component.name.ends_with("::ToolRecord"))
                    && let Some(scope) = component.mounted_scope_id(index, node, dom)
                {
                    records.insert(key.clone(), scope);
                }
                if let Some(scope) = component.mounted_scope(index, node, dom) {
                    record_scopes(dom, scope.root_node(), records);
                }
            }
            DynamicNode::Fragment(children) => {
                for child in children {
                    record_scopes(dom, child, records);
                }
            }
            _ => {}
        }
    }
}

#[test]
fn stable_native_and_offset_keys_preserve_mounted_records_across_history_changes()
-> Result<(), Box<dyn std::error::Error>> {
    use portal_types::agents::{Role, ToolStatus};
    let handle = HistoryHandle::default();
    let initial = history(vec![
        history_turn("native-reply", Role::Assistant, "Kept reply."),
        tool_turn(
            "pi:offset:7:tool:call-id",
            Some("read"),
            ToolStatus::Unknown,
        ),
        history_turn("pi:offset:8", Role::User, "Kept input."),
    ]);
    let mut dom = VirtualDom::new_with_props(changing_history_root, (initial, handle.clone()));
    dom.rebuild_in_place();
    let mut before = std::collections::BTreeMap::new();
    record_scopes(&dom, dom.base_scope().root_node(), &mut before);
    assert_eq!(before.len(), 3, "keys were not mounted: {before:?}");
    assert!(before.contains_key("native-reply"));
    assert!(before.contains_key("pi:offset:7:tool:call-id"));
    assert!(before.contains_key("pi:offset:8"));
    let mut signal = handle.get().ok_or("Missing history signal")?;
    dom.in_runtime(|| {
        let mut snapshot = signal.write();
        snapshot
            .turns
            .insert(0, history_turn("earlier", Role::User, "Earlier message."));
        if let Some(tool) = snapshot
            .turns
            .get_mut(2)
            .and_then(|turn| turn.tool.as_mut())
        {
            tool.status = ToolStatus::Completed;
        }
    });
    dom.render_immediate_to_vec();
    let mut after = std::collections::BTreeMap::new();
    record_scopes(&dom, dom.base_scope().root_node(), &mut after);
    for (key, scope) in before {
        assert_eq!(after.get(&key), Some(&scope), "remounted {key}");
    }
    assert!(dioxus_ssr::render(&dom).contains("Result recorded"));
    Ok(())
}

fn activity_root(props: (crate::activity::Live, String, bool, Option<String>)) -> Element {
    rsx! { crate::agent_view::ActivityRow { live: props.0, coarse: props.1, connected: props.2, waiting: props.3 } }
}

fn render_activity(
    live: crate::activity::Live,
    coarse: &str,
    connected: bool,
    waiting: Option<&str>,
) -> String {
    let mut dom = VirtualDom::new_with_props(
        activity_root,
        (live, coarse.into(), connected, waiting.map(str::to_owned)),
    );
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

fn counted_activity_root() -> Element {
    use crate::activity::Live;
    use portal_types::agents::{ActiveTool, LivePhase, LiveState};
    let live = Live::Detail(LiveState {
        phase: LivePhase::ToolRunning,
        reasoning: Some("Still checking.".into()),
        active_tools: vec![
            ActiveTool {
                call_id: "call-call".into(),
                name: "read".into(),
                parent_call_id: None,
                call: Some("read src/lib.rs".into()),
            },
            ActiveTool {
                call_id: "new-call".into(),
                name: "bash".into(),
                parent_call_id: None,
                call: Some("$ git status".into()),
            },
        ],
    });
    rsx! { crate::agent_view::ActivityRow { live, coarse: "working", connected: true, waiting: None, turns: worked_history(true).turns, elapsed_seconds: Some(131) } }
}

#[test]
fn footer_keeps_phase_elapsed_and_current_turn_counts_without_counting_a_call_twice() {
    let html = dioxus_ssr::render(&VirtualDom::prebuilt(counted_activity_root));
    for text in [
        "Running tools: read, bash",
        "2m 11s",
        "3 thoughts · 3 tool calls",
    ] {
        assert!(html.contains(text), "missing {text}: {html}");
    }
    assert!(!html.contains("Still checking."), "{html}");
    assert!(!html.contains("$ git status"), "{html}");
}

#[test]
fn every_live_phase_uses_only_actual_detail_and_names_all_active_tools() {
    use crate::activity::Live;
    use portal_types::agents::{ActiveTool, LivePhase, LiveState};
    for (phase, expected) in [
        (LivePhase::Idle, "Idle"),
        (LivePhase::Working, "Working…"),
        (LivePhase::Thinking, "Thinking…"),
        (LivePhase::Responding, "Responding…"),
        (LivePhase::ToolRunning, "Running tools: read, bash"),
    ] {
        let html = render_activity(
            Live::Detail(LiveState {
                reasoning: None,
                phase,
                active_tools: vec![
                    ActiveTool {
                        call: None,
                        call_id: "read".into(),
                        name: "read".into(),
                        parent_call_id: None,
                    },
                    ActiveTool {
                        call: None,
                        call_id: "bash".into(),
                        name: "bash".into(),
                        parent_call_id: Some("read".into()),
                    },
                ],
            }),
            "working",
            true,
            None,
        );
        assert!(html.contains(expected), "{html}");
        assert!(html.contains(">Live</p>"), "{html}");
        assert_eq!(html.matches(r#"role="status""#).count(), 1, "{html}");
        assert!(html.contains(r#"aria-live="polite""#), "{html}");
        assert!(html.contains(r#"aria-atomic="true""#), "{html}");
    }
}

#[test]
fn blocked_pending_requests_and_agent_disconnection_take_priority_over_live_detail() {
    use crate::activity::Live;
    use portal_types::agents::{LivePhase, LiveState};
    let live = Live::Detail(LiveState {
        reasoning: None,
        phase: LivePhase::Thinking,
        active_tools: Vec::new(),
    });
    for (coarse, connected, waiting, expected) in [
        (
            "working",
            true,
            Some("Waiting for your answers"),
            "Waiting for your answers",
        ),
        ("blocked", true, None, "Blocked"),
        ("working", false, None, "Disconnected"),
        ("disconnected", true, None, "Disconnected"),
    ] {
        let html = render_activity(live.clone(), coarse, connected, waiting);
        assert!(html.contains(expected), "{html}");
        assert!(!html.contains("Thinking"), "{html}");
        assert!(!html.contains(">Live</p>"), "{html}");
    }
}

#[test]
fn every_gap_and_pending_falls_back_to_coarse_state_without_claiming_completion() {
    use crate::activity::{Gap, Live};
    use portal_types::agents::UnavailableReason;
    let mut values = vec![
        Live::Pending,
        Live::Unavailable(Gap::Disconnected),
        Live::Unavailable(Gap::SessionMismatch),
        Live::Unavailable(Gap::Invalid),
    ];
    values.extend(
        [
            UnavailableReason::UnsupportedHarness,
            UnavailableReason::SessionUnverified,
            UnavailableReason::NativeUnavailable,
            UnavailableReason::NoProducer,
            UnavailableReason::NoSnapshot,
            UnavailableReason::Expired,
            UnavailableReason::ProducerClosed,
            UnavailableReason::Resyncing,
            UnavailableReason::Overflow,
        ]
        .into_iter()
        .map(|reason| Live::Unavailable(Gap::Server(reason))),
    );
    for live in values {
        for (coarse, expected) in [
            ("working", "Working…"),
            ("blocked", "Blocked"),
            ("idle", "Idle"),
            ("disconnected", "Disconnected"),
            ("unknown", "unknown"),
        ] {
            let html = render_activity(live.clone(), coarse, true, None);
            assert!(html.contains(expected), "{live:?}: {html}");
            assert!(!html.contains("Thinking"), "{html}");
            assert!(!html.contains("Responding"), "{html}");
            assert!(!html.contains("Running tools"), "{html}");
            assert!(!html.contains(">Live</p>"), "{html}");
            assert!(!html.contains("Finished"), "{html}");
            if coarse == "working" {
                assert!(!html.contains(">Idle</p>"), "{html}");
            }
        }
    }
}

#[test]
fn telemetry_loss_and_session_mismatch_remove_detail_but_keep_the_history_and_working_state()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::activity::{Gap, Live};
    use portal_types::agents::{LivePhase, LiveState, Role};
    type LiveHandle = std::rc::Rc<std::cell::Cell<Option<Signal<Live>>>>;
    fn live_root(handle: LiveHandle) -> Element {
        let live = use_signal(|| {
            Live::Detail(LiveState {
                reasoning: None,
                phase: LivePhase::Responding,
                active_tools: Vec::new(),
            })
        });
        handle.set(Some(live));
        rsx! {
            crate::agent_view::Conversation { snapshot: Some(history(vec![history_turn("kept", Role::Assistant, "Durable history.")])), loading: false, assistant: "Assistant" }
            crate::agent_view::ActivityRow { live: live(), coarse: "working", connected: true, waiting: None }
        }
    }
    let handle = LiveHandle::default();
    let mut dom = VirtualDom::new_with_props(live_root, handle.clone());
    dom.rebuild_in_place();
    let mut initial = std::collections::BTreeMap::new();
    record_scopes(&dom, dom.base_scope().root_node(), &mut initial);
    assert!(dioxus_ssr::render(&dom).contains("Responding…"));
    let mut signal = handle.get().ok_or("Missing activity signal")?;
    for gap in [
        Gap::Disconnected,
        Gap::SessionMismatch,
        Gap::Server(portal_types::agents::UnavailableReason::Expired),
    ] {
        dom.in_runtime(|| signal.set(Live::Unavailable(gap)));
        dom.render_immediate_to_vec();
        let html = dioxus_ssr::render(&dom);
        assert!(html.contains("Durable history."), "{html}");
        assert!(html.contains("Working…"), "{html}");
        assert!(!html.contains("Responding…"), "{html}");
        assert!(!html.contains(">Idle</p>"), "{html}");
        let mut current = std::collections::BTreeMap::new();
        record_scopes(&dom, dom.base_scope().root_node(), &mut current);
        assert_eq!(initial, current);
    }
    Ok(())
}

#[test]
fn official_controls_forward_labels_values_and_attributes() {
    let dom = VirtualDom::prebuilt(root);
    let html = dioxus_ssr::render(&dom);

    for expected in [
        "<form>",
        "<label",
        r#"for="smoke-subject""#,
        ">Subject</label>",
        "<input",
        r#"id="smoke-subject""#,
        r#"name="subject""#,
        r#"type="text""#,
        r#"value="Foundation check""#,
        "required",
        r#"for="smoke-message""#,
        ">Message</label>",
        "<textarea",
        r#"id="smoke-message""#,
        r#"name="message""#,
        r#"value="Keep the existing portal layout""#,
        r#"aria-describedby="smoke-help""#,
        "<button",
        r#"type="submit""#,
        r#"name="action""#,
        r#"value="send""#,
        "disabled",
        r#"aria-label="Send message""#,
        ">Send</button>",
    ] {
        assert!(html.contains(expected), "missing {expected:?} in {html}");
    }
}
