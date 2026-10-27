//! Plan reviews, their source-mapped Markdown, and the owner's draft feedback.

use dioxus::prelude::*;
use futures_util::StreamExt;
use gloo_net::http::{Request, RequestBuilder, Response};
use portal_types::reviews::{
    Annotation, AnnotationsBody, ApproveBody, ChangeKind, DiffView, GeneralBody, Review,
    ReviewList, ReviewSummary,
};
use pulldown_cmark::{Event, Parser, Tag, TagEnd, html};
use serde::{Deserialize, de::DeserializeOwned};

use crate::agent_view::Draft;

const REVIEW_CSS: Asset = asset!("/assets/review_view.css");

fn segment(value: &str) -> String {
    String::from(js_sys::encode_uri_component(value))
}

async fn response<T: DeserializeOwned>(response: Response) -> Result<T, String> {
    if !response.ok() {
        let status = response.status();
        let body = response.json::<serde_json::Value>().await.ok();
        return Err(body
            .as_ref()
            .and_then(|body| body["error"].as_str())
            .map_or_else(
                || format!("The review request failed ({status}). Try again."),
                str::to_owned,
            ));
    }
    response.json().await.map_err(|error| error.to_string())
}

async fn get<T: DeserializeOwned>(url: &str) -> Result<T, String> {
    response(
        Request::get(url)
            .send()
            .await
            .map_err(|error| error.to_string())?,
    )
    .await
}

async fn put<T: serde::Serialize>(id: &str, field: &str, body: &T) -> Result<Review, String> {
    send(
        Request::put(&format!("/api/reviews/{}/{field}", segment(id))),
        body,
    )
    .await
}

async fn send<T: serde::Serialize>(request: RequestBuilder, body: &T) -> Result<Review, String> {
    response(
        request
            .json(body)
            .map_err(|error| error.to_string())?
            .send()
            .await
            .map_err(|error| error.to_string())?,
    )
    .await
}

async fn pause() -> Result<(), String> {
    let mut timer = document::eval(
        "await new Promise(resolve => setTimeout(resolve, 2000)); dioxus.send(true);",
    );
    timer
        .recv::<bool>()
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[component]
pub fn ReviewView(#[props(default)] review_id: Option<String>) -> Element {
    let mut selected = use_context::<Signal<Option<String>>>();
    let mut list = use_signal(Vec::<ReviewSummary>::new);
    let mut opened = use_signal(|| None::<String>);
    let mut error = use_signal(|| None::<String>);
    let mut tick = use_signal(|| 0_u64);
    let mut loading = use_signal(|| true);
    use_future(move || async move {
        loop {
            match get::<ReviewList>("/api/reviews").await {
                Ok(value) => {
                    list.set(
                        value
                            .reviews
                            .into_iter()
                            .filter(|item| item.decision.is_none())
                            .collect(),
                    );
                    error.set(None);
                    tick += 1;
                }
                Err(message) => error.set(Some(message)),
            }
            loading.set(false);
            if let Err(message) = pause().await {
                error.set(Some(format!(
                    "Review refresh stopped: {message}. Reload the page."
                )));
                break;
            }
        }
    });
    let reviews = list();
    let current = review_id.clone().or_else(|| {
        reviews
            .iter()
            .find(|item| {
                Some(&item.pane) == selected().as_ref() && Some(&item.id) == opened().as_ref()
            })
            .or_else(|| {
                reviews
                    .iter()
                    .find(|item| Some(&item.pane) == selected().as_ref())
            })
            .map(|item| item.id.clone())
    });
    rsx! {
        document::Stylesheet { href: REVIEW_CSS }
        section { class: "rv", aria_label: "Plan reviews",
            if review_id.is_none() {
                h2 { "Plans · Needs you: {reviews.len()}" }
                nav { class: "rv-needs", aria_label: "Plans needing review",
                    for item in reviews {
                        button {
                            key: "{item.id}", r#type: "button",
                            class: if current.as_ref() == Some(&item.id) { "rv-item active" } else { "rv-item" },
                            onclick: move |_| { selected.set(Some(item.pane.clone())); opened.set(Some(item.id.clone())); },
                            "{item.path} · {item.pane}"
                        }
                    }
                }
            }
            if let Some(message) = error() { p { class: "error", role: "alert", "{message}" } }
            if let Some(id) = current {
                ReviewLoader {
                    key: "{id}", id, tick: tick(), standalone: review_id.is_some(),
                    on_done: move |id: String| list.write().retain(|item| item.id != id),
                }
            } else if loading() {
                p { role: "status", "Loading plans…" }
            } else {
                p { class: "rv-muted", "Select a plan above, or an agent with an open plan." }
            }
        }
    }
}

#[component]
fn ReviewLoader(id: String, tick: u64, standalone: bool, on_done: EventHandler<String>) -> Element {
    let mut review = use_signal(|| None::<Review>);
    let mut error = use_signal(|| None::<String>);
    use_resource(use_reactive((&id, &tick), move |(id, _)| async move {
        match get::<Review>(&format!("/api/reviews/{}", segment(&id))).await {
            Ok(value) => {
                review.set(Some(value));
                error.set(None);
            }
            Err(message) => error.set(Some(message)),
        }
    }));
    rsx! {
        if let Some(message) = error() { p { class: "error", role: "alert", "{message}" } }
        if let Some(value) = review() {
            if value.decision.is_some() {
                p { role: "status", "This review is closed." }
            } else {
                ReviewEditor { key: "{value.id}-{value.revisions.last().map_or(0, |r| r.n)}", initial: value, standalone, on_done }
            }
        } else { p { role: "status", "Loading plan…" } }
    }
}

/// A source range counts its last content line, not the next line after a newline.
fn lines(source: &str, range: std::ops::Range<usize>) -> (usize, usize) {
    let end = source[..range.end]
        .trim_end_matches(['\n', '\r'])
        .len()
        .max(range.start);
    (
        source[..range.start]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1,
        source[..end].bytes().filter(|byte| *byte == b'\n').count() + 1,
    )
}

fn block(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::BlockQuote(_)
            | Tag::CodeBlock(_)
            | Tag::List(_)
            | Tag::Item
            | Tag::HtmlBlock
    )
}

/// Raw HTML is text, and links cannot execute scripts or load active data URLs.
fn safe_url(url: &str) -> bool {
    let normalized: String = url
        .chars()
        .filter(|ch| !ch.is_control() && !ch.is_whitespace())
        .collect();
    let normalized = normalized.to_ascii_lowercase();
    !normalized.contains(':')
        || normalized.starts_with("https:")
        || normalized.starts_with("http:")
        || normalized.starts_with("mailto:")
}

fn render_plan(source: &str, annotations: &[Annotation]) -> String {
    let events = Parser::new(source)
        .into_offset_iter()
        .map(|(event, range)| match event {
            Event::Start(tag) if block(&tag) => {
                let (start, end) = lines(source, range);
                let marked = annotations
                    .iter()
                    .any(|item| item.start_line <= end && item.end_line >= start);
                let mut opening = String::new();
                if matches!(tag, Tag::HtmlBlock) {
                    opening.push_str("<pre>");
                } else {
                    html::push_html(&mut opening, std::iter::once(Event::Start(tag)));
                }
                if let Some(index) = opening.find('>') {
                    opening.insert_str(
                        index,
                        &format!(
                            " data-start-line=\"{start}\" data-end-line=\"{end}\"{}",
                            if marked {
                                " class=\"rv-annotated\""
                            } else {
                                ""
                            }
                        ),
                    );
                }
                Event::Html(opening.into())
            }
            Event::End(TagEnd::HtmlBlock) => Event::Html("</pre>".into()),
            Event::Rule => {
                let (start, end) = lines(source, range);
                Event::Html(
                    format!("<hr data-start-line=\"{start}\" data-end-line=\"{end}\" />").into(),
                )
            }
            Event::Html(text) | Event::InlineHtml(text) => Event::Text(text),
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => Event::Start(Tag::Link {
                link_type,
                dest_url: if safe_url(&dest_url) {
                    dest_url
                } else {
                    "#".into()
                },
                title,
                id,
            }),
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }) => Event::Start(Tag::Image {
                link_type,
                dest_url: if safe_url(&dest_url) {
                    dest_url
                } else {
                    "".into()
                },
                title,
                id,
            }),
            other => other,
        });
    let mut output = String::new();
    html::push_html(&mut output, events);
    output
}

fn quote_message(quote: &str, start: usize, end: usize) -> String {
    let quoted = quote
        .lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{quoted}\n(plan lines {start}–{end})")
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct Selection {
    quote: String,
    start: usize,
    end: usize,
}

const SELECTION: &str = r#"
const root = document.getElementById('review-plan');
const selection = window.getSelection();
let result = null;
if (selection && selection.rangeCount && !selection.isCollapsed) {
  const range = selection.getRangeAt(0);
  const element = node => (node.nodeType === Node.ELEMENT_NODE ? node : node.parentElement)?.closest('[data-start-line]');
  const first = element(range.startContainer), last = element(range.endContainer);
  if (first && last && root?.contains(first) && root.contains(last)) {
    result = {quote: selection.toString(), start: Number(first.dataset.startLine), end: Number(last.dataset.endLine)};
  }
}
dioxus.send(result);
"#;

enum ReviewAction {
    SaveAnnotation(Annotation),
    DeleteAnnotation(String),
    SaveGeneral(String),
    Decide {
        approve: bool,
        general: String,
        notes: String,
    },
}

#[component]
fn ReviewEditor(initial: Review, standalone: bool, on_done: EventHandler<String>) -> Element {
    let mut review = use_signal(|| initial);
    let mut general = use_signal(|| review.peek().general.clone());
    let mut notes = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut editing = use_signal(|| None::<Annotation>);
    let mut show_diff = use_signal(|| false);
    let mut diff = use_signal(|| None::<DiffView>);
    // One queue owns all writes. A blur save must finish before a decision,
    // and cannot replace newer annotations with an older response.
    let actions = use_coroutine(
        move |mut queue: UnboundedReceiver<ReviewAction>| async move {
            while let Some(action) = queue.next().await {
                let foreground = !matches!(action, ReviewAction::SaveGeneral(_));
                let closes_editor = matches!(action, ReviewAction::SaveAnnotation(_));
                error.set(None);
                let current = review();
                let result = async {
                    match action {
                        ReviewAction::SaveAnnotation(annotation) => {
                            let mut annotations = current.annotations;
                            if let Some(item) = annotations
                                .iter_mut()
                                .find(|item| !annotation.id.is_empty() && item.id == annotation.id)
                            {
                                *item = annotation;
                            } else {
                                annotations.push(annotation);
                            }
                            put(&current.id, "annotations", &AnnotationsBody { annotations }).await
                        }
                        ReviewAction::DeleteAnnotation(id) => {
                            let annotations = current
                                .annotations
                                .into_iter()
                                .filter(|item| item.id != id)
                                .collect();
                            put(&current.id, "annotations", &AnnotationsBody { annotations }).await
                        }
                        ReviewAction::SaveGeneral(general) => {
                            put(&current.id, "general", &GeneralBody { general }).await
                        }
                        ReviewAction::Decide {
                            approve,
                            general,
                            notes,
                        } => {
                            put(&current.id, "general", &GeneralBody { general }).await?;
                            let url = format!(
                                "/api/reviews/{}/{}",
                                segment(&current.id),
                                if approve {
                                    "approve"
                                } else {
                                    "request-changes"
                                }
                            );
                            if approve {
                                send(
                                    Request::post(&url),
                                    &ApproveBody {
                                        notes: if notes.trim().is_empty() {
                                            None
                                        } else {
                                            Some(notes)
                                        },
                                    },
                                )
                                .await
                            } else {
                                response(
                                    Request::post(&url)
                                        .send()
                                        .await
                                        .map_err(|error| error.to_string())?,
                                )
                                .await
                            }
                        }
                    }
                }
                .await;
                match result {
                    Ok(value) => {
                        if closes_editor {
                            editing.set(None);
                        }
                        if value.decision.is_some() {
                            on_done.call(value.id.clone());
                        }
                        review.set(value);
                    }
                    Err(message) => error.set(Some(message)),
                }
                if foreground {
                    busy.set(false);
                }
            }
        },
    );
    let value = review();
    let latest = value.revisions.last().cloned();
    let n = latest.as_ref().map_or(0, |revision| revision.n);
    let content = latest
        .as_ref()
        .map_or("", |revision| revision.content.as_str());
    let rendered = render_plan(content, &value.annotations);
    let select = move || async move {
        if busy() || editing().is_some() {
            return;
        }
        let mut script = document::eval(SELECTION);
        match script.recv::<Option<Selection>>().await {
            Ok(Some(selection)) if !selection.quote.trim().is_empty() => {
                editing.set(Some(Annotation {
                    id: String::new(),
                    revision: n,
                    start_line: selection.start,
                    end_line: selection.end,
                    quote: selection.quote,
                    comment: String::new(),
                    images: vec![],
                }))
            }
            Ok(_) => {}
            Err(message) => error.set(Some(format!("Could not read the selection: {message}"))),
        }
    };
    let decide = move |approve: bool| {
        if busy() {
            return;
        }
        busy.set(true);
        actions.send(ReviewAction::Decide {
            approve,
            general: general(),
            notes: notes(),
        });
    };
    let mut approve = decide;
    let mut request = decide;
    rsx! {
        article { class: "rv-review",
            header { class: "rv-header",
                h3 { "{value.path}" }
                p { class: "rv-muted", "{value.pane} · Revision {n}" }
            }
            if let Some(message) = error() { p { class: "error", role: "alert", "{message}" } }
            if value.decision.is_some() {
                p { role: "status", "Review sent to the agent." }
            } else {
                p { class: "rv-muted", "Select plan text to annotate it or quote it in a message." }
                div { id: "review-plan", class: "rv-prose", tabindex: "0", aria_label: "Plan text",
                    onmouseup: move |_| select(), onkeyup: move |_| select(), dangerous_inner_html: rendered,
                }
                button { r#type: "button", onclick: move |_| select(), "Annotate selection" }
                if let Some(annotation) = editing() {
                    AnnotationEditor {
                        key: "{annotation.id}-{annotation.start_line}-{annotation.end_line}", annotation, standalone,
                        busy: busy(),
                        on_close: move |_| editing.set(None),
                        on_save: move |annotation: Annotation| {
                            if busy() { return; }
                            busy.set(true);
                            actions.send(ReviewAction::SaveAnnotation(annotation));
                        },
                    }
                }
                if value.revisions.len() > 1 {
                    button {
                        r#type: "button", aria_expanded: show_diff(), disabled: busy(),
                        onclick: move |_| async move {
                            show_diff.toggle();
                            if show_diff() && diff().is_none() {
                                busy.set(true);
                                let id = review.peek().id.clone();
                                match get::<DiffView>(&format!("/api/reviews/{}/diff?from={}&to={n}", segment(&id), n - 1)).await {
                                    Ok(value) => diff.set(Some(value)),
                                    Err(message) => error.set(Some(message)),
                                }
                                busy.set(false);
                            }
                        },
                        "Changes since revision {n - 1}"
                    }
                    if show_diff() {
                        if let Some(diff) = diff() {
                            pre { class: "rv-diff", tabindex: "0", aria_label: "Revision changes",
                                for (index, line) in diff.changes.iter().enumerate() {
                                    div { key: "{index}", class: match line.kind { ChangeKind::Added => "added", ChangeKind::Removed => "removed", ChangeKind::Same => "same" },
                                        {format!("{} {} {} {}", match line.kind { ChangeKind::Added => "+", ChangeKind::Removed => "−", ChangeKind::Same => " " }, line.old.map_or(String::new(), |n| n.to_string()), line.new.map_or(String::new(), |n| n.to_string()), line.text)}
                                    }
                                }
                            }
                        }
                    }
                }
                section { class: "rv-annotations", aria_label: "Annotations",
                    h3 { "Annotations ({value.annotations.len()})" }
                    for annotation in value.annotations.iter().cloned() {
                        div { class: "rv-annotation", key: "{annotation.id}",
                            p { class: "rv-muted", "Lines {annotation.start_line}–{annotation.end_line}" }
                            blockquote { "{annotation.quote}" }
                            p { "{annotation.comment}" }
                            Images { paths: annotation.images.clone() }
                            div { class: "rv-actions",
                                button { r#type: "button", disabled: busy() || editing().is_some(), onclick: {
                                    let annotation = annotation.clone(); move |_| editing.set(Some(annotation.clone()))
                                }, "Edit" }
                                button { r#type: "button", disabled: busy() || editing().is_some(), onclick: move |_| {
                                    if busy() { return; }
                                    busy.set(true);
                                    actions.send(ReviewAction::DeleteAnnotation(annotation.id.clone()));
                                }, "Delete" }
                            }
                        }
                    }
                }
                label { r#for: "review-general", "General comment" }
                textarea { id: "review-general", name: "general", rows: "3", value: "{general}", disabled: busy(),
                    oninput: move |event: FormEvent| general.set(event.value()),
                    onblur: move |_| {
                        if general() != review.peek().general {
                            actions.send(ReviewAction::SaveGeneral(general()));
                        }
                    },
                }
                label { r#for: "review-notes", "Approval notes (optional)" }
                textarea { id: "review-notes", name: "notes", rows: "2", value: "{notes}", disabled: busy(),
                    oninput: move |event: FormEvent| notes.set(event.value()),
                }
                div { class: "rv-actions",
                    button { r#type: "button", class: "rv-primary", disabled: busy() || editing().is_some(), onclick: move |_| approve(true), "Approve" }
                    button { r#type: "button", disabled: busy() || editing().is_some() || (value.annotations.is_empty() && general().trim().is_empty()), onclick: move |_| request(false), "Request changes" }
                }
                if busy() { p { role: "status", "Saving…" } }
            }
        }
    }
}

fn image_url(path: &str) -> String {
    format!(
        "/api/attachments/{}",
        segment(path.rsplit('/').next().unwrap_or_default())
    )
}

#[component]
fn Images(paths: Vec<String>) -> Element {
    rsx! { div { class: "rv-images",
        for (index, path) in paths.iter().enumerate() {
            a { key: "{path}", href: image_url(path), target: "_blank", rel: "noopener noreferrer",
                img { src: image_url(path), alt: "Annotation image {index + 1}" }
            }
        }
    } }
}

#[component]
fn AnnotationEditor(
    annotation: Annotation,
    standalone: bool,
    busy: bool,
    on_save: EventHandler<Annotation>,
    on_close: EventHandler<()>,
) -> Element {
    let mut draft = use_context::<Draft>().0;
    let mut comment = use_signal(|| annotation.comment.clone());
    let mut images = use_signal(|| annotation.images.clone());
    let mut uploading = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut mounted = use_signal(|| false);
    // The DOM component owns the listeners; AbortController removes them on unmount.
    use_resource(move || async move {
        if !mounted() {
            return;
        }
        let mut script = document::eval(UPLOADS);
        loop {
            match script.recv::<UploadEvent>().await {
                Ok(event) => {
                    uploading.set(event.busy);
                    if let Some(path) = event.path {
                        images.write().push(path);
                    }
                    if let Some(message) = event.error {
                        error.set(Some(message));
                    }
                }
                Err(message) => {
                    error.set(Some(format!("Image upload stopped: {message}")));
                    break;
                }
            }
        }
    });
    let quote = annotation.quote.clone();
    let save = annotation.clone();
    rsx! {
        section { id: "review-annotation", class: "rv-popover", role: "dialog", aria_label: "Annotate plan selection",
            onmounted: move |_| { mounted.set(true); },
            onkeydown: move |event: KeyboardEvent| { if event.key() == Key::Escape && !busy && !uploading() { on_close.call(()); } },
            h3 { "Lines {annotation.start_line}–{annotation.end_line}" }
            blockquote { "{annotation.quote}" }
            label { r#for: "annotation-comment", "Comment" }
            textarea { id: "annotation-comment", name: "comment", rows: "3", autofocus: true, value: "{comment}", disabled: busy,
                oninput: move |event: FormEvent| comment.set(event.value()),
            }
            label { r#for: "annotation-images", "Images (PNG, JPEG, WebP; up to 10 MiB each)" }
            input { id: "annotation-images", name: "images", r#type: "file", accept: "image/png,image/jpeg,image/webp", multiple: true, disabled: busy || uploading() }
            p { class: "rv-muted", "You can also paste or drop images here." }
            Images { paths: images() }
            for (index, path) in images().iter().enumerate() {
                button { key: "{path}-{index}", r#type: "button", disabled: busy || uploading(), onclick: move |_| { images.write().remove(index); }, "Remove image {index + 1}" }
            }
            if let Some(message) = error() { p { class: "error", role: "alert", "{message}" } }
            if uploading() { p { role: "status", "Uploading images…" } }
            div { class: "rv-actions",
                button { r#type: "button", class: "rv-primary", disabled: busy || uploading(), onclick: move |_| {
                    let mut value = save.clone(); value.comment = comment(); value.images = images(); on_save.call(value);
                }, "Save" }
                button { r#type: "button", disabled: standalone || busy || uploading(), title: if standalone { "Open the agent conversation to quote a passage" } else { "Append quote to the message draft" }, onclick: move |_| {
                    let message = quote_message(&quote, annotation.start_line, annotation.end_line);
                    let mut text = draft();
                    if !text.is_empty() { text.push_str("\n\n"); }
                    text.push_str(&message); draft.set(text); on_close.call(());
                }, "Quote in message" }
                button { r#type: "button", disabled: busy || uploading(), onclick: move |_| on_close.call(()), "Close" }
            }
        }
    }
}

#[derive(Deserialize)]
struct UploadEvent {
    busy: bool,
    path: Option<String>,
    error: Option<String>,
}

const UPLOADS: &str = r#"
const root = document.getElementById('review-annotation');
const controller = new AbortController();
const signal = controller.signal;
let running = false;
const send = (busy, path = null, error = null) => dioxus.send({busy, path, error});
const upload = async files => {
  if (running || !root.isConnected) return;
  running = true;
  send(true);
  try {
    for (const file of files) {
      if (!['image/png', 'image/jpeg', 'image/webp'].includes(file.type)) throw new Error('Use PNG, JPEG or WebP images.');
      if (file.size > 10 * 1024 * 1024) throw new Error('Each image must be at most 10 MiB.');
      const response = await fetch('/api/attachments', {method: 'POST', body: file, signal});
      const body = await response.json();
      if (!response.ok) throw new Error(body.error || `Image upload failed (${response.status}).`);
      send(true, body.path);
    }
  } catch (error) { if (!signal.aborted) send(true, null, error.message); }
  finally { running = false; if (!signal.aborted) send(false); }
};
const observer = new MutationObserver(() => {
  if (!root.isConnected) { controller.abort(); observer.disconnect(); }
});
observer.observe(document.body, {childList: true, subtree: true});
root.addEventListener('paste', event => {
  const files = Array.from(event.clipboardData?.files || []);
  if (files.length) { event.preventDefault(); upload(files); }
}, {signal});
root.addEventListener('dragover', event => { event.preventDefault(); }, {signal});
root.addEventListener('drop', event => {
  event.preventDefault(); upload(Array.from(event.dataTransfer?.files || []));
}, {signal});
root.querySelector('input[type=file]').addEventListener('change', event => {
  upload(Array.from(event.target.files || [])); event.target.value = '';
}, {signal});
// Keep the eval channel open for event callbacks until this editor unmounts.
await new Promise(resolve => signal.addEventListener('abort', resolve, {once: true}));
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_list_and_code_have_source_line_ranges() {
        let plan = "# Plan\n\n- first\n- second\n\n```rust\nlet x = 1;\n```\n";
        let rendered = render_plan(plan, &[]);
        assert!(rendered.contains("<h1 data-start-line=\"1\" data-end-line=\"1\">"));
        assert!(rendered.contains("<ul data-start-line=\"3\" data-end-line=\"4\">"));
        assert!(rendered.contains("<li data-start-line=\"4\" data-end-line=\"4\">"));
        assert!(rendered.contains("<pre data-start-line=\"6\" data-end-line=\"8\">"));
        assert!(rendered.contains("<code class=\"language-rust\">"));
    }

    #[test]
    fn quote_in_message_preserves_lines() {
        assert_eq!(
            quote_message("First\n\nLast", 2, 4),
            "> First\n> \n> Last\n(plan lines 2–4)"
        );
    }

    #[test]
    fn markdown_does_not_execute_agent_html_or_script_links() {
        let rendered = render_plan(
            "<script>alert(1)</script>\n\n[go](javascript:alert%281%29)",
            &[],
        );
        assert!(!rendered.contains("<script>"));
        assert!(!rendered.contains("href=\"javascript:"));
        assert!(rendered.contains("&lt;script&gt;"));
        assert!(!safe_url("jav\nascript:alert(1)"));
    }

    #[test]
    fn annotations_highlight_overlapping_blocks() {
        let annotation = Annotation {
            id: "a".into(),
            revision: 1,
            start_line: 3,
            end_line: 3,
            quote: "two".into(),
            comment: "change".into(),
            images: vec![],
        };
        let rendered = render_plan("# One\n\ntwo\n", &[annotation]);
        assert!(
            rendered
                .contains("<p data-start-line=\"3\" data-end-line=\"3\" class=\"rv-annotated\">")
        );
        assert!(!rendered.contains("<h1 data-start-line=\"1\" data-end-line=\"1\" class="));
    }
}
