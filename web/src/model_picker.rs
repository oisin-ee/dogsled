//! Reusable controlled choices, independent of a conversation or launch state.
use crate::components::select::{Select, SelectGroup, SelectGroupLabel, SelectOption};
use dioxus::prelude::*;
use portal_types::models::{ModelChoice, ModelRef, SettingsChange, ThinkingLevel};
use std::collections::BTreeMap;

#[component]
pub(crate) fn ModelPicker(
    choices: Vec<ModelChoice>,
    selected_model: Option<ModelRef>,
    selected_level: Option<ThinkingLevel>,
    busy: bool,
    on_change: EventHandler<SettingsChange>,
) -> Element {
    let model = use_memo(use_reactive(&selected_model, Some));
    let level = use_memo(use_reactive(&selected_level, Some));
    let supported = choices
        .iter()
        .find(|choice| Some(&choice.model) == selected_model.as_ref())
        .map(|choice| choice.thinking.clone())
        .unwrap_or_default();
    let mut sorted_choices = choices.clone();
    sorted_choices.sort_by(|left, right| left.model.provider.cmp(&right.model.provider));
    let mut groups = BTreeMap::<String, Vec<_>>::new();
    for (index, choice) in sorted_choices.into_iter().enumerate() {
        groups
            .entry(choice.model.provider.clone())
            .or_default()
            .push((index + usize::from(selected_model.is_none()), choice));
    }
    rsx! {
        div { class: "model-picker", aria_label: "Model settings", aria_busy: busy,
            Select::<Option<ModelRef>> { class: "model-select", name: "model", aria_label: "Model",
                disabled: busy || choices.is_empty(), value: Some(ReadSignal::from(model)),
                on_value_change: move |model: Option<Option<ModelRef>>| {
                    if let Some(Some(model)) = model {
                        on_change.call(SettingsChange { model: Some(model), thinking: None });
                    }
                },
                if selected_model.is_none() {
                    SelectOption::<Option<ModelRef>> { index: 0usize, value: None, text_value: "Pi default", "Pi default" }
                }
                for (provider, choices) in groups {
                    SelectGroup { key: "{provider}",
                        SelectGroupLabel { "{provider}" }
                        for (index, choice) in choices {
                            SelectOption::<Option<ModelRef>> { key: "{choice.model.pattern()}", index,
                                value: Some(choice.model.clone()), text_value: choice.name.clone().unwrap_or_else(|| choice.model.id.clone()),
                                div { class: "model-option",
                                    div { class: "repo-slug", "{choice.name.as_deref().unwrap_or(&choice.model.id)}" }
                                    div { class: "repo-description", "{choice.model.pattern()}" }
                                }
                            }
                        }
                    }
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
