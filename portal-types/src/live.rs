//! Browser live socket. Portal publications include the inbox projection.

use serde::{Deserialize, Serialize};

use crate::PortalEvent;
use crate::agents::{ActivityView, AgentAddress, AgentEvent};
use crate::inbox::InboxView;

/// Replace the selected-agent subscription. The browser assigns a fresh ID even when
/// returning to the same address, so already-sent frames cannot reach the new view.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientFrame {
    Open {
        id: u64,
        address: Option<AgentAddress>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    Portal { event: PortalEvent },
    Inbox { result: Result<InboxView, String> },
    Agent { id: u64, event: Box<AgentEvent> },
    AgentUnavailable { id: u64, error: String },
    Activity { id: u64, view: Box<ActivityView> },
    ActivityUnavailable { id: u64 },
}
