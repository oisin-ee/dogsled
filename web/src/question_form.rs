//! The form that answers an agent's question, beside the conversation.
//!
//! The page polls `GET /api/questions`, lists the forms that wait for the
//! owner, and posts the answers of the open one to
//! `POST /api/questions/{id}/answer`.

use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_primitives::checkbox::CheckboxState;
use dioxus_sdk_time::sleep;
use gloo_net::http::Request;
use portal_types::questions::{
    AnswerBody, Media, MediaPosition, Pending, PendingList, Question, QuestionKind, Rejection,
    ResponseItem,
};
use serde_json::Value;

use crate::app::clock_ms;
use crate::components::{
    aspect_ratio::AspectRatio,
    badge::{Badge, BadgeVariant},
    button::{Button, ButtonVariant},
    card::{Card, CardContent, CardDescription, CardHeader, CardTitle},
    checkbox::Checkbox,
    item::{
        Item, ItemActions, ItemContent, ItemDescription, ItemFooter, ItemGroup, ItemMedia,
        ItemSize, ItemTitle, ItemVariant,
    },
    label::Label,
    radio_group::{RadioGroup, RadioItem},
    scroll_area::ScrollArea,
    textarea::Textarea,
};
use crate::notice::{Alert, Status};

/// Where the API lives. Empty means the page's own origin; a dev build can
/// set `DOGSLED_API` at compile time.
const BASE: &str = match option_env!("DOGSLED_API") {
    Some(base) => base,
    None => "",
};

/// The time between two reads of the pending questions.
const POLL: Duration = Duration::from_secs(2);

/// The values the owner has chosen or typed, by question id.
type Values = Signal<BTreeMap<String, Value>>;

#[component]
pub fn QuestionForm(
    #[props(default)] requested: Option<String>,
    on_done: EventHandler<String>,
) -> Element {
    let selected = use_context::<Signal<Option<String>>>();
    let mut pending = use_signal(Vec::<Pending>::new);
    let mut opened = use_signal(|| None::<String>);
    let mut error = use_signal(|| None::<String>);
    let mut now = use_signal(clock_ms);

    // The page reads the waiting questions every two seconds.
    use_future(move || async move {
        loop {
            match fetch_pending().await {
                Ok(list) => {
                    pending.set(list);
                    error.set(None);
                }
                Err(message) => error.set(Some(message)),
            }
            sleep(POLL).await;
            now.set(clock_ms());
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
        if requested.is_none() {
            Item { ItemContent { ItemTitle { "Needs you: {count}" } } }
        }
        if let Some(message) = error() {
            Alert { "{message}" }
        }
        if list.is_empty() {
            Status { "No question waits." }
        }
        // An agent screen opens one form by id; only the standalone page lists them.
        ItemGroup {
            for item in list.iter().filter(|_| requested.is_none()) {
                {
                    let id = item.id.clone();
                    let open = current.as_ref().is_some_and(|form| form.id == item.id);
                    let title = form_title(item);
                    let age = age(now().saturating_sub(item.created_ms));
                    rsx! {
                        Button {
                            key: "{item.id}",
                            variant: ButtonVariant::Ghost,
                            aria_current: open.then_some("true"),
                            onclick: move |_| opened.set(Some(id.clone())),
                            "{title}"
                            Badge { variant: BadgeVariant::Secondary, "{item.pane} · {age}" }
                        }
                    }
                }
            }
        }
        if requested.is_some() && current.is_none() && error().is_none() {
            Status { "This question is loading, answered, or no longer available." }
        }
        if let Some(form) = current {
            AskForm {
                key: "{form.id}",
                pending: form,
                on_done: move |id: String| {
                    pending.write().retain(|item| item.id != id);
                    on_done.call(id);
                },
            }
        }
    }
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

/// Reject the form separately from its draft answers.
async fn send_rejection(id: &str, reason: String) -> Result<Sent, String> {
    let response = Request::post(&format!("{BASE}/api/questions/{id}/reject"))
        .json(&Rejection { reason })
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;
    match response.status() {
        200..=299 => Ok(Sent::Saved),
        409 | 410 => Ok(Sent::Closed),
        status => Err(format!(
            "Could not reject the interview (HTTP {status}). Try again."
        )),
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
    let mut rejection_reason = use_signal(String::new);
    let reject_id = pending.id.clone();
    let id = pending.id.clone();
    let questions = pending.questions.questions.clone();
    let heading = pending.questions.title.clone();
    let description = pending.questions.description.clone();
    let total = pending
        .questions
        .questions
        .iter()
        .filter(|q| takes_an_answer(&q.kind))
        .count();

    rsx! {
        form {
            onsubmit: move |event: FormEvent| {
                // The page answers the form itself, so the browser must not
                // send it.
                event.prevent_default();
                if busy() { return; }
                busy.set(true);
                error.set(None);
                let id = id.clone();
                let responses = collect_responses(&questions, &values.read());
                spawn(async move {
                    match send_answer(&id, responses).await {
                        Ok(Sent::Saved | Sent::Closed) => on_done.call(id),
                        Err(message) => {
                            error.set(Some(message));
                            busy.set(false);
                        }
                    }
                });
            },
            ItemGroup {
                if heading.is_some() || description.is_some() {
                    Item {
                        ItemContent {
                            if let Some(heading) = heading {
                                ItemTitle { "{heading}" }
                            }
                            if let Some(description) = description {
                                ItemDescription { "{description}" }
                            }
                        }
                    }
                }
                for (index, question) in pending.questions.questions.iter().cloned().enumerate() {
                    QuestionField {
                        key: "{question.id}",
                        form: pending.id.clone(),
                        number: index + 1,
                        question,
                        values,
                    }
                }
                if let Some(message) = error() {
                    Alert { "{message}" }
                }
                Item {
                    ItemContent {
                        ItemDescription { "Answered {answered_count(&questions, &values.read())} of {total}" }
                    }
                    ItemActions {
                        Button { r#type: "submit", disabled: busy(), "Send answers" }
                    }
                }
                Card { aria_label: "Reject interview",
                    CardContent {
                        Item {
                            ItemContent {
                                Label { html_for: "rejection-reason", "Reason for rejecting (optional)" }
                                Textarea {
                                    id: "rejection-reason", name: "rejection-reason", rows: "2", value: "{rejection_reason}", disabled: busy(),
                                    placeholder: "Tell the agent why these questions do not help, or what you need instead",
                                    oninput: move |event: FormEvent| rejection_reason.set(event.value()),
                                }
                            }
                            ItemActions {
                                Button {
                                    variant: ButtonVariant::Outline,
                                    r#type: "button", disabled: busy(),
                                    onclick: move |_| {
                                        if busy() { return; }
                                        busy.set(true);
                                        error.set(None);
                                        let id = reject_id.clone();
                                        let reason = rejection_reason();
                                        spawn(async move {
                                            match send_rejection(&id, reason).await {
                                                Ok(Sent::Saved | Sent::Closed) => on_done.call(id),
                                                Err(message) => { error.set(Some(message)); busy.set(false); }
                                            }
                                        });
                                    },
                                    "Reject questions"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The questions of a form that hold an answer, so the footer can count them.
fn answered_count(questions: &[Question], values: &BTreeMap<String, Value>) -> usize {
    questions
        .iter()
        .filter(|question| {
            takes_an_answer(&question.kind) && !chosen(values, &question.id).is_empty()
        })
        .count()
}

/// Whether a question takes an answer. An info question is a panel of
/// content, and an image question is not answered in the portal.
fn takes_an_answer(kind: &QuestionKind) -> bool {
    matches!(
        kind,
        QuestionKind::Single | QuestionKind::Multi | QuestionKind::Text
    )
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

/// Every choice question offers a writable alternative, even when its author did not.
fn choice_options(question: &Question) -> Vec<Value> {
    let mut options = question.options.clone().unwrap_or_default();
    if !options.iter().any(|option| option_label(option) == "Other") {
        options.push(Value::String("Other".to_owned()));
    }
    options
}

/// An answer can be a label alone or a choice with the owner's note.
fn answer_label(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.get("option").and_then(Value::as_str))
}

fn selected_values(values: &BTreeMap<String, Value>, id: &str) -> Vec<Value> {
    match values.get(id) {
        Some(Value::Array(answers)) => answers.clone(),
        Some(answer) => vec![answer.clone()],
        None => Vec::new(),
    }
}

/// The chosen labels of a question, independent of any attached notes.
fn chosen(values: &BTreeMap<String, Value>, id: &str) -> Vec<String> {
    selected_values(values, id)
        .iter()
        .filter_map(|value| {
            answer_label(value)
                .filter(|label| !label.trim().is_empty())
                .map(str::to_owned)
        })
        .collect()
}

fn set_note(values: &mut BTreeMap<String, Value>, id: &str, label: &str, note: String) {
    let Some(value) = values.get_mut(id) else {
        return;
    };
    let answers = match value {
        Value::Array(answers) => answers.as_mut_slice(),
        answer => std::slice::from_mut(answer),
    };
    if let Some(answer) = answers
        .iter_mut()
        .find(|answer| answer_label(answer) == Some(label))
    {
        *answer = if note.trim().is_empty() {
            Value::String(label.to_owned())
        } else {
            serde_json::json!({ "option": label, "note": note })
        };
    }
}

/// Choose an option: replace the choice of a single question, flip it in a
/// multi one.
fn choose(values: &mut BTreeMap<String, Value>, id: &str, label: &str, multi: bool) {
    if multi {
        let mut answers = selected_values(values, id);
        match answers
            .iter()
            .position(|answer| answer_label(answer) == Some(label))
        {
            Some(position) => {
                answers.remove(position);
            }
            None => answers.push(Value::String(label.to_owned())),
        }
        values.insert(id.to_owned(), Value::Array(answers));
    } else if !chosen(values, id).iter().any(|selected| selected == label) {
        values.insert(id.to_owned(), Value::String(label.to_owned()));
    }
}

#[component]
fn QuestionField(form: String, number: usize, question: Question, values: Values) -> Element {
    let weight = match question.weight.as_ref().and_then(Value::as_str) {
        Some("critical") => "critical",
        Some("minor") => "minor",
        _ => "normal",
    };
    let recommended = recommended_labels(&question);
    let options = choice_options(&question);
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
    // The id of the heading, which names the group for the screen reader.
    let label = format!("{group}-label");
    // An info question is a panel of content, not a field with an answer.
    let panel = question.kind == QuestionKind::Info;
    let media = question.media.clone();
    // The media sits above, below or beside the answer.
    let position = media
        .as_ref()
        .map_or(MediaPosition::Above, |media| media.position());

    let answers = rsx! {
        match question.kind {
            QuestionKind::Single | QuestionKind::Multi => rsx! {
                if multi {
                    ItemGroup {
                        for (index, option) in options.into_iter().enumerate() {
                            ChoiceOption {
                                key: "{option_label(&option)}",
                                group: group.clone(),
                                id: id.clone(),
                                index,
                                recommended: recommended.contains(&option_label(&option)),
                                option,
                                multi,
                                values,
                            }
                        }
                    }
                } else {
                    RadioGroup {
                        name: group.clone(),
                        value: chosen(&values.read(), &id).into_iter().next(),
                        on_value_change: {
                            let id = id.clone();
                            move |label: String| choose(&mut values.write(), &id, &label, false)
                        },
                        for (index, option) in options.into_iter().enumerate() {
                            ChoiceOption {
                                key: "{option_label(&option)}",
                                group: group.clone(),
                                id: id.clone(),
                                index,
                                recommended: recommended.contains(&option_label(&option)),
                                option,
                                multi,
                                values,
                            }
                        }
                    }
                }
            },
            QuestionKind::Text => rsx! {
                Textarea {
                    name: "{group}",
                    rows: "3",
                    value: "{text}",
                    oninput: move |event: FormEvent| {
                        values.write().insert(id.clone(), Value::String(event.value()));
                    },
                }
            },
            QuestionKind::Image => rsx! {
                Status { "Image answers: not supported in the portal yet" }
            },
            QuestionKind::Info => rsx! {},
            QuestionKind::Other(kind) => rsx! {
                Status { "Question type {kind}: not supported in the portal yet" }
            },
        }
    };

    rsx! {
        Card {
            role: if panel { None } else { Some("group") },
            aria_labelledby: if panel { None } else { Some(label.clone()) },
            CardHeader {
                CardTitle { id: "{label}",
                    Badge { variant: BadgeVariant::Outline, "{number}" }
                    if !panel && weight != "normal" {
                        Badge { variant: if weight == "critical" { BadgeVariant::Destructive } else { BadgeVariant::Secondary }, "{weight}" }
                    }
                    "{question.question}"
                }
                if let Some(context) = question.context.clone() {
                    CardDescription { "{context}" }
                }
            }
            CardContent {
                if let Some(content) = content {
                    pre { "{content}" }
                }
                if !panel {
                    if position == MediaPosition::Above {
                        if let Some(media) = media.clone() {
                            MediaView { media }
                        }
                    }
                    {answers}
                    if position != MediaPosition::Above {
                        if let Some(media) = media.clone() {
                            MediaView { media }
                        }
                    }
                } else if let Some(media) = media.clone() {
                    MediaView { media }
                }
            }
        }
    }
}

/// The media of a question: the image, table, chart, diagram or HTML the
/// question shows.
#[component]
fn MediaView(media: Media) -> Element {
    rsx! {
        match &media {
            Media::Image { src, alt, caption, .. } => rsx! {
                Item { variant: ItemVariant::Outline,
                    ItemContent {
                        AspectRatio { ratio: 16.0 / 9.0, img { src: "{src}", alt: alt.clone().unwrap_or_default() } }
                        if let Some(caption) = caption {
                            ItemDescription { "{caption}" }
                        }
                    }
                }
            },
            Media::Table { table, caption, .. } => rsx! {
                Item { variant: ItemVariant::Outline,
                    ItemContent {
                        ScrollArea {
                            table {
                                if !table.headers.is_empty() {
                                    thead {
                                        tr {
                                            for header in table.headers.iter() {
                                                th { scope: "col", "{header}" }
                                            }
                                        }
                                    }
                                }
                                tbody {
                                    for (index, row) in table.rows.iter().enumerate() {
                                        tr {
                                            for cell in row.iter() {
                                                td {
                                                    if table.highlights.contains(&index) { mark { "{cell}" } } else { "{cell}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(caption) = caption {
                            ItemDescription { "{caption}" }
                        }
                    }
                }
            },
            Media::Chart { chart, caption, .. } => rsx! {
                Item { variant: ItemVariant::Outline,
                    ItemContent {
                        ItemTitle { "Chart" }
                        ScrollArea { pre { "{chart_source(chart)}" } }
                        if let Some(caption) = caption {
                            ItemDescription { "{caption}" }
                        }
                    }
                }
            },
            Media::Mermaid { source, caption, .. } => rsx! {
                Item { variant: ItemVariant::Outline,
                    ItemContent {
                        ItemTitle { "Diagram" }
                        ScrollArea { pre { "{source}" } }
                        if let Some(caption) = caption {
                            ItemDescription { "{caption}" }
                        }
                    }
                }
            },
            Media::Html { html, caption, .. } => rsx! {
                Item { variant: ItemVariant::Outline,
                    ItemContent {
                        AspectRatio { ratio: 16.0 / 9.0, iframe { srcdoc: "{html}", "sandbox": "", title: "HTML content" } }
                        if let Some(caption) = caption {
                            ItemDescription { "{caption}" }
                        }
                    }
                }
            },
            Media::Unknown(_) => rsx! {
                Status { "Media type {media.kind_name()}: not supported in the portal yet" }
            },
        }
    }
}

/// The source of a chart, as the text the portal shows.
fn chart_source(chart: &portal_types::questions::ChartMedia) -> String {
    let data = serde_json::to_string_pretty(&chart.data).unwrap_or_else(|_| chart.data.to_string());
    match &chart.options {
        Some(options) => {
            let options =
                serde_json::to_string_pretty(options).unwrap_or_else(|_| options.to_string());
            format!("{data}\n{options}")
        }
        None => data,
    }
}

#[component]
fn ChoiceOption(
    group: String,
    id: String,
    index: usize,
    option: Value,
    recommended: bool,
    multi: bool,
    mut values: Values,
) -> Element {
    let label = option_label(&option);
    let content = option.get("content").map(content_text);
    let checked = chosen(&values.read(), &id).contains(&label);
    let pick = label.clone();
    let choice_id = id.clone();
    let note_label = label.clone();
    let note = selected_values(&values.read(), &id)
        .iter()
        .find(|answer| answer_label(answer) == Some(label.as_str()))
        .and_then(|answer| answer.get("note"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let other = label == "Other";
    let control = format!("{group}-{label}-control");
    let note_id = format!("{group}-{label}-note");

    rsx! {
        Item { size: ItemSize::Sm,
            if multi {
                ItemMedia {
                    Checkbox {
                        id: "{control}",
                        name: "{group}",
                        value: "{label}",
                        checked: Some(if checked { CheckboxState::Checked } else { CheckboxState::Unchecked }),
                        on_checked_change: move |_| choose(&mut values.write(), &choice_id, &pick, true),
                    }
                }
            }
            ItemContent {
                ItemTitle {
                    if multi {
                        Label { html_for: "{control}", "{label}" }
                    } else {
                        RadioItem { id: "{control}", value: label.clone(), index, "{label}" }
                    }
                    if recommended {
                        Badge { "Recommended" }
                    }
                }
                if let Some(content) = content {
                    ItemDescription { "{content}" }
                }
            }
            if checked {
                ItemFooter {
                    Label { html_for: "{note_id}", if other { "Your answer" } else { "More context (optional)" } }
                    Textarea {
                        id: "{note_id}",
                        name: "{group}-{label}-note",
                        rows: "2",
                        required: other,
                        value: "{note}",
                        placeholder: if other { "Write your own answer" } else { "Explain your choice or add another idea" },
                        oninput: move |event: FormEvent| set_note(&mut values.write(), &id, &note_label, event.value()),
                    }
                }
            }
        }
    }
}
