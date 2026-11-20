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
    owner: Option<&AgentAddress>,
) -> Vec<RowView> {
    match (board, owner) {
        (Some(board), Some(owner)) => direct_rows(board, &owner.host, Some(&owner.pane)),
        _ => Vec::new(),
    }
}

pub(crate) fn direct_rows(
    board: &portal_types::BoardView,
    host: &str,
    pane: Option<&str>,
) -> Vec<RowView> {
    board
        .repos
        .iter()
        .flat_map(|repo| &repo.rows)
        .filter(|row| {
            pane.is_some()
                && crate::board::is_navigation_row(row)
                && row.owner_host.as_deref().unwrap_or(&row.host) == host
                && row.owner_pane.as_deref() == pane
        })
        .cloned()
        .collect()
}

pub(crate) fn job_finished(state: &str) -> bool {
    matches!(
        state,
        "closed" | "done" | "exited" | "stopped" | "failed" | "cancelled"
    ) || state.starts_with("exit ")
}

/// Workers that have not finished. A finished worker stays in its group until its branch
/// lands, but it no longer counts as one of the lead's workers.
pub(crate) fn active_count(rows: &[RowView]) -> usize {
    rows.iter()
        .filter(|row| row.kind != "job" && !finished(&row.state))
        .count()
}

pub(crate) fn finished(state: &str) -> bool {
    WorkerGroup::from_state(state, false) == WorkerGroup::Finished
}

pub(crate) fn working_count(rows: &[RowView]) -> usize {
    rows.iter()
        .filter(|row| row.kind != "job" && matches!(row.state.as_str(), "working" | "running"))
        .count()
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
    let mut now = use_signal(crate::agent_view::now_ms);
    dioxus_sdk_time::use_interval(std::time::Duration::from_secs(1), move |()| {
        now.set(crate::agent_view::now_ms())
    });
    let working = working_count(&rows);
    let (jobs, rows): (Vec<_>, Vec<_>) = rows.into_iter().partition(|row| row.kind == "job");
    rsx! {
        section { class: "workers-panel", aria_label: "Lead workers",
            if header {
                header { class: "console-column-header", h2 { "Workers" } span { "{working} working" } }
            }
            ScrollArea { class: "console-scroll workers-list", direction: ScrollDirection::Vertical,
                if !jobs.is_empty() {
                    section { class: "worker-group", aria_label: "Command jobs",
                        h3 { "Jobs · {jobs.len()}" }
                        for job in &jobs {
                            div { class: "worker-row", key: "{job.owner_host:?}:{job.id}",
                                span { class: "worker-title", "{job.title.as_deref().unwrap_or(&job.name)}" }
                                span { class: "worker-phase", "{job.state} · {job.host}" }
                                if job.ownership_stale { small { "Last known · ownership unavailable" } }
                            }
                        }
                    }
                }
                if rows.is_empty() && jobs.is_empty() { Status { "data-part": "column-empty", "No workers are recorded for this lead." } }
                for category in [WorkerGroup::Waiting, WorkerGroup::Working, WorkerGroup::Idle, WorkerGroup::Finished] {
                    {
                        let members: Vec<_> = rows.iter().filter(|row| group(row, &inbox) == category).collect();
                        rsx! {
                            if !members.is_empty() {
                                section { class: "worker-group", "data-state": category.value(),
                                    h3 { "{category.title()} · {members.len()}" }
                                    for row in members {
                                        {
                                            let address = row.pane.clone().filter(|_| !row.ownership_stale && row.state != "closed").map(|pane| AgentAddress { host: row.host.clone(), pane });
                                            let open = address.is_some() && selected() == address;
                                            let title = row.title.as_deref().filter(|title| !title.trim().is_empty() && !title.starts_with("u-"))
                                                .unwrap_or(&row.name);
                                            let elapsed = row.turn_clock.filter(|_| address.is_some()).and_then(|clock| clock.seconds(now())).map(crate::agent_view::elapsed_label);
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
                                                    span { class: "worker-title", "{title}" if let Some(pane) = &row.pane { crate::unread::UnreadDot { host: row.host.clone(), pane: pane.clone() } } }
                                                    span { class: "worker-phase", "{state} · " if let Some(elapsed) = elapsed { "{elapsed} · " } "{row.host}" }
                                                    span { class: "worker-activity", if row.ownership_stale { "Last known · ownership unavailable" } else { "{row.activity}" } }
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
