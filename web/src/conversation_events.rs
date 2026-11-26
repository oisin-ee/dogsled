//! Present recorded Dogsled activity without changing the transcript or guessing outcomes.
use dioxus::prelude::*;
use dioxus_icons::lucide::{
    Activity, ArrowUpRight, CircleX, ClipboardList, CornerDownRight, Inbox, TriangleAlert,
};
use portal_types::agents::{Source, ToolStatus, ToolSummary};
use serde_json::Value;

/// Accept the Dogsled MCP server (including Claude's plugin namespace), not a generic
/// tool called `run` or a tool whose argument happens to mention Dogsled.
pub(crate) fn operation(name: &str) -> Option<&str> {
    let (server, method) = name.strip_prefix("mcp__")?.rsplit_once("__")?;
    ((server == "dogsled" || (server.starts_with("plugin_") && server.ends_with("_dogsled")))
        && !method.is_empty())
    .then_some(method)
}

fn action(method: &str) -> &str {
    match method {
        "run" => "Run job",
        "spawn" => "Spawn worker",
        "read" => "Read output",
        "send" => "Send message",
        "inbox" => "Read inbox",
        "status" => "Inspect fleet",
        "stop" => "Stop job",
        "close" => "Close session",
        "place" => "Reserve host",
        "cancel" => "Cancel request",
        "lead" => "Start lead",
        "accept" => "Accept work",
        "claim" => "Claim work",
        "drop" => "Drop branch",
        "end_turn" => "End turn",
        "decide" => "Record decision",
        other => other,
    }
}

/// The history wire carries a bounded call line, not an argument object. Only use fields
/// from complete recorded JSON following this tool's exact name. Keep the original line
/// available when the history window cuts the JSON or the harness recorded another form.
fn arguments(name: &str, call: Option<&str>) -> Option<Value> {
    let json = call?.strip_prefix(name)?.strip_prefix(' ')?;
    serde_json::from_str(json).ok()
}

fn field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str().filter(|s| !s.is_empty())
}

fn receipt<'a>(
    method: &str,
    status: ToolStatus,
    result: &'a Value,
) -> (&'static str, Option<&'a str>) {
    match status {
        ToolStatus::Unknown => ("No recorded outcome", None),
        ToolStatus::Error => ("Call failed", None),
        ToolStatus::Completed if method == "decide" => ("Decision recorded", None),
        ToolStatus::Completed => {
            if matches!(method, "run" | "spawn")
                && result["queued"] == true
                && let Some(ticket) = field(result, "ticket")
            {
                ("Ticket returned", Some(ticket))
            } else if method == "run"
                && result["launch_confirmed_ms"].is_u64()
                && let Some(id) = field(result, "id")
            {
                ("Launch confirmed", Some(id))
            } else {
                ("Result recorded", None)
            }
        }
    }
}

#[component]
pub(crate) fn CallPanel(tool: ToolSummary, method: String) -> Element {
    let shown = tool
        .output
        .as_deref()
        .map_or(0, |s| s.lines().count() as u32);
    let earlier = tool.output_lines.unwrap_or(shown).saturating_sub(shown);
    let result = tool
        .output
        .as_deref()
        .filter(|_| earlier == 0 && tool.status == ToolStatus::Completed)
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or_default();
    let (outcome, id) = receipt(&method, tool.status, &result);
    let error = tool.status == ToolStatus::Error;
    let complete = tool.status == ToolStatus::Completed && earlier == 0;
    let read = complete && method == "read";
    let job_log = read && field(&result, "kind") == Some("job");
    let reply = (read && field(&result, "kind") == Some("agent"))
        .then(|| field(&result, "reply"))
        .flatten();
    let log = job_log.then(|| field(&result, "tail")).flatten();
    let counts = if complete && method == "inbox" {
        result["received"]
            .as_array()
            .zip(result["sent"].as_array())
            .map(|(received, sent)| format!("{} received · {} sent", received.len(), sent.len()))
    } else if complete && method == "status" {
        result["rows"]
            .as_array()
            .map(|rows| format!("{} fleet items", rows.len()))
    } else {
        None
    };
    rsx! {
        div { class: "ds-call-panel", "data-error": error,
            CallHeading { method: method.clone(), name: tool.name.clone().unwrap_or_default(), call: tool.call.clone() }
            div { class: "ds-receipt", CornerDownRight { size: 15 } strong { "{outcome}" } if let Some(id) = id { code { "{id}" } } }
            if let Some(counts) = counts { p { class: "ds-result-note", "{counts}" } }
            if let Some(reply) = reply { blockquote { class: "ds-result-note", strong { "Worker reply" } p { "{reply}" } } }
            if let Some(log) = log { details { class: "ds-event-detail ds-result", open: true, summary { "Job output" } pre { "{log}" } } }
            if let Some(output) = tool.output.as_deref() {
                details { class: "ds-event-detail ds-result", open: error,
                    summary { if log.is_some() || reply.is_some() { "Recorded result" } else { "Output" } } if earlier > 0 { small { "{earlier} earlier lines" } }
                    pre { "{output}" }
                }
            }
        }
    }
}

/// Shared by history and the live edge, so running and recorded calls keep their identity.
#[component]
pub(crate) fn CallHeading(method: String, name: String, call: Option<String>) -> Element {
    let arguments = arguments(&name, call.as_deref()).unwrap_or_default();
    let argv = (method == "run")
        .then(|| arguments["cmd"].as_array())
        .flatten()
        .and_then(|args| args.iter().map(Value::as_str).collect::<Option<Vec<_>>>());
    let target = ["name", "target", "pane", "id"]
        .into_iter()
        .find_map(|key| field(&arguments, key))
        .or_else(|| field(&arguments["unit"], "id"));
    let fields = [
        ("Host", "host"),
        ("Directory", "cwd"),
        ("Timeout", "timeout"),
        ("Idle timeout", "idle_timeout"),
        ("Branch", "branch"),
        ("Profile", "profile"),
    ];
    let title = action(&method);
    let decision = (method == "decide")
        .then(|| field(&arguments, "title"))
        .flatten();
    rsx! {
            div { class: "ds-event-kind", ArrowUpRight { size: 16 } "Dogsled call" }
            div { class: "ds-event-title", "{title}" if let Some(target) = target { code { "{target}" } } }
            if let Some(decision) = decision { p { class: "ds-result-note", "{decision}" } }
            if let Some(argv) = argv {
                div { class: "ds-command", aria_label: "Command arguments",
                    for arg in argv { code { "{arg}" } }
                }
            }
            dl { class: "ds-call-fields",
                for (label, key) in fields {
                    if let Some(value) = field(&arguments, key) { div { dt { "{label}" } dd { "{value}" } } }
                }
            }
            if let Some(call) = call.as_deref() {
                details { class: "ds-event-detail", summary { "Recorded call" } pre { "{call}" } }
            }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Tone {
    Inbound,
    Task,
    Attention,
    Error,
    Notice,
}

impl Tone {
    pub(crate) fn class(self) -> &'static str {
        match self {
            Self::Attention => "attention",
            Self::Error => "error",
            Self::Notice => "notice",
            Self::Inbound | Self::Task => "incoming",
        }
    }
}

pub(crate) fn delivery(source: &Source) -> Option<(Tone, &str)> {
    match source {
        Source::Dogsled { delivery_kind, .. } => Some(match delivery_kind.as_str() {
            "reply" => (Tone::Inbound, "Worker reply"),
            "message" => (Tone::Inbound, "Incoming message"),
            "task" => (Tone::Task, "Task received"),
            "ask" => (Tone::Attention, "Worker question"),
            "blocked" => (Tone::Attention, "Worker blocked"),
            "failed" => (Tone::Error, "Not delivered"),
            "check-in" => (Tone::Notice, "Fleet check-in"),
            "ticket" => (Tone::Notice, "Ticket update"),
            "transfer" => (Tone::Notice, "Transfer notice"),
            _ => (Tone::Inbound, "Dogsled delivery"),
        }),
        Source::Custom { custom_type } => Some((
            Tone::Notice,
            match custom_type.as_str() {
                "todo-reminder" => "Todo reminder",
                "todo-plan-approved" => "Plan approved",
                "dogsled:plan-mode" => "Plan mode",
                "dogsled" => "Dogsled notice",
                _ => "Extension notice",
            },
        )),
        Source::Harness { .. } => None,
    }
}

#[component]
pub(crate) fn DeliveryLabel(tone: Tone, label: String) -> Element {
    rsx! { div { class: "ds-event-kind",
        match tone {
            Tone::Inbound => rsx!(Inbox { size: 16 }), Tone::Task => rsx!(ClipboardList { size: 16 }),
            Tone::Notice => rsx!(Activity { size: 16 }), Tone::Attention => rsx!(TriangleAlert { size: 16 }),
            Tone::Error => rsx!(CircleX { size: 16 }),
        }
        "{label}"
    } }
}
