//! Actionable items in the portal's unified inbox.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxKind {
    Review,
    Question,
    Prompt,
    UnseenWorker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxItem {
    pub kind: InboxKind,
    pub id: String,
    pub pane: String,
    pub title: String,
    pub agent_title: Option<String>,
    pub repository: Option<String>,
    pub age_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxView {
    pub count: usize,
    pub items: Vec<InboxItem>,
}
