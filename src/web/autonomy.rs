//! Switch mutations run on the owning host. Decisions come from one local publication.

use super::{
    AppState,
    agents::{PeerOperation, peer_response},
};
use crate::autonomy::{self, PublishedAutonomy};
use crate::hosts::LOCAL;
use crate::ledger::{Ledger, now_ms};
use crate::observation::{ipc, owned_work, read};
use axum::extract::{Path, State, rejection::JsonRejection};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use portal_types::PortalSnapshot;
use portal_types::autonomy::{DecisionList, SwitchBody};
use portal_types::inbox::HostGap;
use serde_json::json;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/autonomy/fleet", post(fleet_switch))
        .route("/api/agents/{host}/{pane}/autonomy", post(agent_switch))
        .route("/api/agents/{host}/{pane}/decisions", get(decisions))
        .layer(axum::extract::DefaultBodyLimit::max(1024))
}

fn failure(status: StatusCode, message: &str) -> Response {
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"error": message})),
    )
        .into_response()
}

fn unavailable() -> Response {
    failure(
        StatusCode::SERVICE_UNAVAILABLE,
        "Autonomy observation unavailable",
    )
}

fn ok<T: serde::Serialize>(body: T) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}

async fn fleet_switch(
    State(state): State<AppState>,
    body: Result<Json<SwitchBody>, JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return failure(
            StatusCode::BAD_REQUEST,
            "Expected an autonomy switch body with an on flag",
        );
    };
    if body.on {
        return local_fleet(state.web, body).await;
    }
    let ledger = state.web.clone();
    let Ok(Ok(Some(view))) = tokio::task::spawn_blocking(move || {
        let published = ipc::Client::at(&ledger.dir).current(&ipc::Select::views(["portal"]))?;
        Ok::<_, crate::error::Error>(read::view::<PortalSnapshot>(&published, "portal")?.autonomy)
    })
    .await
    else {
        return unavailable();
    };
    // Clear the local row even if the last publication predates a recent local switch-on.
    // Try every published holder before returning a failed host's response.
    let remote = view
        .fleet
        .into_iter()
        .filter(|label| label != LOCAL)
        .map(|host| {
            peer_response(
                state.clone(),
                host,
                String::new(),
                PeerOperation::SetFleetAutonomy(body),
            )
        });
    let (local, remote) = tokio::join!(
        local_fleet(state.web.clone(), body),
        futures_util::future::join_all(remote)
    );
    std::iter::once(local)
        .chain(remote)
        .find(|response| !response.status().is_success())
        .unwrap_or_else(|| ok(body))
}

pub(super) async fn local_fleet(ledger: Ledger, body: SwitchBody) -> Response {
    match tokio::task::spawn_blocking(move || autonomy::set_fleet(&ledger, body.on, now_ms())).await
    {
        // Autonomy's store writes notify the observer after the transaction commits.
        Ok(Ok(())) => ok(body),
        _ => failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not save the fleet autonomy switch",
        ),
    }
}

async fn agent_switch(
    State(state): State<AppState>,
    Path((host, pane)): Path<(String, String)>,
    body: Result<Json<SwitchBody>, JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return failure(
            StatusCode::BAD_REQUEST,
            "Expected an autonomy switch body with an on flag",
        );
    };
    if host != LOCAL {
        return peer_response(state, host, pane, PeerOperation::SetAgentAutonomy(body)).await;
    }
    local_agent(state.web, pane, body).await
}

pub(super) async fn local_agent(ledger: Ledger, pane: String, body: SwitchBody) -> Response {
    use AgentSwitch::{Missing, Owned, Saved, SessionMissing};
    match tokio::task::spawn_blocking(move || save_agent_switch(&ledger, &pane, body)).await {
        Ok(Ok(Saved)) => ok(body),
        Ok(Ok(Owned)) => failure(
            StatusCode::CONFLICT,
            "Workers inherit autonomy from their owner",
        ),
        Ok(Ok(Missing)) => failure(StatusCode::NOT_FOUND, "Agent pane not found"),
        Ok(Ok(SessionMissing)) => {
            failure(StatusCode::CONFLICT, "The agent pane has no live session")
        }
        _ => failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "Could not read or save the agent autonomy switch",
        ),
    }
}

#[derive(Debug)]
enum AgentSwitch {
    Missing,
    Owned,
    Saved,
    SessionMissing,
}

fn save_agent_switch(
    ledger: &Ledger,
    pane: &str,
    body: SwitchBody,
) -> crate::error::Result<AgentSwitch> {
    let client = ipc::Client::at(&ledger.dir);
    let published = client.current(&ipc::Select::local(["host"]))?;
    if published
        .local
        .get("host")
        .and_then(owned_work::decode)
        .is_some_and(|evidence| {
            evidence.executions.iter().any(|execution| {
                execution.kind == owned_work::Kind::Agent && execution.pane == pane
            })
        })
    {
        return Ok(AgentSwitch::Owned);
    }
    let Some(agent) = crate::probe::agent_list(&crate::shell::System::default())?
        .into_iter()
        .find(|agent| agent.pane == pane)
    else {
        return Ok(AgentSwitch::Missing);
    };
    if crate::launch::has_owned_launch(ledger, &agent)? {
        return Ok(AgentSwitch::Owned);
    }
    let Some(session) = agent
        .agent_session
        .as_deref()
        .filter(|session| !session.is_empty())
    else {
        return Ok(AgentSwitch::SessionMissing);
    };
    autonomy::set_agent(ledger, pane, session, body.on, now_ms())?;
    Ok(AgentSwitch::Saved)
}

async fn decisions(
    State(state): State<AppState>,
    Path((host, pane)): Path<(String, String)>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || decision_list(&state.web, &host, &pane)).await;
    match result {
        Ok(Ok(list)) => ok(list),
        _ => unavailable(),
    }
}

fn decision_list(ledger: &Ledger, host: &str, pane: &str) -> crate::error::Result<DecisionList> {
    let published =
        ipc::Client::at(&ledger.dir).current(&ipc::Select::views([autonomy::VIEW, "portal"]))?;
    let view = read::projected::<PublishedAutonomy>(&published, autonomy::VIEW)?;
    let snapshot = read::view::<PortalSnapshot>(&published, "portal")?;
    let local_hostname = (host == LOCAL).then(crate::inbox::hostname);
    let hostname = snapshot
        .board
        .as_ref()
        .and_then(|board| {
            board
                .hosts
                .iter()
                .find(|candidate| candidate.label == host)?
                .hostname
                .as_deref()
        })
        .or(local_hostname.as_deref());
    let mut decisions: Vec<_> = view
        .data
        .into_iter()
        .flat_map(|data| data.decisions)
        .filter(|decision| {
            (decision.host == host && decision.pane == pane)
                || hostname.is_some_and(|hostname| {
                    autonomy::decision_owner(decision) == Some((hostname, pane))
                })
        })
        .collect();
    decisions.sort_unstable_by(|left, right| {
        right
            .created_ms
            .cmp(&left.created_ms)
            .then_with(|| right.id.cmp(&left.id))
    });
    let gaps = view
        .gaps
        .into_iter()
        .filter_map(|gap| {
            Some(HostGap {
                host: gap.domain.strip_prefix(autonomy::GAP_PREFIX)?.to_owned(),
                error: gap.detail,
            })
        })
        .collect();
    Ok(DecisionList { decisions, gaps })
}

#[cfg(test)]
mod tests;
