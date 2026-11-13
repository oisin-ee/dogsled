//! The portal stream, independent agent detail reads, and actions.

use futures_util::StreamExt;
use gloo_net::eventsource::futures::EventSource;
use gloo_net::http::Request;
use portal_types::PortalEvent;
use portal_types::agents::{ActivityView, AgentEvent, Conversation, KeysBody, PromptBody, Screen};
use serde::de::DeserializeOwned;

/// Where the API lives. Empty means the page's own origin; a dev build can
/// set `DOGSLED_API` at compile time.
const BASE: &str = match option_env!("DOGSLED_API") {
    Some(base) => base,
    None => "",
};

const MAX_IMAGE_BYTES: f64 = 10.0 * 1024.0 * 1024.0;

/// Store an image and return the server-owned reference path.
pub async fn upload_image(file: &web_sys::File) -> Result<String, String> {
    if !matches!(
        file.type_().as_str(),
        "image/png" | "image/jpeg" | "image/webp"
    ) {
        return Err("Use PNG, JPEG or WebP images.".to_owned());
    }
    if file.size() > MAX_IMAGE_BYTES {
        return Err("Each image must be at most 10 MiB.".to_owned());
    }
    let request = Request::post(&format!("{BASE}/api/attachments"))
        .body(file.clone())
        .map_err(|error| error.to_string())?;
    let attachment: portal_types::reviews::Attachment =
        read_json(request.send().await.map_err(|error| error.to_string())?).await?;
    Ok(attachment.path)
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

/// The turns of the agent in `pane`, or `None` when the agent no longer exists.
pub async fn fetch_conversation(
    address: &portal_types::agents::AgentAddress,
) -> Result<Option<Conversation>, String> {
    let response = Request::get(&agent_url(address, "conversation"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status() == 404 {
        return Ok(None);
    }
    read_json(response).await.map(Some)
}

/// Read a complete activity snapshot, separately from transcript and lifecycle reads.
pub async fn fetch_activity(
    address: &portal_types::agents::AgentAddress,
) -> Result<ActivityView, crate::activity::Gap> {
    use crate::activity::Gap;
    let response = Request::get(&agent_url(address, "activity"))
        .header("Cache-Control", "no-cache")
        .send()
        .await
        .map_err(|_| Gap::Disconnected)?;
    if !response.ok() {
        return Err(Gap::Disconnected);
    }
    response.json().await.map_err(|_| Gap::Invalid)
}

/// Follow only activity. Subscribe to error events explicitly: EventSource reconnecting
/// errors must clear detail too, even while the browser intends to reconnect.
pub async fn watch_activity(
    address: &portal_types::agents::AgentAddress,
    mut on_frame: impl FnMut(ActivityView) -> Result<(), crate::activity::Gap>,
) -> Result<(), crate::activity::Gap> {
    use crate::activity::Gap;
    let mut source =
        EventSource::new(&agent_url(address, "activity/events")).map_err(|_| Gap::Disconnected)?;
    let activity = source
        .subscribe("activity")
        .map_err(|_| Gap::Disconnected)?;
    let errors = source.subscribe("error").map_err(|_| Gap::Disconnected)?;
    let mut stream = futures_util::stream::select(
        activity.map(|item| (false, item)),
        errors.map(|item| (true, item)),
    );
    while let Some((error, item)) = stream.next().await {
        if error {
            return Err(Gap::Disconnected);
        }
        let (_, message) = item.map_err(|_| Gap::Disconnected)?;
        let text = message.data().as_string().ok_or(Gap::Invalid)?;
        if text.len() > 128 * 1024 {
            return Err(Gap::Invalid);
        }
        let view = serde_json::from_str::<ActivityView>(&text).map_err(|_| Gap::Invalid)?;
        on_frame(view)?;
    }
    Err(Gap::Disconnected)
}

/// Read the terminal of the agent in `pane`.
pub async fn fetch_screen(address: &portal_types::agents::AgentAddress) -> Result<Screen, String> {
    let response = Request::get(&agent_url(address, "screen"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    read_json(response).await
}

/// Read checkout changes independently from the conversation and terminal.
pub async fn fetch_files(
    address: &portal_types::agents::AgentAddress,
) -> Result<portal_types::files::FilesState, String> {
    let response = Request::get(&agent_url(address, "files"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    read_json(response).await
}

/// Open the agent in Herdr.
pub async fn focus_agent(address: &portal_types::agents::AgentAddress) -> Result<(), String> {
    let response = Request::post(&agent_url(address, "focus"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.ok() {
        Ok(())
    } else {
        Err(failure(response).await)
    }
}

/// Close the terminal at this host-qualified address.
pub async fn close_agent(address: &portal_types::agents::AgentAddress) -> Result<(), String> {
    let response = Request::post(&agent_url(address, "close"))
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
pub async fn send_prompt(
    address: &portal_types::agents::AgentAddress,
    text: &str,
    reply: bool,
) -> Result<(), String> {
    let body = PromptBody {
        text: text.to_owned(),
        reply,
    };
    post(&agent_url(address, "prompt"), &body).await
}

/// Start a lead in `repo` with its first prompt. The result is the lead's pane.
pub async fn create_lead(
    repo: &str,
    prompt: &str,
) -> Result<portal_types::agents::AgentAddress, String> {
    let body = portal_types::agents::CreateLeadBody {
        repo: repo.to_owned(),
        prompt: prompt.to_owned(),
        model: None,
        thinking: None,
    };
    let response = Request::post(&format!("{BASE}/api/leads"))
        .json(&body)
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;
    read_json::<portal_types::agents::AgentAddress>(response).await
}

/// Answer a blocked prompt with one key.
pub async fn send_key(
    address: &portal_types::agents::AgentAddress,
    key: &str,
) -> Result<(), String> {
    let body = KeysBody {
        keys: vec![key.to_owned()],
    };
    post(&agent_url(address, "keys"), &body).await
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

fn agent_url(address: &portal_types::agents::AgentAddress, operation: &str) -> String {
    format!(
        "{BASE}/api/agents/{}/{}/{operation}",
        encode(&address.host),
        encode(&address.pane)
    )
}

/// A pane id as a path segment.
fn encode(pane: &str) -> String {
    String::from(js_sys::encode_uri_component(pane))
}

/// Call `on_event` for each change the agent in `pane` pushes, until the stream fails.
pub async fn watch_agent(
    address: &portal_types::agents::AgentAddress,
    mut on_event: impl FnMut(AgentEvent),
) -> Result<(), String> {
    let mut source =
        EventSource::new(&agent_url(address, "events")).map_err(|error| error.to_string())?;
    let agents = source
        .subscribe("agent")
        .map_err(|error| error.to_string())?;
    let errors = source
        .subscribe("error")
        .map_err(|error| error.to_string())?;
    let mut stream = futures_util::stream::select(
        agents.map(|item| (false, item)),
        errors.map(|item| (true, item)),
    );
    while let Some((failed, item)) = stream.next().await {
        if failed {
            return Err(format!("host {}: agent stream disconnected", address.host));
        }
        let (_, message) = item.map_err(|error| error.to_string())?;
        if let Some(text) = message.data().as_string() {
            let event = serde_json::from_str::<AgentEvent>(&text)
                .map_err(|error| format!("the agent stream sent an invalid event: {error}"))?;
            on_event(event);
        }
    }
    Err("the agent stream ended".to_owned())
}

/// Deliver snapshots and connection statuses in their original wire order.
pub async fn watch(
    mut on_publication: impl FnMut(Result<PortalEvent, String>),
) -> Result<(), String> {
    let mut source =
        EventSource::new(&format!("{BASE}/api/stream")).map_err(|error| error.to_string())?;
    let mut stream = source
        .subscribe("portal")
        .map_err(|error| error.to_string())?;
    let mut stamp: Option<(String, u64)> = None;
    while let Some(item) = stream.next().await {
        let (_, message) = match item {
            Ok(item) => item,
            Err(error) => {
                on_publication(Err(error.to_string()));
                continue;
            }
        };
        if let Some(text) = message.data().as_string() {
            let event = decode_portal(&text, &mut stamp)?;
            on_publication(Ok(event));
        }
    }
    Err("the portal stream ended".to_owned())
}

/// Validate the portal payload independently of the observation IPC version.
fn decode_portal(text: &str, stamp: &mut Option<(String, u64)>) -> Result<PortalEvent, String> {
    let event = serde_json::from_str::<PortalEvent>(text)
        .map_err(|error| format!("the portal stream sent invalid data: {error}"))?;
    if let PortalEvent::Snapshot(snapshot) = &event {
        if snapshot.version != portal_types::PORTAL_VERSION {
            return Err(format!(
                "unsupported portal snapshot version: {}",
                snapshot.version
            ));
        }
        if snapshot.epoch.is_empty() || snapshot.revision == 0 {
            return Err("invalid observation revision".to_owned());
        }
        if stamp.as_ref().is_some_and(|(epoch, revision)| {
            *epoch == snapshot.epoch && *revision > snapshot.revision
        }) {
            return Err("observation publication is out of order".to_owned());
        }
        *stamp = Some((snapshot.epoch.clone(), snapshot.revision));
    }
    Ok(event)
}

#[cfg(test)]
mod catalogue_tests {
    use super::decode_portal;
    use serde_json::json;

    #[test]
    fn portal_decoder_requires_version_two_and_strict_ordered_shapes() {
        let mut stamp = None;
        let mut value = json!({"version":portal_types::PORTAL_VERSION,"epoch":"serve","revision":2,
            "published_ms":1,"board":null,"agents":null,"inbox":null,"repos":null,"gaps":[]});
        assert!(decode_portal(&value.to_string(), &mut stamp).is_ok());
        for version in [0, 1, 3] {
            value["version"] = json!(version);
            assert_eq!(
                decode_portal(&value.to_string(), &mut stamp).unwrap_err(),
                format!("unsupported portal snapshot version: {version}")
            );
        }
        value["version"] = json!(portal_types::PORTAL_VERSION);
        value["revision"] = json!(1);
        assert!(
            decode_portal(&value.to_string(), &mut stamp)
                .unwrap_err()
                .contains("out of order")
        );
        value["epoch"] = json!("restarted");
        assert!(decode_portal(&value.to_string(), &mut stamp).is_ok());
        value["observation"] = json!("disconnected");
        value["error"] = json!("offline");
        assert!(decode_portal(&value.to_string(), &mut stamp).is_err());
        assert!(
            decode_portal(
                r#"{"observation":"disconnected","error":"offline"}"#,
                &mut stamp
            )
            .is_ok()
        );
        assert_eq!(stamp, Some(("restarted".into(), 1)));
    }
}
