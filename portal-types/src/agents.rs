//! Wire types for the agent view: the list, one agent's conversation and screen, the blocked
//! prompt an agent waits on, and the bodies of the requests that act on an agent.

use serde::{Deserialize, Serialize};

/// One choice in a blocked prompt. `key` is what the portal sends to answer it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptOption {
    pub key: String,
    pub label: String,
}

/// What a blocked agent asks, read from its screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockedPrompt {
    pub question: String,
    pub options: Vec<PromptOption>,
}

/// One agent in the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentView {
    pub pane: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Set only while the agent is blocked and its screen shows a prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<BlockedPrompt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentList {
    pub agents: Vec<AgentView>,
}

/// Who spoke a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

/// One turn of a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub role: Role,
    pub text: String,
}

/// An agent's turns from its transcript, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    pub pane: String,
    pub turns: Vec<Turn>,
}

/// The text on an agent's terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screen {
    pub pane: String,
    pub text: String,
    pub revision: u64,
}

/// Body of the prompt request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptBody {
    pub text: String,
}

/// Body of the keys request. Only the keys the server allows are accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeysBody {
    pub keys: Vec<String>,
}

/// A change the agent stream pushes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentEvent {
    /// The Herdr event name, such as `pane.agent_status_changed`.
    pub kind: String,
    pub pane: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<BlockedPrompt>,
}
