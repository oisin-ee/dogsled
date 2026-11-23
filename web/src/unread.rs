//! Sidebar receipts refer to an exact rendered assistant record, never to selection alone.
use dioxus::prelude::*;
use portal_types::agents::{AgentView, Conversation, reply_edge};

fn receipt(agent: &AgentView) -> Option<String> {
    let reply = agent.reply.as_ref()?;
    serde_json::to_string(&(&agent.host, &agent.pane, &reply.session, &reply.id)).ok()
}

pub(crate) fn matching_reply(agent: &AgentView, snapshot: &Conversation) -> Option<String> {
    let reply = agent.reply.as_ref()?;
    let edge = reply_edge(&snapshot.turns)?;
    (agent.pane == snapshot.pane && reply.session == snapshot.session && edge.id == reply.id)
        .then(|| receipt(agent))
        .flatten()
}

#[component]
pub(crate) fn UnreadDot(host: String, pane: String) -> Element {
    let Some(portal) = try_consume_context::<crate::app::AgentPortal>() else {
        return rsx! {};
    };
    let agents = portal.agents.read();
    let key = agents
        .as_ref()
        .and_then(|agents| agents.iter().find(|a| a.host == host && a.pane == pane))
        .and_then(receipt);
    rsx! {
        if let Some(key) = key {
            dogsled-unread-dot { "data-receipt": key, "role": "img", "aria-label": "Unread reply", "title": "Unread reply" }
        }
    }
}
