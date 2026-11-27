//! The selected agent's read-only decision log.

use dioxus::prelude::*;
use dioxus_primitives::scroll_area::ScrollDirection;
use portal_types::agents::AgentAddress;
use portal_types::autonomy::{
    AutonomyCause, AutonomyView, DecisionList, DecisionSource, DecisionView,
};

use crate::components::accordion::{Accordion, AccordionContent, AccordionItem, AccordionTrigger};
use crate::components::scroll_area::ScrollArea;
use crate::components::switch::Switch;
use crate::notice::{Alert, Status};

/// The fleet switch. It names the hosts whose release ignores both switches, because the
/// agents there keep asking the owner while the switch reads on.
#[component]
pub(crate) fn FleetAutonomySwitch(
    view: Option<AutonomyView>,
    busy: bool,
    on_change: EventHandler<bool>,
) -> Element {
    let known = view.is_some();
    let on = view.as_ref().is_some_and(|view| !view.fleet.is_empty());
    let ignored = view.map(|view| view.unsupported).unwrap_or_default();
    let hosts = ignored.join(", ");
    rsx! {
        div { class: "autonomy-switch",
            Switch { id: "fleet-autonomy", checked: Some(on), disabled: !known || busy,
                on_checked_change: move |on| { if known && !busy { on_change.call(on); } },
            }
            label { r#for: "fleet-autonomy", "Autonomous" }
            if !ignored.is_empty() {
                span { class: "autonomy-note", "data-part": "autonomy-unsupported",
                    title: "These hosts run a release without autonomy. Their agents still ask you.",
                    "not on {hosts}"
                }
            }
        }
    }
}

#[component]
pub(crate) fn AgentAutonomySwitch(
    cause: Option<AutonomyCause>,
    worker: bool,
    disabled: bool,
    on_change: EventHandler<bool>,
) -> Element {
    let label = match cause {
        Some(AutonomyCause::Fleet) => "Autonomous · fleet",
        Some(AutonomyCause::Owner) => "Autonomous · from lead",
        _ => "Autonomous",
    };
    let locked = worker || matches!(cause, Some(AutonomyCause::Fleet | AutonomyCause::Owner));
    rsx! {
        div { class: "autonomy-switch",
            Switch { id: "agent-autonomy", checked: Some(cause.is_some()), disabled: disabled || locked,
                on_checked_change: move |on| { if !disabled && !locked { on_change.call(on); } },
            }
            label { r#for: "agent-autonomy", "{label}" }
        }
    }
}

#[component]
pub(crate) fn Decisions(
    address: AgentAddress,
    autonomous: bool,
    result: Option<Result<DecisionList, String>>,
) -> Element {
    rsx! {
        ScrollArea { class: "console-scroll decisions-list", direction: ScrollDirection::Vertical,
            aria_label: "Recorded decisions",
            match result {
                None => rsx! { Status { "data-part": "column-empty", "Loading decisions…" } },
                Some(Err(error)) => rsx! { Alert { "Decisions are unavailable. {error}" } },
                Some(Ok(list)) => rsx! {
                    if !list.gaps.is_empty() {
                        // The hosts that answered are current; only the named hosts are missing.
                        Alert {
                            div { class: "decision-gaps",
                                p { if list.decisions.is_empty() { "Decisions are unavailable." } else { "Some decisions are unavailable." } }
                                for gap in &list.gaps {
                                    p { key: "{gap.host}", "Host {gap.host}: {gap.error}." }
                                }
                            }
                        }
                    } else if list.decisions.is_empty() {
                        Status { "data-part": "column-empty",
                            if autonomous {
                                p { "No decisions yet." }
                                p { "Autonomous is on. The agent records each open question it settles here." }
                            } else {
                                p { "No decisions." }
                                p { "This agent asks you when a question is open." }
                                p { "Switch Autonomous on to let it decide and record each decision here." }
                            }
                        }
                    }
                    if !list.decisions.is_empty() {
                        Accordion { "data-part": "decision-rows", allow_multiple_open: true, horizontal: false,
                            for (index, record) in list.decisions.into_iter().enumerate() {
                                DecisionRow { key: "{record.host}:{record.id}", index,
                                    own: record.host == address.host && record.pane == address.pane, record }
                            }
                        }
                        p { class: "decision-meta decisions-order", "Newest first · Read only" }
                    }
                },
            }
        }
    }
}

#[component]
fn DecisionRow(index: usize, own: bool, record: DecisionView) -> Element {
    let who = if own {
        "lead".to_owned()
    } else {
        match record
            .name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
        {
            Some(name) => format!("worker {name}"),
            None => format!("worker {}", record.pane),
        }
    };
    let source = match record.source {
        DecisionSource::Question => "question",
        DecisionSource::Plan => "plan",
        DecisionSource::Agent => "agent",
    };
    let time = crate::agent_view::platform_time(record.created_ms);
    let short = time.as_ref().map_or("—", |(short, _, _)| short.as_str());
    let fields = [
        ("Question", record.question.as_deref()),
        ("Decision", Some(record.decision.as_str())),
        ("Why", Some(record.why.as_str())),
        ("Other options", record.alternatives.as_deref()),
    ];
    rsx! {
        AccordionItem { index,
            AccordionTrigger { "data-part": "decision-trigger",
                span { class: "decision-summary",
                    strong { "{record.title}" }
                    span { class: "decision-meta",
                        time { datetime: time.as_ref().map(|(_, _, iso)| iso.clone()),
                            title: time.as_ref().map(|(_, full, _)| full.clone()),
                            "data-timestamp-ms": "{record.created_ms}", "{short}" }
                        " · {who} · {source}"
                    }
                }
            }
            AccordionContent {
                div { class: "decision-body",
                    dl {
                        for (label, value) in fields {
                            if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
                                div { dt { "{label}" } dd { "{value}" } }
                            }
                        }
                    }
                    if record.source == DecisionSource::Plan && let Some(review) = &record.review {
                        a { href: "/?host={crate::api::encode(&record.host)}&recorded_review={crate::api::encode(review)}",
                            target: "_blank", rel: "noopener", "Open plan review" }
                    }
                }
            }
        }
    }
}
