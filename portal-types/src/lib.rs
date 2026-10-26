//! Wire types for `dogsled web`. The server builds them from its board; the portal UI reads
//! them. This crate holds data only, so it compiles to wasm32 as well.

use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RowView {
    pub group: GroupView,
    pub kind: String,
    pub id: String,
    pub name: String,
    pub what: String,
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    pub state: String,
    pub activity: String,
    pub age_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u64>,
    #[serde(default)]
    pub dirty: bool,
    /// The canonical `host/owner/name` of the agent's repository, when it is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostView {
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
