//! Fleet metrics, with tool filters and histogram-derived durations.

use std::cmp::Ordering;

use dioxus::prelude::*;
use portal_types::metrics::{
    Histogram, MetricsReport, RunGroup, ToolGroup, ToolRow, WaitKind, Window,
};

use crate::api;
use crate::components::badge::{Badge, BadgeVariant};
use crate::components::button::{Button, ButtonVariant};
use crate::components::card::{Card, CardContent, CardHeader, CardTitle};
use crate::components::input::Input;
use crate::components::select::{Select, SelectOption};
use crate::components::skeleton::Skeleton;
use crate::components::tabs::{TabList, TabTrigger};
use crate::notice::Alert;

const GROUPS: [(ToolGroup, &str); 6] = [
    (ToolGroup::Tool, "Tool"),
    (ToolGroup::Repository, "Repository"),
    (ToolGroup::Model, "Model"),
    (ToolGroup::Host, "Host"),
    (ToolGroup::Lead, "Lead"),
    (ToolGroup::Harness, "Harness"),
];

#[component]
pub(crate) fn Metrics() -> Element {
    let mut window = use_signal(Window::default);
    let mut group = use_signal(ToolGroup::default);
    let mut tool = use_signal(String::new);
    let mut run_group = use_signal(RunGroup::default);
    let selected_group = use_memo(move || Some(group()));
    let mut report = use_resource(move || {
        let window = window();
        let group = group();
        let tool = tool();
        let run_group = run_group();
        async move { api::fetch_metrics(window, group, run_group, tool.trim()).await }
    });
    let width = use_context::<crate::app::ConsoleWidth>().0;
    // A resource retains its old result during a refetch. Do not show that result
    // under the new window or grouping while its replacement is pending.
    let pending = matches!(*report.state().read(), UseResourceState::Pending);
    rsx! {
        section { class: "metrics-page", aria_label: "Metrics",
            header { class: "lead-header metrics-header",
                h1 { "Metrics" }
                if width() < 1100 {
                    TabList { aria_label: "Portal view",
                        TabTrigger { value: "agents", index: 0usize, "Agents" }
                        TabTrigger { value: "chat", index: 1usize, "Metrics" }
                    }
                }
                div { class: "metrics-window", role: "group", aria_label: "Metrics window",
                    for (value, label) in [(Window::Day, "Day"), (Window::Week, "Week"), (Window::Month, "Month")] {
                        Button { variant: ButtonVariant::Ghost, aria_pressed: window() == value,
                            onclick: move |_| window.set(value), "{label}" }
                    }
                }
            }
            div { class: "metrics-scroll",
                div { class: "metrics-controls",
                    Select::<ToolGroup> { aria_label: "Tool group", value: Some(ReadSignal::from(selected_group)),
                        on_value_change: move |value: Option<ToolGroup>| { if let Some(value) = value { group.set(value); } },
                        for (index, (value, label)) in GROUPS.into_iter().enumerate() {
                            SelectOption::<ToolGroup> { index, value, text_value: label, "{label}" }
                        }
                    }
                    Input { r#type: "search", aria_label: "Filter tools", placeholder: "Filter tools", value: tool(),
                        oninput: move |event: FormEvent| tool.set(event.value()) }
                }
                if pending {
                    div { class: "metrics-loading", role: "status", aria_label: "Loading metrics",
                        Skeleton {} Skeleton {} Skeleton {} Skeleton {}
                    }
                } else {
                    match report() {
                        Some(Ok(report)) => rsx! { MetricsReportView { report, now_ms: crate::app::clock_ms(),
                            on_run_group: move |value| run_group.set(value) } },
                        Some(Err(error)) => rsx! {
                            div { class: "metrics-failure",
                                Alert { "Metrics unavailable: {error}" }
                                Button { variant: ButtonVariant::Secondary, onclick: move |_| report.restart(), "Retry" }
                            }
                        },
                        None => rsx! { Skeleton { role: "status", aria_label: "Loading metrics" } },
                    }
                }
            }
        }
    }
}

fn duration(value: Option<u64>) -> String {
    let Some(ms) = value else {
        return "—".to_owned();
    };
    if ms < 1_000 {
        return format!("{ms} ms");
    }
    let (unit, scale) = if ms < 60_000 {
        ("s", 1_000)
    } else if ms < 3_600_000 {
        ("min", 60_000)
    } else {
        ("h", 3_600_000)
    };
    let tenths = (u128::from(ms) * 10 + scale / 2) / scale;
    if tenths % 10 == 0 {
        format!("{} {unit}", tenths / 10)
    } else {
        format!("{}.{:01} {unit}", tenths / 10, tenths % 10)
    }
}

fn known_calls(row: &ToolRow) -> u64 {
    row.calls
        .saturating_sub(row.unknown_status)
        .saturating_sub(row.running)
}

fn error_rate(row: &ToolRow) -> Option<(u64, u64)> {
    let known = known_calls(row);
    (known > 0).then_some((row.errors, known))
}

fn error_text(row: &ToolRow) -> String {
    error_rate(row).map_or_else(
        || "—".to_owned(),
        |(errors, known)| {
            let tenths = (u128::from(errors) * 1_000 + u128::from(known) / 2) / u128::from(known);
            format!("{}.{:01}%", tenths / 10, tenths % 10)
        },
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Column {
    Tool,
    Group,
    Calls,
    Error,
    P50,
    P95,
    Max,
}

impl Column {
    fn label(self) -> &'static str {
        match self {
            Self::Tool => "Tool",
            Self::Group => "Group",
            Self::Calls => "Calls",
            Self::Error => "Error %",
            Self::P50 => "p50",
            Self::P95 => "p95",
            Self::Max => "Max",
        }
    }

    fn compare(self, left: &ToolRow, right: &ToolRow) -> Ordering {
        match self {
            Self::Tool => left.tool.cmp(&right.tool),
            Self::Group => left.group.cmp(&right.group),
            Self::Calls => left.calls.cmp(&right.calls),
            Self::Error => match (error_rate(left), error_rate(right)) {
                (Some((a, b)), Some((c, d))) => {
                    (u128::from(a) * u128::from(d)).cmp(&(u128::from(c) * u128::from(b)))
                }
                (a, b) => a.cmp(&b),
            },
            Self::P50 => left
                .duration
                .quantile(0.5)
                .cmp(&right.duration.quantile(0.5)),
            Self::P95 => left
                .duration
                .quantile(0.95)
                .cmp(&right.duration.quantile(0.95)),
            Self::Max => left
                .duration
                .quantile(1.0)
                .cmp(&right.duration.quantile(1.0)),
        }
    }
}

/// The same report surface is used by live fetching and recorded-report rendering.
#[component]
pub(crate) fn MetricsReportView(
    report: MetricsReport,
    now_ms: u64,
    #[props(default)] on_run_group: EventHandler<RunGroup>,
) -> Element {
    let mut sort = use_signal(|| (Column::Calls, true));
    let run_group_value = report.run_group;
    let selected_run_group = use_memo(use_reactive!(|run_group_value| Some(run_group_value)));
    let (column, descending) = sort();
    let mut tools: Vec<_> = report.tools.iter().collect();
    tools.sort_by(|left, right| {
        let order = column.compare(left, right);
        (if descending { order.reverse() } else { order })
            .then_with(|| left.tool.cmp(&right.tool))
            .then_with(|| left.group.cmp(&right.group))
    });
    let grouped = report.tool_group != ToolGroup::Tool;
    let group_label = GROUPS
        .iter()
        .find(|(value, _)| *value == report.tool_group)
        .map_or("Group", |(_, label)| *label);
    let run_group = match report.run_group {
        RunGroup::Profile => "Profile",
        RunGroup::Model => "Model",
        RunGroup::Harness => "Harness",
    };
    rsx! {
        div { class: "metrics-report",
            div { class: "metrics-coverage", aria_label: "Host coverage",
                for host in &report.hosts {
                    if let Some(error) = &host.error {
                        Badge { key: "{host.host}", variant: BadgeVariant::Secondary, class: "metrics-filled", "{host.host} · {error}" }
                    } else {
                        Badge { key: "{host.host}", variant: BadgeVariant::Outline,
                            match host.indexed_ms {
                                Some(indexed) => rsx! { "{host.host} · indexed {now_ms.saturating_sub(indexed) / 60_000} min ago" },
                                None => rsx! { "{host.host} · not indexed" },
                            }
                        }
                    }
                }
            }
            Card { class: "metrics-section",
                CardHeader { CardTitle { h2 { "Tool calls" } } }
                CardContent {
                    if tools.is_empty() {
                        p { class: "metrics-empty", role: "status", "No tool calls in this window match the filter." }
                    } else {
                        div { class: "metrics-table-scroll", role: "region", aria_label: "Tool calls", tabindex: "0",
                            table {
                                thead { tr {
                                    for field in [Column::Tool, Column::Group, Column::Calls, Column::Error, Column::P50, Column::P95, Column::Max].into_iter().filter(|field| grouped || *field != Column::Group) {
                                        th { scope: "col", aria_sort: if column != field { "none" } else if descending { "descending" } else { "ascending" },
                                            Button { variant: ButtonVariant::Ghost, onclick: move |_| sort.set((field, if column == field { !descending } else { !matches!(field, Column::Tool | Column::Group) })),
                                                {if field == Column::Group { group_label } else { field.label() }}
                                                if column == field { if descending { " ▾" } else { " ▴" } }
                                            }
                                        }
                                    }
                                } }
                                tbody {
                                    for row in tools {
                                        tr { key: "{row.tool}:{row.group:?}",
                                            th { scope: "row", class: "metrics-tool", "{row.tool}" }
                                            if grouped { td { "{row.group.as_deref().unwrap_or(portal_types::metrics::UNATTRIBUTED)}" } }
                                            td { "{row.calls}" }
                                            td {
                                                if error_rate(row).is_some_and(|(errors, known)| u128::from(errors) * 100 >= u128::from(known) * 15) {
                                                    Badge { variant: BadgeVariant::Secondary, class: "metrics-filled", "{error_text(row)}" }
                                                } else { "{error_text(row)}" }
                                            }
                                            td { "{duration(row.duration.quantile(0.5))}" }
                                            td { "{duration(row.duration.quantile(0.95))}" }
                                            td { "{duration(row.duration.quantile(1.0))}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Card { class: "metrics-section",
                CardHeader { CardTitle { h2 { "Waiting on you" } } }
                CardContent {
                    div { class: "metrics-table-scroll", role: "region", aria_label: "Owner wait", tabindex: "0",
                        table {
                            thead { tr { th { scope: "col", "Kind" } th { scope: "col", "Answered" } th { scope: "col", "Open" } th { scope: "col", "p50" } th { scope: "col", "p95" } } }
                            tbody {
                                for kind in [WaitKind::Question, WaitKind::PlanReview, WaitKind::Ask, WaitKind::Blocked] {
                                    {
                                        let row = report.owner_waits.iter().find(|row| row.kind == kind);
                                        let label = match kind { WaitKind::Question => "Questions", WaitKind::PlanReview => "Plan reviews", WaitKind::Ask => "Ask", WaitKind::Blocked => "Blocked" };
                                        rsx! { tr { key: "{label}",
                                            th { scope: "row", "{label}" }
                                            td { "{row.map_or(0, |row| row.answered)}" }
                                            td { "{row.map_or(0, |row| row.open)}" }
                                            td { "{duration(row.and_then(|row| row.wait.quantile(0.5)))}" }
                                            td { "{duration(row.and_then(|row| row.wait.quantile(0.95)))}" }
                                        } }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Card { class: "metrics-section",
                CardHeader { class: "metrics-runs-header",
                    CardTitle { h2 { "Agent runs" } }
                    Select::<RunGroup> { aria_label: "Run group", value: Some(ReadSignal::from(selected_run_group)),
                        on_value_change: move |value: Option<RunGroup>| { if let Some(value) = value { on_run_group.call(value); } },
                        for (index, (value, label)) in [(RunGroup::Profile, "Profile"), (RunGroup::Model, "Model"), (RunGroup::Harness, "Harness")].into_iter().enumerate() {
                            SelectOption::<RunGroup> { index, value, text_value: label, "{label}" }
                        }
                    }
                }
                CardContent {
                    if report.runs.is_empty() {
                        p { class: "metrics-empty", "No agent runs in this window." }
                    } else {
                        div { class: "metrics-table-scroll", role: "region", aria_label: "Agent runs", tabindex: "0",
                            table {
                                thead {
                                    tr {
                                        th { scope: "col", rowspan: "2", "{run_group}" }
                                        th { scope: "col", rowspan: "2", "Done" } th { scope: "col", rowspan: "2", "Ask" }
                                        th { scope: "col", rowspan: "2", "Blocked" } th { scope: "col", rowspan: "2", "Closed" }
                                        th { scope: "col", rowspan: "2", "Dropped" } th { scope: "col", rowspan: "2", "Running" }
                                        th { scope: "colgroup", colspan: "2", "Queue" } th { scope: "colgroup", colspan: "2", "Spawn" } th { scope: "colgroup", colspan: "2", "To done" }
                                    }
                                    tr { for _ in 0..3 { th { scope: "col", "p50" } th { scope: "col", "p95" } } }
                                }
                                tbody {
                                    for row in &report.runs {
                                        tr { key: "{row.group}",
                                            th { scope: "row", "{row.group}" }
                                            td { "{row.outcomes.done}" } td { "{row.outcomes.ask}" } td { "{row.outcomes.blocked}" }
                                            td { "{row.outcomes.closed}" } td { "{row.outcomes.dropped}" } td { "{row.outcomes.running}" }
                                            for histogram in [&row.queue_wait, &row.spawn, &row.to_done] {
                                                {percentiles(histogram)}
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

fn percentiles(histogram: &Histogram) -> Element {
    rsx! { td { "{duration(histogram.quantile(0.5))}" } td { "{duration(histogram.quantile(0.95))}" } }
}
