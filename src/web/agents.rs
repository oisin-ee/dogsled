//! The agent view of the portal: the list of agents, one agent's conversation
//! and screen, and the calls that act on an agent.
//!
//! Every read comes from Herdr's socket API, so the portal shows what Herdr
//! sees and not what dogsled last cached. A blocked agent shows the prompt it
//! waits on, read from its screen.
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
use portal_types::agents::{
    AgentEvent, AgentList, AgentView, Conversation, KeysBody, PromptBody, Screen, Turn,
};
use serde_json::{Value, json};
use tokio_stream::StreamExt;

use crate::herdr_socket::{Client, Next};
use crate::web::AppState;

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
}

/// The client the agent routes talk to Herdr through, if there is one.
fn socket(state: &AppState) -> Option<&Client> {
    state.agents.as_deref()
}

/// The agents Herdr sees, with the prompt a blocked agent waits on.
async fn list(State(state): State<AppState>) -> Response {
    let Some(socket) = socket(&state) else {
        return no_socket();
    };
    let socket = socket.clone();
    match tokio::task::spawn_blocking(move || views(&socket)).await {
        Ok(Ok(agents)) => json_response(Json(AgentList { agents })),
        Ok(Err(error)) => error_response(&error),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

/// Shared read for the agent list and inbox; callers run it on a blocking thread.
pub(super) fn views(socket: &Client) -> crate::error::Result<Vec<AgentView>> {
    let agents = socket.agent_list()?;
    let mut views = Vec::new();
    for agent in agents {
        let pane = agent["pane_id"].as_str().unwrap_or_default().to_owned();
        let state = agent["agent_status"]
            .as_str()
            .unwrap_or("unknown")
            .to_owned();
        let blocked = if state == "blocked" {
            read_blocked(socket, &pane)
        } else {
            None
        };
        views.push(AgentView {
            pane,
            name: agent["name"].as_str().map(str::to_owned),
            title: title_of(&agent),
            agent: agent["agent"].as_str().map(str::to_owned),
            state,
            cwd: agent["cwd"].as_str().map(str::to_owned),
            repository: agent["cwd"]
                .as_str()
                .and_then(|cwd| repository(&crate::shell::System::default(), cwd)),
            blocked,
        });
    }
    Ok(views)
}

/// Use the same origin normalization as the board's observed agents.
fn repository(shell: &dyn crate::shell::Shell, cwd: &str) -> Option<String> {
    let output = shell
        .run(
            "git",
            &[
                "-C".into(),
                cwd.into(),
                "remote".into(),
                "get-url".into(),
                "origin".into(),
            ],
        )
        .ok()?;
    output
        .ok()
        .then(|| crate::checkout::identity(output.stdout.trim()))
        .flatten()
}

/// One agent's turns, read from its transcript.
async fn conversation(State(state): State<AppState>, Path(pane): Path<String>) -> Response {
    let Some(socket) = socket(&state) else {
        return no_socket();
    };
    let Some(agent) = agent_of(socket, &pane) else {
        return not_found(&pane);
    };
    let harness = agent["agent"].as_str().unwrap_or_default();
    let transcript = agent["agent_session"]["value"].as_str().unwrap_or_default();
    let turns = match crate::transcript::resolve(Some(harness), kind_of(&agent), transcript) {
        Some(path) => crate::activity::conversation(harness, &path)
            .into_iter()
            .map(|turn| Turn {
                role: turn.role,
                text: turn.text,
            })
            .collect(),
        None => Vec::new(),
    };
    json_response(Json(Conversation { pane, turns }))
}

/// The text on an agent's terminal.
async fn screen(State(state): State<AppState>, Path(pane): Path<String>) -> Response {
    let Some(socket) = socket(&state) else {
        return no_socket();
    };
    match socket.pane_read(&pane, "visible", Some(SCREEN_LINES)) {
        Ok(read) => json_response(Json(Screen {
            pane,
            text: read["text"].as_str().unwrap_or_default().to_owned(),
            revision: read["revision"].as_u64().unwrap_or_default(),
        })),
        Err(error) => error_response(&error),
    }
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

/// The agent's events, pushed as Herdr reports them.
async fn events(State(state): State<AppState>, Path(pane): Path<String>) -> Response {
    let Some(socket) = socket(&state) else {
        return no_socket();
    };
    let mut subscription = match socket.subscribe(&[json!({
        "type": "pane.agent_status_changed",
        "pane_id": pane,
    })]) {
        Ok(subscription) => subscription,
        Err(error) => return error_response(&error),
    };
    // The subscription blocks, so a task reads it and hands the events over.
    let (sender, receiver) = tokio::sync::mpsc::channel::<String>(16);
    let stop = state.stop.clone();
    tokio::task::spawn_blocking(move || {
        while !stop.is_cancelled() && !sender.is_closed() {
            match subscription.next() {
                Ok(Next::Event(event)) => {
                    let payload = agent_event(&pane, &event);
                    if sender.blocking_send(payload).is_err() {
                        break;
                    }
                }
                // No event arrived before the poll ended. The connection stays
                // open: the browser waits for the next one.
                Ok(Next::Idle) => {}
                // Herdr closed the subscription. The stream ends.
                Ok(Next::Closed) => break,
                Err(error) => {
                    let payload = serde_json::to_string(&json!({"error": error.to_string()}))
                        .unwrap_or_else(|_| "{}".to_owned());
                    let _ = sender.blocking_send(payload);
                    break;
                }
            }
        }
    });
    let stream = tokio_stream::wrappers::ReceiverStream::new(receiver)
        .map(|payload| Ok::<_, std::convert::Infallible>(sse_event("agent", payload)));
    axum::response::sse::Sse::new(state.until_shutdown(stream))
        .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
        .into_response()
}

/// One SSE event with a name and a JSON body.
fn sse_event(name: &str, data: String) -> axum::response::sse::Event {
    axum::response::sse::Event::default().event(name).data(data)
}

/// The event the portal pushes for one Herdr event.
fn agent_event(pane: &str, event: &Value) -> String {
    let kind = event["event"].as_str().unwrap_or_default().to_owned();
    let state = event["data"]["agent_status"].as_str().map(str::to_owned);
    let payload = AgentEvent {
        kind,
        pane: pane.to_owned(),
        state,
        blocked: None,
    };
    serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_owned())
}

/// The agent Herdr sees in `pane`, if it sees one.
fn agent_of(socket: &Client, pane: &str) -> Option<Value> {
    socket
        .agent_list()
        .ok()?
        .into_iter()
        .find(|agent| agent["pane_id"].as_str() == Some(pane))
}

/// The kind of a native session reference: a path or an id.
/// The title of an agent, from the start of its transcript.
///
/// A transcript only grows, and its first message never changes, so a title
/// read once for a path is kept. A path read before it had a first message
/// is read again when its size changes.
fn title_of(agent: &Value) -> Option<String> {
    static TITLES: std::sync::Mutex<std::collections::BTreeMap<String, (u64, Option<String>)>> =
        std::sync::Mutex::new(std::collections::BTreeMap::new());
    let harness = agent["agent"].as_str()?;
    let transcript = agent["agent_session"]["value"].as_str()?;
    let path = crate::transcript::resolve(Some(harness), kind_of(agent), transcript)?;
    let size = std::fs::metadata(&path).ok()?.len();
    let mut titles = TITLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((seen, title)) = titles.get(&path)
        && (title.is_some() || *seen == size)
    {
        return title.clone();
    }
    let title = crate::activity::title(harness, &path);
    titles.insert(path, (size, title.clone()));
    title
}

fn kind_of(agent: &Value) -> &str {
    agent["agent_session"]["kind"].as_str().unwrap_or("path")
}

/// The prompt a blocked agent waits on, read from its screen.
fn read_blocked(socket: &Client, pane: &str) -> Option<portal_types::agents::BlockedPrompt> {
    let read = socket.pane_read(pane, "visible", Some(SCREEN_LINES)).ok()?;
    let text = read["text"].as_str()?;
    crate::activity::blocked_prompt(text)
}

/// A JSON body, as the API serves it.
fn json_response<T: serde::Serialize>(body: Json<T>) -> Response {
    body.into_response()
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
            repository(&shell, "/checkout").as_deref(),
            Some("github.com/owner/repo")
        );
        shell.on("git -C /checkout remote get-url origin", "/local/repo");
        assert_eq!(repository(&shell, "/checkout"), None);
    }

    /// The state of the API, with or without a Herdr socket.
    fn state(socket: Option<Client>) -> AppState {
        let (sender, receiver) = watch::channel(String::new());
        sender.send_replace(String::new());
        AppState::new(
            receiver,
            PORT,
            socket.map(Arc::new),
            crate::web::guard::Token::new(String::new()),
            crate::ledger::Ledger::new(
                std::env::temp_dir().join(format!("dogsled-agents-{}", crate::ledger::new_id())),
            )
            .child("web"),
        )
    }

    /// The state of the API, talking to a fake Herdr.
    fn with_socket(server: &fake::Server) -> AppState {
        state(Some(server.client()))
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
        use std::io::{BufRead as _, Read as _, Write as _};
        use std::os::unix::net::UnixListener;
        use std::time::Duration;

        let path = std::env::temp_dir().join(format!("hs-stop-{}.sock", crate::ledger::new_id()));
        let listener = UnixListener::bind(&path).expect("fake Herdr socket");
        let peer = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("subscription connects");
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .expect("read timeout");
            let mut line = String::new();
            std::io::BufReader::new(&socket)
                .read_line(&mut line)
                .expect("subscription request");
            let request: serde_json::Value = serde_json::from_str(&line).expect("request JSON");
            assert_eq!(request["method"], "events.subscribe");
            writeln!(
                socket,
                "{}",
                json!({"id": request["id"], "result": {"type": "subscription_started"}})
            )
            .expect("subscription acknowledgement");
            // No events arrive. Only dropping the subscription can wake its reader.
            let mut buffer = [0; 1];
            loop {
                match socket.read(&mut buffer) {
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    result => break result.expect("read until the subscription closes"),
                }
            }
        });
        let state = state(Some(Client::at(&path)));
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
        assert_eq!(peer.join().expect("fake Herdr finishes"), 0);
        std::fs::remove_file(path).expect("remove socket");
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
            with_socket(&server),
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
            with_socket(&server),
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
        let response = call(
            with_socket(&server),
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
        let response = call(
            with_socket(&server),
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
            other => panic!("unexpected method {other}"),
        });
        let response = call(
            with_socket(&server),
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
        let server = fake::EventServer::start(vec![
            r#"{"event":"pane.agent_status_changed","data":{"pane_id":"w1:p1","agent_status":"blocked"}}"#
                .to_owned(),
        ]);
        let response = call(
            state(Some(server.client())),
            request(Method::GET, "/api/agents/w1:p1/events", None),
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
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("the body");
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("event: agent"), "the event name: {text}");
        assert!(text.contains("\"kind\":\"pane.agent_status_changed\""));
        assert!(text.contains("\"state\":\"blocked\""));
    }

    #[tokio::test]
    async fn the_agent_api_reports_a_missing_herdr_socket() {
        let response = call(state(None), request(Method::GET, "/api/agents", None)).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
