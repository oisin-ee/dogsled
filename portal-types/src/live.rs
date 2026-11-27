//! Browser live socket. Portal publications include the inbox projection.

use serde::{Deserialize, Serialize};

use crate::PortalEvent;
use crate::agents::{ActivityView, AgentAddress, AgentEvent, Turn};
use crate::inbox::InboxView;

/// Replace the selected-agent subscription. The browser assigns a fresh ID even when
/// returning to the same address, so already-sent frames cannot reach the new view.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientFrame {
    Open {
        id: u64,
        address: Option<AgentAddress>,
        #[serde(default)]
        conversation: Option<ConversationCursor>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    Portal {
        event: PortalEvent,
    },
    Inbox {
        result: Result<InboxView, String>,
    },
    Agent {
        id: u64,
        event: Box<AgentEvent>,
    },
    AgentUnavailable {
        id: u64,
        error: String,
    },
    Activity {
        id: u64,
        view: Box<ActivityView>,
    },
    ActivityUnavailable {
        id: u64,
    },
    Conversation {
        id: u64,
        update: Box<ConversationUpdate>,
    },
}

/// Identifies a conversation snapshot within a server's publication epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationCursor {
    pub token: String,
    pub revision: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversationUpdate {
    Confirm {
        cursor: ConversationCursor,
    },
    Reset {
        cursor: ConversationCursor,
        session: String,
        turns: Vec<Turn>,
        truncated: bool,
    },
    Splice {
        base: ConversationCursor,
        cursor: ConversationCursor,
        drop_front: usize,
        keep: usize,
        turns: Vec<Turn>,
        truncated: bool,
    },
    Unavailable {
        error: String,
    },
}

/// Keep the specified range from the base, then append the supplied turns.
#[must_use]
pub fn splice(
    base: &[Turn],
    drop_front: usize,
    keep: usize,
    turns: Vec<Turn>,
) -> Option<Vec<Turn>> {
    let kept = base.get(drop_front..drop_front.checked_add(keep)?)?;
    let mut result = kept.to_vec();
    result.extend(turns);
    Some(result)
}

/// Find the run to keep at the start of the next history. Prefer a common prefix;
/// if the history lost turns at the front, start at the first matching turn instead.
#[must_use]
pub fn diff(base: &[Turn], next: &[Turn]) -> Option<(usize, usize)> {
    let first = next.first()?;
    let drop_front = base.iter().position(|turn| turn == first)?;
    let keep = base[drop_front..]
        .iter()
        .zip(next)
        .take_while(|(base, next)| base == next)
        .count();
    Some((drop_front, keep))
}
