//! The portal's plan reviews: the plan an agent submits, the revisions of it,
//! the owner's annotations, and the decision that goes back to the agent.
//!
//! Every review is one file in the portal's own state directory. The file is
//! the only source of truth, so the reviews survive a restart and a request
//! never reads half a review.
//!
//! A decision stays undelivered until the session that owns the review takes
//! it. A session that was not listening gets the decision when it comes back,
//! so a decision never gets lost.

use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use axum::body::Body;
use axum::extract::{Path as RoutePath, RawQuery, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use portal_types::reviews::{
    Annotation, AnnotationsBody, ApproveBody, Attachment, ChangeKind, Decision, DecisionEvent,
    DiffView, GeneralBody, LineChange, Review, ReviewList, ReviewSummary, Revision, SubmitBody,
    Submitted,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use similar::{ChangeTag, DiffOp, TextDiff};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

use crate::error::{Error, Result};
use crate::ledger::{self, Ledger};
use crate::web::AppState;

/// The biggest image the portal accepts.
const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;
/// The number of decisions the server keeps for the sessions that listen.
const DECISION_CHANNEL: usize = 64;
/// The stream sends a comment this often, so an idle connection stays open.
const KEEP_ALIVE: std::time::Duration = std::time::Duration::from_secs(15);
/// The name of the lock that guards the review files.
const REVIEW_LOCK: &str = "reviews";
/// The digits of a hex number.
const HEX: &[u8; 16] = b"0123456789abcdef";

/// The routes of the reviews API.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/reviews", get(list).post(submit))
        .route("/api/reviews/{id}", get(show))
        .route("/api/reviews/{id}/diff", get(diff))
        .route("/api/reviews/{id}/annotations", put(annotations))
        .route("/api/reviews/{id}/general", put(general))
        .route("/api/reviews/{id}/approve", post(approve))
        .route("/api/reviews/{id}/request-changes", post(request_changes))
        .route("/api/attachments", post(attachment))
        .route("/api/attachments/{name}", get(attachment_file))
        .route("/api/sessions/{session}/decisions", get(decisions))
        .route("/api/decisions/{review_id}/ack", post(ack))
}

/// A review and the delivery state of its decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    review: Review,
    /// Whether the session that owns the review took the decision.
    delivered: bool,
}

/// A decision on its way to a session.
#[derive(Debug, Clone)]
struct Delivery {
    session: String,
    event: DecisionEvent,
}

/// The ledger of the reviews.
fn reviews(web: &Ledger) -> Ledger {
    web.child("reviews")
}

/// The directory the images live in.
fn attachments(web: &Ledger) -> PathBuf {
    web.child("attachments").dir
}

/// The decisions the server pushes to the sessions that listen.
///
/// The channel belongs to the portal's own directory, so two portals with their own state
/// never see each other's decisions.
fn decisions_channel(web: &Ledger) -> broadcast::Sender<Delivery> {
    static CHANNELS: OnceLock<Mutex<HashMap<PathBuf, broadcast::Sender<Delivery>>>> =
        OnceLock::new();
    let channels = CHANNELS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut channels = channels.lock().unwrap_or_else(PoisonError::into_inner);
    channels
        .entry(web.dir.clone())
        .or_insert_with(|| broadcast::channel(DECISION_CHANNEL).0)
        .clone()
}

/// The result of work that reads or writes the portal's own files.
///
/// The files live on disk, so the work runs on a blocking thread and never
/// holds a runtime thread while it waits for the disk.
async fn blocking<T: Send + 'static, E: Into<Error> + Send + 'static>(
    work: impl FnOnce() -> std::result::Result<T, E> + Send + 'static,
) -> Result<T> {
    match tokio::task::spawn_blocking(work).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(error.into()),
        Err(error) => Err(Error::Invalid(format!(
            "the portal's own work failed: {error}"
        ))),
    }
}

/// The reviews the portal holds, newest first.
async fn list(State(state): State<AppState>) -> Response {
    let web = state.web.clone();
    let records = match blocking(move || load_all(&web)).await {
        Ok(records) => records,
        Err(error) => return failed(&error),
    };
    let mut records = records;
    records.sort_by(|left, right| {
        latest_ms(right)
            .cmp(&latest_ms(left))
            .then_with(|| right.review.id.cmp(&left.review.id))
    });
    let reviews = records.iter().map(summary).collect();
    Json(ReviewList { reviews }).into_response()
}

/// One review, with its revisions and annotations.
async fn show(State(state): State<AppState>, RoutePath(id): RoutePath<String>) -> Response {
    let owned = id.clone();
    let web = state.web.clone();
    let found = blocking(move || load(&web, &owned)).await;
    match found {
        Ok(Some(record)) => Json(record.review).into_response(),
        Ok(None) => not_found(&id),
        Err(error) => failed(&error),
    }
}

/// The line diff between two revisions of a review.
async fn diff(
    State(state): State<AppState>,
    RoutePath(id): RoutePath<String>,
    RawQuery(query): RawQuery,
) -> Response {
    let owned = id.clone();
    let web = state.web.clone();
    let found = blocking(move || load(&web, &owned)).await;
    let record = match found {
        Ok(Some(record)) => record,
        Ok(None) => return not_found(&id),
        Err(error) => return failed(&error),
    };
    let latest = record
        .review
        .revisions
        .last()
        .map_or(0, |revision| revision.n);
    let from = match number(query.as_deref(), "from") {
        Ok(value) => value.unwrap_or(1),
        Err(detail) => return bad_request(&detail),
    };
    let to = match number(query.as_deref(), "to") {
        Ok(value) => value.unwrap_or(latest),
        Err(detail) => return bad_request(&detail),
    };
    match diff_view(&record.review, from, to) {
        Some(view) => Json(view).into_response(),
        None => bad_request(&format!("the review has no revisions {from} and {to}")),
    }
}

/// A new plan, or a new revision of the plan the session already submitted.
///
/// The session and the path decide which review is open. A review with a
/// decision is closed, so the plan starts a new one.
async fn submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SubmitBody>,
) -> Response {
    if !crate::web::guard::bearer_allowed(&headers, &state.token) {
        return refused("the request needs the portal token");
    }
    match submit_plan(&state.web, body).await {
        Ok(submitted) => Json(submitted).into_response(),
        Err(error) => failed(&error),
    }
}

/// The draft annotations of a review. The body holds the whole list.
async fn annotations(
    State(state): State<AppState>,
    RoutePath(id): RoutePath<String>,
    Json(body): Json<AnnotationsBody>,
) -> Response {
    for annotation in &body.annotations {
        if annotation.start_line == 0 || annotation.end_line < annotation.start_line {
            return bad_request("an annotation needs a line range that starts at 1");
        }
    }
    let owned = id.clone();
    let web = state.web.clone();
    let changed = blocking(move || {
        web.with_lock(REVIEW_LOCK, || {
            let Some(mut record) = load(&web, &owned)? else {
                return Ok(None);
            };
            let revision = record.review.revisions.last().map_or(1, |last| last.n);
            record.review.annotations = body
                .annotations
                .iter()
                .map(|annotation| {
                    let mut annotation = annotation.clone();
                    if annotation.id.is_empty() {
                        annotation.id = random_id();
                    }
                    annotation.revision = revision;
                    annotation
                })
                .collect();
            save(&web, &record)?;
            Ok(Some(record.review))
        })
    })
    .await;
    match changed {
        Ok(Some(review)) => Json(review).into_response(),
        Ok(None) => not_found(&id),
        Err(error) => failed(&error),
    }
}

/// The comment on the plan as a whole.
async fn general(
    State(state): State<AppState>,
    RoutePath(id): RoutePath<String>,
    Json(body): Json<GeneralBody>,
) -> Response {
    let owned = id.clone();
    let web = state.web.clone();
    let changed = blocking(move || {
        web.with_lock(REVIEW_LOCK, || {
            let Some(mut record) = load(&web, &owned)? else {
                return Ok(None);
            };
            record.review.general.clone_from(&body.general);
            save(&web, &record)?;
            Ok(Some(record.review))
        })
    })
    .await;
    match changed {
        Ok(Some(review)) => Json(review).into_response(),
        Ok(None) => not_found(&id),
        Err(error) => failed(&error),
    }
}

/// The owner approves the plan. The decision goes to the session.
async fn approve(
    State(state): State<AppState>,
    RoutePath(id): RoutePath<String>,
    body: Option<Json<ApproveBody>>,
) -> Response {
    let notes = body.and_then(|Json(body)| body.notes);
    let owned = id.clone();
    let web = state.web.clone();
    let decided = blocking(move || {
        web.with_lock(REVIEW_LOCK, || {
            let Some(mut record) = load(&web, &owned)? else {
                return Ok(None);
            };
            record.review.decision = Some(Decision::Approved {
                notes: notes.clone(),
            });
            record.delivered = false;
            save(&web, &record)?;
            Ok(Some(record.review))
        })
    })
    .await;
    match decided {
        Ok(Some(review)) => {
            let _ = decisions_channel(&state.web).send(Delivery {
                session: review.session.clone(),
                event: decision_event(&review),
            });
            Json(review).into_response()
        }
        Ok(None) => not_found(&id),
        Err(error) => failed(&error),
    }
}

/// The owner asks for changes. The decision goes to the session.
async fn request_changes(
    State(state): State<AppState>,
    RoutePath(id): RoutePath<String>,
) -> Response {
    let owned = id.clone();
    let web = state.web.clone();
    let decided = blocking(move || {
        web.with_lock(REVIEW_LOCK, || {
            let Some(mut record) = load(&web, &owned)? else {
                return Ok(None);
            };
            let message = change_request(&record.review);
            record.review.decision = Some(Decision::ChangesRequested {
                message: message.clone(),
            });
            record.delivered = false;
            save(&web, &record)?;
            Ok(Some(record.review))
        })
    })
    .await;
    match decided {
        Ok(Some(review)) => {
            let _ = decisions_channel(&state.web).send(Delivery {
                session: review.session.clone(),
                event: decision_event(&review),
            });
            Json(review).into_response()
        }
        Ok(None) => not_found(&id),
        Err(error) => failed(&error),
    }
}

/// An image the owner added to an annotation. The body is the image itself.
async fn attachment(State(state): State<AppState>, body: Body) -> Response {
    let Ok(bytes) = axum::body::to_bytes(body, MAX_ATTACHMENT_BYTES + 1).await else {
        return too_large();
    };
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return too_large();
    }
    let Some(extension) = image_extension(&bytes) else {
        return not_an_image();
    };
    let name = format!("{}.{extension}", random_id());
    let owned = name.clone();
    let web = state.web.clone();
    let written = blocking(move || write_attachment(&web, &owned, &bytes)).await;
    if let Err(error) = written {
        return failed(&error);
    }
    let path = attachments(&state.web).join(&name);
    Json(Attachment {
        path: path.display().to_string(),
    })
    .into_response()
}

/// An image the owner added to an annotation.
async fn attachment_file(
    State(state): State<AppState>,
    RoutePath(name): RoutePath<String>,
) -> Response {
    if !valid_attachment_name(&name) {
        return not_found(&name);
    }
    let owned = name.clone();
    let web = state.web.clone();
    let bytes = blocking(move || {
        let path = attachments(&web).join(&owned);
        fs::read(&path).map_err(Error::from)
    })
    .await;
    match bytes {
        Ok(bytes) => ([(header::CONTENT_TYPE, image_type(&name))], bytes).into_response(),
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => not_found(&name),
        Err(error) => failed(&error),
    }
}

/// The decisions of a session, pushed as they happen.
///
/// The decisions the session did not take yet are sent again when it comes
/// back, so a decision never gets lost.
async fn decisions(
    State(state): State<AppState>,
    RoutePath(session): RoutePath<String>,
    headers: HeaderMap,
) -> Response {
    if !crate::web::guard::bearer_allowed(&headers, &state.token) {
        return refused("the request needs the portal token");
    }
    let stream = match decision_stream(&state.web, session).await {
        Ok(stream) => stream.map(|event| Ok::<Event, Infallible>(decision_frame(event))),
        Err(error) => return failed(&error),
    };
    Sse::new(state.until_shutdown(stream))
        .keep_alive(KeepAlive::new().interval(KEEP_ALIVE))
        .into_response()
}

/// The session took the decision of a review.
async fn ack(
    State(state): State<AppState>,
    RoutePath(review_id): RoutePath<String>,
    headers: HeaderMap,
) -> Response {
    if !crate::web::guard::bearer_allowed(&headers, &state.token) {
        return refused("the request needs the portal token");
    }
    match acknowledge(&state.web, review_id.clone()).await {
        Ok(Some(delivered)) => Json(json!({ "delivered": delivered })).into_response(),
        Ok(None) => not_found(&review_id),
        Err(error) => failed(&error),
    }
}

/// Submit through the same locked store as the HTTP endpoint.
pub(crate) async fn submit_plan(web: &Ledger, body: SubmitBody) -> Result<Submitted> {
    let web = web.clone();
    blocking(move || web.with_lock(REVIEW_LOCK, || submit_locked(&web, &body))).await
}

/// Replay pending decisions, then follow live decisions without a subscribe/read race.
async fn decision_stream(
    web: &Ledger,
    session: String,
) -> Result<impl Stream<Item = DecisionEvent> + use<>> {
    let receiver = decisions_channel(web).subscribe();
    let owned = session.clone();
    let web = web.clone();
    let replay = blocking(move || replay_events(&web, &owned)).await?;
    let replayed = Arc::new(replay.clone());
    Ok(tokio_stream::iter(replay).chain(live_events(receiver, session, replayed)))
}

/// Wait for this review, not another pending decision of the same session.
pub(crate) async fn wait_for_decision(
    web: &Ledger,
    session: String,
    review_id: &str,
) -> Result<DecisionEvent> {
    let stream = decision_stream(web, session).await?;
    tokio::pin!(stream);
    while let Some(event) = stream.next().await {
        if event.review_id == review_id {
            return Ok(event);
        }
    }
    Err(Error::Invalid(
        "the review decisions stream ended".to_owned(),
    ))
}

/// Mark a decision delivered through either transport.
pub(crate) async fn acknowledge(web: &Ledger, owned: String) -> Result<Option<bool>> {
    let web = web.clone();
    blocking(move || {
        web.with_lock(REVIEW_LOCK, || {
            let Some(mut record) = load(&web, &owned)? else {
                return Ok(None);
            };
            record.delivered = true;
            save(&web, &record)?;
            Ok(Some(record.delivered))
        })
    })
    .await
}

/// The plan of a session, with the revision it adds.
fn submit_locked(web: &Ledger, body: &SubmitBody) -> Result<Submitted> {
    let mut records = load_all(web)?;
    let now = ledger::now_ms();
    // The open review of the same session and path takes the new revision.
    let open = records
        .iter()
        .enumerate()
        .filter(|(_, record)| is_open(record, body))
        .max_by_key(|(_, record)| latest_ms(record))
        .map(|(index, _)| index);
    let Some(index) = open else {
        let review = Review {
            id: random_id(),
            pane: body.pane.clone(),
            session: body.session.clone(),
            cwd: body.cwd.clone(),
            path: body.path.clone(),
            revisions: vec![Revision {
                n: 1,
                content: body.content.clone(),
                submitted_ms: now,
            }],
            annotations: Vec::new(),
            general: String::new(),
            decision: None,
        };
        let record = Record {
            review,
            delivered: false,
        };
        save(web, &record)?;
        return Ok(Submitted {
            id: record.review.id.clone(),
            revision: 1,
        });
    };
    let record = &mut records[index];
    let revision = record.review.revisions.last().map_or(0, |last| last.n) + 1;
    let annotations = reanchor(&record.review, &body.content, revision);
    record.review.pane.clone_from(&body.pane);
    record.review.cwd.clone_from(&body.cwd);
    record.review.revisions.push(Revision {
        n: revision,
        content: body.content.clone(),
        submitted_ms: now,
    });
    record.review.annotations = annotations;
    save(web, record)?;
    Ok(Submitted {
        id: record.review.id.clone(),
        revision,
    })
}

/// Whether the review takes the plan of this submission.
fn is_open(record: &Record, body: &SubmitBody) -> bool {
    record.review.session == body.session
        && record.review.path == body.path
        && record.review.decision.is_none()
}

/// The time of the latest revision of a review.
fn latest_ms(record: &Record) -> u64 {
    record
        .review
        .revisions
        .last()
        .map_or(0, |revision| revision.submitted_ms)
}

/// The annotations that still fit the new plan, with their new line numbers.
///
/// An annotation whose lines changed does not fit the new plan, so it goes.
fn reanchor(review: &Review, content: &str, revision: u32) -> Vec<Annotation> {
    let mut kept = Vec::new();
    for annotation in &review.annotations {
        let Some(old) = review
            .revisions
            .iter()
            .find(|candidate| candidate.n == annotation.revision)
        else {
            continue;
        };
        let Some((start, end)) = moved_quote(
            &old.content,
            content,
            annotation.start_line,
            annotation.end_line,
            &annotation.quote,
        ) else {
            continue;
        };
        let mut annotation = annotation.clone();
        annotation.revision = revision;
        annotation.start_line = start;
        annotation.end_line = end;
        kept.push(annotation);
    }
    kept
}

/// The new line numbers of an annotated range whose quoted text did not change.
///
/// The quote is the text the comment is about, so it decides whether the
/// comment still fits the new plan. The lines of the quote must all be in the
/// new plan, at the place the diff gives.
fn moved_quote(
    old: &str,
    new: &str,
    start: usize,
    end: usize,
    quote: &str,
) -> Option<(usize, usize)> {
    if start == 0 || end < start {
        return None;
    }
    let map = line_map(old, new);
    // Every line of the range must be unchanged.
    let mut first = None;
    let mut last = None;
    for line in start..=end {
        let moved = *map.get(&line)?;
        first.get_or_insert(moved);
        last = Some(moved);
    }
    let (first, last) = (first?, last?);
    let quoted = quote.trim_end_matches('\n');
    if quoted.is_empty() {
        // Without a quote the range must still be one block of lines.
        for line in start..=end {
            if map[&line] != first + (line - start) {
                return None;
            }
        }
        return Some((first, last));
    }
    // The quoted text must be the text of the new range.
    let lines: Vec<&str> = new.lines().collect();
    if last > lines.len() {
        return None;
    }
    let text = lines[first - 1..last].join("\n");
    if text.trim_end_matches('\n') != quoted {
        return None;
    }
    Some((first, last))
}

/// The new line number of every line that did not change.
fn line_map(old: &str, new: &str) -> HashMap<usize, usize> {
    let mut map = HashMap::new();
    for op in TextDiff::from_lines(old, new).ops() {
        if let DiffOp::Equal {
            old_index,
            new_index,
            len,
        } = *op
        {
            for offset in 0..len {
                map.insert(old_index + offset + 1, new_index + offset + 1);
            }
        }
    }
    map
}

/// The line diff between two revisions of a review.
fn diff_view(review: &Review, from: u32, to: u32) -> Option<DiffView> {
    let old = review
        .revisions
        .iter()
        .find(|revision| revision.n == from)?;
    let new = review.revisions.iter().find(|revision| revision.n == to)?;
    let mut changes = Vec::new();
    for change in
        TextDiff::from_lines(old.content.as_str(), new.content.as_str()).iter_all_changes()
    {
        let kind = match change.tag() {
            ChangeTag::Equal => ChangeKind::Same,
            ChangeTag::Delete => ChangeKind::Removed,
            ChangeTag::Insert => ChangeKind::Added,
        };
        changes.push(LineChange {
            kind,
            old: change.old_index().map(|index| index + 1),
            new: change.new_index().map(|index| index + 1),
            text: change
                .value()
                .trim_end_matches('\n')
                .trim_end_matches('\r')
                .to_owned(),
        });
    }
    Some(DiffView { from, to, changes })
}

/// The message the agent gets when the owner asks for changes.
///
/// Every annotation comes first, with its quote and its line range. The images
/// of the annotations follow, and the comment on the plan as a whole comes
/// last.
fn change_request(review: &Review) -> String {
    let mut blocks = Vec::new();
    for annotation in &review.annotations {
        let mut block = String::new();
        for line in annotation.quote.lines() {
            block.push_str("> ");
            block.push_str(line);
            block.push('\n');
        }
        let _ = writeln!(
            block,
            "plan lines {}-{}",
            annotation.start_line, annotation.end_line
        );
        block.push_str(&annotation.comment);
        blocks.push(block);
    }
    let images: Vec<&str> = review
        .annotations
        .iter()
        .flat_map(|annotation| annotation.images.iter().map(String::as_str))
        .collect();
    if !images.is_empty() {
        let mut block = String::from("Reference images:\n");
        for image in images {
            let _ = writeln!(block, "- {image}");
        }
        blocks.push(block);
    }
    if !review.general.trim().is_empty() {
        blocks.push(review.general.clone());
    }
    blocks.join("\n\n")
}

/// The event of a decision, with the text the agent receives.
fn decision_event(review: &Review) -> DecisionEvent {
    let decision = review
        .decision
        .clone()
        .unwrap_or(Decision::Approved { notes: None });
    let message = match &decision {
        Decision::Approved { notes } => notes.clone().unwrap_or_default(),
        Decision::ChangesRequested { message } => message.clone(),
    };
    DecisionEvent {
        review_id: review.id.clone(),
        path: review.path.clone(),
        decision: decision.kind(),
        message,
        plan_content: review
            .revisions
            .last()
            .map_or_else(String::new, |revision| revision.content.clone()),
    }
}

/// The decisions of a session that the session did not take yet.
fn replay_events(web: &Ledger, session: &str) -> Result<Vec<DecisionEvent>> {
    Ok(load_all(web)?
        .iter()
        .filter(|record| {
            record.review.session == session
                && record.review.decision.is_some()
                && !record.delivered
        })
        .map(|record| decision_event(&record.review))
        .collect())
}

/// The decisions that arrive while the session listens.
fn live_events(
    receiver: broadcast::Receiver<Delivery>,
    session: String,
    replayed: Arc<Vec<DecisionEvent>>,
) -> impl Stream<Item = DecisionEvent> {
    BroadcastStream::new(receiver).filter_map(move |item| {
        let replayed = Arc::clone(&replayed);
        let session = session.clone();
        let delivery = item.ok()?;
        if delivery.session != session {
            return None;
        }
        // A decision that was already replayed does not go out twice. A later
        // decision on the same review is a different decision, so it goes.
        let known = replayed.iter().any(|event| {
            event.review_id == delivery.event.review_id && event.decision == delivery.event.decision
        });
        if known {
            return None;
        }
        Some(delivery.event)
    })
}

/// One decision, as an SSE frame.
fn decision_frame(event: DecisionEvent) -> Event {
    let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_owned());
    Event::default()
        .event("decision")
        .id(event.review_id)
        .data(data)
}

/// The summary of a review, for the list.
fn summary(record: &Record) -> ReviewSummary {
    ReviewSummary {
        id: record.review.id.clone(),
        pane: record.review.pane.clone(),
        session: record.review.session.clone(),
        path: record.review.path.clone(),
        revisions: record.review.revisions.len(),
        annotations: record.review.annotations.len(),
        decision: record.review.decision.clone(),
    }
}

/// Read every review the portal holds.
fn load_all(web: &Ledger) -> Result<Vec<Record>> {
    let dir = reviews(web).dir;
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
            continue;
        };
        if !extension.eq_ignore_ascii_case("json") {
            continue;
        }
        let bytes = fs::read(&path)?;
        records.push(serde_json::from_slice(&bytes)?);
    }
    Ok(records)
}

/// Read one review.
fn load(web: &Ledger, id: &str) -> Result<Option<Record>> {
    if !valid_id(id) {
        return Ok(None);
    }
    reviews(web).read(&format!("{id}.json"))
}

/// Write one review.
fn save(web: &Ledger, record: &Record) -> Result<()> {
    reviews(web).write(&format!("{}.json", record.review.id), record)
}

/// Write an image, readable by its owner only.
fn write_attachment(web: &Ledger, name: &str, bytes: &[u8]) -> Result<()> {
    let dir = attachments(web);
    fs::create_dir_all(&dir)?;
    let path = dir.join(name);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(&path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// The extension of an image, from its first bytes.
///
/// The bytes decide the type, not the name the caller gave.
fn image_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Some("png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("jpg");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    None
}

/// The type the browser reads an image with.
fn image_type(name: &str) -> &'static str {
    match name.rsplit('.').next() {
        Some("jpg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => "image/png",
    }
}

/// Whether a review id is one this server made.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Whether an attachment name is one this server made.
fn valid_attachment_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// A random id for a review or an image.
fn random_id() -> String {
    let mut bytes = [0_u8; 16];
    match getrandom::fill(&mut bytes) {
        Ok(()) => hex(&bytes),
        Err(_) => ledger::new_id(),
    }
}

/// The bytes as hex.
fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from(HEX[usize::from(byte >> 4)]));
        text.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    text
}

/// A number from the query string.
fn number(query: Option<&str>, key: &str) -> std::result::Result<Option<u32>, String> {
    let Some(value) = value_of(query, key) else {
        return Ok(None);
    };
    value
        .parse::<u32>()
        .map(Some)
        .map_err(|_| format!("{key} must be a revision number"))
}

/// A value from the query string.
fn value_of(query: Option<&str>, key: &str) -> Option<String> {
    let query = query?;
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let name = parts.next().unwrap_or_default();
        let value = parts.next().unwrap_or_default();
        if name == key {
            return Some(decode(value));
        }
    }
    None
}

/// The text of a query value.
fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or_default();
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    index += 3;
                } else {
                    out.push(bytes[index]);
                    index += 1;
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The review is not there.
fn not_found(id: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": format!("no review {id}") })),
    )
        .into_response()
}

/// The request was refused.
fn refused(detail: &str) -> Response {
    (StatusCode::FORBIDDEN, Json(json!({ "error": detail }))).into_response()
}

/// The request was not understood.
fn bad_request(detail: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": detail }))).into_response()
}

/// The image is bigger than the portal accepts.
fn too_large() -> Response {
    (
        StatusCode::PAYLOAD_TOO_LARGE,
        Json(json!({ "error": "the image is bigger than 10 MiB" })),
    )
        .into_response()
}

/// The body is not an image.
fn not_an_image() -> Response {
    (
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        Json(json!({ "error": "the body is not a PNG, JPEG or WebP image" })),
    )
        .into_response()
}

/// The portal could not read or write its own state.
fn failed(error: &Error) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": error.to_string() })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::guard;
    use axum::extract::Request;
    use axum::http::Method;
    use axum::middleware::Next;
    use tower::ServiceExt;

    /// The port the tests ask for.
    const PORT: u16 = 7465;

    /// The portal's token, in the state the tests share.
    const TOKEN: &str = "secret";

    /// A state over a fresh directory. Every test has its own, so the tests
    /// run side by side.
    fn fresh_state() -> AppState {
        let (_sender, receiver) = tokio::sync::watch::channel(String::new());
        let web =
            Ledger::new(std::env::temp_dir().join(format!("dogsled-reviews-{}", ledger::new_id())))
                .child("web");
        AppState::new(
            receiver,
            PORT,
            None,
            guard::Token::new(TOKEN.to_owned()),
            web,
        )
    }

    /// The same guard the server uses.
    async fn test_guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
        let method = request.method().clone();
        let headers = request.headers().clone();
        if let Some(response) = guard::check(&method, &headers, state.port, &state.token) {
            return response;
        }
        next.run(request).await
    }

    /// One request against the reviews API.
    async fn call(state: &AppState, request: Request<Body>) -> Response {
        let app = Router::new()
            .merge(routes())
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                test_guard,
            ))
            .with_state(state.clone());
        app.oneshot(request).await.expect("the router answers")
    }

    /// A request with the given method, path and JSON body.
    fn request(method: Method, path: &str, body: Option<&str>) -> Request<Body> {
        let builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, format!("127.0.0.1:{PORT}"))
            .header(header::ORIGIN, format!("http://127.0.0.1:{PORT}"));
        let builder = match body {
            Some(body) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_owned())),
            None => builder.body(Body::empty()),
        };
        builder.expect("the request")
    }

    /// A request with the portal's token.
    fn token_request(method: Method, path: &str, body: Option<&str>) -> Request<Body> {
        let request = request(method, path, body);
        let (mut parts, body) = request.into_parts();
        parts.headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {TOKEN}").parse().expect("a header"),
        );
        Request::from_parts(parts, body)
    }

    /// The body of a response, as JSON.
    async fn json_body(response: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("the body");
        serde_json::from_slice(&bytes).expect("the payload")
    }

    /// The first events of an event stream.
    ///
    /// The stream of a decision never ends, so the test reads the frames that
    /// arrive and stops.
    async fn stream_events(response: Response) -> String {
        let mut body = response.into_body().into_data_stream();
        let mut text = String::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            let Ok(Some(Ok(frame))) = tokio::time::timeout_at(deadline, body.next()).await else {
                break;
            };
            text.push_str(&String::from_utf8_lossy(&frame));
            if text.contains("event: decision") {
                break;
            }
        }
        text
    }

    /// The data of the first event of a stream.
    fn event_data(text: &str) -> String {
        let mut data = String::new();
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("data:") {
                data.push_str(value.trim_start());
            }
        }
        data
    }

    /// The plan a session submits.
    fn submit_body(path: &str, content: &str) -> String {
        json!({
            "path": path,
            "content": content,
            "pane": "%1",
            "session": "session-1",
            "cwd": "/tmp/work",
        })
        .to_string()
    }

    /// Submit a plan and return the review id and the revision.
    async fn submit_plan(state: &AppState, path: &str, content: &str) -> (String, u32) {
        let response = call(
            state,
            token_request(
                Method::POST,
                "/api/reviews",
                Some(&submit_body(path, content)),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        (
            body["id"].as_str().expect("the review id").to_owned(),
            body["revision"].as_u64().expect("the revision") as u32,
        )
    }

    /// The bytes of a small PNG.
    fn png() -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        bytes.extend_from_slice(b"not a real image, but it has the magic bytes");
        bytes
    }

    #[tokio::test]
    async fn a_new_plan_opens_a_review_with_revision_one() {
        let state = fresh_state();
        let (id, revision) = submit_plan(&state, "plans/one.md", "first\n").await;
        assert_eq!(revision, 1);
        let response = call(
            &state,
            request(Method::GET, &format!("/api/reviews/{id}"), None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["path"], "plans/one.md");
        assert_eq!(body["session"], "session-1");
        assert_eq!(body["revisions"][0]["n"], 1);
        assert_eq!(body["revisions"][0]["content"], "first\n");
    }

    #[tokio::test]
    async fn the_same_plan_again_adds_a_revision() {
        let state = fresh_state();
        let (id, _) = submit_plan(&state, "plans/one.md", "first\n").await;
        let response = call(
            &state,
            token_request(
                Method::POST,
                "/api/reviews",
                Some(&submit_body("plans/one.md", "first\nsecond\n")),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["id"], id);
        assert_eq!(body["revision"], 2);
    }

    #[tokio::test]
    async fn another_path_opens_another_review() {
        let state = fresh_state();
        let (first, _) = submit_plan(&state, "plans/one.md", "first\n").await;
        let (second, revision) = submit_plan(&state, "plans/two.md", "first\n").await;
        assert_eq!(revision, 1);
        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn an_annotation_on_an_unchanged_line_moves_with_the_revision() {
        let state = fresh_state();
        let (id, _) = submit_plan(&state, "plans/one.md", "one\ntwo\nthree\n").await;
        let annotations = json!({
            "annotations": [{
                "id": "a1",
                "revision": 1,
                "start_line": 3,
                "end_line": 3,
                "quote": "three",
                "comment": "say more",
                "images": [],
            }]
        })
        .to_string();
        let response = call(
            &state,
            request(
                Method::PUT,
                &format!("/api/reviews/{id}/annotations"),
                Some(&annotations),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        // A line is added above the annotated one, so the annotation moves down.
        let response = call(
            &state,
            token_request(
                Method::POST,
                "/api/reviews",
                Some(&submit_body("plans/one.md", "zero\none\ntwo\nthree\n")),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["revision"], 2);

        let response = call(
            &state,
            request(Method::GET, &format!("/api/reviews/{id}"), None),
        )
        .await;
        let body = json_body(response).await;
        let kept = &body["annotations"][0];
        assert_eq!(kept["id"], "a1");
        assert_eq!(kept["revision"], 2);
        assert_eq!(kept["start_line"], 4);
        assert_eq!(kept["end_line"], 4);
        assert_eq!(kept["quote"], "three");
    }

    #[tokio::test]
    async fn an_annotation_on_a_changed_line_goes() {
        let state = fresh_state();
        let (id, _) = submit_plan(&state, "plans/one.md", "one\ntwo\nthree\n").await;
        let annotations = json!({
            "annotations": [{
                "id": "a1",
                "revision": 1,
                "start_line": 3,
                "end_line": 3,
                "quote": "three",
                "comment": "say more",
                "images": [],
            }]
        })
        .to_string();
        let response = call(
            &state,
            request(
                Method::PUT,
                &format!("/api/reviews/{id}/annotations"),
                Some(&annotations),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        // The annotated line changes, so the comment no longer fits the plan.
        let response = call(
            &state,
            token_request(
                Method::POST,
                "/api/reviews",
                Some(&submit_body("plans/one.md", "one\ntwo\nthree and more\n")),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let response = call(
            &state,
            request(Method::GET, &format!("/api/reviews/{id}"), None),
        )
        .await;
        let body = json_body(response).await;
        assert_eq!(body["annotations"].as_array().map(Vec::len), Some(0));
    }

    #[tokio::test]
    async fn an_annotation_moves_when_the_quoted_text_stays() {
        let state = fresh_state();
        let (id, _) = submit_plan(&state, "plans/one.md", "one\ntwo\nthree\n").await;
        let annotations = json!({
            "annotations": [{
                "id": "a1",
                "revision": 1,
                "start_line": 2,
                "end_line": 2,
                "quote": "two",
                "comment": "say more",
                "images": [],
            }]
        })
        .to_string();
        let response = call(
            &state,
            request(
                Method::PUT,
                &format!("/api/reviews/{id}/annotations"),
                Some(&annotations),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        // The quoted text stays and moves down by one line.
        let response = call(
            &state,
            token_request(
                Method::POST,
                "/api/reviews",
                Some(&submit_body("plans/one.md", "zero\none\ntwo\nthree\n")),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = call(
            &state,
            request(Method::GET, &format!("/api/reviews/{id}"), None),
        )
        .await;
        let body = json_body(response).await;
        let kept = &body["annotations"][0];
        assert_eq!(kept["revision"], 2);
        assert_eq!(kept["start_line"], 3);
        assert_eq!(kept["end_line"], 3);
    }

    #[tokio::test]
    async fn the_diff_between_two_revisions_shows_the_lines() {
        let state = fresh_state();
        let (id, _) = submit_plan(&state, "plans/one.md", "one\ntwo\n").await;
        let response = call(
            &state,
            token_request(
                Method::POST,
                "/api/reviews",
                Some(&submit_body("plans/one.md", "one\ntwo and more\n")),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = call(
            &state,
            request(
                Method::GET,
                &format!("/api/reviews/{id}/diff?from=1&to=2"),
                None,
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["from"], 1);
        assert_eq!(body["to"], 2);
        let changes = body["changes"].as_array().expect("the changes");
        let kinds: Vec<&str> = changes
            .iter()
            .map(|change| change["kind"].as_str().unwrap_or_default())
            .collect();
        assert!(kinds.contains(&"added"), "{kinds:?}");
        assert!(kinds.contains(&"removed"), "{kinds:?}");
        assert!(kinds.contains(&"same"), "{kinds:?}");
    }

    #[tokio::test]
    async fn the_change_request_message_holds_the_quote_the_lines_the_comment_and_the_images() {
        let state = fresh_state();
        let (id, _) = submit_plan(&state, "plans/one.md", "one\ntwo\n").await;
        let annotations = json!({
            "annotations": [{
                "id": "a1",
                "revision": 1,
                "start_line": 2,
                "end_line": 2,
                "quote": "two",
                "comment": "use three instead",
                "images": ["/tmp/web/attachments/a.png"],
            }]
        })
        .to_string();
        let response = call(
            &state,
            request(
                Method::PUT,
                &format!("/api/reviews/{id}/annotations"),
                Some(&annotations),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = call(
            &state,
            request(
                Method::PUT,
                &format!("/api/reviews/{id}/general"),
                Some(&json!({ "general": "the plan needs a test" }).to_string()),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let response = call(
            &state,
            request(
                Method::POST,
                &format!("/api/reviews/{id}/request-changes"),
                None,
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        let message = body["decision"]["message"].as_str().expect("the message");
        assert!(message.contains("> two"), "{message}");
        assert!(message.contains("plan lines 2-2"), "{message}");
        assert!(message.contains("use three instead"), "{message}");
        assert!(message.contains("Reference images:"), "{message}");
        assert!(
            message.contains("- /tmp/web/attachments/a.png"),
            "{message}"
        );
        assert!(message.contains("the plan needs a test"), "{message}");
        assert_eq!(body["decision"]["kind"], "changes_requested");
    }

    #[tokio::test]
    async fn an_approval_carries_its_notes() {
        let state = fresh_state();
        let (id, _) = submit_plan(&state, "plans/one.md", "one\n").await;
        let response = call(
            &state,
            request(
                Method::POST,
                &format!("/api/reviews/{id}/approve"),
                Some(&json!({ "notes": "looks good" }).to_string()),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["decision"]["kind"], "approved");
        assert_eq!(body["decision"]["notes"], "looks good");
    }

    #[tokio::test]
    async fn an_attachment_that_is_not_an_image_is_refused() {
        let state = fresh_state();
        let response = call(
            &state,
            Request::builder()
                .method(Method::POST)
                .uri("/api/attachments")
                .header(header::HOST, format!("127.0.0.1:{PORT}"))
                .header(header::ORIGIN, format!("http://127.0.0.1:{PORT}"))
                .body(Body::from("this is not an image"))
                .expect("the request"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn an_attachment_is_written_for_its_owner_only() {
        let state = fresh_state();
        let response = call(
            &state,
            Request::builder()
                .method(Method::POST)
                .uri("/api/attachments")
                .header(header::HOST, format!("127.0.0.1:{PORT}"))
                .header(header::ORIGIN, format!("http://127.0.0.1:{PORT}"))
                .body(Body::from(png()))
                .expect("the request"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        let path = body["path"].as_str().expect("the path");
        assert!(
            std::path::Path::new(path)
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("png")),
            "{path}"
        );
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(path).expect("the image").permissions(),
        );
        assert_eq!(mode & 0o777, 0o600);
    }

    #[tokio::test]
    async fn an_attachment_is_served_back() {
        let state = fresh_state();
        let response = call(
            &state,
            Request::builder()
                .method(Method::POST)
                .uri("/api/attachments")
                .header(header::HOST, format!("127.0.0.1:{PORT}"))
                .header(header::ORIGIN, format!("http://127.0.0.1:{PORT}"))
                .body(Body::from(png()))
                .expect("the request"),
        )
        .await;
        let body = json_body(response).await;
        let path = body["path"].as_str().expect("the path");
        let name = path.rsplit('/').next().expect("the name");
        let response = call(
            &state,
            request(Method::GET, &format!("/api/attachments/{name}"), None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("image/png")
        );
    }

    #[tokio::test]
    async fn an_attachment_name_that_is_not_one_of_ours_is_refused() {
        let state = fresh_state();
        let response = call(
            &state,
            request(Method::GET, "/api/attachments/..%2Ftoken", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_decision_is_replayed_until_the_session_takes_it() {
        let state = fresh_state();
        let (id, _) = submit_plan(&state, "plans/one.md", "one\n").await;
        let response = call(
            &state,
            request(
                Method::POST,
                &format!("/api/reviews/{id}/approve"),
                Some(&json!({ "notes": "looks good" }).to_string()),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        // The session connects and gets the decision again.
        let response = call(
            &state,
            token_request(Method::GET, "/api/sessions/session-1/decisions", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let text = stream_events(response).await;
        assert!(text.contains("event: decision"), "{text}");
        assert!(text.contains("looks good"), "{text}");

        // The event carries the name of the decision as a plain string, so the
        // Pi client can read it.
        let payload = event_data(&text);
        let event: serde_json::Value =
            serde_json::from_str(&payload).expect("the decision payload");
        assert_eq!(event["review_id"], id);
        assert_eq!(event["decision"], "approved");
        assert_eq!(event["message"], "looks good");
        assert_eq!(event["plan_content"], "one\n");
        assert_eq!(event["path"], "plans/one.md");

        // The session takes the decision, so it is not replayed again.
        let response = call(
            &state,
            token_request(Method::POST, &format!("/api/decisions/{id}/ack"), None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["delivered"], true);

        let response = call(
            &state,
            token_request(Method::GET, "/api/sessions/session-1/decisions", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let text = stream_events(response).await;
        assert!(!text.contains("event: decision"), "{text}");
    }

    #[tokio::test]
    async fn the_decisions_of_another_session_stay_out() {
        let state = fresh_state();
        let (id, _) = submit_plan(&state, "plans/one.md", "one\n").await;
        let response = call(
            &state,
            request(
                Method::POST,
                &format!("/api/reviews/{id}/approve"),
                Some(&json!({ "notes": "looks good" }).to_string()),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = call(
            &state,
            token_request(Method::GET, "/api/sessions/other-session/decisions", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let text = stream_events(response).await;
        assert!(!text.contains("event: decision"), "{text}");
    }

    #[tokio::test]
    async fn a_submit_without_the_token_is_refused() {
        let state = fresh_state();
        let response = call(
            &state,
            request(
                Method::POST,
                "/api/reviews",
                Some(&submit_body("plans/one.md", "one\n")),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_decision_stream_without_the_token_is_refused() {
        let state = fresh_state();
        let response = call(
            &state,
            request(Method::GET, "/api/sessions/session-1/decisions", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn an_ack_without_the_token_is_refused() {
        let state = fresh_state();
        let (id, _) = submit_plan(&state, "plans/one.md", "one\n").await;
        let response = call(
            &state,
            request(Method::POST, &format!("/api/decisions/{id}/ack"), None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_write_from_another_origin_is_refused() {
        let state = fresh_state();
        let response = call(
            &state,
            Request::builder()
                .method(Method::PUT)
                .uri("/api/reviews/x/general")
                .header(header::HOST, format!("127.0.0.1:{PORT}"))
                .header(header::ORIGIN, "http://evil.example")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "general": "no" }).to_string()))
                .expect("the request"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_request_from_another_host_is_refused() {
        let state = fresh_state();
        let response = call(
            &state,
            Request::builder()
                .method(Method::GET)
                .uri("/api/reviews")
                .header(header::HOST, "evil.example")
                .body(Body::empty())
                .expect("the request"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_review_that_is_not_there_is_refused() {
        let state = fresh_state();
        let response = call(&state, request(Method::GET, "/api/reviews/nope", None)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_list_holds_the_reviews_newest_first() {
        let state = fresh_state();
        submit_plan(&state, "plans/one.md", "one\n").await;
        submit_plan(&state, "plans/two.md", "two\n").await;
        let response = call(&state, request(Method::GET, "/api/reviews", None)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        let reviews = body["reviews"].as_array().expect("the reviews");
        assert_eq!(reviews.len(), 2);
        assert_eq!(reviews[0]["path"], "plans/two.md");
        assert_eq!(reviews[0]["revisions"], 1);
    }

    #[tokio::test]
    async fn a_decided_review_starts_a_new_one() {
        let state = fresh_state();
        let (id, _) = submit_plan(&state, "plans/one.md", "one\n").await;
        let response = call(
            &state,
            request(
                Method::POST,
                &format!("/api/reviews/{id}/approve"),
                Some(&json!({ "notes": "looks good" }).to_string()),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let (second, revision) = submit_plan(&state, "plans/one.md", "one\n").await;
        assert_eq!(revision, 1);
        assert_ne!(second, id);
    }
}
