//! Wire types for `dogsled web`. The server builds them from its board; the portal UI reads
//! them. This crate holds data only, so it compiles to wasm32 as well.

pub mod agents;
pub mod files;
pub mod inbox;
pub mod metrics;
pub mod models;
pub mod questions;
pub mod reviews;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

/// Portal payload version; independent of the host observation IPC envelope.
pub const PORTAL_VERSION: u32 = 2;

/// One ordered portal stream carries either a coherent snapshot or connection status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PortalEvent {
    Snapshot(Box<PortalSnapshot>),
    Status(PortalStatus),
}

/// The required discriminator cannot occur in a snapshot's status shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortalStatus {
    pub observation: PortalAvailability,
    pub error: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortalAvailability {
    Unavailable,
    Disconnected,
}

/// One coherent portal update. All payloads and gaps belong to this exact publication.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortalSnapshot {
    pub version: u32,
    /// The Dogsled release of the server that serves the portal, such as `0.42.0`. The
    /// observation leaves it out; the portal server sets it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dogsled: String,
    pub epoch: String,
    pub revision: u64,
    pub published_ms: u64,
    pub board: Option<BoardView>,
    pub agents: Option<Vec<agents::AgentView>>,
    pub inbox: Option<inbox::InboxView>,
    pub repos: Option<agents::RepoList>,
    pub gaps: Vec<ObservationGap>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationAvailability {
    Available,
    Unavailable,
    Failed,
    Unsupported,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationGap {
    pub domain: String,
    pub availability: ObservationAvailability,
    pub detail: String,
}

/// Everything the board shows, grouped by repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardView {
    pub hosts: Vec<HostView>,
    pub repos: Vec<RepoGroup>,
    pub generated_ms: u64,
}

/// One repository's rows and todos. `repository: None` is the "Unsorted" group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoGroup {
    pub repository: Option<String>,
    pub rows: Vec<RowView>,
    pub todos: Vec<TodoView>,
}

/// What the owner must do about a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupView {
    ToLand,
    Attention,
    Running,
    Waiting,
}

/// A worker clock is usable only while its sampled live phase is valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerClock {
    pub clock: agents::TurnClock,
    pub valid_until_ms: u64,
}

impl WorkerClock {
    #[must_use]
    pub fn seconds(self, now_ms: u64) -> Option<u64> {
        if now_ms >= self.valid_until_ms {
            return None;
        }
        self.clock.seconds(now_ms)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RowView {
    pub group: GroupView,
    pub kind: String,
    /// The last known owner or execution is not current. Never route actions from this row.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ownership_stale: bool,
    pub id: String,
    pub name: String,
    pub what: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_pane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<Cleanup>,
    /// A worker in idle/done/closed state whose newest message to its owner is unread.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unseen: bool,
    pub state: String,
    pub activity: String,
    pub age_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_clock: Option<WorkerClock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u64>,
    #[serde(default)]
    pub dirty: bool,
    /// The canonical `host/owner/name` of the agent's repository, when it is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
}

/// The known reason a finished row needs cleanup, and the next action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cleanup {
    pub cause: CleanupCause,
    pub action: CleanupAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupCause {
    NeverMerged,
    Dirty,
    MergedNotPushed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupAction {
    Land,
    Commit,
    PushMain,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostView {
    /// Name reported by the machine, independent of the viewer’s routing alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    pub label: String,
    pub service: String,
    pub reachable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy: Option<f64>,
    pub cores: u32,
    pub eligible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatusView {
    InProgress,
    Pending,
    Completed,
    Abandoned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoChildView {
    pub status: TodoStatusView,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub unit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItemView {
    pub status: TodoStatusView,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub review: bool,
    pub unit: Option<String>,
    pub children: Vec<TodoChildView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoSectionView {
    pub status: TodoStatusView,
    pub count: usize,
    pub items: Vec<TodoItemView>,
}

/// One agent's todo sections.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoView {
    pub session: String,
    pub branch: Option<String>,
    pub revision: u64,
    pub current: String,
    pub pane: String,
    pub sections: Vec<TodoSectionView>,
}
