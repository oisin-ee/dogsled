//! Shared composer control. The caller owns the draft or Pi-confirmed selection.
use crate::components::input::Input;
use crate::components::popover::{PopoverContent, PopoverRoot, PopoverTrigger};
use dioxus::html::geometry::PixelsVector2D;
use dioxus::prelude::*;
use dioxus_primitives::{ContentAlign, ContentSide};
use portal_types::models::{ModelChoice, ModelRef, SettingsChange, ThinkingLevel};

// Two single-line labels per row keep scroll offsets and keyboard jumps exact.
const ROW_HEIGHT: usize = 44;
const OVERSCAN: usize = 3;

#[component]
fn ModelOptions(
    choices: ReadSignal<Vec<ModelChoice>>,
    selected_model: Option<ModelRef>,
    busy: bool,
    on_change: EventHandler<SettingsChange>,
) -> Element {
    let mut query = use_signal(String::new);
    let results = use_memo(move || {
        let needle = query.read().trim().to_lowercase();
        choices
            .read()
            .iter()
            .enumerate()
            .filter_map(|(index, choice)| {
                (needle.is_empty()
                    || format!(
                        "{} {}",
                        choice.name.as_deref().unwrap_or_default(),
                        choice.model.pattern()
                    )
                    .to_lowercase()
                    .contains(&needle))
                .then_some(index)
            })
            .collect::<Vec<_>>()
    });
    let mut active = use_signal(|| None::<usize>);
    let mut top = use_signal(|| 0usize);
    let mut height = use_signal(|| 320usize);
    let mut list = use_signal(|| None::<std::rc::Rc<MountedData>>);
    let mut scroll_to = move |offset: usize| {
        top.set(offset);
        if let Some(list) = list.peek().as_ref() {
            let request = list.scroll(
                PixelsVector2D::new(0.0, offset as f64),
                ScrollBehavior::Instant,
            );
            spawn(async move {
                let _ = request.await;
            });
        }
    };
    let indices = results.read();
    let count = indices.len();
    let offset = top().min((count * ROW_HEIGHT).saturating_sub(height()));
    let start = (offset / ROW_HEIGHT).saturating_sub(OVERSCAN);
    let end = ((offset + height()).div_ceil(ROW_HEIGHT) + OVERSCAN).min(count);
    let list_id = format!("model-list-{}", dioxus::core::current_scope_id().0);
    let focused = active().filter(|index| (start..end).contains(index));
    let models = choices.read();
    let choose = move |index: usize| {
        if !busy
            && let Some(&index) = results.peek().get(index)
            && let Some(choice) = choices.peek().get(index)
        {
            on_change.call(SettingsChange {
                model: Some(choice.model.clone()),
                thinking: None,
            });
        }
    };
    let search = move |event: FormEvent| {
        query.set(event.value());
        active.set(None);
        scroll_to(0);
    };
    let navigate = move |event: KeyboardEvent| {
        if busy || count == 0 || event.is_composing() {
            return;
        }
        let current = focused;
        let next = match event.key() {
            Key::ArrowDown => current.map_or(0, |i| (i + 1) % count),
            Key::ArrowUp => current.map_or(count - 1, |i| (i + count - 1) % count),
            Key::Home => 0,
            Key::End => count - 1,
            Key::Enter => {
                event.prevent_default();
                event.stop_propagation();
                if let Some(i) = current {
                    choose(i);
                }
                return;
            }
            _ => return,
        };
        event.prevent_default();
        event.stop_propagation();
        active.set(Some(next));
        let row_top = next * ROW_HEIGHT;
        if row_top < offset {
            scroll_to(row_top);
        } else if row_top + ROW_HEIGHT > offset + height() {
            scroll_to((row_top + ROW_HEIGHT).saturating_sub(height()));
        }
    };
    rsx! {
        Input { class: "repo-search", role: "combobox", name: "model", autocomplete: "off", "spellcheck": "false",
            aria_label: "Search models", placeholder: "Search models…", aria_expanded: "true", aria_autocomplete: "list",
            aria_controls: list_id.clone(), aria_activedescendant: focused.map(|i| format!("{list_id}-{i}")),
            value: query(), disabled: busy,
            oninput: search, onkeydown: navigate,
        }
        div { id: list_id.clone(), class: "model-options", role: "listbox", aria_label: "Models", aria_disabled: busy,
            onmounted: move |event| list.set(Some(event.data())),
            onresize: move |event| { if let Ok(size) = event.get_content_box_size() { height.set(size.height.ceil() as usize); } },
            onscroll: move |event| top.set(event.scroll_top().max(0.0) as usize),
            div { role: "presentation", style: "padding-top: {start * ROW_HEIGHT}px; padding-bottom: {(count - end) * ROW_HEIGHT}px;",
                for i in start..end {
                    { let choice = &models[indices[i]];
                      let selected = selected_model.as_ref() == Some(&choice.model);
                      let name = choice.name.as_deref().unwrap_or(&choice.model.id);
                      let pattern = choice.model.pattern();
                      rsx! { div { key: "{pattern}", id: "{list_id}-{i}", role: "option", style: "height: {ROW_HEIGHT}px",
                        aria_posinset: "{i + 1}", aria_setsize: "{count}", aria_selected: selected, "data-selected": selected,
                        "data-highlighted": focused == Some(i), title: "{name} · {pattern}",
                        onmousemove: move |_| { if *active.peek() != Some(i) { active.set(Some(i)); } }, onmousedown: move |event| event.prevent_default(),
                        onclick: move |_| choose(i),
                        div { class: "repo-slug", "{name}" if choice.scoped { small { class: "model-scope", "Scoped" } } }
                        div { class: "repo-description", "{pattern}" }
                      } }
                    }
                }
            }
            if count == 0 { div { class: "repo-empty", "No matching models" } }
        }
    }
}

#[component]
pub(crate) fn ModelPicker(
    choices: Vec<ModelChoice>,
    selected_model: Option<ModelRef>,
    selected_level: Option<ThinkingLevel>,
    busy: bool,
    on_change: EventHandler<SettingsChange>,
) -> Element {
    let mut open = use_signal(|| false);
    let current = choices
        .iter()
        .find(|choice| Some(&choice.model) == selected_model.as_ref());
    let name = current
        .and_then(|choice| choice.name.as_deref())
        .or_else(|| selected_model.as_ref().map(|model| model.id.as_str()))
        .unwrap_or("Pi default");
    let effort = selected_level.map_or("Pi default", ThinkingLevel::as_str);
    rsx! {
        PopoverRoot { class: "model-picker", open: Some(open()), on_open_change: move |value| open.set(value),
            PopoverTrigger { class: "model-trigger", disabled: busy || choices.is_empty(),
                title: "Model: {name} · Effort: {effort}", aria_label: "Model: {name}, effort: {effort}", aria_haspopup: "dialog", aria_expanded: open(),
                span { "{name}" }
                span { class: "effort-ring", "data-level": effort, aria_hidden: "true" }
            }
            PopoverContent { class: "model-menu", side: ContentSide::Top, align: ContentAlign::End,
                ModelMenu { choices, selected_model, selected_level, busy, on_change: move |change| { on_change.call(change); open.set(false); } }
            }
        }
    }
}

#[component]
pub(crate) fn ModelMenu(
    choices: ReadSignal<Vec<ModelChoice>>,
    selected_model: Option<ModelRef>,
    selected_level: Option<ThinkingLevel>,
    busy: bool,
    on_change: EventHandler<SettingsChange>,
) -> Element {
    let catalogue = choices.read();
    let levels = catalogue
        .iter()
        .find(|choice| Some(&choice.model) == selected_model.as_ref())
        .map(|choice| choice.thinking.as_slice())
        .unwrap_or_default();
    let effort = selected_level.map_or("Pi default", ThinkingLevel::as_str);
    rsx! {
        ModelOptions { choices, selected_model, busy, on_change }
        label { class: "model-effort", "Effort"
            select { aria_label: "Effort", disabled: busy || levels.is_empty(), value: effort,
                onchange: { let levels = levels.to_vec(); move |event| {
                    if let Some(thinking) = levels.iter().copied().find(|level| level.as_str() == event.value()) {
                        on_change.call(SettingsChange { model: None, thinking: Some(thinking) });
                    }
                } },
                if selected_level.is_none() { option { value: "Pi default", selected: true, disabled: true, "Pi default" } }
                for level in levels.iter().copied() {
                    option { value: level.as_str(), selected: Some(level) == selected_level, "{level.as_str()}" }
                }
            }
        }
    }
}

#[component]
pub(crate) fn ReadOnlyModel(model: Option<String>) -> Element {
    rsx! {
        div { class: "model-readonly",
            p { "Recorded model: {model.as_deref().unwrap_or(\"unavailable\")}" }
            p { "Model changes work for Pi sessions only" }
        }
    }
}
