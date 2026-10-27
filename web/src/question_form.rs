//! The form that answers an agent's question, beside the conversation.
//!
//! The page polls `GET /api/questions`, lists the forms that wait for the
//! owner, and posts the answers of the open one to
//! `POST /api/questions/{id}/answer`.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use gloo_net::http::Request;
use portal_types::questions::{
    AnswerBody, Pending, PendingList, Question, QuestionKind, ResponseItem,
};
use serde_json::Value;

const QUESTION_CSS: Asset = asset!("/assets/question_form.css");

/// Where the API lives. Empty means the page's own origin; a dev build can
/// set `DOGSLED_API` at compile time.
const BASE: &str = match option_env!("DOGSLED_API") {
    Some(base) => base,
    None => "",
};

/// The time between two reads of the pending questions.
const POLL_MS: u32 = 2000;

/// The values the owner has chosen or typed, by question id.
type Values = Signal<BTreeMap<String, Value>>;

#[component]
pub fn QuestionForm(#[props(default)] requested: Option<String>) -> Element {
    let selected = use_context::<Signal<Option<String>>>();
    let mut pending = use_signal(Vec::<Pending>::new);
    let mut opened = use_signal(|| None::<String>);
    let mut error = use_signal(|| None::<String>);
    let mut now = use_signal(|| 0_u64);

    // The page reads the waiting questions every two seconds. The wait runs in
    // the browser, so the page needs no timer of its own.
    use_future(move || async move {
        if let Some(ms) = browser_clock().await {
            now.set(ms);
        }
        loop {
            match fetch_pending().await {
                Ok(list) => {
                    pending.set(list);
                    error.set(None);
                }
                Err(message) => error.set(Some(message)),
            }
            match sleep(POLL_MS).await {
                Some(ms) => now.set(ms),
                None => break,
            }
        }
    });

    let list = pending();
    let current = match requested.as_deref() {
        Some(id) => list.iter().find(|item| item.id == id),
        None => current_form(&list, opened().as_deref(), selected().as_deref()),
    }
    .cloned();
    let count = list.len();

    rsx! {
        document::Stylesheet { href: QUESTION_CSS }
        section { class: "qf",
            h2 { class: "group-title", "Needs you: {count}" }
            if let Some(message) = error() {
                p { class: "error", "{message}" }
            }
            if list.is_empty() {
                p { class: "empty", "No question waits." }
            }
            for item in list.iter().filter(|item| requested.as_ref().is_none_or(|id| &item.id == id)) {
                {
                    let id = item.id.clone();
                    let open = current.as_ref().is_some_and(|form| form.id == item.id);
                    let title = form_title(item);
                    let age = age(now().saturating_sub(item.created_ms));
                    rsx! {
                        button {
                            key: "{item.id}",
                            class: if open { "qf-item open" } else { "qf-item" },
                            onclick: move |_| opened.set(Some(id.clone())),
                            span { class: "qf-title", "{title}" }
                            span { class: "qf-meta", "{item.pane} · {age}" }
                        }
                    }
                }
            }
            if requested.is_some() && current.is_none() && error().is_none() {
                p { class: "empty", "This question is loading, answered, or no longer available." }
            }
            if let Some(form) = current {
                AskForm {
                    key: "{form.id}",
                    pending: form,
                    on_done: move |id: String| pending.write().retain(|item| item.id != id),
                }
            }
        }
    }
}

/// Wait for the given milliseconds, then give the browser's clock.
///
/// The page has no timer of its own, so the wait runs in the browser.
async fn sleep(delay_ms: u32) -> Option<u64> {
    let mut script = document::eval(&format!(
        "await new Promise(resolve => setTimeout(resolve, {delay_ms})); dioxus.send(Math.floor(Date.now()));"
    ));
    let value = script.recv::<f64>().await.ok()?;
    Some(value.max(0.0) as u64)
}

/// The browser's clock, in milliseconds.
async fn browser_clock() -> Option<u64> {
    let mut script = document::eval("dioxus.send(Math.floor(Date.now()));");
    let value = script.recv::<f64>().await.ok()?;
    Some(value.max(0.0) as u64)
}

/// The questions that wait for an answer.
async fn fetch_pending() -> Result<Vec<Pending>, String> {
    let response = Request::get(&format!("{BASE}/api/questions"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("the questions answered {}", response.status()));
    }
    let list: PendingList = response.json().await.map_err(|error| error.to_string())?;
    Ok(list.pending)
}

/// What the portal did with an answer.
enum Sent {
    /// The answers are saved.
    Saved,
    /// The question is already answered or withdrawn, so it leaves the list.
    Closed,
}

/// Send the answers of one question.
async fn send_answer(id: &str, responses: Vec<ResponseItem>) -> Result<Sent, String> {
    let response = Request::post(&format!("{BASE}/api/questions/{id}/answer"))
        .json(&AnswerBody { responses })
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;
    match response.status() {
        200..=299 => Ok(Sent::Saved),
        409 | 410 => Ok(Sent::Closed),
        status => Err(format!("the answer was refused with {status}")),
    }
}

/// The form to show: the one clicked, else the first of the selected pane,
/// else the first.
fn current_form<'a>(
    list: &'a [Pending],
    opened: Option<&str>,
    selected: Option<&str>,
) -> Option<&'a Pending> {
    opened
        .and_then(|id| list.iter().find(|item| item.id == id))
        .or_else(|| selected.and_then(|pane| list.iter().find(|item| item.pane == pane)))
        .or_else(|| list.first())
}

/// The title of a form, or its first question when it has none.
fn form_title(item: &Pending) -> String {
    item.questions
        .title
        .clone()
        .or_else(|| {
            item.questions
                .questions
                .first()
                .map(|question| question.question.clone())
        })
        .unwrap_or_else(|| "Question".to_owned())
}

/// A short age: seconds, minutes, hours, or days.
fn age(ms: u64) -> String {
    let seconds = ms / 1000;
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86399 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86400),
    }
}

#[component]
fn AskForm(pending: Pending, on_done: EventHandler<String>) -> Element {
    let values: Values = use_signal(|| initial_values(&pending.questions.questions));
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let id = pending.id.clone();
    let questions = pending.questions.questions.clone();
    let heading = pending.questions.title.clone();
    let description = pending.questions.description.clone();

    rsx! {
        form {
            class: "qf-form",
            onsubmit: move |event: FormEvent| {
                // The page answers the form itself, so the browser must not
                // send it.
                event.prevent_default();
                let id = id.clone();
                let responses = collect_responses(&questions, &values.read());
                async move {
                    busy.set(true);
                    error.set(None);
                    match send_answer(&id, responses).await {
                        Ok(Sent::Saved | Sent::Closed) => on_done.call(id),
                        Err(message) => {
                            error.set(Some(message));
                            busy.set(false);
                        }
                    }
                }
            },
            if let Some(heading) = heading {
                h3 { "{heading}" }
            }
            if let Some(description) = description {
                p { class: "qf-muted", "{description}" }
            }
            for question in pending.questions.questions.iter().cloned() {
                QuestionField { key: "{question.id}", form: pending.id.clone(), question, values }
            }
            if let Some(message) = error() {
                p { class: "error", "{message}" }
            }
            button { class: "qf-submit", r#type: "submit", disabled: busy(), "Send answers" }
        }
    }
}

/// The recommended labels of a question: a string or a list of strings.
fn recommended_labels(question: &Question) -> Vec<String> {
    match &question.recommended {
        Some(Value::String(label)) => vec![label.clone()],
        Some(Value::Array(labels)) => labels
            .iter()
            .filter_map(|label| label.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

/// The values a form starts with: the recommended options, unless the
/// recommendation is slight.
fn initial_values(questions: &[Question]) -> BTreeMap<String, Value> {
    let mut values = BTreeMap::new();
    for question in questions {
        let slight = question.conviction.as_ref().and_then(Value::as_str) == Some("slight");
        let labels = recommended_labels(question);
        if slight || labels.is_empty() {
            continue;
        }
        match question.kind {
            QuestionKind::Single => {
                values.insert(question.id.clone(), Value::String(labels[0].clone()));
            }
            QuestionKind::Multi => {
                values.insert(
                    question.id.clone(),
                    Value::Array(labels.into_iter().map(Value::String).collect()),
                );
            }
            _ => {}
        }
    }
    values
}

/// The answers to send: one per question that takes a value.
fn collect_responses(
    questions: &[Question],
    values: &BTreeMap<String, Value>,
) -> Vec<ResponseItem> {
    questions
        .iter()
        .filter_map(|question| {
            let value = match question.kind {
                QuestionKind::Single | QuestionKind::Text => values
                    .get(&question.id)
                    .cloned()
                    .unwrap_or_else(|| Value::String(String::new())),
                QuestionKind::Multi => values
                    .get(&question.id)
                    .cloned()
                    .unwrap_or_else(|| Value::Array(Vec::new())),
                QuestionKind::Image | QuestionKind::Info | QuestionKind::Other(_) => return None,
            };
            Some(ResponseItem {
                id: question.id.clone(),
                value,
                attachments: None,
            })
        })
        .collect()
}

/// The label of an option: the string, or the `label` of the object.
fn option_label(option: &Value) -> String {
    match option {
        Value::String(label) => label.clone(),
        other => other
            .get("label")
            .and_then(Value::as_str)
            .map_or_else(|| other.to_string(), str::to_owned),
    }
}

/// The text of a content block: the string, or the `source` of the object.
fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        other => other
            .get("source")
            .and_then(Value::as_str)
            .map_or_else(|| other.to_string(), str::to_owned),
    }
}

/// The chosen labels of a question.
fn chosen(values: &BTreeMap<String, Value>, id: &str) -> Vec<String> {
    match values.get(id) {
        Some(Value::String(label)) => vec![label.clone()],
        Some(Value::Array(labels)) => labels
            .iter()
            .filter_map(|label| label.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

/// Choose an option: replace the choice of a single question, flip it in a
/// multi one.
fn choose(mut values: Values, id: &str, label: &str, multi: bool) {
    let mut map = values.write();
    if multi {
        let mut labels = chosen(&map, id);
        match labels.iter().position(|item| item == label) {
            Some(position) => {
                labels.remove(position);
            }
            None => labels.push(label.to_owned()),
        }
        map.insert(
            id.to_owned(),
            Value::Array(labels.into_iter().map(Value::String).collect()),
        );
    } else {
        map.insert(id.to_owned(), Value::String(label.to_owned()));
    }
}

#[component]
fn QuestionField(form: String, question: Question, values: Values) -> Element {
    let weight = match question.weight.as_ref().and_then(Value::as_str) {
        Some("critical") => "critical",
        Some("minor") => "minor",
        _ => "normal",
    };
    let recommended = recommended_labels(&question);
    let options = question.options.clone().unwrap_or_default();
    let content = question.content.as_ref().map(content_text);
    let id = question.id.clone();
    let text = values
        .read()
        .get(&id)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let multi = question.kind == QuestionKind::Multi;
    let group = format!("{form}-{id}");

    rsx! {
        fieldset { class: "qf-field {weight}",
            legend { "{question.question}" }
            if let Some(context) = question.context.clone() {
                p { class: "qf-muted", "{context}" }
            }
            if let Some(content) = content {
                pre { class: "qf-content", "{content}" }
            }
            if question.media.is_some() {
                p { class: "qf-muted", "Media: not supported in the portal yet" }
            }
            match question.kind {
                QuestionKind::Single | QuestionKind::Multi => rsx! {
                    for option in options {
                        ChoiceOption {
                            key: "{option_label(&option)}",
                            group: group.clone(),
                            id: id.clone(),
                            recommended: recommended.contains(&option_label(&option)),
                            option,
                            multi,
                            values,
                        }
                    }
                },
                QuestionKind::Text => rsx! {
                    textarea {
                        name: "{group}",
                        rows: "3",
                        value: "{text}",
                        oninput: move |event: FormEvent| {
                            values.write().insert(id.clone(), Value::String(event.value()));
                        },
                    }
                },
                QuestionKind::Image => rsx! {
                    p { class: "qf-muted", "Image answers: not supported in the portal yet" }
                },
                QuestionKind::Info => rsx! {},
                QuestionKind::Other(kind) => rsx! {
                    p { class: "qf-muted", "Question type {kind}: not supported in the portal yet" }
                },
            }
        }
    }
}

#[component]
fn ChoiceOption(
    group: String,
    id: String,
    option: Value,
    recommended: bool,
    multi: bool,
    values: Values,
) -> Element {
    let label = option_label(&option);
    let content = option.get("content").map(content_text);
    let checked = chosen(&values.read(), &id).contains(&label);
    let pick = label.clone();

    rsx! {
        label { class: if checked { "qf-option checked" } else { "qf-option" },
            input {
                r#type: if multi { "checkbox" } else { "radio" },
                name: "{group}",
                value: "{label}",
                checked,
                onchange: move |_| choose(values, &id, &pick, multi),
            }
            span { "{label}" }
            if recommended {
                span { class: "qf-badge", "Recommended" }
            }
            if let Some(content) = content {
                pre { class: "qf-content", "{content}" }
            }
        }
    }
}
