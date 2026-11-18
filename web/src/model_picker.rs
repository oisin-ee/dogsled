//! Shared composer control. The caller owns the draft or Pi-confirmed selection.
use crate::components::popover::{PopoverContent, PopoverRoot, PopoverTrigger};
use dioxus::prelude::*;
use dioxus_primitives::{
    ContentAlign, ContentSide,
    combobox::{Combobox, ComboboxEmpty, ComboboxInput, ComboboxList, ComboboxOption},
};
use portal_types::models::{ModelChoice, ModelRef, SettingsChange, ThinkingLevel};

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
    choices: Vec<ModelChoice>,
    selected_model: Option<ModelRef>,
    selected_level: Option<ThinkingLevel>,
    busy: bool,
    on_change: EventHandler<SettingsChange>,
) -> Element {
    let levels = choices
        .iter()
        .find(|choice| Some(&choice.model) == selected_model.as_ref())
        .map(|choice| choice.thinking.as_slice())
        .unwrap_or_default();
    let selection = use_memo(use_reactive(&selected_model, |model| model));
    let effort = selected_level.map_or("Pi default", ThinkingLevel::as_str);
    rsx! {
        Combobox::<ModelRef> { open: Some(true), disabled: busy, value: Some(ReadSignal::from(selection)),
            on_value_change: move |model: Option<ModelRef>| {
                if model.is_some() { on_change.call(SettingsChange { model, thinking: None }); }
            },
            ComboboxInput { class: "repo-search", aria_label: "Search models", placeholder: "Search models…", "name": "model" }
            ComboboxList { class: "model-options", aria_label: "Models",
                for (index, choice) in choices.iter().enumerate() {
                    ComboboxOption::<ModelRef> { key: "{choice.model.pattern()}", index, value: choice.model.clone(),
                        text_value: format!("{} {}", choice.name.as_deref().unwrap_or_default(), choice.model.pattern()),
                        div { class: "repo-slug", "{choice.name.as_deref().unwrap_or(&choice.model.id)}"
                            if choice.scoped { small { class: "model-scope", "Scoped" } }
                        }
                        div { class: "repo-description", "{choice.model.pattern()}" }
                    }
                }
                ComboboxEmpty { div { class: "repo-empty", "No matching models" } }
            }
        }
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
