//! Read-only checkout changes, projected through the shared By Stage model.

use dioxus::prelude::*;
use dioxus_primitives::scroll_area::ScrollDirection;
use portal_types::files::{Change, Entry, FilesState, by_stage};

use crate::components::{
    accordion::{Accordion, AccordionContent, AccordionItem, AccordionTrigger},
    item::{Item, ItemContent, ItemDescription, ItemGroup, ItemSize, ItemTitle},
    scroll_area::ScrollArea,
    skeleton::Skeleton,
};
use crate::notice::Status;

fn snapshot(state: Option<&FilesState>) -> Option<&portal_types::files::Files> {
    match state {
        Some(FilesState::Current(files) | FilesState::Stale { files, .. }) => Some(files),
        _ => None,
    }
}

pub(crate) fn path_count(state: Option<&FilesState>) -> Option<usize> {
    snapshot(state).map(|files| by_stage(&files.entries).paths)
}

fn read_time(ms: u64) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new(&js_sys::wasm_bindgen::JsValue::from_f64(ms as f64));
        if date.get_time().is_finite() {
            return date.to_iso_string().as_string().unwrap_or_default();
        }
    }
    format!("{ms} ms since Unix epoch")
}

#[component]
pub(crate) fn Files(state: Option<FilesState>) -> Element {
    let files = snapshot(state.as_ref());
    rsx! {
        section { class: "files-panel", aria_label: "Checkout changes",
            ScrollArea { class: "console-scroll files-list", direction: ScrollDirection::Vertical,
                match &state {
                    None | Some(FilesState::Pending) => rsx! {
                        div { class: "files-loading", role: "status", aria_label: "Loading file status",
                            Skeleton {} Skeleton {} Skeleton {}
                        }
                    },
                    Some(FilesState::NotRepository) => rsx! { Status { "Not a Git checkout" } },
                    Some(FilesState::TooMany) => rsx! { Status { "Too many changes to list" } },
                    Some(FilesState::Unavailable(reason)) => rsx! { Status { "{reason}" } },
                    Some(FilesState::Stale { files, error }) => rsx! {
                        Status { "Last read at {read_time(files.captured_ms)}: {error}" }
                    },
                    Some(FilesState::Current(_)) => rsx! {},
                }
                if let Some(files) = files {
                    if files.entries.is_empty() {
                        Status { "Working tree clean" }
                    } else {
                        {
                            let stages = by_stage(&files.entries);
                            rsx! {
                                Accordion { allow_multiple_open: true, horizontal: false,
                                    for (index, (title, entries)) in [
                                        ("Conflicts", stages.conflicts),
                                        ("Staged", stages.staged),
                                        ("Unstaged", stages.unstaged),
                                        ("Untracked", stages.untracked),
                                    ].into_iter().enumerate() {
                                        if !entries.is_empty() {
                                            AccordionItem { key: "{title}", index, default_open: true,
                                                AccordionTrigger { "{title} · {entries.len()}" }
                                                AccordionContent {
                                                    ItemGroup {
                                                        for entry in entries {
                                                            FileRow { key: "{entry.path:?}", entry: entry.clone(), stage: index }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(crate) fn FileRow(entry: Entry, stage: usize) -> Element {
    let status = match &entry.change {
        Change::Conflict { code } => code.iter().collect::<String>(),
        Change::Untracked => "?".to_owned(),
        Change::Ordinary { index, worktree }
        | Change::Renamed {
            index, worktree, ..
        } => if stage == 1 { index } else { worktree }.to_string(),
    };
    rsx! {
        Item { class: "files-row", size: ItemSize::Sm, role: "listitem",
            r#as: move |attributes: Vec<Attribute>| rsx! { div { ..attributes, span { class: "files-code", "{status}" }
                ItemContent {
                    ItemTitle { "{entry.path.text}" }
                    if let Change::Renamed { from, .. } = &entry.change {
                        ItemDescription { class: "files-previous", "from {from.text}" }
                    }
                }
            } }
        }
    }
}
