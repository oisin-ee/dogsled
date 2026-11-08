//! A lead's owned workers, grouped by their next useful state.

use dioxus::prelude::*;
use dioxus_icons::lucide::ChevronLeft;
use dioxus_primitives::scroll_area::ScrollDirection;
use portal_types::RowView;
use portal_types::agents::AgentAddress;

use crate::app::{InboxItem, InboxKind, InboxSelection, LeadPanel};
use crate::board::NavigationRow;
use crate::components::button::{Button, ButtonVariant};
use crate::components::scroll_area::ScrollArea;
use crate::notice::Status;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkerGroup {
    Waiting,
    Working,
    Idle,
    Finished,
}

impl WorkerGroup {
    pub(crate) fn from_state(state: &str, waiting: bool) -> Self {
        if waiting {
            Self::Waiting
        } else {
            match state {
                "working" | "running" => Self::Working,
                "closed" | "done" => Self::Finished,
                _ => Self::Idle,
            }
        }
    }

    pub(crate) fn value(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Working => "working",
            Self::Idle => "idle",
            Self::Finished => "finished",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Waiting => "Waiting on you",
            Self::Working => "Working",
            Self::Idle => "Idle",
            Self::Finished => "Finished",
        }
    }
}

pub(crate) fn group(row: &RowView, inbox: &[InboxItem]) -> WorkerGroup {
    WorkerGroup::from_state(
        &row.state,
        row.unseen
            || inbox.iter().any(|item| {
                item.selection.kind == InboxKind::Prompt
                    && item.selection.host == row.host
                    && Some(&item.selection.pane) == row.pane.as_ref()
            }),
    )
}

pub(crate) fn owned_rows(
    board: Option<&portal_types::BoardView>,
    lead: Option<&NavigationRow>,
) -> Vec<RowView> {
    match (board, lead) {
        (Some(board), Some(lead)) => board
            .repos
            .iter()
            .flat_map(|repo| &repo.rows)
            .filter(|row| {
                crate::board::is_navigation_row(row) && crate::board::owns_worker(lead, row)
            })
            .cloned()
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn working_count(rows: &[RowView]) -> usize {
    rows.iter()
        .filter(|row| matches!(row.state.as_str(), "working" | "running"))
        .count()
}

fn elapsed(ms: u64) -> String {
    let seconds = ms / 1_000;
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m {}s", seconds / 60, seconds % 60),
        _ => format!("{}h {}m", seconds / 3600, seconds % 3600 / 60),
    }
}

#[component]
pub(crate) fn Workers(
    rows: Vec<RowView>,
    inbox: Vec<InboxItem>,
    #[props(default = true)] header: bool,
) -> Element {
    let mut selected = use_context::<Signal<Option<AgentAddress>>>();
    let mut selection = use_context::<Signal<Option<InboxSelection>>>();
    let mut panel = use_context::<Signal<LeadPanel>>();
    let working = working_count(&rows);
    rsx! {
        section { class: "workers-panel", aria_label: "Lead workers",
            if header {
                header { class: "console-column-header", h2 { "Workers" } span { "{working} working" } }
            }
            ScrollArea { class: "console-scroll workers-list", direction: ScrollDirection::Vertical,
                if rows.is_empty() { Status { "data-part": "column-empty", "No workers are recorded for this lead." } }
                for category in [WorkerGroup::Waiting, WorkerGroup::Working, WorkerGroup::Idle, WorkerGroup::Finished] {
                    {
                        let members: Vec<_> = rows.iter().filter(|row| group(row, &inbox) == category).collect();
                        rsx! {
                            if !members.is_empty() {
                                section { class: "worker-group", "data-state": category.value(),
                                    h3 { "{category.title()} · {members.len()}" }
                                    for row in members {
                                        {
                                            let address = row.pane.clone().map(|pane| AgentAddress { host: row.host.clone(), pane });
                                            let open = address.is_some() && selected() == address;
                                            let title = row.title.as_deref().filter(|title| !title.trim().is_empty() && !title.starts_with("u-"))
                                                .unwrap_or(&row.name);
                                            let elapsed = elapsed(row.quiet_ms.unwrap_or(row.age_ms));
                                            let state = if matches!(row.state.as_str(), "closed" | "done") { "Finished" } else { &row.state };
                                            rsx! {
                                                Button { key: "{row.host}:{row.id}", class: "worker-row", variant: ButtonVariant::Ghost,
                                                    r#type: "button", disabled: address.is_none(),
                                                    aria_current: open.then_some("true"), "data-state": category.value(),
                                                    onclick: move |_| {
                                                        selected.set(address.clone());
                                                        selection.set(None);
                                                        panel.set(LeadPanel::Chat);
                                                    },
                                                    span { class: "worker-dot", "data-state": category.value(), aria_hidden: "true" }
                                                    span { class: "worker-title", "{title}" }
                                                    span { class: "worker-phase", "{state} · {elapsed} · {row.host}" }
                                                    span { class: "worker-activity", "{row.activity}" }
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
pub(crate) fn WorkerHeading(
    title: String,
    lead: NavigationRow,
    what: String,
    host: String,
) -> Element {
    let mut selected = use_context::<Signal<Option<AgentAddress>>>();
    let mut selection = use_context::<Signal<Option<InboxSelection>>>();
    let mut panel = use_context::<Signal<LeadPanel>>();
    let lead_title = lead.title.clone();
    let open_lead = use_callback(move |()| {
        selected.set(lead.pane.clone().map(|pane| AgentAddress {
            host: lead.host.clone(),
            pane,
        }));
        selection.set(None);
        panel.set(LeadPanel::Chat);
    });
    rsx! {
        div { class: "worker-heading",
            Button { class: "icon-button", variant: ButtonVariant::Outline, r#type: "button",
                aria_label: "Back to lead", title: "Back to lead", onclick: move |_| open_lead.call(()),
                ChevronLeft { size: 16, attributes: dioxus_primitives::dioxus_attributes::attributes!(svg { "aria-hidden": "true" }) }
            }
            div { class: "lead-heading", h1 { "{title}" }
                p { class: "lead-meta", "Worker of "
                    Button { class: "worker-lead-link", variant: ButtonVariant::Ghost, r#type: "button",
                        onclick: move |_| open_lead.call(()), "{lead_title}" }
                    " · {what} · {host}"
                }
            }
        }
    }
}
