//! Reusable controlled choices, independent of a conversation or launch state.
use crate::components::select::{Select, SelectOption};
use dioxus::prelude::*;
use dioxus_primitives::combobox::{
    Combobox, ComboboxEmpty, ComboboxInput, ComboboxList, ComboboxOption, default_combobox_filter,
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
    let model = use_memo(use_reactive(&selected_model, move |model| {
        model.map(|model| model.pattern())
    }));
    let level = use_memo(use_reactive(&selected_level, Some));
    let supported = choices
        .iter()
        .find(|choice| Some(&choice.model) == selected_model.as_ref())
        .map(|choice| choice.thinking.clone())
        .unwrap_or_default();
    let searchable = choices.clone();
    rsx! {
        div { class: "model-picker", aria_label: "Model settings", aria_busy: busy,
            Combobox::<String> { class: "model-combobox", disabled: busy || choices.is_empty(),
                value: Some(ReadSignal::from(model)),
                on_value_change: {
                    let choices = choices.clone();
                    move |model: Option<String>| {
                        if let Some(choice) = choices.iter().find(|choice| model.as_deref() == Some(choice.model.pattern().as_str())) {
                            on_change.call(SettingsChange { model: Some(choice.model.clone()), thinking: None });
                        }
                    }
                },
                filter: move |(query, model): (String, String)| {
                    searchable.iter().find(|choice| choice.model.pattern() == model).is_some_and(|choice| default_combobox_filter(&query, &format!("{} {}", model, choice.name.as_deref().unwrap_or(""))))
                },
                ComboboxInput { class: "repo-search", aria_label: "Model", "name": "model", placeholder: "Pi default" }
                ComboboxList { class: "repo-options", aria_label: "Models",
                    for (index, choice) in choices.into_iter().enumerate() {
                        ComboboxOption::<String> { key: "{choice.model.pattern()}", index,
                            value: choice.model.pattern(), text_value: choice.model.pattern(),
                            div { class: "repo-slug", "{choice.name.as_deref().unwrap_or(&choice.model.id)}" }
                            div { class: "repo-description", "{choice.model.pattern()}" }
                        }
                    }
                    ComboboxEmpty { div { class: "repo-empty", "No matching models" } }
                }
            }
            Select::<Option<ThinkingLevel>> { class: "thinking-select", name: "thinking", aria_label: "Thinking level",
                value: Some(ReadSignal::from(level)), disabled: busy || supported.is_empty(),
                on_value_change: move |level: Option<Option<ThinkingLevel>>| {
                    if let Some(Some(level)) = level { on_change.call(SettingsChange { model: None, thinking: Some(level) }); }
                },
                if selected_level.is_none() {
                    SelectOption::<Option<ThinkingLevel>> { index: 0usize, value: None, text_value: "Pi default", "Pi default" }
                }
                for (index, level) in supported.into_iter().enumerate() {
                    SelectOption::<Option<ThinkingLevel>> { key: "{level.as_str()}", index: index + usize::from(selected_level.is_none()), value: Some(level), text_value: level.as_str(), "{level.as_str()}" }
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
