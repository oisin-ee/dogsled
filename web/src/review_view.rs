//! Plan reviews, their source-mapped Markdown, and the owner's draft feedback.

use std::collections::BTreeMap;
use std::ops::Range;
use std::time::Duration;

use dioxus::html::FileData;
use dioxus::prelude::*;
use dioxus::web::WebEventExt;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_sdk_time::sleep;
use futures_util::StreamExt;
use gloo_net::http::{Request, RequestBuilder, Response};
use js_sys::wasm_bindgen::JsCast;
use portal_types::reviews::{
    Annotation, AnnotationsBody, ApproveBody, Attachment, ChangeKind, DiffView, GeneralBody,
    Review, ReviewList, ReviewSummary,
};
use pulldown_cmark::{CowStr, Event, Parser, Tag, TagEnd, html};
use serde::de::DeserializeOwned;

use crate::agent_view::Draft;
use crate::components::{
    badge::{Badge, BadgeVariant},
    button::{Button, ButtonVariant},
    card::{Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle},
    collapsible::{Collapsible, CollapsibleContent, CollapsibleTrigger},
    dialog::{Dialog, DialogDescription, DialogTitle},
    input::Input,
    item::{
        Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemHeader, ItemMedia,
        ItemMediaVariant, ItemSize, ItemTitle, ItemVariant,
    },
    label::Label,
    scroll_area::ScrollArea,
    skeleton::Skeleton,
    textarea::Textarea,
};
use crate::notice::{Alert, Status};

/// The time between two reads of the open plan reviews.
const POLL: Duration = Duration::from_secs(2);

/// The largest image the server stores.
const MAX_IMAGE_BYTES: f64 = 10.0 * 1024.0 * 1024.0;

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

#[component]
pub fn ReviewView(
    #[props(default)] review_id: Option<String>,
    #[props(default)] embedded: bool,
) -> Element {
    let local_selected = use_signal(|| None::<String>);
    let mut selected = try_use_context::<Signal<Option<String>>>().unwrap_or(local_selected);
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
            sleep(POLL).await;
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
        if !embedded && review_id.is_none() {
            Item { ItemContent { ItemTitle { "Plans · Needs you: {reviews.len()}" } } }
            ItemGroup { aria_label: "Plans needing review",
                for item in reviews {
                    Button { variant: ButtonVariant::Outline,
                        key: "{item.id}", r#type: "button",
                        aria_current: (current.as_ref() == Some(&item.id)).then_some("true"),
                        onclick: {
                            let pane = item.pane.clone();
                            let id = item.id.clone();
                            move |_| { selected.set(Some(pane.clone())); opened.set(Some(id.clone())); }
                        },
                        "{item.path} · {item.pane}"
                    }
                }
            }
        }
        if let Some(message) = error() { Alert { "{message}" } }
        if let Some(id) = current {
            ReviewLoader {
                key: "{id}", id, tick: tick(), embedded,
                on_done: move |id: String| list.write().retain(|item| item.id != id),
            }
        } else if loading() {
            Skeleton { role: "status", aria_label: "Loading plans" }
        } else {
            Status { "Select a plan above, or an agent with an open plan." }
        }
    }
}

#[component]
fn ReviewLoader(id: String, tick: u64, embedded: bool, on_done: EventHandler<String>) -> Element {
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
        if let Some(message) = error() { Alert { "{message}" } }
        if let Some(value) = review() {
            if value.decision.is_some() {
                Status { "This review is closed." }
            } else {
                ReviewEditor { key: "{value.id}-{value.revisions.last().map_or(0, |r| r.n)}", initial: value, embedded, on_done }
            }
        } else { Status { "Loading plan…" } }
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

/// Markdown as safe HTML: raw HTML stays text and unsafe links are dropped.
pub(crate) fn render_markdown(source: &str) -> String {
    render_plan(source, &[])
}

fn render_plan(source: &str, annotations: &[Annotation]) -> String {
    let parsed: Vec<_> = Parser::new(source).into_offset_iter().collect();
    let mut marks = quoted_ranges(source, &parsed, annotations);
    let events = parsed
        .into_iter()
        .enumerate()
        .flat_map(|(index, (event, range))| {
            let ranges = marks.remove(&index).unwrap_or_default();
            match event {
                Event::Start(tag) if block(&tag) => {
                    let (start, end) = lines(source, range);
                    let mut opening = String::new();
                    if matches!(tag, Tag::HtmlBlock) {
                        opening.push_str("<pre>");
                    } else {
                        html::push_html(&mut opening, std::iter::once(Event::Start(tag)));
                    }
                    if let Some(position) = opening.find('>') {
                        opening.insert_str(
                            position,
                            &format!(" data-start-line=\"{start}\" data-end-line=\"{end}\""),
                        );
                    }
                    vec![Event::Html(opening.into())]
                }
                Event::End(TagEnd::HtmlBlock) => vec![Event::Html("</pre>".into())],
                Event::Rule => {
                    let (start, end) = lines(source, range);
                    vec![Event::Html(
                        format!("<hr data-start-line=\"{start}\" data-end-line=\"{end}\" />")
                            .into(),
                    )]
                }
                Event::Html(text) | Event::InlineHtml(text) | Event::Text(text) => {
                    marked(text, &ranges)
                }
                Event::Code(text) if !ranges.is_empty() => {
                    let mut pieces = vec![Event::Html("<code>".into())];
                    pieces.extend(marked(text, &ranges));
                    pieces.push(Event::Html("</code>".into()));
                    pieces
                }
                Event::Start(Tag::Link {
                    link_type,
                    dest_url,
                    title,
                    id,
                }) => vec![Event::Start(Tag::Link {
                    link_type,
                    dest_url: if safe_url(&dest_url) {
                        dest_url
                    } else {
                        "#".into()
                    },
                    title,
                    id,
                })],
                Event::Start(Tag::Image {
                    link_type,
                    dest_url,
                    title,
                    id,
                }) => vec![Event::Start(Tag::Image {
                    link_type,
                    dest_url: if safe_url(&dest_url) {
                        dest_url
                    } else {
                        "".into()
                    },
                    title,
                    id,
                })],
                other => vec![other],
            }
        });
    let mut output = String::new();
    html::push_html(&mut output, events);
    output
}

/// A run of rendered text that a note's quote can match.
struct Run<'a> {
    /// The event that holds the text, or none for a line break.
    event: Option<usize>,
    /// The event that opened the enclosing block.
    block: usize,
    /// The source lines of that block.
    lines: (usize, usize),
    text: &'a str,
}

/// Whether a block closes with this tag.
fn block_end(tag: &TagEnd) -> bool {
    matches!(
        tag,
        TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::BlockQuote(_)
            | TagEnd::CodeBlock
            | TagEnd::List(_)
            | TagEnd::Item
            | TagEnd::HtmlBlock
    )
}

/// The byte ranges of each text event that a note's quote covers. The quote
/// matches across inline nodes and across runs of white space, and only in
/// the blocks that touch the note's lines.
fn quoted_ranges(
    source: &str,
    parsed: &[(Event<'_>, Range<usize>)],
    annotations: &[Annotation],
) -> BTreeMap<usize, Vec<Range<usize>>> {
    let mut runs = Vec::new();
    let mut blocks: Vec<(usize, (usize, usize))> = Vec::new();
    let mut images = 0_usize;
    for (index, (event, range)) in parsed.iter().enumerate() {
        match event {
            Event::Start(tag) => {
                if block(tag) {
                    blocks.push((index, lines(source, range.clone())));
                }
                if matches!(tag, Tag::Image { .. }) {
                    images += 1;
                }
            }
            Event::End(tag) => {
                if block_end(tag) {
                    blocks.pop();
                }
                if matches!(tag, TagEnd::Image) {
                    images = images.saturating_sub(1);
                }
            }
            // The text of an image becomes its alt attribute and cannot hold markup.
            Event::Text(text) | Event::Code(text) | Event::Html(text) | Event::InlineHtml(text)
                if images == 0 =>
            {
                if let Some(&(block, lines)) = blocks.last() {
                    runs.push(Run {
                        event: Some(index),
                        block,
                        lines,
                        text,
                    });
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(&(block, lines)) = blocks.last() {
                    runs.push(Run {
                        event: None,
                        block,
                        lines,
                        text: " ",
                    });
                }
            }
            _ => {}
        }
    }
    let mut found: BTreeMap<usize, Vec<Range<usize>>> = BTreeMap::new();
    for note in annotations {
        let quote = note.quote.split_whitespace().collect::<Vec<_>>().join(" ");
        if quote.is_empty() {
            continue;
        }
        let mut chars: Vec<(char, Option<(usize, usize)>)> = Vec::new();
        let mut previous = None;
        for run in &runs {
            if run.lines.1 < note.start_line || run.lines.0 > note.end_line {
                continue;
            }
            if previous.is_some_and(|block| block != run.block) {
                chars.push((' ', None));
            }
            previous = Some(run.block);
            for (offset, ch) in run.text.char_indices() {
                let ch = if ch.is_whitespace() { ' ' } else { ch };
                if ch == ' ' && chars.last().is_some_and(|(last, _)| *last == ' ') {
                    continue;
                }
                chars.push((ch, run.event.map(|event| (event, offset))));
            }
        }
        let text: String = chars.iter().map(|(ch, _)| *ch).collect();
        let Some(at) = text.find(&quote) else {
            continue;
        };
        let first = text[..at].chars().count();
        let covered = &chars[first..first + quote.chars().count()];
        let mut spans: BTreeMap<usize, Range<usize>> = BTreeMap::new();
        for (ch, position) in covered {
            if let Some((event, offset)) = *position {
                let end = offset + ch.len_utf8();
                spans
                    .entry(event)
                    .and_modify(|span| span.end = end)
                    .or_insert(offset..end);
            }
        }
        for (event, span) in spans {
            found.entry(event).or_default().push(span);
        }
    }
    for ranges in found.values_mut() {
        ranges.sort_by_key(|range| range.start);
        let mut merged: Vec<Range<usize>> = Vec::new();
        for range in ranges.drain(..) {
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        *ranges = merged;
    }
    found
}

/// The text as events, with each of the ranges wrapped in a mark.
fn marked<'a>(text: CowStr<'a>, ranges: &[Range<usize>]) -> Vec<Event<'a>> {
    let owned = |piece: &str| Event::Text(CowStr::from(piece.to_owned()));
    if ranges.is_empty() {
        return vec![Event::Text(text)];
    }
    let mut pieces = Vec::new();
    let mut done = 0;
    for range in ranges {
        if range.start > done {
            pieces.push(owned(&text[done..range.start]));
        }
        pieces.push(Event::Html("<mark>".into()));
        pieces.push(owned(&text[range.clone()]));
        pieces.push(Event::Html("</mark>".into()));
        done = range.end;
    }
    if done < text.len() {
        pieces.push(owned(&text[done..]));
    }
    pieces
}

fn quote_message(quote: &str, start: usize, end: usize) -> String {
    let quoted = quote
        .lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{quoted}\n(plan lines {start}–{end})")
}

/// What the owner selected in the plan.
#[derive(Clone, Debug, PartialEq)]
struct Selection {
    quote: String,
    start: usize,
    end: usize,
}

/// The owner's selection, when it starts and ends inside the rendered plan.
fn read_selection() -> Option<Selection> {
    let selection = web_sys::window()?.get_selection().ok()??;
    if selection.is_collapsed() || selection.range_count() == 0 {
        return None;
    }
    let range = selection.get_range_at(0).ok()?;
    let first = plan_block(&range.start_container().ok()?)?;
    let last = plan_block(&range.end_container().ok()?)?;
    Some(Selection {
        quote: String::from(selection.to_string()),
        start: first.get_attribute("data-start-line")?.parse().ok()?,
        end: last.get_attribute("data-end-line")?.parse().ok()?,
    })
}

/// The source-mapped block of the plan that holds a node.
fn plan_block(node: &web_sys::Node) -> Option<web_sys::Element> {
    node.dyn_ref::<web_sys::Element>()
        .cloned()
        .or_else(|| node.parent_element())?
        .closest("#review-plan [data-start-line]")
        .ok()?
}

/// Both decision bars use the editor's serialized write queue.
#[derive(Clone, Copy, PartialEq)]
struct ReviewControls {
    busy: Signal<bool>,
    editing: Signal<Option<Annotation>>,
    can_request: Memo<bool>,
    decide: EventHandler<bool>,
}

#[component]
fn ReviewDecisionActions(controls: ReviewControls) -> Element {
    rsx! {
        ItemActions {
            Button { variant: ButtonVariant::Outline,
                r#type: "button", disabled: (controls.busy)() || (controls.editing)().is_some() || !(controls.can_request)(),
                onclick: move |_| controls.decide.call(false), "Request changes"
            }
            Button {
                r#type: "button", disabled: (controls.busy)() || (controls.editing)().is_some(),
                onclick: move |_| controls.decide.call(true), "Approve plan"
            }
        }
    }
}

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
fn ReviewEditor(initial: Review, embedded: bool, on_done: EventHandler<String>) -> Element {
    let mut review = use_signal(|| initial);
    let mut general = use_signal(|| review.peek().general.clone());
    let mut notes = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let uploading = use_signal(|| false);
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
    // The passage being annotated stays marked while its note is open.
    let mut marked = value.annotations.clone();
    if let Some(annotation) = editing().filter(|annotation| annotation.id.is_empty()) {
        marked.push(annotation);
    }
    let rendered = render_plan(content, &marked);
    let mut select = move || {
        if busy() || editing().is_some() {
            return;
        }
        if let Some(selection) =
            read_selection().filter(|selection| !selection.quote.trim().is_empty())
        {
            editing.set(Some(Annotation {
                id: String::new(),
                revision: n,
                start_line: selection.start,
                end_line: selection.end,
                quote: selection.quote,
                comment: String::new(),
                images: vec![],
            }));
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
    let can_request =
        use_memo(move || !review().annotations.is_empty() || !general().trim().is_empty());
    let decide = use_hook(|| EventHandler::new(decide));
    let controls = ReviewControls {
        busy,
        editing,
        can_request,
        decide,
    };
    rsx! {
        if let Some(message) = error() { Alert { "{message}" } }
        if value.decision.is_some() {
            Status { "Review sent to the agent." }
        } else {
            if embedded {
                Item {
                    ItemContent {
                        ItemTitle { "{value.path}" Badge { variant: BadgeVariant::Outline, "Revision {n}" } }
                    }
                    ReviewDecisionActions { controls }
                }
            }
            Card { aria_label: "Plan",
                if !embedded {
                    CardHeader {
                        CardTitle { "{value.path}" }
                        CardDescription { "Revision {n}" }
                    }
                }
                CardContent {
                    // innerHTML needs a host element of its own, with no other children.
                    div {
                        id: "review-plan",
                        tabindex: "0",
                        aria_label: "Plan text",
                        onmouseup: move |_| select(),
                        onkeyup: move |_| select(),
                        dangerous_inner_html: rendered,
                    }
                }
                if value.revisions.len() > 1 {
                    CardFooter {
                        Button { variant: ButtonVariant::Outline,
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
                    }
                    if show_diff() {
                        if let Some(diff) = diff() {
                            CardContent {
                                ScrollArea { aria_label: "Revision changes",
                                    pre {
                                        {diff.changes.iter().map(|line| format!(
                                            "{} {} {} {}",
                                            match line.kind { ChangeKind::Added => "+", ChangeKind::Removed => "−", ChangeKind::Same => " " },
                                            line.old.map_or(String::new(), |n| n.to_string()),
                                            line.new.map_or(String::new(), |n| n.to_string()),
                                            line.text,
                                        )).collect::<Vec<_>>().join("\n")}
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Card { aria_label: "Your notes",
                CardHeader {
                    CardTitle { "Your notes" Badge { variant: BadgeVariant::Secondary, "{value.annotations.len()}" } }
                    CardDescription { "Drafts. The lead gets them when you request changes." }
                }
                CardContent {
                    ItemGroup {
                        for annotation in value.annotations.iter().cloned() {
                            Item { key: "{annotation.id}", variant: ItemVariant::Outline,
                                ItemHeader {
                                    Badge { variant: BadgeVariant::Outline, "Line {annotation.start_line}" }
                                    if annotation.end_line != annotation.start_line {
                                        Badge { variant: BadgeVariant::Outline, "Through line {annotation.end_line}" }
                                    }
                                }
                                ItemContent {
                                    ItemDescription { "{annotation.quote}" }
                                    ItemTitle { "{annotation.comment}" }
                                    Images { paths: annotation.images.clone() }
                                }
                                ItemActions {
                                    Button { variant: ButtonVariant::Outline, r#type: "button", disabled: busy() || editing().is_some(), onclick: {
                                        let annotation = annotation.clone(); move |_| { editing.set(Some(annotation.clone())); }
                                    }, "Edit" }
                                    Button { variant: ButtonVariant::Outline, r#type: "button", disabled: busy() || editing().is_some(), onclick: move |_| {
                                        if busy() { return; }
                                        busy.set(true);
                                        actions.send(ReviewAction::DeleteAnnotation(annotation.id.clone()));
                                    }, "Delete" }
                                }
                            }
                        }
                    }
                    Label { html_for: "review-general", "Note for the whole plan" }
                    Textarea { id: "review-general", name: "general", rows: "3", value: "{general}", disabled: busy(),
                        oninput: move |event: FormEvent| general.set(event.value()),
                        onblur: move |_| {
                            if general() != review.peek().general {
                                actions.send(ReviewAction::SaveGeneral(general()));
                            }
                        },
                    }
                }
            }
            Dialog {
                open: editing().is_some(),
                aria_label: "Annotate plan selection",
                on_open_change: move |open: bool| {
                    if !open && !busy() && !uploading() { editing.set(None); }
                },
                if let Some(annotation) = editing() {
                    AnnotationEditor {
                        key: "{annotation.id}-{annotation.start_line}-{annotation.end_line}", annotation,
                        busy: busy(), uploading,
                        on_close: move |_| editing.set(None),
                        on_save: move |annotation: Annotation| {
                            if busy() { return; }
                            busy.set(true);
                            actions.send(ReviewAction::SaveAnnotation(annotation));
                        },
                    }
                }
            }
            if !embedded {
                Item {
                    ItemContent {
                        Collapsible {
                            CollapsibleTrigger { "Approval notes (optional)" }
                            CollapsibleContent {
                                Textarea { id: "review-notes", aria_label: "Approval notes", name: "notes", rows: "2", value: "{notes}", disabled: busy(),
                                    oninput: move |event: FormEvent| notes.set(event.value()),
                                }
                            }
                        }
                    }
                    ReviewDecisionActions { controls }
                }
            }
            if busy() { Status { "Saving…" } }
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
    rsx! {
        for (index, path) in paths.iter().enumerate() {
            Item {
                key: "{path}",
                size: ItemSize::Sm,
                r#as: {
                    let url = image_url(path);
                    move |attributes: Vec<Attribute>| rsx! {
                        a { href: "{url}", target: "_blank", rel: "noopener noreferrer", ..attributes,
                            ItemMedia { variant: ItemMediaVariant::Image,
                                img { src: "{url}", alt: "Annotation image {index + 1}" }
                            }
                        }
                    }
                },
            }
        }
    }
}

/// The files that an input or a drop carries.
fn dropped_files(files: Vec<FileData>) -> Vec<web_sys::File> {
    files
        .iter()
        .filter_map(|file| file.inner().downcast_ref::<web_sys::File>().cloned())
        .collect()
}

/// The files that a paste carries.
fn pasted_files(event: &ClipboardEvent) -> Vec<web_sys::File> {
    let list = event
        .data()
        .try_as_web_event()
        .and_then(|event| event.dyn_into::<web_sys::ClipboardEvent>().ok())
        .and_then(|event| event.clipboard_data())
        .and_then(|data| data.files());
    list.map_or_else(Vec::new, |list| {
        (0..list.length())
            .filter_map(|index| list.item(index))
            .collect()
    })
}

/// Store one image and give the path the server chose.
async fn upload_image(file: &web_sys::File) -> Result<String, String> {
    if !matches!(
        file.type_().as_str(),
        "image/png" | "image/jpeg" | "image/webp"
    ) {
        return Err("Use PNG, JPEG or WebP images.".to_owned());
    }
    if file.size() > MAX_IMAGE_BYTES {
        return Err("Each image must be at most 10 MiB.".to_owned());
    }
    let request = Request::post("/api/attachments")
        .body(file.clone())
        .map_err(|error| error.to_string())?;
    let attachment: Attachment =
        response(request.send().await.map_err(|error| error.to_string())?).await?;
    Ok(attachment.path)
}

#[component]
fn AnnotationEditor(
    annotation: Annotation,
    busy: bool,
    mut uploading: Signal<bool>,
    on_save: EventHandler<Annotation>,
    on_close: EventHandler<()>,
) -> Element {
    let draft = try_use_context::<Draft>().map(|draft| draft.0);
    let mut comment = use_signal(|| annotation.comment.clone());
    let mut images = use_signal(|| annotation.images.clone());
    let mut error = use_signal(|| None::<String>);
    // A new input replaces the old one after each upload, so the same file can be chosen again.
    let mut uploads = use_signal(|| 0_u32);
    // Leaving the editor stops its upload; the page must not stay locked.
    use_drop(move || uploading.set(false));
    let upload = use_callback(move |files: Vec<web_sys::File>| {
        if uploading() || files.is_empty() {
            return;
        }
        uploading.set(true);
        error.set(None);
        spawn(async move {
            for file in files {
                match upload_image(&file).await {
                    Ok(path) => images.write().push(path),
                    Err(message) => {
                        error.set(Some(message));
                        break;
                    }
                }
            }
            uploads += 1;
            uploading.set(false);
        });
    });
    let drop_zone = attributes!(div {
        role: "group",
        onpaste: move |event: ClipboardEvent| {
            let files = pasted_files(&event);
            if !files.is_empty() {
                event.prevent_default();
                upload.call(files);
            }
        },
        ondragover: move |event: DragEvent| event.prevent_default(),
        ondrop: move |event: DragEvent| {
            event.prevent_default();
            upload.call(dropped_files(event.data_transfer().files()));
        },
    });
    let quote = annotation.quote.clone();
    let save = annotation.clone();
    rsx! {
        ItemGroup { attributes: drop_zone,
            Item {
                ItemContent { DialogTitle { "Line {annotation.start_line}" } }
                ItemActions {
                    Button { variant: ButtonVariant::Outline, r#type: "button", aria_label: "Close note", disabled: busy || uploading(), onclick: move |_| on_close.call(()), "×" }
                }
            }
            Item { variant: ItemVariant::Muted,
                ItemContent { ItemDescription { "{annotation.quote}" } }
            }
            Textarea { id: "annotation-comment", name: "comment", aria_label: "Note for the lead", placeholder: "Add a note for the lead", rows: "3", "autofocus": true, value: "{comment}", disabled: busy,
                oninput: move |event: FormEvent| comment.set(event.value()),
            }
            Label { html_for: "annotation-images", "Add images (PNG, JPEG, WebP; up to 10 MiB each)" }
            for round in [uploads()] {
                Input { key: "{round}", id: "annotation-images", name: "images", r#type: "file", accept: "image/png,image/jpeg,image/webp", multiple: true, disabled: busy || uploading(),
                    onchange: move |event: FormEvent| upload.call(dropped_files(event.files())),
                }
            }
            DialogDescription { "Paste, drop or add images." }
            Images { paths: images() }
            for (index, path) in images().iter().enumerate() {
                Button { variant: ButtonVariant::Outline, key: "{path}-{index}", r#type: "button", disabled: busy || uploading(), onclick: move |_| { images.write().remove(index); }, "Remove image {index + 1}" }
            }
            if let Some(message) = error() { Alert { "{message}" } }
            if uploading() { Status { "Uploading images…" } }
            Item {
                ItemActions {
                    Button { variant: ButtonVariant::Outline, r#type: "button", disabled: draft.is_none() || busy || uploading(), title: if draft.is_none() { "Open the agent conversation to quote a passage" } else { "Append quote to the message draft" }, onclick: move |_| {
                        let Some(mut draft) = draft else { return; };
                        let message = quote_message(&quote, annotation.start_line, annotation.end_line);
                        let mut text = draft();
                        if !text.is_empty() { text.push_str("\n\n"); }
                        text.push_str(&message); draft.set(text); on_close.call(());
                    }, "Quote in message" }
                    Button { r#type: "button", disabled: busy || uploading(), onclick: move |_| {
                        let mut value = save.clone(); value.comment = comment(); value.images = images(); on_save.call(value);
                    }, "Save note" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decision_controls_fixture(props: (bool, bool, bool)) -> Element {
        let busy = use_signal(|| props.0);
        let editing = use_signal(|| {
            props.1.then(|| Annotation {
                id: "note".into(),
                revision: 1,
                start_line: 1,
                end_line: 1,
                quote: "Plan".into(),
                comment: String::new(),
                images: vec![],
            })
        });
        let can_request = use_memo(move || props.2);
        let decide = use_callback(|_| {});
        rsx! { ReviewDecisionActions { controls: ReviewControls { busy, editing, can_request, decide } } }
    }

    #[test]
    fn official_decision_buttons_preserve_disabled_states() {
        for (state, disabled) in [
            ((false, false, true), 0),
            ((false, false, false), 1),
            ((true, false, true), 2),
            ((false, true, true), 2),
        ] {
            let mut dom = VirtualDom::new_with_props(decision_controls_fixture, state);
            dom.rebuild_in_place();
            let html = dioxus_ssr::render(&dom);
            assert_eq!(html.matches("<button").count(), 2, "{html}");
            assert_eq!(html.matches(r#"type="button""#).count(), 2, "{html}");
            assert_eq!(html.matches("disabled").count(), disabled, "{html}");
            assert!(html.contains(r#"data-style="outline""#), "{html}");
            assert!(html.contains(r#"data-style="primary""#), "{html}");
            assert!(html.contains("Request changes"), "{html}");
            assert!(html.contains("Approve plan"), "{html}");
        }
    }

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
            rendered.contains("<p data-start-line=\"3\" data-end-line=\"3\"><mark>two</mark></p>")
        );
        assert!(!rendered.contains("<mark>One"));
    }

    #[test]
    fn a_quote_is_marked_across_inline_nodes_and_white_space() {
        let annotation = Annotation {
            id: "a".into(),
            revision: 1,
            start_line: 1,
            end_line: 2,
            quote: "say hello   world".into(),
            comment: String::new(),
            images: vec![],
        };
        let rendered = render_plan("say **hello**\nworld and more\n", &[annotation]);
        assert!(
            rendered.contains(
                "<mark>say </mark><strong><mark>hello</mark></strong>\n<mark>world</mark> and more"
            ),
            "{rendered}"
        );
    }

    #[test]
    fn a_quote_outside_its_lines_or_inside_an_image_is_not_marked() {
        let annotation = |quote: &str, line: usize| Annotation {
            id: "a".into(),
            revision: 1,
            start_line: line,
            end_line: line,
            quote: quote.into(),
            comment: String::new(),
            images: vec![],
        };
        let source = "one\n\n![two](a.png)\n";
        assert!(!render_plan(source, &[annotation("one", 3)]).contains("<mark>"));
        let image = render_plan(source, &[annotation("two", 3)]);
        assert!(!image.contains("<mark>"), "{image}");
    }
}
