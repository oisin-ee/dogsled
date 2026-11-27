//! The portal stream, independent agent detail reads, and actions.

use futures_util::{SinkExt, StreamExt};
use gloo_net::http::Request;
use gloo_net::websocket::{Message, futures::WebSocket};
use portal_types::PortalEvent;
use portal_types::agents::{CreateLeadBody, KeysBody, LeadStartView, PromptBody, Screen};
use portal_types::autonomy::{DecisionList, SwitchBody};
use portal_types::live::{ClientFrame, ServerFrame};
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
    upload_image_on("local", file).await
}

pub async fn upload_image_on(host: &str, file: &web_sys::File) -> Result<String, String> {
    if !matches!(
        file.type_().as_str(),
        "image/png" | "image/jpeg" | "image/webp"
    ) {
        return Err("Use PNG, JPEG or WebP images.".to_owned());
    }
    if file.size() > MAX_IMAGE_BYTES {
        return Err("Each image must be at most 10 MiB.".to_owned());
    }
    let request = Request::post(&host_url(host, "attachments"))
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

/// Read the live Pi session's settings from its execution host.
/// The live settings of a Pi session, or `None` while the session has published none. A Pi
/// session started before its Dogsled extension could publish settings never does; that is
/// not a failure to show.
pub async fn agent_settings(
    address: &portal_types::agents::AgentAddress,
) -> Result<Option<portal_types::models::AgentSettings>, String> {
    let response = Request::get(&agent_url(address, "settings"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.ok() {
        return response
            .json()
            .await
            .map(Some)
            .map_err(|error| error.to_string());
    }
    let status = response.status();
    let body = response.json::<serde_json::Value>().await.ok();
    settings_failure(status, body.as_ref())
}

/// A missing publication reads as no settings; any other failure keeps its reason.
fn settings_failure(
    status: u16,
    body: Option<&serde_json::Value>,
) -> Result<Option<portal_types::models::AgentSettings>, String> {
    match body {
        Some(body) if status == 404 && body["code"] == "settings_unavailable" => Ok(None),
        Some(body) => Err(body["error"]
            .as_str()
            .map_or_else(|| format!("the server answered {status}"), str::to_owned)),
        None => Err(format!("the server answered {status}")),
    }
}

/// Apply a live change; the reply contains Pi's effective settings, not an optimistic selection.
pub async fn change_settings(
    address: &portal_types::agents::AgentAddress,
    change: &portal_types::models::SettingsChange,
) -> Result<portal_types::models::AgentSettings, String> {
    let request = Request::post(&agent_url(address, "settings"))
        .json(change)
        .map_err(|error| error.to_string())?;
    read_json(request.send().await.map_err(|error| error.to_string())?).await
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

/// Read the serve-owned model catalogue for new conversations.
pub async fn models() -> Result<portal_types::models::ModelCatalogue, String> {
    read_json(
        Request::get(&format!("{BASE}/api/models"))
            .send()
            .await
            .map_err(|error| error.to_string())?,
    )
    .await
}

/// Read fleet metrics for the selected window, tool grouping, and run grouping.
pub async fn fetch_metrics(
    window: portal_types::metrics::Window,
    group: portal_types::metrics::ToolGroup,
    run_group: portal_types::metrics::RunGroup,
) -> Result<portal_types::metrics::MetricsReport, String> {
    use portal_types::metrics::{RunGroup, ToolGroup, Window};
    let window = match window {
        Window::Day => "day",
        Window::Week => "week",
        Window::Month => "month",
    };
    let group = match group {
        ToolGroup::Tool => "tool",
        ToolGroup::Repository => "repository",
        ToolGroup::Model => "model",
        ToolGroup::Host => "host",
        ToolGroup::Lead => "lead",
        ToolGroup::Harness => "harness",
    };
    let run_group = match run_group {
        RunGroup::Profile => "profile",
        RunGroup::Model => "model",
        RunGroup::Harness => "harness",
    };
    let url = format!("{BASE}/api/metrics?window={window}&group={group}&run_group={run_group}");
    read_json(
        Request::get(&url)
            .send()
            .await
            .map_err(|error| error.to_string())?,
    )
    .await
}

/// Build the first-message request, omitting settings that use Pi's defaults.
pub(crate) fn create_lead_body(
    repo: &str,
    prompt: &str,
    model: Option<portal_types::models::ModelRef>,
    thinking: Option<portal_types::models::ThinkingLevel>,
) -> portal_types::agents::CreateLeadBody {
    portal_types::agents::CreateLeadBody {
        repo: repo.to_owned(),
        prompt: prompt.to_owned(),
        model,
        thinking,
    }
}

pub(crate) fn lead_key() -> Result<String, String> {
    let mut bytes = [0; 16];
    let unavailable = "Could not generate a lead request key.";
    let crypto = web_sys::window()
        .ok_or(unavailable)?
        .crypto()
        .map_err(|_| unavailable)?;
    crypto
        .get_random_values_with_u8_array(&mut bytes)
        .map_err(|_| unavailable)?;
    Ok(format!("{:032x}", u128::from_be_bytes(bytes)))
}

enum LeadPostFailure {
    Lost,
    Refused(String),
}

/// Accept a lead. Only a lost reply gets another attempt, always with the original key and body.
pub(crate) async fn create_lead(key: &str, body: &CreateLeadBody) -> Result<LeadStartView, String> {
    retry_lead(key, body, post_lead, dioxus_sdk_time::sleep).await
}

async fn retry_lead<F: std::future::Future<Output = ()>>(
    key: &str,
    body: &CreateLeadBody,
    mut post: impl AsyncFnMut(&str, &CreateLeadBody) -> Result<LeadStartView, LeadPostFailure>,
    mut sleep: impl FnMut(std::time::Duration) -> F,
) -> Result<LeadStartView, String> {
    for delay in [250, 500, 1000] {
        match post(key, body).await {
            Ok(view) => return Ok(view),
            Err(LeadPostFailure::Refused(error)) => return Err(error),
            Err(LeadPostFailure::Lost) => sleep(std::time::Duration::from_millis(delay)).await,
        }
    }
    match post(key, body).await {
        Ok(view) => Ok(view),
        Err(LeadPostFailure::Refused(error)) => Err(error),
        Err(LeadPostFailure::Lost) => {
            Err("Could not confirm that the request was saved. The lead may still start.".into())
        }
    }
}

async fn post_lead(key: &str, body: &CreateLeadBody) -> Result<LeadStartView, LeadPostFailure> {
    let response = Request::post(&format!("{BASE}/api/leads"))
        .header("Idempotency-Key", key)
        .json(body)
        .map_err(|error| LeadPostFailure::Refused(error.to_string()))?
        .send()
        .await
        .map_err(|_| LeadPostFailure::Lost)?;
    let status = response.status();
    let text = response.text().await.map_err(|_| LeadPostFailure::Lost)?;
    if status == 202 {
        return serde_json::from_str(&text).map_err(|_| {
            LeadPostFailure::Refused(
                "Could not read the saved request. The lead may still start.".into(),
            )
        });
    }
    if (500..600).contains(&status) && text.trim().is_empty() {
        return Err(LeadPostFailure::Lost);
    }
    let error = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|body| body["error"].as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("The server answered {status}."));
    Err(LeadPostFailure::Refused(error))
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

pub async fn switch_fleet(on: bool) -> Result<(), String> {
    post(&format!("{BASE}/api/autonomy/fleet"), &SwitchBody { on }).await
}

pub async fn switch_agent(
    address: &portal_types::agents::AgentAddress,
    on: bool,
) -> Result<(), String> {
    post(&agent_url(address, "autonomy"), &SwitchBody { on }).await
}

pub async fn decisions(
    address: &portal_types::agents::AgentAddress,
) -> Result<DecisionList, String> {
    let response = Request::get(&agent_url(address, "decisions"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    read_json(response).await
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
pub(crate) fn encode(value: &str) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[derive(Clone)]
pub(crate) struct InteractionHost(pub String);

pub(crate) fn host_url(host: &str, resource: &str) -> String {
    format!("{BASE}/api/hosts/{}/{resource}", encode(host))
}

pub(crate) fn attachment_url(host: &str, path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or_default();
    host_url(host, &format!("attachments/{}", encode(name)))
}

/// Follow the portal and the current selection without reopening the socket on navigation.
pub async fn watch(
    changes: &mut (impl futures_util::Stream<Item = ()> + Unpin),
    mut selection: impl FnMut() -> ClientFrame,
    mut on_frame: impl FnMut(ServerFrame),
) -> Result<(), String> {
    let socket = WebSocket::open(&format!("{BASE}/api/live")).map_err(|error| error.to_string())?;
    use futures_util::future::{Either, select};

    let (mut send, mut receive) = socket.split();
    let mut stamp = None;
    // Wait for the server's first publication before sending. A failed upgrade
    // reaches the stream, not the sink waiting for the socket to open.
    let mut incoming = Some(
        receive
            .next()
            .await
            .ok_or("the portal live socket closed")?,
    );
    let mut last_sent = None;
    loop {
        if let Some(item) = incoming.take() {
            match item {
                Ok(Message::Text(text)) => on_frame(decode_live(&text, &mut stamp)?),
                Ok(Message::Bytes(_)) => return Err("invalid portal live frame".into()),
                Err(error) => return Err(error.to_string()),
            }
        }
        let command @ ClientFrame::Open { id, .. } = selection();
        if last_sent != Some(id) {
            let text = serde_json::to_string(&command).map_err(|error| error.to_string())?;
            send.send(Message::Text(text))
                .await
                .map_err(|error| error.to_string())?;
            last_sent = Some(id);
        }
        match select(receive.next(), changes.next()).await {
            Either::Left((Some(item), _)) => incoming = Some(item),
            Either::Right((Some(()), _)) => {}
            _ => return Err("the portal live socket closed".into()),
        }
    }
}

pub(crate) fn decode_live(
    text: &str,
    stamp: &mut Option<(String, u64)>,
) -> Result<ServerFrame, String> {
    let frame: ServerFrame = serde_json::from_str(text)
        .map_err(|error| format!("the portal live socket sent invalid data: {error}"))?;
    if let ServerFrame::Portal { event } = &frame {
        validate_portal(event, stamp)?;
    }
    Ok(frame)
}

/// Validate the portal payload independently of the observation IPC version.
fn validate_portal(event: &PortalEvent, stamp: &mut Option<(String, u64)>) -> Result<(), String> {
    if let PortalEvent::Snapshot(snapshot) = event {
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
    Ok(())
}

#[cfg(test)]
mod lead_tests {
    use super::*;
    use futures_util::FutureExt;

    #[test]
    fn lost_post_reuses_the_key_and_body_for_at_most_three_retries() {
        let body = create_lead_body("owner/repo", "First prompt", None, None);
        let receipt = LeadStartView {
            id: "saved".into(),
            revision: 1,
            state: portal_types::agents::LeadStartState::Accepted,
            name: None,
            address: None,
            phase: None,
            error: None,
            created_ms: 1,
        };
        for succeeds in [true, false] {
            let mut attempts = 0;
            let mut delays = Vec::new();
            let actual = retry_lead(
                "one-send",
                &body,
                async |key, attempted| {
                    assert_eq!(key, "one-send");
                    assert_eq!(attempted, &body);
                    attempts += 1;
                    if succeeds && attempts == 2 {
                        Ok(receipt.clone())
                    } else {
                        Err(LeadPostFailure::Lost)
                    }
                },
                |delay| {
                    delays.push(delay.as_millis());
                    std::future::ready(())
                },
            )
            .now_or_never()
            .unwrap();
            if succeeds {
                assert_eq!(actual, Ok(receipt.clone()));
                assert_eq!(attempts, 2);
                assert_eq!(delays, [250]);
            } else {
                assert!(actual.unwrap_err().contains("may still start"));
                assert_eq!(attempts, 4);
                assert_eq!(delays, [250, 500, 1000]);
            }
        }
    }

    #[test]
    fn a_server_refusal_does_not_retry() {
        let body = create_lead_body("owner/repo", "First prompt", None, None);
        let actual = retry_lead(
            "one-send",
            &body,
            async |_, _| Err(LeadPostFailure::Refused("Conflict".into())),
            |_| async { panic!("a refusal cannot enter backoff") },
        )
        .now_or_never()
        .unwrap();
        assert_eq!(actual, Err("Conflict".into()));
    }
}

#[cfg(test)]
mod catalogue_tests {
    use super::{decode_live, settings_failure};

    fn decode_portal(
        text: &str,
        stamp: &mut Option<(String, u64)>,
    ) -> Result<portal_types::PortalEvent, String> {
        match decode_live(&format!(r#"{{"type":"portal","event":{text}}}"#), stamp)? {
            portal_types::live::ServerFrame::Portal { event } => Ok(event),
            _ => unreachable!("decoded portal frame"),
        }
    }
    use serde_json::json;

    #[test]
    fn an_unpublished_session_has_no_settings_and_other_failures_keep_their_reason() {
        let unpublished =
            json!({"code":"settings_unavailable","error":"Pi settings are unavailable"});
        assert_eq!(settings_failure(404, Some(&unpublished)), Ok(None));
        let stopped = json!({"code":"settings_busy","error":"too many settings requests"});
        assert_eq!(
            settings_failure(503, Some(&stopped)),
            Err("too many settings requests".to_owned())
        );
        assert_eq!(
            settings_failure(502, Some(&unpublished)),
            Err("Pi settings are unavailable".to_owned())
        );
        assert_eq!(
            settings_failure(500, None),
            Err("the server answered 500".to_owned())
        );
    }

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
