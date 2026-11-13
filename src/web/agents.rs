//! The agent view of the portal: the list of agents, one agent's conversation
//! and screen, and the calls that act on an agent.
//!
//! Agent reads come from serve's publication. Only explicit actions use Herdr.
//!
//! Keys are a whitelist. The portal may answer a prompt with the keys Herdr
//! accepts, and nothing else. A key outside the list is refused, so a request
//! cannot type an arbitrary sequence into a terminal.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::KeepAlive;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use portal_types::agents::{AgentEvent, AgentView, KeysBody, PromptBody};
use serde_json::{Value, json};
use tokio_stream::StreamExt;

use crate::herdr_socket::Client;
use crate::observation::{
    details::{Content, Detail, DetailFrame},
    ipc,
};
use crate::web::AppState;
use std::time::{Duration, Instant};

/// The keys the portal may send. Every key is a single key press, so a
/// request cannot carry a command.
const ALLOWED_KEYS: &[&str] = &[
    "enter",
    "esc",
    "tab",
    "space",
    "backspace",
    "delete",
    // Ctrl+C interrupts a working agent.
    "ctrl+c",
    "up",
    "down",
    "left",
    "right",
    "home",
    "end",
    "pageup",
    "pagedown",
    "y",
    "n",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
    "0",
];

/// The lines of a screen the portal reads.
const SCREEN_LINES: u32 = 200;

/// SSH carries only published detail or a bounded activity command, never foreign paths.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "operation",
    content = "body",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum PeerOperation {
    Conversation,
    Screen,
    Files,
    Event,
    Activity,
    Settings,
    ChangeSettings(portal_types::models::SettingsChange),
    Register(portal_types::agents::RegisterProducer),
    Publish(portal_types::agents::PublishActivity),
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PeerReply {
    pub(super) status: u16,
    pub(super) body: Value,
}

pub(crate) fn peer_command(
    pane: String,
    input: &str,
    ledger: crate::ledger::Ledger,
) -> crate::error::Result<PeerReply> {
    let operation: PeerOperation = serde_json::from_str(input)?;
    let state = AppState::new(
        tokio::sync::watch::channel(super::PortalState::default()).1,
        7465,
        None,
        super::guard::Token::new(String::new()),
        ledger,
    );
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(peer_read(state, pane, operation))
}

/// Hold serve-owned detail until the relay closes stdin or the publication disconnects.
pub(crate) fn peer_follow(
    pane: &str,
    input: &str,
    ledger: &crate::ledger::Ledger,
) -> crate::error::Result<()> {
    use std::io::Read as _;
    let operation: PeerOperation = serde_json::from_str(input)?;
    if !matches!(operation, PeerOperation::Event | PeerOperation::Activity) {
        return Err(crate::error::Error::Invalid(
            "unsupported peer stream".into(),
        ));
    }
    let stop = tokio_util::sync::CancellationToken::new();
    let eof = stop.clone();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buffer = [0; 1024];
        while matches!(stdin.read(&mut buffer), Ok(1..)) {}
        eof.cancel();
    });
    follow_output(
        pane,
        &operation,
        ledger,
        &stop,
        &mut std::io::stdout().lock(),
    )
}

fn follow_output(
    pane: &str,
    operation: &PeerOperation,
    ledger: &crate::ledger::Ledger,
    stop: &tokio_util::sync::CancellationToken,
    output: &mut impl std::io::Write,
) -> crate::error::Result<()> {
    let client = ipc::Client::at(&ledger.dir);
    match operation {
        PeerOperation::Event => follow_subscription(client.detail(pane)?, stop, output, |frame| {
            event_reply(frame, pane)
        }),
        PeerOperation::Activity => match client.activity_subscribe(pane) {
            Ok(subscription) => follow_subscription(subscription, stop, output, |view| {
                Ok(PeerReply {
                    status: 200,
                    body: serde_json::to_value(view)?,
                })
            }),
            Err(error) => {
                let response = super::activity::failure(error.clone());
                let reply = PeerReply {
                    status: response.status().as_u16(),
                    body: serde_json::to_value(error)?,
                };
                let _ = write_follow_line(output, &serde_json::to_vec(&reply)?);
                Ok(())
            }
        },
        _ => Err(crate::error::Error::Invalid(
            "unsupported peer stream".into(),
        )),
    }
}

fn follow_subscription<T: ipc::Wire>(
    mut subscription: ipc::Subscription<T>,
    stop: &tokio_util::sync::CancellationToken,
    output: &mut impl std::io::Write,
    reply: impl Fn(&T) -> crate::error::Result<PeerReply>,
) -> crate::error::Result<()> {
    let mut previous = None;
    let mut current = subscription.current();
    while !stop.is_cancelled() {
        let line = serde_json::to_vec(&reply(&current.publication)?)?;
        if previous.as_ref() != Some(&line) {
            if write_follow_line(output, &line).is_err() {
                break;
            }
            previous = Some(line);
        }
        if matches!(current.status, ipc::Status::Disconnected(_)) {
            break;
        }
        current = subscription.changed(Duration::from_millis(100));
    }
    Ok(())
}

fn write_follow_line(output: &mut impl std::io::Write, line: &[u8]) -> std::io::Result<()> {
    output.write_all(line)?;
    output.write_all(b"\n")?;
    output.flush()
}

fn event_reply(publication: &DetailFrame, pane: &str) -> crate::error::Result<PeerReply> {
    Ok(if publication.found {
        PeerReply {
            status: 200,
            body: serde_json::from_str(&agent_event(publication, pane)?)?,
        }
    } else {
        PeerReply {
            status: 404,
            body: json!({"error": format!("no agent in pane {pane}")}),
        }
    })
}

/// Execute on the named peer, with the same serve-owned read boundary as local HTTP.
async fn peer_read(
    state: AppState,
    pane: String,
    operation: PeerOperation,
) -> crate::error::Result<PeerReply> {
    let client = ipc::Client::at(&state.web.dir);
    let response = match operation {
        PeerOperation::Conversation => {
            detail_response(
                state,
                pane,
                |detail| matches!(detail.conversation, Content::Pending),
                |detail| content_response(&detail.conversation),
            )
            .await
        }
        PeerOperation::Screen => {
            detail_response(
                state,
                pane,
                |detail| matches!(detail.screen, Content::Pending),
                |detail| content_response(&detail.screen),
            )
            .await
        }
        PeerOperation::Files => {
            detail_response(
                state,
                pane,
                |detail| matches!(detail.files, portal_types::files::FilesState::Pending),
                |detail| Json(&detail.files).into_response(),
            )
            .await
        }
        PeerOperation::Event => {
            let subscription = client.detail(&pane)?;
            let current = subscription.current();
            let reply = event_reply(&current.publication, &pane)?;
            (
                StatusCode::from_u16(reply.status).unwrap_or(StatusCode::BAD_GATEWAY),
                Json(reply.body),
            )
                .into_response()
        }
        PeerOperation::Activity => match client.activity(&pane) {
            Ok(view) => Json(view).into_response(),
            Err(error) => super::activity::failure(error),
        },
        PeerOperation::Settings => {
            super::settings::local(state, pane, crate::observation::settings::Command::Current)
                .await
        }
        PeerOperation::ChangeSettings(change) => {
            super::settings::local(
                state,
                pane,
                crate::observation::settings::Command::Change(change),
            )
            .await
        }
        PeerOperation::Register(body) => activity_command(
            &client,
            &pane,
            crate::observation::live_activity::Command::Register(body),
        ),
        PeerOperation::Publish(body) => activity_command(
            &client,
            &pane,
            crate::observation::live_activity::Command::Publish(body),
        ),
    };
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(
        response.into_body(),
        crate::observation::details::CODEC.wire_limit(),
    )
    .await
    .map_err(|error| crate::error::Error::Invalid(error.to_string()))?;
    Ok(PeerReply {
        status,
        body: serde_json::from_slice(&bytes)?,
    })
}

fn activity_command(
    client: &ipc::Client,
    pane: &str,
    command: crate::observation::live_activity::Command,
) -> Response {
    match client.activity_command(pane, command) {
        Ok(ack) => Json(ack).into_response(),
        Err(error) => super::activity::failure(error),
    }
}

fn peer_call(
    state: &AppState,
    host: &str,
    pane: &str,
    operation: &PeerOperation,
) -> crate::error::Result<PeerReply> {
    let launch = state
        .leads
        .as_ref()
        .ok_or_else(|| crate::error::Error::Invalid("peer transport unavailable".into()))?;
    let policy = crate::config::load(&launch.config_path)?;
    let fleet = crate::hosts::ActionFleet::current(
        launch.shell.as_ref(),
        &state.web,
        &policy,
        launch.home.clone(),
    )?;
    let target = fleet.find(host)?;
    let started = Instant::now();
    let mut reply: PeerReply = fleet.remote(
        target,
        &[
            "agent-detail".into(),
            pane.into(),
            "--input".into(),
            serde_json::to_string(operation)?,
        ],
    )?;
    if reply.status == 200 && matches!(operation, PeerOperation::Activity) {
        let mut view: portal_types::agents::ActivityView =
            serde_json::from_value(reply.body.clone())?;
        if let portal_types::agents::ActivityDetail::Available { valid_for_ms, .. } =
            &mut view.detail
        {
            *valid_for_ms = valid_for_ms
                .saturating_sub(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
        }
        reply.body = serde_json::to_value(view)?;
    }
    Ok(reply)
}

fn peer_error(host: &str, error: &crate::error::Error) -> Response {
    (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("host {host}: {}", crate::hosts::unreachable(crate::hosts::Failure::of(error)))}))).into_response()
}

pub(super) async fn peer_response(
    state: AppState,
    host: String,
    pane: String,
    operation: PeerOperation,
) -> Response {
    let label = host.clone();
    let files = matches!(operation, PeerOperation::Files);
    match tokio::task::spawn_blocking(move || peer_call(&state, &host, &pane, &operation)).await {
        Ok(Ok(reply)) => peer_reply_response(&label, reply),
        Ok(Err(error))
            if files
                && matches!(
                    error.root(),
                    crate::error::Error::Ssh {
                        refusal: Some(crate::error::Refusal::AgentFilesUnsupported),
                        ..
                    }
                ) =>
        {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("host {label}: files unsupported")})),
            )
                .into_response()
        }
        Ok(Err(error)) => peer_error(&label, &error),
        Err(_) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("host {label}: peer transport unavailable")})),
        )
            .into_response(),
    }
}

fn peer_reply_response(label: &str, mut reply: PeerReply) -> Response {
    let status = StatusCode::from_u16(reply.status).unwrap_or(StatusCode::BAD_GATEWAY);
    if status.is_server_error() {
        let error = json!(format!("host {label}: agent detail unavailable"));
        if let Some(body) = reply.body.as_object_mut() {
            body.insert("error".into(), error);
        } else {
            reply.body = json!({"error": error});
        }
    }
    (status, [("cache-control", "no-store")], Json(reply.body)).into_response()
}

enum MachineAction {
    Prompt {
        pane: String,
        text: String,
        reply: bool,
    },
    Keys {
        pane: String,
        keys: Vec<String>,
    },
    Focus {
        pane: String,
    },
    Close {
        pane: String,
    },
}

async fn machine_action(state: AppState, host: String, action: MachineAction) -> Response {
    let label = host.clone();
    let closing = matches!(&action, MachineAction::Close { .. });
    let outcome = tokio::task::spawn_blocking(move || -> crate::error::Result<Response> {
        let launch = state
            .leads
            .as_ref()
            .ok_or_else(|| crate::error::Error::Invalid("peer transport unavailable".into()))?;
        let policy = crate::config::load(&launch.config_path)?;
        let fleet = crate::hosts::ActionFleet::current(
            launch.shell.as_ref(),
            &state.web,
            &policy,
            launch.home.clone(),
        )?;
        let target = fleet.find(&host)?;
        let mut command = vec![
            "--machine".into(),
            target.label.clone(),
            if closing { "pane" } else { "agent" }.into(),
        ];
        match action {
            MachineAction::Prompt { pane, text, reply } => {
                let mut get = command.clone();
                get.extend(["get".into(), pane.clone()]);
                let output = launch.shell.run_ok("herdr", &get)?;
                let agent: Value = serde_json::from_str(&output.stdout)?;
                let status = agent
                    .pointer("/result/agent/agent_status")
                    .and_then(Value::as_str);
                if let Some(response) = prompt_refusal(&host, status, reply) {
                    return Ok(response);
                }
                command.extend(["prompt".into(), pane, text]);
            }
            MachineAction::Keys { pane, keys } => {
                command.extend(["send-keys".into(), pane]);
                command.extend(keys);
            }
            MachineAction::Focus { pane } => command.extend(["focus".into(), pane]),
            MachineAction::Close { pane } => command.extend(["close".into(), pane]),
        }
        launch.shell.run_ok("herdr", &command)?;
        Ok(StatusCode::ACCEPTED.into_response())
    })
    .await;
    match outcome {
        Ok(Ok(response)) => response,
        Ok(Err(error)) if closing && matches!(error, crate::error::Error::Command { .. }) => {
            error_response(&error)
        }
        Ok(Err(error)) => peer_error(&label, &error),
        Err(_) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("host {label}: peer transport unavailable")})),
        )
            .into_response(),
    }
}

/// Share a held peer stream without observing unrelated portal publications.
pub(super) async fn peer_events(
    state: AppState,
    host: String,
    pane: String,
    operation: PeerOperation,
    event: &'static str,
) -> Response {
    let mut lines = match state
        .followers
        .subscribe(&state, host.clone(), pane, &operation)
    {
        Ok(lines) => lines,
        Err(error) => return peer_error(&host, &error),
    };
    let first = tokio::select! {
        () = state.stop.cancelled() => Err(format!("host {host}: agent stream unavailable")),
        result = tokio::time::timeout(crate::budget::CALL_LIMIT, first_peer_line(&mut lines)) => {
            result.unwrap_or_else(|_| Err(format!("host {host}: agent stream unavailable")))
        }
    };
    let initial = match first {
        Ok(reply) if reply.status < 400 && StatusCode::from_u16(reply.status).is_ok() => reply,
        Ok(reply) => return peer_reply_response(&host, reply),
        Err(error) => {
            return (StatusCode::BAD_GATEWAY, Json(json!({"error":error}))).into_response();
        }
    };
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let stop = state.stop.clone();
    tokio::spawn(async move {
        let mut line = Ok(initial);
        loop {
            let (name, body, failed) = match line {
                Ok(mut reply) => {
                    reply.status = StatusCode::from_u16(reply.status)
                        .unwrap_or(StatusCode::BAD_GATEWAY)
                        .as_u16();
                    if reply.status >= 500 {
                        reply.body =
                            json!({"error":format!("host {host}: agent detail unavailable")});
                    }
                    (
                        if reply.status >= 400 { "error" } else { event },
                        reply.body,
                        reply.status >= 400,
                    )
                }
                Err(error) => ("error", json!({"error":error}), true),
            };
            if sender.send((name, body.to_string())).await.is_err() || failed {
                break;
            }
            tokio::select! {
                () = stop.cancelled() => break,
                () = sender.closed() => break,
                changed = lines.changed() => if changed.is_err() { break; },
            }
            let Some(next) = lines.borrow_and_update().clone() else {
                break;
            };
            line = next;
        }
    });
    let stream = tokio_stream::wrappers::ReceiverStream::new(receiver).map(|(event, data)| {
        Ok::<_, std::convert::Infallible>(
            axum::response::sse::Event::default()
                .event(event)
                .data(data),
        )
    });
    axum::response::sse::Sse::new(state.until_shutdown(stream))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

async fn first_peer_line(
    lines: &mut tokio::sync::watch::Receiver<Option<super::peer_stream::Line>>,
) -> super::peer_stream::Line {
    loop {
        if let Some(line) = lines.borrow_and_update().clone() {
            return line;
        }
        if lines.changed().await.is_err() {
            return Err("agent stream unavailable".into());
        }
    }
}

/// The routes of the agent API.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/agents", get(list))
        .route("/api/agents/{host}/{pane}/conversation", get(conversation))
        .route("/api/agents/{host}/{pane}/screen", get(screen))
        .route("/api/agents/{host}/{pane}/files", get(files))
        .route("/api/agents/{host}/{pane}/prompt", post(prompt))
        .route("/api/agents/{host}/{pane}/keys", post(keys))
        .route("/api/agents/{host}/{pane}/focus", post(focus))
        .route("/api/agents/{host}/{pane}/close", post(close))
        .route("/api/agents/{host}/{pane}/events", get(events))
        .route("/api/leads", post(create_lead))
        .route("/api/repos", get(repos))
        .route("/api/models", get(models))
}

/// The serve-owned GitHub catalogue, without configuration or shell reads.
async fn repos(State(state): State<AppState>) -> Response {
    let directory = state.web.dir.clone();
    match tokio::task::spawn_blocking(move || {
        let published = ipc::Client::at(&directory).current(&ipc::Select::views(["repos"]))?;
        crate::observation::read::view::<portal_types::agents::RepoList>(&published, "repos")
    })
    .await
    {
        Ok(Ok(list)) => ([("cache-control", "no-store")], Json(list)).into_response(),
        Ok(Err(error)) => super::observation_error(&error.to_string()),
        Err(error) => super::observation_error(&format!("observation unavailable: {error}")),
    }
}

/// The serve-owned Pi model catalogue, without launching Pi from a request.
async fn models(State(state): State<AppState>) -> Response {
    let directory = state.web.dir.clone();
    match tokio::task::spawn_blocking(move || {
        let published = ipc::Client::at(&directory).current(&ipc::Select::views(["models"]))?;
        crate::observation::read::view::<portal_types::models::ModelCatalogue>(&published, "models")
    })
    .await
    {
        Ok(Ok(list)) => ([("cache-control", "no-store")], Json(list)).into_response(),
        Ok(Err(error)) => super::observation_error(&error.to_string()),
        Err(error) => super::observation_error(&format!("observation unavailable: {error}")),
    }
}

/// Start a lead through the same creator as `dogsled lead`. 201 only after its first
/// prompt was accepted; a partial launch keeps its pane and reports its phase.
async fn create_lead(
    State(state): State<AppState>,
    body: std::result::Result<
        Json<portal_types::agents::CreateLeadBody>,
        axum::extract::rejection::JsonRejection,
    >,
) -> Response {
    let Some(launch) = state.leads.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "this server cannot start leads"})),
        )
            .into_response();
    };
    let Json(body) = match body {
        Ok(body) => body,
        Err(rejection) => return refuse(&rejection.body_text()),
    };
    let socket = socket(&state).cloned();
    let ledger = state.web.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let mut ledger = ledger;
        ledger.budget =
            crate::budget::Budget::until(std::time::Instant::now() + crate::budget::SETUP_LIMIT);
        let context = crate::leads::CreateContext {
            shell: launch.shell.as_ref(),
            ledger: &ledger,
            config_path: &launch.config_path,
            home: &launch.home,
            herdr: socket.as_ref(),
        };
        crate::leads::create(
            &context,
            &body.repo,
            &body.prompt,
            body.model,
            body.thinking,
        )
    })
    .await;
    match outcome {
        Ok(Ok(started)) => (
            StatusCode::CREATED,
            Json(portal_types::agents::CreatedLead {
                host: started.host,
                pane: started.pane,
            }),
        )
            .into_response(),
        Ok(Err(failure)) => lead_error_response(&failure),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error.to_string()})),
        )
            .into_response(),
    }
}

fn lead_error_response(failure: &crate::leads::Failure) -> Response {
    let status = match failure.kind {
        crate::leads::FailureKind::Invalid => StatusCode::BAD_REQUEST,
        crate::leads::FailureKind::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        crate::leads::FailureKind::Launch => StatusCode::BAD_GATEWAY,
    };
    (
        status,
        Json(json!({
            "error": failure.error.to_string(),
            "id": failure.id,
            "pane": failure.pane,
            "phase": failure.phase,
        })),
    )
        .into_response()
}

/// The client the agent routes talk to Herdr through, if there is one.
fn socket(state: &AppState) -> Option<&Client> {
    state
        .agents
        .get_or_init(|| Client::discover().ok().map(std::sync::Arc::new))
        .as_deref()
}

/// The agents from the same publication as the portal board and inbox.
async fn list(State(state): State<AppState>) -> Response {
    state.projection(|snapshot| {
        snapshot
            .agents
            .clone()
            .map(|agents| portal_types::agents::AgentList { agents })
    })
}

fn view_of(
    agent: &Value,
    title: Option<String>,
    repository: Option<String>,
    blocked: Option<portal_types::agents::BlockedPrompt>,
) -> AgentView {
    AgentView {
        host: crate::hosts::LOCAL.into(),
        pane: agent["pane_id"].as_str().unwrap_or_default().to_owned(),
        name: agent["name"].as_str().map(str::to_owned),
        title,
        agent: agent["agent"].as_str().map(str::to_owned),
        state: agent["agent_status"]
            .as_str()
            .unwrap_or("unknown")
            .to_owned(),
        cwd: agent["cwd"].as_str().map(str::to_owned),
        repository,
        blocked,
    }
}

/// One agent's turns, captured by serve for this connection's interest.
async fn conversation(
    State(state): State<AppState>,
    Path((host, pane)): Path<(String, String)>,
) -> Response {
    if host != crate::hosts::LOCAL {
        return peer_response(state, host, pane, PeerOperation::Conversation).await;
    }
    detail_response(
        state,
        pane,
        |detail| matches!(detail.conversation, Content::Pending),
        |detail| content_response(&detail.conversation),
    )
    .await
}

/// The last published screen, including its native revision.
async fn screen(
    State(state): State<AppState>,
    Path((host, pane)): Path<(String, String)>,
) -> Response {
    if host != crate::hosts::LOCAL {
        return peer_response(state, host, pane, PeerOperation::Screen).await;
    }
    detail_response(
        state,
        pane,
        |detail| matches!(detail.screen, Content::Pending),
        |detail| content_response(&detail.screen),
    )
    .await
}

/// The checkout state has its own capture and does not wait for other detail parts.
async fn files(
    State(state): State<AppState>,
    Path((host, pane)): Path<(String, String)>,
) -> Response {
    if host != crate::hosts::LOCAL {
        return peer_response(state, host, pane, PeerOperation::Files).await;
    }
    detail_response(
        state,
        pane,
        |detail| matches!(detail.files, portal_types::files::FilesState::Pending),
        |detail| Json(&detail.files).into_response(),
    )
    .await
}

fn content_response<T: serde::Serialize>(content: &Content<T>) -> Response {
    match content {
        Content::Available(content) => Json(content).into_response(),
        Content::Unavailable(error) => crate::web::observation_error(error),
        Content::Failed(error) => {
            (StatusCode::BAD_GATEWAY, Json(json!({"error":error}))).into_response()
        }
        Content::Pending => crate::web::observation_error("agent detail pending"),
    }
}

async fn detail_response(
    state: AppState,
    pane: String,
    pending: fn(&Detail) -> bool,
    select: fn(&Detail) -> Response,
) -> Response {
    match tokio::task::spawn_blocking(move || {
        let mut subscription = ipc::Client::at(&state.web.dir).detail(&pane)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let connection = subscription.current();
            if !connection.publication.found {
                return Ok(not_found(&pane));
            }
            if let ipc::Status::Disconnected(error) = connection.status {
                return Err(crate::error::Error::Invalid(error));
            }
            let detail = &connection.publication.detail;
            if !pending(detail) {
                let response = select(detail);
                return Ok(stamped(response, &connection.publication));
            }
            if state.stop.is_cancelled() || Instant::now() >= deadline {
                return Err(crate::error::Error::Invalid(
                    "agent detail unavailable".into(),
                ));
            }
            subscription.changed(Duration::from_millis(100));
        }
    })
    .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => crate::web::observation_error(&error.to_string()),
        Err(error) => crate::web::observation_error(&error.to_string()),
    }
}

fn stamped(mut response: Response, publication: &DetailFrame) -> Response {
    for (name, value) in [
        ("x-observation-epoch", publication.epoch.clone()),
        ("x-observation-revision", publication.revision.to_string()),
        ("cache-control", "no-store".into()),
    ] {
        match value.parse() {
            Ok(value) => {
                response.headers_mut().insert(name, value);
            }
            Err(error) => {
                return crate::web::observation_error(&format!(
                    "invalid observation stamp: {error}"
                ));
            }
        }
    }
    response
}

/// Refuse unrelated text while blocked, or any prompt without a readable live status.
fn prompt_refusal(host: &str, status: Option<&str>, reply: bool) -> Option<Response> {
    match status.filter(|status| !status.is_empty()) {
        None => Some((
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("host {host}: agent status unavailable")})),
        ).into_response()),
        Some("blocked") if !reply => Some((
            StatusCode::CONFLICT,
            Json(json!({"error": "The agent needs a response. Select its prompt in Waiting on you to reply."})),
        ).into_response()),
        Some(_) => None,
    }
}

/// Send a prompt to an agent.
/// Herdr's `agent.prompt` has no conditional form: an agent that blocks between
/// the check and the send still receives the text.
async fn prompt(
    State(state): State<AppState>,
    Path((host, pane)): Path<(String, String)>,
    Json(body): Json<PromptBody>,
) -> Response {
    let text = body.text.trim();
    if text.is_empty() {
        return refuse("the prompt is empty");
    }
    if host != crate::hosts::LOCAL {
        return machine_action(
            state,
            host,
            MachineAction::Prompt {
                pane,
                text: text.into(),
                reply: body.reply,
            },
        )
        .await;
    }
    let Some(socket) = socket(&state) else {
        return no_socket();
    };
    let agents = match socket.agent_list() {
        Ok(agents) => agents,
        Err(error) => return peer_error(&host, &error),
    };
    let Some(agent) = agents
        .iter()
        .find(|agent| agent["pane_id"].as_str() == Some(&pane))
    else {
        return not_found(&pane);
    };
    if let Some(response) = prompt_refusal(&host, agent["agent_status"].as_str(), body.reply) {
        return response;
    }
    match socket.agent_prompt(&pane, text) {
        Ok(_) => StatusCode::ACCEPTED.into_response(),
        Err(error) => error_response(&error),
    }
}

/// Send key presses to an agent. Only the allowed keys are accepted.
async fn keys(
    State(state): State<AppState>,
    Path((host, pane)): Path<(String, String)>,
    Json(body): Json<KeysBody>,
) -> Response {
    if body.keys.is_empty() {
        return refuse("no key was given");
    }
    let mut keys = Vec::with_capacity(body.keys.len());
    for key in &body.keys {
        let lower = key.to_lowercase();
        if !ALLOWED_KEYS.contains(&lower.as_str()) {
            return refuse(&format!("the key `{key}` is not allowed"));
        }
        // A chord is case-insensitive in Herdr, so send the canonical form. A
        // plain key keeps its case, so `Y` still types an upper-case letter.
        keys.push(if lower.contains('+') {
            lower
        } else {
            key.clone()
        });
    }
    if host != crate::hosts::LOCAL {
        return machine_action(state, host, MachineAction::Keys { pane, keys }).await;
    }
    let Some(socket) = socket(&state) else {
        return no_socket();
    };
    match socket.agent_send_keys(&pane, &keys) {
        Ok(_) => StatusCode::ACCEPTED.into_response(),
        Err(error) => error_response(&error),
    }
}

/// Focus an agent.
async fn focus(
    State(state): State<AppState>,
    Path((host, pane)): Path<(String, String)>,
) -> Response {
    if host != crate::hosts::LOCAL {
        return machine_action(state, host, MachineAction::Focus { pane }).await;
    }
    let Some(socket) = socket(&state) else {
        return no_socket();
    };
    match socket.agent_focus(&pane) {
        Ok(_) => StatusCode::ACCEPTED.into_response(),
        Err(error) => error_response(&error),
    }
}

/// Close the selected terminal, not its Dogsled work or sibling panes.
async fn close(
    State(state): State<AppState>,
    Path((host, pane)): Path<(String, String)>,
) -> Response {
    if host != crate::hosts::LOCAL {
        return machine_action(state, host, MachineAction::Close { pane }).await;
    }
    let Some(socket) = socket(&state) else {
        return no_socket();
    };
    let agents = match socket.agent_list() {
        Ok(agents) => agents,
        Err(error) => return error_response(&error),
    };
    if !agents
        .iter()
        .any(|agent| agent["pane_id"].as_str() == Some(&pane))
    {
        return not_found(&pane);
    }
    match socket.close_pane(&pane) {
        Ok(_) => StatusCode::ACCEPTED.into_response(),
        Err(error) => error_response(&error),
    }
}

/// Forward serve-owned detail changes. The observation connection owns the interest.
async fn events(
    State(state): State<AppState>,
    Path((host, pane)): Path<(String, String)>,
) -> Response {
    if host != crate::hosts::LOCAL {
        return peer_events(state, host, pane, PeerOperation::Event, "agent").await;
    }
    let client = ipc::Client::at(&state.web.dir);
    let selected = pane.clone();
    let mut subscription = match tokio::task::spawn_blocking(move || client.detail(&selected)).await
    {
        Ok(Ok(subscription)) => subscription,
        Ok(Err(error)) => return crate::web::observation_error(&error.to_string()),
        Err(error) => return crate::web::observation_error(&error.to_string()),
    };
    let (sender, receiver) = tokio::sync::watch::channel(None::<String>);
    let stop = state.stop.clone();
    tokio::task::spawn_blocking(move || {
        let mut previous = None;
        loop {
            if stop.is_cancelled() || sender.is_closed() {
                break;
            }
            let connection = subscription.current();
            if let ipc::Status::Disconnected(error) = connection.status {
                sender.send_replace(Some(json!({"error": error}).to_string()));
                break;
            }
            match agent_event(&connection.publication, &pane) {
                Ok(payload) => {
                    if previous.as_ref() != Some(&payload) {
                        sender.send_replace(Some(payload.clone()));
                        previous = Some(payload);
                    }
                }
                Err(error) => {
                    sender.send_replace(Some(json!({"error": error.to_string()}).to_string()));
                    break;
                }
            }
            subscription.changed(Duration::from_millis(100));
        }
    });
    let stream = tokio_stream::wrappers::WatchStream::new(receiver).filter_map(|payload| {
        payload.map(|payload| {
            Ok::<_, std::convert::Infallible>(
                axum::response::sse::Event::default()
                    .event("agent")
                    .data(payload),
            )
        })
    });
    axum::response::sse::Sse::new(state.until_shutdown(stream))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

fn agent_event(publication: &DetailFrame, pane: &str) -> crate::error::Result<String> {
    let agent = publication.agent.as_ref();
    Ok(serde_json::to_string(&AgentEvent {
        revisions: publication.parts.clone(),
        pending: Some(portal_types::agents::PendingParts {
            conversation: matches!(publication.detail.conversation, Content::Pending),
            screen: matches!(publication.detail.screen, Content::Pending),
            files: matches!(
                publication.detail.files,
                portal_types::files::FilesState::Pending
            ),
        }),
        kind: "pane.agent_status_changed".into(),
        pane: pane.into(),
        state: Some(agent.map_or_else(
            || publication.detail.state.clone(),
            |agent| agent.state.clone(),
        )),
        blocked: agent.and_then(|agent| agent.blocked.clone()),
        session: if publication.detail.session.is_some() {
            portal_types::agents::DetailSession::Available(publication.detail.key()?)
        } else {
            portal_types::agents::DetailSession::Unavailable
        },
    })?)
}

#[derive(Debug, Default)]
pub(crate) struct Titles(std::collections::BTreeMap<String, (u64, Option<String>)>);

impl Titles {
    pub(crate) fn read(&mut self, agent: &Value) -> Option<String> {
        let harness = agent["agent"].as_str()?;
        let transcript = agent["agent_session"]["value"].as_str()?;
        let path = crate::transcript::resolve(Some(harness), kind_of(agent), transcript)?;
        let size = std::fs::metadata(&path).ok()?.len();
        if let Some((seen, title)) = self.0.get(&path)
            && (title.is_some() || *seen == size)
        {
            return title.clone();
        }
        let title = crate::activity::title(harness, &path);
        self.0.insert(path, (size, title.clone()));
        title
    }
}

/// Producer input uses the already captured agent set and host-owned repository facts.
pub(crate) fn capture(
    agents: &[crate::probe::Agent],
    titles: &mut Titles,
) -> crate::error::Result<Vec<AgentView>> {
    let socket = if agents.iter().any(|agent| agent.state == "blocked") {
        Some(Client::discover()?)
    } else {
        None
    };
    capture_with(agents, titles, socket.as_ref())
}

pub(crate) fn capture_with(
    agents: &[crate::probe::Agent],
    titles: &mut Titles,
    socket: Option<&Client>,
) -> crate::error::Result<Vec<AgentView>> {
    agents
        .iter()
        .map(|agent| {
            let blocked = if agent.state == "blocked" {
                let socket = socket.ok_or_else(|| {
                    crate::error::Error::Invalid("agent screen unavailable".into())
                })?;
                let read = socket.pane_read(&agent.pane, "visible", Some(SCREEN_LINES))?;
                let text = read["text"].as_str().ok_or_else(|| {
                    crate::error::Error::Invalid("agent screen has no text".into())
                })?;
                crate::activity::blocked_prompt(text)
            } else {
                None
            };
            let raw = json!({"pane_id":agent.pane,"name":agent.name,"agent":agent.agent,
            "agent_status":agent.state,"cwd":agent.cwd,
            "agent_session":{"kind":"path","value":agent.transcript}});
            Ok(view_of(
                &raw,
                titles.read(&raw),
                agent.repository.clone(),
                blocked,
            ))
        })
        .collect()
}

fn kind_of(agent: &Value) -> &str {
    agent["agent_session"]["kind"].as_str().unwrap_or("path")
}

/// The agent is not there.
fn not_found(pane: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"error": format!("no agent in pane {pane}")})),
    )
        .into_response()
}

/// The request was refused.
fn refuse(detail: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": detail}))).into_response()
}

/// Herdr's socket is not there, so the API cannot reach the agents.
fn no_socket() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "herdr socket is not available"})),
    )
        .into_response()
}

/// A Herdr failure, as the API reports it.
fn error_response(error: &crate::error::Error) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({"error": error.to_string()})),
    )
        .into_response()
}

#[cfg(test)]
pub(crate) async fn test_request(
    state: AppState,
    request: axum::http::Request<axum::body::Body>,
) -> Response {
    use tower::ServiceExt as _;
    crate::web::router(state)
        .oneshot(request)
        .await
        .expect("portal route")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr_socket::fake;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use std::sync::Arc;
    use tokio::sync::watch;
    use tower::ServiceExt;

    const PORT: u16 = 7465;

    #[test]
    fn repository_uses_the_board_origin_identity() {
        let shell = crate::shell::Fake::new();
        shell.on(
            "git -C /checkout remote get-url origin",
            "git@GitHub.com:owner/repo.git\n",
        );
        assert_eq!(
            crate::facts::repository(&shell, "/checkout").as_deref(),
            Some("github.com/owner/repo")
        );
        shell.on("git -C /checkout remote get-url origin", "/local/repo");
        assert_eq!(crate::facts::repository(&shell, "/checkout"), None);
    }

    /// The state of the API, with or without a Herdr socket.
    fn state(socket: Option<Client>) -> AppState {
        let (_sender, receiver) = watch::channel(crate::web::PortalState::default());
        AppState::new(
            receiver,
            PORT,
            socket.map(Arc::new),
            crate::web::guard::Token::new(String::new()),
            crate::ledger::Ledger::new(
                std::env::temp_dir().join(format!("dogsled-agents-{}", crate::ledger::new_id())),
            )
            .clone(),
        )
    }

    /// The state of the API, talking to a fake Herdr.
    fn with_socket(server: &fake::Server) -> AppState {
        state(Some(server.client()))
    }

    async fn post_lead(state: AppState, body: &str) -> (StatusCode, serde_json::Value) {
        let response = state.oneshot_lead(body).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or_default())
    }

    impl AppState {
        async fn oneshot_lead(self, body: &str) -> Response {
            super::test_request(self, request(Method::POST, "/api/leads", Some(body))).await
        }
    }

    fn launch(config: &str) -> crate::web::LeadLaunch {
        let dir = std::env::temp_dir().join(format!("dogsled-lead-{}", crate::ledger::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), config).unwrap();
        crate::web::LeadLaunch {
            config_path: dir.join("config.toml"),
            home: dir.display().to_string(),
            shell: Arc::new(crate::shell::Fake::new()),
        }
    }

    #[tokio::test]
    async fn lead_route_maps_capability_input_and_selector_failures() {
        let server = fake::Server::start(|_, _| json!({"result":{"agents":[]}}));
        // A one-shot server has no lead capability.
        let (status, body) = post_lead(with_socket(&server), r#"{"repo":"r","prompt":"go"}"#).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        let leads = || with_socket(&server).with_leads(launch(""));
        for bad in [
            r#"{"repo":"r"}"#,
            r#"{"repo":"r","prompt":"go","owner":"w1:p1"}"#,
            r#"{"repo":"r","prompt":"go","thinking":"invalid"}"#,
            r#"{"repo":"r","prompt":"go","model":{"provider":"p","id":"m","extra":true}}"#,
            "{",
        ] {
            let (status, body) = post_lead(leads(), bad).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
        }
        let (status, body) =
            post_lead(leads(), r#"{"repo":"https://example.org/r","prompt":"go"}"#).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body["error"].as_str().unwrap().contains("owner/name"),
            "{body}"
        );
        assert_eq!(body["id"], serde_json::Value::Null);
        // Invalid input is rejected even without a local Herdr socket.
        let (status, body) = post_lead(
            state(None).with_leads(launch("")),
            r#"{"repo":"r","prompt":"go"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        // Both request forms reach placement, rather than failing JSON validation.
        for input in [
            r#"{"repo":"owner/repo","prompt":"go"}"#,
            r#"{"repo":"owner/repo","prompt":"go","model":{"provider":"openai","id":"gpt-5"},"thinking":"high"}"#,
        ] {
            let (status, body) = post_lead(state(None).with_leads(launch("")), input).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        }
        assert!(!server.methods().iter().any(|m| m == "agent.prompt"));
    }

    #[tokio::test]
    async fn repos_route_requires_a_publication() {
        let response =
            super::test_request(state(None), request(Method::GET, "/api/repos", None)).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body["error"].as_str().unwrap().contains("unavailable"));
    }

    #[tokio::test]
    async fn models_route_requires_a_publication() {
        let response =
            super::test_request(state(None), request(Method::GET, "/api/models", None)).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = body_json(response).await;
        assert!(body["error"].as_str().unwrap().contains("unavailable"));
    }

    #[tokio::test]
    async fn models_route_reads_the_published_catalogue_with_its_error() {
        let state = state(None);
        let catalogue = json!({"models":[{
            "model":{"provider":"openai","id":"gpt-5"},"name":"GPT-5","thinking":["off","high"]
        }],"error":"last refresh failed"});
        let mut publisher = crate::observation::Publisher::new().unwrap();
        publisher
            .replace(crate::observation::Publication {
                published_ms: 1,
                sources: crate::observation::Sources::default(),
                local: crate::observation::Domains::new(),
                views: crate::observation::Domains::from([(
                    "models".into(),
                    json!({"data":catalogue,"gaps":[]}),
                )]),
                gaps: vec![],
            })
            .unwrap();
        let mut server = ipc::Server::new(&state.web.dir).unwrap();
        server.bind(publisher.subscribe()).unwrap();
        let directory = state.web.dir.clone();
        let response = super::test_request(state, request(Method::GET, "/api/models", None)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(body_json(response).await, catalogue);
        drop(server);
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// Capture native source records before installing a publication in the reader.
    fn with_published_agents(server: &fake::Server) -> AppState {
        let state = with_socket(server);
        let socket = server.client();
        let agents: Vec<_> = socket
            .agent_list()
            .expect("agent source")
            .iter()
            .map(|raw| crate::probe::Agent {
                pane: raw["pane_id"].as_str().unwrap_or_default().to_owned(),
                name: raw["name"].as_str().map(str::to_owned),
                agent: raw["agent"].as_str().map(str::to_owned),
                state: raw["agent_status"].as_str().unwrap_or_default().to_owned(),
                cwd: raw["cwd"].as_str().map(str::to_owned),
                transcript: raw["agent_session"]["value"].as_str().map(str::to_owned),
                ..crate::probe::Agent::default()
            })
            .collect();
        let views =
            capture_with(&agents, &mut Titles::default(), Some(&socket)).expect("capture agents");
        let board = crate::web::board::view(
            &crate::board::Board {
                hosts: vec![],
                rows: vec![],
            },
            &[],
            &[],
            7,
        );
        let publication = crate::web::test_support::publish(
            &mut crate::observation::Publisher::new().expect("publisher"),
            &state.web,
            board,
            views,
        )
        .expect("publish captured views");
        let (_sender, receiver) = watch::channel(publication);
        AppState {
            portal: receiver,
            ..state
        }
    }

    /// A request to the API.
    fn request(method: Method, uri: &str, body: Option<&str>) -> Request<Body> {
        let builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(axum::http::header::HOST, format!("127.0.0.1:{PORT}"))
            .header(
                axum::http::header::ORIGIN,
                format!("http://127.0.0.1:{PORT}"),
            );
        let builder = match body {
            Some(body) => builder
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_owned())),
            None => builder.body(Body::empty()),
        };
        builder.expect("the request")
    }

    /// One request against the agent routes.
    async fn call(state: AppState, request: Request<Body>) -> Response {
        crate::web::router(state)
            .oneshot(request)
            .await
            .expect("the router answers")
    }

    /// The body of a response, as JSON.
    async fn body_json(response: Response) -> serde_json::Value {
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("the body");
        serde_json::from_slice(&body).expect("the payload")
    }

    struct PeerFixture {
        state: AppState,
        shell: Arc<crate::shell::Fake>,
        _publisher: crate::observation::Publisher,
        server: Option<ipc::Server>,
    }

    impl PeerFixture {
        fn new() -> Self {
            let mut state = state(None);
            let shell = Arc::new(crate::shell::Fake::new());
            let config_path = state.web.dir.join("config.toml");
            std::fs::create_dir_all(&state.web.dir).unwrap();
            std::fs::write(
                &config_path,
                "remote_command = ['mise', 'x', '--', 'dogsled']\n",
            )
            .unwrap();
            state = state.with_leads(crate::web::LeadLaunch {
                config_path,
                home: "/home/test".into(),
                shell: shell.clone(),
            });
            let mut publisher = crate::observation::Publisher::new().unwrap();
            publisher.replace(crate::observation::Publication {
                published_ms: 1,
                sources: crate::observation::Sources::default(),
                local: crate::observation::Domains::new(),
                views: crate::observation::Domains::from([("hosts".into(), json!({"data": [{
                    "id":"peer-id", "label":"netcup", "target":"netcup-dev", "service":"up", "reachable":true,
                    "sampled_ms":1, "probe":null, "headroom":null, "error":null, "jobs":[]
                }], "gaps": []}))]),
                gaps: vec![],
            }).unwrap();
            let mut server = ipc::Server::new(&state.web.dir).unwrap();
            server.bind(publisher.subscribe()).unwrap();
            Self {
                state,
                shell,
                _publisher: publisher,
                server: Some(server),
            }
        }

        fn command(&self, pane: &str, operation: &PeerOperation) -> String {
            let policy = crate::config::Policy::default();
            let fleet = crate::hosts::Fleet::cold(
                self.shell.as_ref(),
                &self.state.web,
                &policy,
                "/home/test".into(),
                vec![],
            );
            let target = crate::hosts::Host {
                id: "peer-id".into(),
                label: "netcup".into(),
                target: Some("netcup-dev".into()),
            };
            format!(
                "ssh {}",
                fleet
                    .remote_argv(
                        &target,
                        &[
                            "agent-detail".into(),
                            pane.into(),
                            "--input".into(),
                            serde_json::to_string(operation).unwrap()
                        ]
                    )
                    .join(" ")
            )
        }
    }

    impl Drop for PeerFixture {
        fn drop(&mut self) {
            self.state.stop.cancel();
            self.server.take();
            std::fs::remove_dir_all(&self.state.web.dir).unwrap();
        }
    }

    #[tokio::test]
    async fn remote_conversation_and_actions_target_the_named_peer() {
        let fixture = PeerFixture::new();
        fixture.shell.on(
            "herdr --machine netcup agent get w1:p1",
            r#"{"result":{"agent":{"agent_status":"working"}}}"#,
        );
        fixture.shell.on(&fixture.command("w1:p1", &PeerOperation::Conversation),
            &json!({"status":200,"body":{"pane":"w1:p1","session":"peer-session","turns":[],"truncated":false}}).to_string());
        let local = agent_server();
        let mut state = fixture.state.clone();
        state.agents = Arc::new(std::sync::OnceLock::from(Some(Arc::new(local.client()))));
        let response = call(
            state.clone(),
            request(Method::GET, "/api/agents/netcup/w1:p1/conversation", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["session"], "peer-session");
        for (route, body, command) in [
            (
                "prompt",
                Some(r#"{"text":"go on"}"#),
                "herdr --machine netcup agent prompt w1:p1 go on",
            ),
            (
                "keys",
                Some(r#"{"keys":["enter","CTRL+C"]}"#),
                "herdr --machine netcup agent send-keys w1:p1 enter ctrl+c",
            ),
            ("focus", None, "herdr --machine netcup agent focus w1:p1"),
        ] {
            fixture.shell.on(command, "{}");
            let response = call(
                state.clone(),
                request(
                    Method::POST,
                    &format!("/api/agents/netcup/w1:p1/{route}"),
                    body,
                ),
            )
            .await;
            assert_eq!(response.status(), StatusCode::ACCEPTED, "{route}");
        }
        assert_eq!(local.methods(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn close_local_working_and_blocked_agents_without_cleanup() {
        struct Checkout(std::path::PathBuf);
        impl Drop for Checkout {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).unwrap();
            }
        }
        for status in ["working", "blocked"] {
            let checkout = Checkout(std::env::temp_dir().join(crate::ledger::new_id()));
            std::fs::create_dir(&checkout.0).unwrap();
            std::fs::write(checkout.0.join("work.txt"), "uncommitted work").unwrap();
            let cwd = checkout.0.clone();
            let server = fake::Server::start(move |method, params| match method {
                "agent.list" => json!({"result":{"agents":[
                    {"pane_id":"w1:p2", "agent_status":status, "cwd":cwd},
                    {"pane_id":"w1:p1", "agent_status":"working"}
                ]}}),
                "pane.close" => {
                    assert_eq!(*params, json!({"pane_id":"w1:p2"}));
                    json!({"result":{"type":"ok"}})
                }
                other => panic!("unexpected action {other}"),
            });
            let state = with_socket(&server);
            // A close must not even open the Dogsled ledger or create completion records.
            assert!(!state.web.dir.exists());
            let response = call(
                state.clone(),
                request(Method::POST, "/api/agents/local/w1:p2/close", None),
            )
            .await;
            assert_eq!(response.status(), StatusCode::ACCEPTED, "{status}");
            assert_eq!(server.methods(), ["agent.list", "pane.close"]);
            assert!(!state.web.dir.exists());
            assert_eq!(
                std::fs::read_to_string(checkout.0.join("work.txt")).unwrap(),
                "uncommitted work"
            );
        }
    }

    #[tokio::test]
    async fn close_reports_missing_panes_unavailable_sockets_and_native_refusals() {
        let missing = fake::Server::start(|method, _| {
            assert_eq!(method, "agent.list");
            json!({"result":{"agents":[]}})
        });
        let response = call(
            with_socket(&missing),
            request(Method::POST, "/api/agents/local/missing/close", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            body_json(response).await,
            json!({"error":"no agent in pane missing"})
        );
        assert_eq!(missing.methods(), ["agent.list"]);

        let response = call(
            state(None),
            request(Method::POST, "/api/agents/local/w1:p2/close", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            body_json(response).await,
            json!({"error":"herdr socket is not available"})
        );

        let refused = fake::Server::start(|method, _| match method {
            "agent.list" => json!({"result":{"agents":[{"pane_id":"w1:p2"}]}}),
            "pane.close" => {
                json!({"error":{"code":"confirmation_required", "message":"confirm worktree group"}})
            }
            other => panic!("unexpected action {other}"),
        });
        let response = call(
            with_socket(&refused),
            request(Method::POST, "/api/agents/local/w1:p2/close", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            body_json(response).await,
            json!({"error":"herdr pane.close: confirmation_required: confirm worktree group"})
        );
        assert_eq!(refused.methods(), ["agent.list", "pane.close"]);
    }

    #[tokio::test]
    async fn close_rejects_foreign_origins_before_any_action() {
        let server = fake::Server::start(|_, _| panic!("foreign origin reached Herdr"));
        let mut req = request(Method::POST, "/api/agents/local/w1:p2/close", None);
        req.headers_mut().insert(
            axum::http::header::ORIGIN,
            "http://evil.example".parse().unwrap(),
        );
        let response = call(with_socket(&server), req).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(server.methods(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn close_remote_targets_the_resolved_machine_without_local_fallback() {
        for success in [true, false] {
            let fixture = PeerFixture::new();
            let command = "herdr --machine netcup pane close w1:p2";
            fixture.shell.on_output(
                command,
                crate::shell::Output {
                    status: if success { 0 } else { 2 },
                    stdout: "{}".into(),
                    stderr: if success {
                        ""
                    } else {
                        "confirm worktree group"
                    }
                    .into(),
                },
            );
            let local = fake::Server::start(|_, _| panic!("remote close reached local Herdr"));
            let mut state = fixture.state.clone();
            state.agents = Arc::new(std::sync::OnceLock::from(Some(Arc::new(local.client()))));
            // Resolve the immutable peer id to its Herdr machine label.
            let response = call(
                state,
                request(Method::POST, "/api/agents/peer-id/w1:p2/close", None),
            )
            .await;
            assert_eq!(
                response.status(),
                if success {
                    StatusCode::ACCEPTED
                } else {
                    StatusCode::BAD_GATEWAY
                }
            );
            if !success {
                assert!(
                    body_json(response).await["error"]
                        .as_str()
                        .unwrap()
                        .contains("confirm worktree group")
                );
            }
            assert_eq!(*fixture.shell.calls.lock().unwrap(), [command]);
            assert_eq!(local.methods(), Vec::<String>::new());
        }
    }

    #[tokio::test]
    async fn remote_files_sends_the_files_operation_and_relays_its_reply() {
        let fixture = PeerFixture::new();
        assert_eq!(
            serde_json::to_value(PeerOperation::Files).unwrap(),
            json!({"operation":"files"})
        );
        let body = json!({"current":{"root":"/peer/checkout","head":{"branch":"main"},"entries":[],"captured_ms":123}});
        let command = fixture.command("w1:p1", &PeerOperation::Files);
        fixture
            .shell
            .on(&command, &json!({"status":200,"body":body}).to_string());
        let response = call(
            fixture.state.clone(),
            request(Method::GET, "/api/agents/netcup/w1:p1/files", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(body_json(response).await, body);
        assert_eq!(*fixture.shell.calls.lock().unwrap(), [command]);
    }

    #[tokio::test]
    async fn older_peer_files_is_unsupported_without_a_fallback() {
        let fixture = PeerFixture::new();
        let command = fixture.command("w1:p1", &PeerOperation::Files);
        fixture.shell.on_output(&command, crate::shell::Output {
            status: 2,
            stdout: json!({"error":"JSON: unknown variant `files`, expected one of `conversation`, `screen`, `event`, `activity`, `register`, `publish` at line 1 column 20"}).to_string(),
            stderr: String::new(),
        });
        let response = call(
            fixture.state.clone(),
            request(Method::GET, "/api/agents/netcup/w1:p1/files", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            body_json(response).await,
            json!({"error":"host netcup: files unsupported"})
        );
        assert_eq!(*fixture.shell.calls.lock().unwrap(), [command]);
    }

    #[tokio::test]
    async fn current_peer_files_failure_is_not_reported_as_unsupported() {
        let fixture = PeerFixture::new();
        let command = fixture.command("w1:p1", &PeerOperation::Files);
        for (status, category) in [(1, "unexpected output"), (2, "remote command failed")] {
            fixture.shell.on_output(
                &command,
                crate::shell::Output {
                    status,
                    stdout: json!({"error":"ledger missing"}).to_string(),
                    stderr: String::new(),
                },
            );
            let response = call(
                fixture.state.clone(),
                request(Method::GET, "/api/agents/netcup/w1:p1/files", None),
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
            assert_eq!(
                body_json(response).await,
                json!({"error":format!("host netcup: unreachable: {category}")})
            );
        }
        fixture.shell.on_output(
            &command,
            crate::shell::Output {
                status: 2,
                stdout: json!({"error":"serve failed","diagnostic":{"version":1}}).to_string(),
                stderr: String::new(),
            },
        );
        let response = call(
            fixture.state.clone(),
            request(Method::GET, "/api/agents/netcup/w1:p1/files", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            body_json(response).await,
            json!({"error":"host netcup: unreachable: other"})
        );
    }

    #[tokio::test]
    async fn files_transport_failure_keeps_its_transport_category() {
        let fixture = PeerFixture::new();
        fixture.shell.on_output(
            &fixture.command("w1:p1", &PeerOperation::Files),
            crate::shell::Output {
                status: 255,
                stdout: String::new(),
                stderr: "Connection refused".into(),
            },
        );
        let response = call(
            fixture.state.clone(),
            request(Method::GET, "/api/agents/netcup/w1:p1/files", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            body_json(response).await,
            json!({"error":"host netcup: unreachable: connection refused"})
        );
    }

    #[tokio::test]
    async fn a_remote_blocked_agent_only_receives_a_reply() {
        let fixture = PeerFixture::new();
        let get = "herdr --machine netcup agent get w1:p1";
        let prompt = "herdr --machine netcup agent prompt w1:p1 go on";
        fixture
            .shell
            .on(get, r#"{"result":{"agent":{"agent_status":"blocked"}}}"#);
        fixture.shell.on(prompt, "{}");
        let response = call(
            fixture.state.clone(),
            request(
                Method::POST,
                "/api/agents/netcup/w1:p1/prompt",
                Some(r#"{"text":"go on"}"#),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            body_json(response).await,
            json!({"error": "The agent needs a response. Select its prompt in Waiting on you to reply."})
        );
        assert_eq!(*fixture.shell.calls.lock().unwrap(), vec![get]);

        let response = call(
            fixture.state.clone(),
            request(
                Method::POST,
                "/api/agents/netcup/w1:p1/prompt",
                Some(r#"{"text":"go on","reply":true}"#),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(*fixture.shell.calls.lock().unwrap(), vec![get, get, prompt]);
    }

    #[tokio::test]
    async fn a_remote_prompt_requires_a_readable_status_even_for_a_reply() {
        for output in [
            "not json",
            "{}",
            r#"{"result":{"agent":{"agent_status":42}}}"#,
            r#"{"result":{"agent":{"agent_status":""}}}"#,
        ] {
            let fixture = PeerFixture::new();
            let get = "herdr --machine netcup agent get w1:p1";
            fixture.shell.on(get, output);
            let response = call(
                fixture.state.clone(),
                request(
                    Method::POST,
                    "/api/agents/netcup/w1:p1/prompt",
                    Some(r#"{"text":"go on","reply":true}"#),
                ),
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY, "{output}");
            assert!(
                body_json(response).await["error"]
                    .as_str()
                    .unwrap()
                    .contains("host netcup:")
            );
            assert_eq!(*fixture.shell.calls.lock().unwrap(), vec![get]);
        }
    }

    #[tokio::test]
    async fn remote_screen_activity_and_streams_forward_peer_publications() {
        let mut fixture = PeerFixture::new();
        fixture.state.followers = super::super::peer_stream::Followers::with_launcher("sh", vec![
            "-c".into(),
            "printf '%s\\n' '{\"status\":200,\"body\":{\"session\":\"peer-session\"}}'; cat >/dev/null".into(),
        ]);
        let activity = json!({"version":1,"pane":"w1:p1","epoch":"peer-serve","revision":2,
            "session":"peer-session","coarse_state":"working","producer":{"generation":1,"id":"producer"},
            "detail":{"availability":"available","sequence":1,"received_at_ms":1,"valid_for_ms":5000,
                "state":{"phase":"thinking","active_tools":[]}}});
        for (operation, body) in [
            (
                PeerOperation::Screen,
                json!({"pane":"w1:p1","session":"peer-session","text":"remote screen","revision":3}),
            ),
            (PeerOperation::Activity, activity),
            (
                PeerOperation::Event,
                json!({"kind":"pane.agent_status_changed","pane":"w1:p1","state":"working",
                "session":{"availability":"available","key":"peer-session"}}),
            ),
        ] {
            fixture.shell.on(
                &fixture.command("w1:p1", &operation),
                &json!({"status":200,"body":body}).to_string(),
            );
        }
        let response = call(
            fixture.state.clone(),
            request(Method::GET, "/api/agents/netcup/w1:p1/screen", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["text"], "remote screen");
        let response = call(
            fixture.state.clone(),
            request(Method::GET, "/api/agents/netcup/w1:p1/activity", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let payload = body_json(response).await;
        assert_eq!(payload["session"], "peer-session");
        assert!(payload["detail"]["valid_for_ms"].as_u64().unwrap() <= 5000);
        fixture.shell.calls.lock().unwrap().clear();
        let (publications, portal) = watch::channel(fixture.state.portal.borrow().clone());
        fixture.state.portal = portal;
        for (route, event) in [("events", "agent"), ("activity/events", "activity")] {
            let response = call(
                fixture.state.clone(),
                request(
                    Method::GET,
                    &format!("/api/agents/netcup/w1:p1/{route}"),
                    None,
                ),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let mut stream = response.into_body().into_data_stream();
            let bytes = tokio::time::timeout(crate::test_support::WAIT_DEADLINE, stream.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let frame = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(frame.contains(&format!("event: {event}")), "{frame}");
            assert!(frame.contains("peer-session"), "{frame}");
            publications.send_replace(super::super::PortalState::default());
            assert!(
                tokio::time::timeout(Duration::from_millis(100), stream.next())
                    .await
                    .is_err(),
                "unrelated portal publication must not emit an event"
            );
        }
        assert!(
            fixture
                .shell
                .calls
                .lock()
                .unwrap()
                .iter()
                .all(|call| !call.contains("agent-detail"))
        );
        let calls = fixture.state.followers.calls();
        assert_eq!(calls.len(), 2);
        for (argv, operation) in calls
            .iter()
            .zip([PeerOperation::Event, PeerOperation::Activity])
        {
            let expected =
                fixture
                    .command("w1:p1", &operation)
                    .replacen(" --input", " --follow --input", 1);
            assert_eq!(format!("ssh {}", argv.join(" ")), expected);
        }
    }

    #[tokio::test]
    async fn remote_stream_errors_preserve_initial_status_and_close_later_events() {
        for (script, status, error) in [
            (
                "exit 0".to_owned(),
                StatusCode::BAD_GATEWAY,
                "host netcup: agent stream unavailable",
            ),
            (
                "printf '%s\\n' '{\"status\":404,\"body\":{\"error\":\"no agent\"}}'".into(),
                StatusCode::NOT_FOUND,
                "no agent",
            ),
            (
                "printf '%s\\n' '{\"status\":503,\"body\":{\"error\":\"secret peer detail\"}}'"
                    .into(),
                StatusCode::SERVICE_UNAVAILABLE,
                "host netcup: agent detail unavailable",
            ),
        ] {
            let mut fixture = PeerFixture::new();
            fixture.state.followers = super::super::peer_stream::Followers::with_launcher(
                "sh",
                vec!["-c".into(), script],
            );
            let response = call(
                fixture.state.clone(),
                request(Method::GET, "/api/agents/netcup/w1:p1/events", None),
            )
            .await;
            assert_eq!(response.status(), status);
            assert_eq!(body_json(response).await["error"], error);
            assert!(
                fixture
                    .shell
                    .calls
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|call| !call.contains("agent-detail"))
            );
        }
        let release = std::env::temp_dir().join(crate::ledger::new_id());
        let mut fixture = PeerFixture::new();
        fixture.state.followers = super::super::peer_stream::Followers::with_launcher(
            "sh",
            vec![
                "-c".into(),
                format!(
                    "printf '%s\\n' '{{\"status\":200,\"body\":{{\"session\":\"peer-session\"}}}}'; while [ ! -f {} ]; do sleep 0.01; done; printf '%s\\n' '{{\"status\":409,\"body\":{{\"error\":\"session changed\"}}}}'; cat >/dev/null",
                    crate::shell::quote(&release.to_string_lossy())
                ),
            ],
        );
        let response = call(
            fixture.state.clone(),
            request(Method::GET, "/api/agents/netcup/w1:p1/events", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body().into_data_stream();
        let first = tokio::time::timeout(crate::test_support::WAIT_DEADLINE, stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&first).contains("event: agent"));
        std::fs::write(&release, "release").unwrap();
        let last = tokio::time::timeout(crate::test_support::WAIT_DEADLINE, stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let last = String::from_utf8_lossy(&last);
        assert!(last.contains("event: error"), "{last}");
        assert!(last.contains("session changed"), "{last}");
        assert!(
            tokio::time::timeout(crate::test_support::WAIT_DEADLINE, stream.next())
                .await
                .unwrap()
                .is_none()
        );
        std::fs::remove_file(release).unwrap();
    }

    #[tokio::test]
    async fn remote_settings_routes_read_and_apply_on_the_named_host() {
        let fixture = PeerFixture::new();
        let settings = crate::observation::settings::tests::selection();
        let change = portal_types::models::SettingsChange {
            model: None,
            thinking: Some(portal_types::models::ThinkingLevel::Low),
        };
        for (method, body, operation) in [
            (Method::GET, None, PeerOperation::Settings),
            (
                Method::POST,
                Some(serde_json::to_string(&change).unwrap()),
                PeerOperation::ChangeSettings(change),
            ),
        ] {
            fixture.shell.on(
                &fixture.command("w1:p1", &operation),
                &json!({"status":200,"body":settings}).to_string(),
            );
            let response = call(
                fixture.state.clone(),
                request(method, "/api/agents/netcup/w1:p1/settings", body.as_deref()),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                body_json(response).await,
                serde_json::to_value(&settings).unwrap()
            );
        }
        let operation = PeerOperation::ChangeSettings(portal_types::models::SettingsChange {
            model: None,
            thinking: Some(portal_types::models::ThinkingLevel::High),
        });
        fixture.shell.on(&fixture.command("w1:p1", &operation), &json!({"status":409,"body":{"code":"settings_refused","error":"no credentials for provider"}}).to_string());
        let response = call(
            fixture.state.clone(),
            request(
                Method::POST,
                "/api/agents/netcup/w1:p1/settings",
                Some("{\"thinking\":\"high\"}"),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            body_json(response).await["error"],
            "no credentials for provider"
        );
    }

    #[tokio::test]
    async fn remote_activity_producer_commands_keep_bearer_and_peer_correlation() {
        let fixture = PeerFixture::new();
        let mut state = fixture.state.clone();
        state.token = crate::web::guard::Token::new("secret".into());
        let registration = portal_types::agents::RegisterProducer {
            version: 1,
            epoch: None,
            expected_generation: 0,
            producer_id: "producer".into(),
            evidence: portal_types::agents::ProducerEvidence {
                session_file: "/peer/session".into(),
                session_id: "native-id".into(),
            },
        };
        let update = portal_types::agents::PublishActivity {
            version: 1,
            epoch: "peer-serve".into(),
            generation: 1,
            producer_id: "producer".into(),
            session: "peer-session".into(),
            sequence: 1,
            payload: portal_types::agents::ProducerPayload::Closed,
        };
        for (method, route, body, operation) in [
            (
                Method::POST,
                "activity/producer",
                serde_json::to_string(&registration).unwrap(),
                PeerOperation::Register(registration),
            ),
            (
                Method::PUT,
                "activity",
                serde_json::to_string(&update).unwrap(),
                PeerOperation::Publish(update),
            ),
        ] {
            fixture.shell.on(
                &fixture.command("w1:p1", &operation),
                &json!({"status":200,"body":{"epoch":"peer-serve","session":"peer-session"}})
                    .to_string(),
            );
            let mut request = request(
                method,
                &format!("/api/agents/netcup/w1:p1/{route}"),
                Some(&body),
            );
            request.headers_mut().insert(
                axum::http::header::AUTHORIZATION,
                "Bearer secret".parse().unwrap(),
            );
            let response = call(state.clone(), request).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(body_json(response).await["epoch"], "peer-serve");
        }
    }

    #[tokio::test]
    async fn unreachable_peer_errors_name_the_host_without_ssh_text() {
        let fixture = PeerFixture::new();
        fixture.shell.on_output(
            &fixture.command("w1:p1", &PeerOperation::Conversation),
            crate::shell::Output {
                status: 255,
                stdout: String::new(),
                stderr: "ssh: connect: Connection refused; secret-token".into(),
            },
        );
        let response = call(
            fixture.state.clone(),
            request(Method::GET, "/api/agents/netcup/w1:p1/conversation", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            body_json(response).await["error"],
            "host netcup: unreachable: connection refused"
        );
        let response = call(
            fixture.state.clone(),
            request(Method::GET, "/api/agents/unknown/w1:p1/conversation", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            body_json(response).await["error"],
            "host unknown: unreachable: other"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_closes_an_idle_agent_subscription() {
        let server = agent_server();
        let state = with_socket(&server);
        let fixture =
            crate::observation::details::fixture::Fixture::at(server.client(), state.web.clone());
        let response = call(
            state.clone(),
            request(Method::GET, "/api/agents/local/w1:p1/events", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let registered = fixture.wait(|frame| {
            frame.views["agent_details"]["data"]
                .as_object()
                .is_some_and(|details| details.contains_key("w1:p1"))
        });
        state.stop.cancel();
        tokio::time::timeout(
            crate::test_support::WAIT_DEADLINE,
            axum::body::to_bytes(response.into_body(), 1024),
        )
        .await
        .expect("agent SSE stops")
        .expect("the body");
        // The body ends at once; the forwarding thread drops its subscription on its next
        // poll, and only then does serve release the lease. A frame with no details can
        // predate the first capture, so it proves nothing about the release.
        let deadline = std::time::Instant::now() + crate::test_support::WAIT_DEADLINE;
        while !fixture.no_viewers() {
            assert!(
                std::time::Instant::now() < deadline,
                "detail lease was not released"
            );
            std::thread::yield_now();
        }
        fixture.wait(|frame| {
            frame.revision > registered.revision
                && frame.views["agent_details"]["data"]
                    .as_object()
                    .is_some_and(serde_json::Map::is_empty)
        });
    }

    /// A fake Herdr that answers the agent calls.
    fn agent_server() -> fake::Server {
        fake::Server::start(|method, params| match method {
            "agent.list" => json!({"result": {"type": "agent_list", "agents": [
                {
                    "pane_id": "w1:p1",
                    "agent": "pi",
                    "name": "worker",
                    "agent_status": "working",
                    "cwd": "/work",
                    "agent_session": {"kind": "path", "value": "/work/session.jsonl"}
                },
                {
                    "pane_id": "w2:p1",
                    "agent": "pi",
                    "agent_status": "blocked",
                    "agent_session": {"kind": "path", "value": "/work/blocked.jsonl"}
                }
            ]}}),
            "pane.read" => {
                let pane = params["pane_id"].as_str().unwrap_or_default();
                if pane == "w2:p1" {
                    json!({"result": {"type": "pane_read", "read": {
                        "pane_id": pane,
                        "text": "Allow this command?\n1. Yes\n2. No\n",
                        "revision": 3
                    }}})
                } else {
                    json!({"result": {"type": "pane_read", "read": {
                        "pane_id": pane,
                        "text": "the screen",
                        "revision": 5
                    }}})
                }
            }
            "agent.prompt" => {
                assert_eq!(params["target"], "w1:p1");
                assert_eq!(params["text"], "go on");
                json!({"result": {"type": "agent_prompted"}})
            }
            "agent.send_keys" => {
                assert_eq!(params["target"], "w1:p1");
                let keys = params["keys"].as_array().expect("the keys");
                assert!(
                    keys.iter().all(|key| key == "enter" || key == "ctrl+c"),
                    "the keys are {keys:?}"
                );
                json!({"result": {"type": "ok"}})
            }
            "agent.focus" => {
                assert_eq!(params["target"], "w1:p1");
                json!({"result": {"type": "agent_info"}})
            }
            other => panic!("unexpected method {other}"),
        })
    }

    #[tokio::test]
    async fn the_agent_list_holds_every_agent_herdr_sees() {
        let server = agent_server();
        let response = call(
            with_published_agents(&server),
            request(Method::GET, "/api/agents", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let payload = body_json(response).await;
        let agents = payload["agents"].as_array().expect("the agents");
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0]["pane"], "w1:p1");
        assert_eq!(agents[0]["name"], "worker");
        assert_eq!(agents[0]["state"], "working");
        assert_eq!(agents[0]["cwd"], "/work");
        // A working agent waits for nothing, so it carries no prompt.
        assert!(agents[0].get("blocked").is_none());
    }

    #[tokio::test]
    async fn a_blocked_agent_carries_the_prompt_it_waits_on() {
        let server = agent_server();
        let response = call(
            with_published_agents(&server),
            request(Method::GET, "/api/agents", None),
        )
        .await;
        let payload = body_json(response).await;
        let blocked = &payload["agents"][1]["blocked"];
        assert_eq!(blocked["question"], "Allow this command?");
        assert_eq!(blocked["options"][0]["key"], "1");
        assert_eq!(blocked["options"][0]["label"], "Yes");
        assert_eq!(blocked["options"][1]["label"], "No");
    }

    #[tokio::test]
    async fn the_screen_of_an_agent_is_served() {
        let server = agent_server();
        let state = with_socket(&server);
        let _fixture =
            crate::observation::details::fixture::Fixture::at(server.client(), state.web.clone());
        let response = call(
            state,
            request(Method::GET, "/api/agents/local/w1:p1/screen", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let payload = body_json(response).await;
        assert_eq!(payload["pane"], "w1:p1");
        assert_eq!(payload["text"], "the screen");
        assert_eq!(payload["revision"], 5);
    }

    #[tokio::test]
    async fn local_files_waits_for_current_state_and_peer_read_serves_the_same_state() {
        use crate::observation::details::fixture::{Fixture, GitShell, Repository};
        let repo = Repository::new();
        std::fs::write(repo.0.join("untracked"), "new").unwrap();
        let cwd = repo.0.clone();
        let server = fake::Server::start(move |method, _| match method {
            "agent.list" => json!({"result":{"agents":[{
                "pane_id":"w1:p1","agent":"pi","agent_status":"working","cwd":cwd,
                "agent_session":{"kind":"path","value":"/work/session.jsonl"}
            }]}}),
            "pane.read" => json!({"result":{"read":{"text":"screen","revision":1}}}),
            other => panic!("unexpected method {other}"),
        });
        let shell = Arc::new(GitShell::default());
        let (entered, release) = shell.hold();
        let fixture = Fixture::with_files_shell(server.client(), shell);
        let _viewer = fixture.client.detail("w1:p1").unwrap();
        entered
            .recv_timeout(crate::test_support::WAIT_DEADLINE)
            .unwrap();
        let mut state = with_socket(&server);
        state.web = fixture.ledger.clone();
        let task_state = state.clone();
        let mut response = tokio::spawn(async move {
            call(
                task_state,
                request(Method::GET, "/api/agents/local/w1:p1/files", None),
            )
            .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut response)
                .await
                .is_err()
        );
        release.send(()).unwrap();
        let response = response.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert!(response.headers().contains_key("x-observation-revision"));
        let body = body_json(response).await;
        assert_eq!(body["current"]["root"], json!(repo.0));
        assert_eq!(body["current"]["entries"][0]["path"]["text"], "untracked");
        let reply = peer_read(state.clone(), "w1:p1".into(), PeerOperation::Files)
            .await
            .unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body, body);
        let response = call(
            state,
            request(Method::GET, "/api/agents/local/missing/files", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn files_change_event_carries_only_the_files_revision_change() {
        use crate::observation::details::fixture::{Fixture, Repository};
        let repo = Repository::new();
        let cwd = repo.0.clone();
        let server = fake::Server::start(move |method, _| match method {
            "agent.list" => json!({"result":{"agents":[{
                "pane_id":"w1:p1","agent":"pi","agent_status":"working","cwd":cwd,
                "agent_session":{"kind":"path","value":"/work/session.jsonl"}
            }]}}),
            "pane.read" => json!({"result":{"read":{"text":"screen","revision":1}}}),
            other => panic!("unexpected method {other}"),
        });
        let state = with_socket(&server);
        let fixture = Fixture::at(server.client(), state.web.clone());
        let _viewer = fixture.client.detail("w1:p1").unwrap();
        fixture.wait(|frame| {
            frame.views["agent_details"]["data"]["w1:p1"]["files"]["availability"] == "empty"
        });
        let response = call(
            state.clone(),
            request(Method::GET, "/api/agents/local/w1:p1/events", None),
        )
        .await;
        let mut stream = response.into_body().into_data_stream();
        let first = tokio::time::timeout(crate::test_support::WAIT_DEADLINE, stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let first = agent_sse_payload(&first).revisions.unwrap();
        std::fs::write(repo.0.join("file"), "edited\n").unwrap();
        assert!(fixture.files_tick("w1:p1"));
        let second = tokio::time::timeout(crate::test_support::WAIT_DEADLINE, stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let second = agent_sse_payload(&second).revisions.unwrap();
        assert!(second.files > first.files);
        assert_eq!(second.conversation, first.conversation);
        assert_eq!(second.screen, first.screen);
        state.stop.cancel();
    }

    #[tokio::test]
    async fn the_conversation_of_an_unknown_agent_is_not_found() {
        let server = agent_server();
        let state = with_socket(&server);
        let _fixture =
            crate::observation::details::fixture::Fixture::at(server.client(), state.web.clone());
        let response = call(
            state,
            request(Method::GET, "/api/agents/local/w9:p9/conversation", None),
        )
        .await;
        let status = response.status();
        let payload = body_json(response).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{payload}");
    }

    #[tokio::test]
    async fn the_conversation_of_an_agent_holds_its_turns() {
        let path = std::env::temp_dir().join(format!("web-{}.jsonl", crate::ledger::new_id()));
        let mut text = String::new();
        for line in [
            json!({"type": "message", "id":"request-id", "message": {"role": "user", "timestamp":10, "content": [{"type": "text", "text": "fix it"}]}}),
            json!({"type": "message", "id":"reply-id", "message": {"role": "assistant", "timestamp":11, "model":"recorded-model", "provider":"recorded-provider", "content": [{"type": "text", "text": "done"},{"type":"toolCall","id":"read-id","name":"read","arguments":{"secret":"not public"}}]}}),
            json!({"type":"message","message":{"role":"toolResult","toolCallId":"read-id","toolName":"read","isError":true,"content":[{"type":"text","text":"private output"}]}}),
            json!({"type":"custom_message","customType":"dogsled","display":true,"details":{"records":[{"id":"delivery-id","sender":"worker-pane","kind":"message","text":"worker report","created_ms":12}]}}),
        ] {
            text.push_str(&line.to_string());
            text.push('\n');
        }
        std::fs::write(&path, text).expect("the transcript");
        let path = path.to_string_lossy().into_owned();
        let server = fake::Server::start(move |method, _| match method {
            "agent.list" => json!({"result": {"type": "agent_list", "agents": [
                {
                    "pane_id": "w1:p1",
                    "agent": "pi",
                    "agent_status": "idle",
                    "agent_session": {"kind": "path", "value": path}
                }
            ]}}),
            "pane.read" => json!({"result":{"type":"pane_read","read":{"text":"","revision":0}}}),
            other => panic!("unexpected method {other}"),
        });
        let state = with_socket(&server);
        let _fixture =
            crate::observation::details::fixture::Fixture::at(server.client(), state.web.clone());
        let response = call(
            state,
            request(Method::GET, "/api/agents/local/w1:p1/conversation", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let payload = body_json(response).await;
        assert_eq!(payload["pane"], "w1:p1");
        let turns = payload["turns"].as_array().expect("the turns");
        assert_eq!(turns.len(), 4);
        assert_eq!(turns[0]["role"], "user");
        assert_eq!(turns[0]["text"], "fix it");
        assert_eq!(turns[0]["id"], "request-id");
        assert_eq!(turns[0]["timestamp"], json!({"ms":10,"meaning":"message"}));
        assert_eq!(turns[1]["role"], "assistant");
        assert_eq!(turns[1]["text"], "done");
        assert_eq!(turns[1]["id"], "reply-id");
        assert_eq!(
            turns[1]["source"],
            json!({"kind":"harness","harness":"pi","model":"recorded-model","provider":"recorded-provider"})
        );
        assert_eq!(turns[2]["role"], "tool");
        assert_eq!(
            turns[2]["tool"],
            json!({"call_id":"read-id","name":"read","status":"error","call":"read {\"secret\":\"not public\"}","output":"private output","output_lines":1})
        );
        assert_eq!(turns[2]["text"], "");
        assert_eq!(turns[3]["role"], "context");
        assert_eq!(turns[3]["id"], "delivery-id");
        assert_eq!(
            turns[3]["source"],
            json!({"kind":"dogsled","sender":"worker-pane","delivery_kind":"message"})
        );
        assert_eq!(turns[3]["timestamp"], json!({"ms":12,"meaning":"created"}));
        assert_eq!(payload["truncated"], false);
        assert_ne!(payload["session"].as_str().expect("session key"), "");
        assert_eq!(turns[2]["tool"]["call"], "read {\"secret\":\"not public\"}");
        assert_eq!(turns[2]["tool"]["output"], "private output");
    }

    #[tokio::test]
    async fn the_conversation_endpoint_preserves_truncation() {
        let path = std::env::temp_dir().join(format!("web-{}.jsonl", crate::ledger::new_id()));
        let mut text = String::new();
        for index in 0..201 {
            text.push_str(&json!({"type":"message","id":format!("message-{index}"),"message":{"role":"user","content":"request"}}).to_string());
            text.push('\n');
        }
        std::fs::write(&path, text).expect("the transcript");
        let server = fake::Server::start(move |method, _| match method {
            "agent.list" => json!({"result":{"type":"agent_list","agents":[{
                "pane_id":"w1:p1","agent":"pi","agent_status":"idle",
                "agent_session":{"kind":"path","value":path}
            }]}}),
            "pane.read" => json!({"result":{"type":"pane_read","read":{"text":"","revision":0}}}),
            other => panic!("unexpected method {other}"),
        });
        let state = with_socket(&server);
        let _fixture =
            crate::observation::details::fixture::Fixture::at(server.client(), state.web.clone());
        let response = call(
            state,
            request(Method::GET, "/api/agents/local/w1:p1/conversation", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let payload = body_json(response).await;
        let turns = payload["turns"].as_array().expect("turns");
        assert_eq!(turns.len(), 200);
        assert_eq!(turns[0]["id"], "message-1");
        assert_eq!(payload["truncated"], true);
        assert_ne!(payload["session"].as_str().expect("session key"), "");
    }

    #[tokio::test]
    async fn a_prompt_reaches_the_agent() {
        let server = agent_server();
        let response = call(
            with_socket(&server),
            request(
                Method::POST,
                "/api/agents/local/w1:p1/prompt",
                Some(r#"{"text":"go on"}"#),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(server.methods().contains(&"agent.prompt".to_owned()));
    }

    #[tokio::test]
    async fn a_local_blocked_agent_only_receives_a_reply() {
        let server = fake::Server::start(|method, params| match method {
            "agent.list" => {
                json!({"result":{"agents":[{"pane_id":"w1:p1","agent_status":"blocked"}]}})
            }
            "agent.prompt" => {
                assert_eq!(params["target"], "w1:p1");
                assert_eq!(params["text"], "go on");
                json!({"result":{"type":"agent_prompted"}})
            }
            other => panic!("unexpected method {other}"),
        });
        let response = call(
            with_socket(&server),
            request(
                Method::POST,
                "/api/agents/local/w1:p1/prompt",
                Some(r#"{"text":"go on"}"#),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            body_json(response).await,
            json!({"error": "The agent needs a response. Select its prompt in Waiting on you to reply."})
        );
        assert_eq!(server.methods(), vec!["agent.list"]);

        let response = call(
            with_socket(&server),
            request(
                Method::POST,
                "/api/agents/local/w1:p1/prompt",
                Some(r#"{"text":"go on","reply":true}"#),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(
            server.methods(),
            vec!["agent.list", "agent.list", "agent.prompt"]
        );
    }

    #[tokio::test]
    async fn a_prompt_to_an_unknown_local_pane_is_not_sent() {
        let server = agent_server();
        let response = call(
            with_socket(&server),
            request(
                Method::POST,
                "/api/agents/local/missing/prompt",
                Some(r#"{"text":"go on"}"#),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            body_json(response).await,
            json!({"error":"no agent in pane missing"})
        );
        assert_eq!(server.methods(), vec!["agent.list"]);
    }

    #[tokio::test]
    async fn a_local_prompt_requires_a_readable_status_even_for_a_reply() {
        for status in [Value::Null, json!(42), json!("")] {
            let server = fake::Server::start(move |method, _| {
                assert_eq!(method, "agent.list");
                json!({"result":{"agents":[{"pane_id":"w1:p1","agent_status":status}]}})
            });
            let response = call(
                with_socket(&server),
                request(
                    Method::POST,
                    "/api/agents/local/w1:p1/prompt",
                    Some(r#"{"text":"go on","reply":true}"#),
                ),
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
            assert_eq!(
                body_json(response).await,
                json!({"error":"host local: agent status unavailable"})
            );
            assert_eq!(server.methods(), vec!["agent.list"]);
        }
    }

    #[tokio::test]
    async fn an_empty_prompt_is_refused() {
        let server = agent_server();
        let response = call(
            with_socket(&server),
            request(
                Method::POST,
                "/api/agents/local/w1:p1/prompt",
                Some(r#"{"text":"  "}"#),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(!server.methods().contains(&"agent.prompt".to_owned()));
    }

    #[tokio::test]
    async fn an_allowed_key_reaches_the_agent() {
        let server = agent_server();
        let response = call(
            with_socket(&server),
            request(
                Method::POST,
                "/api/agents/local/w1:p1/keys",
                Some(r#"{"keys":["enter"]}"#),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(server.methods().contains(&"agent.send_keys".to_owned()));
    }

    #[tokio::test]
    async fn ctrl_c_reaches_the_agent() {
        let server = agent_server();
        let response = call(
            with_socket(&server),
            request(
                Method::POST,
                "/api/agents/local/w1:p1/keys",
                Some(r#"{"keys":["Ctrl+C"]}"#),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(server.methods().contains(&"agent.send_keys".to_owned()));
        let requests = server.requests.lock().unwrap();
        let sent = requests
            .iter()
            .find(|request| request["method"] == "agent.send_keys")
            .expect("the key request");
        assert_eq!(sent["params"]["keys"], json!(["ctrl+c"]));
    }

    #[tokio::test]
    async fn a_key_outside_the_whitelist_is_refused() {
        let server = agent_server();
        for key in ["rm -rf /", "ctrl+d", "shift+tab", "f13", "prefix+x", ""] {
            let response = call(
                with_socket(&server),
                request(
                    Method::POST,
                    "/api/agents/local/w1:p1/keys",
                    Some(&format!(r#"{{"keys":["{key}"]}}"#)),
                ),
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "the key {key} must be refused"
            );
        }
        // No key reached Herdr.
        assert!(!server.methods().contains(&"agent.send_keys".to_owned()));
    }

    #[tokio::test]
    async fn an_empty_key_list_is_refused() {
        let server = agent_server();
        let response = call(
            with_socket(&server),
            request(
                Method::POST,
                "/api/agents/local/w1:p1/keys",
                Some(r#"{"keys":[]}"#),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn focusing_an_agent_reaches_herdr() {
        let server = agent_server();
        let response = call(
            with_socket(&server),
            request(Method::POST, "/api/agents/local/w1:p1/focus", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(server.methods().contains(&"agent.focus".to_owned()));
    }

    #[test]
    fn agent_event_serializes_pending_parts_from_detail() {
        use crate::observation::details::Session;
        use portal_types::files::FilesState;

        let mut publication = DetailFrame {
            version: 1,
            epoch: "serve".into(),
            revision: 1,
            found: true,
            parts: Some(portal_types::agents::DetailRevisions {
                conversation: 1,
                screen: 1,
                files: 1,
            }),
            agent: None,
            detail: Detail {
                session: Some(Session {
                    terminal: Some("terminal".into()),
                    session: "native".into(),
                    harness: Some("pi".into()),
                    transcript: None,
                }),
                state: "blocked".into(),
                conversation: Content::Pending,
                screen: Content::Pending,
                files: FilesState::Pending,
            },
        };
        for (conversation, screen, files) in [
            (true, true, true),
            (false, true, true),
            (false, false, true),
            (false, false, false),
        ] {
            if !conversation {
                publication.detail.conversation = Content::Failed("read failed".into());
            }
            if !screen {
                publication.detail.screen = Content::Unavailable("unsupported".into());
            }
            if !files {
                publication.detail.files = FilesState::NotRepository;
            }
            let wire: Value =
                serde_json::from_str(&agent_event(&publication, "pane").unwrap()).unwrap();
            assert_eq!(
                wire["pending"],
                json!({
                    "conversation": conversation, "screen": screen, "files": files
                })
            );
            assert_eq!(wire["session"]["availability"], "available");
        }
    }

    #[tokio::test]
    async fn agent_events_ignore_portal_revisions_but_publish_screen_changes() {
        let raw = Arc::new(std::sync::Mutex::new(json!({
            "pane_id":"w1:p1", "agent":"pi", "name":"worker",
            "agent_status":"working",
            "agent_session":{"kind":"path","value":"/work/session.jsonl"}
        })));
        let screen = Arc::new(std::sync::Mutex::new(json!({
            "text":"first screen", "revision":7
        })));
        let source = Arc::clone(&raw);
        let output = Arc::clone(&screen);
        let server = fake::Server::start(move |method, _| match method {
            "agent.list" => json!({"result":{"agents":[source.lock().unwrap().clone()]}}),
            "pane.read" => json!({"result":{"read":output.lock().unwrap().clone()}}),
            other => panic!("unexpected method {other}"),
        });
        let state = with_socket(&server);
        let fixture =
            crate::observation::details::fixture::Fixture::at(server.client(), state.web.clone());
        let _viewer = fixture.client.detail("w1:p1").unwrap();
        fixture.wait(|frame| frame.views["agent_details"]["data"]["w1:p1"]["screen_revision"] == 7);
        let response = call(
            state.clone(),
            request(Method::GET, "/api/agents/local/w1:p1/events", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body().into_data_stream();
        let first = tokio::time::timeout(crate::test_support::WAIT_DEADLINE, stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let first = agent_sse_payload(&first);
        for name in ["renamed", "renamed again"] {
            let previous = fixture
                .client
                .current(&crate::observation::ipc::Select::All)
                .unwrap()
                .revision;
            raw.lock().unwrap()["name"] = json!(name);
            fixture.wake();
            fixture.wait(|frame| frame.revision > previous);
            assert!(
                tokio::time::timeout(Duration::from_millis(250), stream.next())
                    .await
                    .is_err(),
                "unchanged event content must not emit another agent event"
            );
        }
        *screen.lock().unwrap() = json!({"text":"second screen", "revision":8});
        assert!(fixture.tick("w1:p1"));
        let second = tokio::time::timeout(crate::test_support::WAIT_DEADLINE, stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let second = agent_sse_payload(&second);
        let first = first.revisions.unwrap();
        let second = second.revisions.unwrap();
        assert_eq!(second.conversation, first.conversation);
        assert!(second.screen > first.screen);
        state.stop.cancel();
    }

    #[test]
    fn peer_follow_refuses_one_shot_operations() {
        let ledger = crate::ledger::Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        for operation in [
            PeerOperation::Conversation,
            PeerOperation::Screen,
            PeerOperation::Files,
        ] {
            let error = peer_follow(
                "w1:p1",
                &serde_json::to_string(&operation).unwrap(),
                &ledger,
            )
            .unwrap_err();
            assert_eq!(error.to_string(), "unsupported peer stream");
        }
        assert!(
            !ledger.dir.exists(),
            "refused streams must not read host files"
        );
    }

    #[test]
    fn peer_follow_emits_only_changed_detail_lines() {
        use std::io::BufRead as _;
        let raw = Arc::new(std::sync::Mutex::new(json!({
            "pane_id":"w1:p1", "agent":"pi", "name":"worker", "agent_status":"working",
            "agent_session":{"kind":"path","value":"/work/session.jsonl"}
        })));
        let screen = Arc::new(std::sync::Mutex::new(json!({"text":"first", "revision":7})));
        let source = Arc::clone(&raw);
        let output = Arc::clone(&screen);
        let server = fake::Server::start(move |method, _| match method {
            "agent.list" => json!({"result":{"agents":[source.lock().unwrap().clone()]}}),
            "pane.read" => json!({"result":{"read":output.lock().unwrap().clone()}}),
            other => panic!("unexpected method {other}"),
        });
        let fixture = crate::observation::details::fixture::Fixture::new(server.client());
        let _viewer = fixture.client.detail("w1:p1").unwrap();
        fixture.wait(|frame| frame.views["agent_details"]["data"]["w1:p1"]["screen_revision"] == 7);
        let (reader, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
        reader
            .set_read_timeout(Some(crate::test_support::WAIT_DEADLINE))
            .unwrap();
        let mut reader = std::io::BufReader::new(reader);
        let stop = tokio_util::sync::CancellationToken::new();
        let _cancel = stop.clone().drop_guard();
        let stopped = stop.clone();
        let ledger = fixture.ledger.clone();
        let thread = std::thread::spawn(move || {
            follow_output(
                "w1:p1",
                &PeerOperation::Event,
                &ledger,
                &stopped,
                &mut writer,
            )
            .unwrap();
        });
        let mut first = String::new();
        reader.read_line(&mut first).unwrap();
        let first: PeerReply = serde_json::from_str(&first).unwrap();
        assert_eq!(first.status, 200);
        let previous = fixture
            .client
            .current(&crate::observation::ipc::Select::All)
            .unwrap()
            .revision;
        raw.lock().unwrap()["name"] = json!("renamed");
        fixture.wake();
        fixture.wait(|frame| frame.revision > previous);
        // Negative check starts only after the initial line proves the follower is live.
        reader
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(250)))
            .unwrap();
        let mut unchanged = String::new();
        assert!(
            reader.read_line(&mut unchanged).is_err(),
            "unchanged detail must emit no line"
        );
        *screen.lock().unwrap() = json!({"text":"second", "revision":8});
        assert!(fixture.tick("w1:p1"));
        reader
            .get_ref()
            .set_read_timeout(Some(crate::test_support::WAIT_DEADLINE))
            .unwrap();
        let mut second = String::new();
        reader.read_line(&mut second).unwrap();
        let second: PeerReply = serde_json::from_str(&second).unwrap();
        assert_eq!(second.status, 200);
        assert_ne!(first.body["revisions"], second.body["revisions"]);
        stop.cancel();
        thread.join().unwrap();
    }

    fn agent_sse_payload(bytes: &[u8]) -> AgentEvent {
        let frame = std::str::from_utf8(bytes).unwrap();
        assert!(frame.contains("event: agent"), "the event name: {frame}");
        let data = frame
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap();
        serde_json::from_str(data).unwrap()
    }

    #[tokio::test]
    async fn the_agent_events_are_pushed_as_they_arrive() {
        let server = agent_server();
        let state = with_socket(&server);
        let _fixture =
            crate::observation::details::fixture::Fixture::at(server.client(), state.web.clone());
        let response = call(
            state.clone(),
            request(Method::GET, "/api/agents/local/w2:p1/events", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
        let mut stream = response.into_body().into_data_stream();
        let mut text = String::new();
        while !text.contains("\"state\":\"blocked\"") {
            let bytes = tokio::time::timeout(crate::test_support::WAIT_DEADLINE, stream.next())
                .await
                .expect("published agent event")
                .expect("stream open")
                .expect("body data");
            text.push_str(&String::from_utf8_lossy(&bytes));
        }
        assert!(text.contains("event: agent"), "the event name: {text}");
        assert!(text.contains("\"kind\":\"pane.agent_status_changed\""));
        assert!(text.contains("\"state\":\"blocked\""));
        state.stop.cancel();
    }

    #[tokio::test]
    async fn the_agent_api_reports_a_missing_herdr_socket() {
        let response = call(state(None), request(Method::GET, "/api/agents", None)).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
