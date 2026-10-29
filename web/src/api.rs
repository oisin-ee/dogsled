//! The portal API: one read of the board, then the stream of changes.

use futures_util::StreamExt;
use gloo_net::eventsource::futures::EventSource;
use gloo_net::http::Request;
use portal_types::BoardView;
use portal_types::agents::{
    AgentEvent, AgentList, AgentView, Conversation, KeysBody, PromptBody, Screen,
};
use portal_types::inbox::InboxView;
use serde::de::DeserializeOwned;

/// Where the API lives. Empty means the page's own origin; a dev build can
/// set `DOGSLED_API` at compile time.
const BASE: &str = match option_env!("DOGSLED_API") {
    Some(base) => base,
    None => "",
};

/// The current board.
pub async fn fetch_board() -> Result<BoardView, String> {
    let response = Request::get(&format!("{BASE}/api/board"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("the board answered {}", response.status()));
    }
    response.json().await.map_err(|error| error.to_string())
}

/// Read a JSON body, or the server's own `error` text when it refused.
async fn read_json<T: DeserializeOwned>(response: gloo_net::http::Response) -> Result<T, String> {
    if !response.ok() {
        return Err(failure(response).await);
    }
    response.json().await.map_err(|error| error.to_string())
}

/// The reason a response failed: the `error` field of its body, else its status.
async fn failure(response: gloo_net::http::Response) -> String {
    let status = response.status();
    match response.json::<serde_json::Value>().await {
        Ok(body) => body["error"]
            .as_str()
            .map_or_else(|| format!("the server answered {status}"), str::to_owned),
        Err(_) => format!("the server answered {status}"),
    }
}

/// The agents Herdr sees, including their blocked prompts.
pub async fn fetch_agents() -> Result<Vec<AgentView>, String> {
    let response = Request::get(&format!("{BASE}/api/agents"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let list: AgentList = read_json(response).await?;
    Ok(list.agents)
}

/// The server-owned actionable inbox. A failed read is not an empty inbox.
pub async fn fetch_inbox() -> Result<InboxView, String> {
    let response = Request::get(&format!("{BASE}/api/inbox"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    read_json(response).await
}

/// The agent in `pane`, with the prompt it waits on while it is blocked.
pub async fn fetch_agent(pane: &str) -> Result<Option<AgentView>, String> {
    Ok(fetch_agents()
        .await?
        .into_iter()
        .find(|agent| agent.pane == pane))
}

/// The turns of the agent in `pane`, or `None` when the agent no longer exists.
pub async fn fetch_conversation(pane: &str) -> Result<Option<Conversation>, String> {
    let response = Request::get(&format!("{BASE}/api/agents/{}/conversation", encode(pane)))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status() == 404 {
        return Ok(None);
    }
    read_json(response).await.map(Some)
}

/// Read the terminal of the agent in `pane`.
pub async fn fetch_screen(pane: &str) -> Result<Screen, String> {
    let response = Request::get(&format!("{BASE}/api/agents/{}/screen", encode(pane)))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    read_json(response).await
}

/// Open the agent in Herdr.
pub async fn focus_agent(pane: &str) -> Result<(), String> {
    let response = Request::post(&format!("{BASE}/api/agents/{}/focus", encode(pane)))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.ok() {
        Ok(())
    } else {
        Err(failure(response).await)
    }
}

/// Send a prompt to the agent in `pane`.
pub async fn send_prompt(pane: &str, text: &str) -> Result<(), String> {
    let body = PromptBody {
        text: text.to_owned(),
    };
    post(&format!("{BASE}/api/agents/{}/prompt", encode(pane)), &body).await
}

/// Answer a blocked prompt with one key.
pub async fn send_key(pane: &str, key: &str) -> Result<(), String> {
    let body = KeysBody {
        keys: vec![key.to_owned()],
    };
    post(&format!("{BASE}/api/agents/{}/keys", encode(pane)), &body).await
}

async fn post<T: serde::Serialize>(url: &str, body: &T) -> Result<(), String> {
    let response = Request::post(url)
        .json(body)
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.ok() {
        Ok(())
    } else {
        Err(failure(response).await)
    }
}

/// A pane id as a path segment.
fn encode(pane: &str) -> String {
    String::from(js_sys::encode_uri_component(pane))
}

/// Call `on_event` for each change the agent in `pane` pushes, until the stream fails.
pub async fn watch_agent(pane: &str, mut on_event: impl FnMut(AgentEvent)) -> Result<(), String> {
    let mut source = EventSource::new(&format!("{BASE}/api/agents/{}/events", encode(pane)))
        .map_err(|error| error.to_string())?;
    let mut stream = source
        .subscribe("agent")
        .map_err(|error| error.to_string())?;
    while let Some(item) = stream.next().await {
        let (_, message) = item.map_err(|error| error.to_string())?;
        if let Some(text) = message.data().as_string() {
            let event = serde_json::from_str::<AgentEvent>(&text)
                .map_err(|error| format!("the agent stream sent an invalid event: {error}"))?;
            on_event(event);
        }
    }
    Err("the agent stream ended".to_owned())
}

/// Hand board and inbox snapshots to their owners, retaining subscriptions
/// across transport failures while native EventSource reconnects.
pub async fn watch(
    mut on_board: impl FnMut(BoardView),
    mut on_inbox: impl FnMut(Result<InboxView, String>),
    mut on_paused: impl FnMut(bool),
) -> Result<(), String> {
    let mut source =
        EventSource::new(&format!("{BASE}/api/stream")).map_err(|error| error.to_string())?;
    let subscriptions = ["board", "inbox", "inbox_error"]
        .into_iter()
        .map(|name| {
            source
                .subscribe(name)
                .map(|stream| stream.map(move |item| (name, item)).boxed_local())
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut stream = futures_util::stream::select_all(subscriptions);
    while let Some((name, item)) = stream.next().await {
        let Ok((_, message)) = item else {
            on_paused(true);
            continue;
        };
        if let Some(text) = message.data().as_string() {
            match name {
                "board" => {
                    let view = serde_json::from_str::<BoardView>(&text)
                        .map_err(|error| format!("the board stream sent invalid data: {error}"))?;
                    on_board(view);
                }
                "inbox" => on_inbox(
                    serde_json::from_str::<InboxView>(&text)
                        .map_err(|error| format!("the inbox stream sent invalid data: {error}")),
                ),
                "inbox_error" => on_inbox(Err(text)),
                _ => {}
            }
            on_paused(false);
        }
    }
    Err("the portal stream ended".to_owned())
}
