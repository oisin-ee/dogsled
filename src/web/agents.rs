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

/// The routes of the agent API.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/agents", get(list))
        .route("/api/agents/{pane}/conversation", get(conversation))
        .route("/api/agents/{pane}/screen", get(screen))
        .route("/api/agents/{pane}/prompt", post(prompt))
        .route("/api/agents/{pane}/keys", post(keys))
        .route("/api/agents/{pane}/focus", post(focus))
        .route("/api/agents/{pane}/events", get(events))
        .route("/api/leads", post(create_lead))
        .route("/api/repos", get(repos))
}

/// The registered repositories, reread from config on every request.
async fn repos(State(state): State<AppState>) -> Response {
    let Some(launch) = state.leads.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "this server cannot start leads"})),
        )
            .into_response();
    };
    match crate::config::load(&launch.config_path) {
        Ok(policy) => Json(portal_types::agents::RepoList {
            repos: policy
                .repos
                .iter()
                .map(|repo| portal_types::agents::RepoChoice {
                    origin: repo.origin.clone(),
                    path: repo.path.clone(),
                })
                .collect(),
        })
        .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error.to_string()})),
        )
            .into_response(),
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
    let Some(socket) = socket(&state).cloned() else {
        return no_socket();
    };
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
            herdr: &socket,
        };
        crate::leads::create(&context, &body.repo, &body.prompt)
    })
    .await;
    match outcome {
        Ok(Ok(started)) => (
            StatusCode::CREATED,
            Json(portal_types::agents::CreatedLead { pane: started.pane }),
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
async fn conversation(State(state): State<AppState>, Path(pane): Path<String>) -> Response {
    detail_response(
        state,
        pane,
        |detail| matches!(detail.conversation, Content::Pending),
        |detail| &detail.conversation,
    )
    .await
}

/// The last published screen, including its native revision.
async fn screen(State(state): State<AppState>, Path(pane): Path<String>) -> Response {
    detail_response(
        state,
        pane,
        |detail| matches!(detail.screen, Content::Pending),
        |detail| &detail.screen,
    )
    .await
}

async fn detail_response<T: serde::Serialize + Send + 'static>(
    state: AppState,
    pane: String,
    pending: fn(&Detail) -> bool,
    select: fn(&Detail) -> &Content<T>,
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
                let response = match select(detail) {
                    Content::Available(content) => Json(content).into_response(),
                    Content::Unavailable(error) => crate::web::observation_error(error),
                    Content::Failed(error) => {
                        (StatusCode::BAD_GATEWAY, Json(json!({"error":error}))).into_response()
                    }
                    Content::Pending => {
                        return Err(crate::error::Error::Invalid("agent detail pending".into()));
                    }
                };
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

/// Send a prompt to an agent.
async fn prompt(
    State(state): State<AppState>,
    Path(pane): Path<String>,
    Json(body): Json<PromptBody>,
) -> Response {
    let Some(socket) = socket(&state) else {
        return no_socket();
    };
    let text = body.text.trim();
    if text.is_empty() {
        return refuse("the prompt is empty");
    }
    match socket.agent_prompt(&pane, text) {
        Ok(_) => StatusCode::ACCEPTED.into_response(),
        Err(error) => error_response(&error),
    }
}

/// Send key presses to an agent. Only the allowed keys are accepted.
async fn keys(
    State(state): State<AppState>,
    Path(pane): Path<String>,
    Json(body): Json<KeysBody>,
) -> Response {
    let Some(socket) = socket(&state) else {
        return no_socket();
    };
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
    match socket.agent_send_keys(&pane, &keys) {
        Ok(_) => StatusCode::ACCEPTED.into_response(),
        Err(error) => error_response(&error),
    }
}

/// Focus an agent.
async fn focus(State(state): State<AppState>, Path(pane): Path<String>) -> Response {
    let Some(socket) = socket(&state) else {
        return no_socket();
    };
    match socket.agent_focus(&pane) {
        Ok(_) => StatusCode::ACCEPTED.into_response(),
        Err(error) => error_response(&error),
    }
}

/// Forward serve-owned detail changes. The observation connection owns the interest.
async fn events(State(state): State<AppState>, Path(pane): Path<String>) -> Response {
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
            let result = agent_event(&connection.publication, &pane)
                .map(|payload| (payload, connection.publication.revision));
            match result {
                Ok((payload, detail)) => {
                    let signature = (payload.clone(), detail);
                    if previous.as_ref() != Some(&signature) {
                        sender.send_replace(Some(payload));
                        previous = Some(signature);
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
            super::test_request(
                self,
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/leads")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_owned()))
                    .unwrap(),
            )
            .await
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
            "{",
        ] {
            let (status, body) = post_lead(leads(), bad).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
        }
        let (status, body) =
            post_lead(leads(), r#"{"repo":"https://example.org/r","prompt":"go"}"#).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body["error"].as_str().unwrap().contains("not registered"),
            "{body}"
        );
        assert_eq!(body["id"], serde_json::Value::Null);
        // No Herdr socket: the capability is unavailable, and nothing is started.
        let (status, body) = post_lead(
            state(None).with_leads(launch("")),
            r#"{"repo":"r","prompt":"go"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(!server.methods().iter().any(|m| m == "agent.prompt"));
    }

    #[tokio::test]
    async fn repos_route_rereads_the_registered_list() {
        let launch = launch("[[repos]]\npath = \"~/a\"\norigin = \"https://example.org/a.git\"\n");
        let config = launch.config_path.clone();
        let get = |state: AppState| async move {
            let response = super::test_request(
                state,
                Request::builder()
                    .uri("/api/repos")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            )
        };
        let (status, body) = get(state(None).with_leads(launch.clone())).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body,
            json!({"repos":[{"origin":"https://example.org/a.git","path":"~/a"}]})
        );
        std::fs::write(&config, "").unwrap();
        let (_, body) = get(state(None).with_leads(launch)).await;
        assert_eq!(body, json!({"repos":[]}));
        let (status, _) = get(state(None)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_closes_an_idle_agent_subscription() {
        let server = agent_server();
        let state = with_socket(&server);
        let fixture =
            crate::observation::details::fixture::Fixture::at(server.client(), state.web.clone());
        let response = call(
            state.clone(),
            request(Method::GET, "/api/agents/w1:p1/events", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        state.stop.cancel();
        tokio::time::timeout(
            Duration::from_secs(2),
            axum::body::to_bytes(response.into_body(), 1024),
        )
        .await
        .expect("agent SSE stops")
        .expect("the body");
        fixture.wait(|frame| {
            frame.views["agent_details"]["data"]
                .as_object()
                .is_some_and(serde_json::Map::is_empty)
        });
        assert!(fixture.no_viewers());
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
            request(Method::GET, "/api/agents/w1:p1/screen", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let payload = body_json(response).await;
        assert_eq!(payload["pane"], "w1:p1");
        assert_eq!(payload["text"], "the screen");
        assert_eq!(payload["revision"], 5);
    }

    #[tokio::test]
    async fn the_conversation_of_an_unknown_agent_is_not_found() {
        let server = agent_server();
        let state = with_socket(&server);
        let _fixture =
            crate::observation::details::fixture::Fixture::at(server.client(), state.web.clone());
        let response = call(
            state,
            request(Method::GET, "/api/agents/w9:p9/conversation", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_conversation_of_an_agent_holds_its_turns() {
        let path = std::env::temp_dir().join(format!("web-{}.jsonl", crate::ledger::new_id()));
        let mut text = String::new();
        for line in [
            json!({"type": "message", "message": {"role": "user", "content": [{"type": "text", "text": "fix it"}]}}),
            json!({"type": "message", "message": {"role": "assistant", "content": [{"type": "text", "text": "done"}]}}),
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
            request(Method::GET, "/api/agents/w1:p1/conversation", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let payload = body_json(response).await;
        assert_eq!(payload["pane"], "w1:p1");
        let turns = payload["turns"].as_array().expect("the turns");
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0]["role"], "user");
        assert_eq!(turns[0]["text"], "fix it");
        assert_eq!(turns[1]["role"], "assistant");
        assert_eq!(turns[1]["text"], "done");
    }

    #[tokio::test]
    async fn a_prompt_reaches_the_agent() {
        let server = agent_server();
        let response = call(
            with_socket(&server),
            request(
                Method::POST,
                "/api/agents/w1:p1/prompt",
                Some(r#"{"text":"go on"}"#),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(server.methods().contains(&"agent.prompt".to_owned()));
    }

    #[tokio::test]
    async fn an_empty_prompt_is_refused() {
        let server = agent_server();
        let response = call(
            with_socket(&server),
            request(
                Method::POST,
                "/api/agents/w1:p1/prompt",
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
                "/api/agents/w1:p1/keys",
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
                "/api/agents/w1:p1/keys",
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
                    "/api/agents/w1:p1/keys",
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
                "/api/agents/w1:p1/keys",
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
            request(Method::POST, "/api/agents/w1:p1/focus", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(server.methods().contains(&"agent.focus".to_owned()));
    }

    #[tokio::test]
    async fn the_agent_events_are_pushed_as_they_arrive() {
        let server = agent_server();
        let state = with_socket(&server);
        let _fixture =
            crate::observation::details::fixture::Fixture::at(server.client(), state.web.clone());
        let response = call(
            state.clone(),
            request(Method::GET, "/api/agents/w2:p1/events", None),
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
            let bytes = tokio::time::timeout(Duration::from_secs(3), stream.next())
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
