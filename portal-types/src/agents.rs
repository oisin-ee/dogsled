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
    /// The task the agent was given: the first line of its first own message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Canonical repository identity derived from the checkout origin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
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
    pub session: String,
    pub turns: Vec<Turn>,
}

/// The text on an agent's terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screen {
    pub pane: String,
    pub session: String,
    pub text: String,
    pub revision: u64,
}

/// Body of the prompt request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptBody {
    pub text: String,
}

/// Body of the lead creation request: a registered repository and the first prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateLeadBody {
    pub repo: String,
    pub prompt: String,
}

/// A registered repository a lead can start in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoChoice {
    pub origin: String,
    pub path: String,
}

/// The registered repositories, in config order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoList {
    pub repos: Vec<RepoChoice>,
}

/// A lead that started and accepted its first prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedLead {
    pub pane: String,
}

/// Body of the keys request. Only the keys the server allows are accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeysBody {
    pub keys: Vec<String>,
}

/// A session-less agent cannot supply a cache key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "availability", content = "key", rename_all = "snake_case")]
pub enum DetailSession {
    Available(String),
    Unavailable,
}

/// One executing tool. No arguments, output, or reasoning cross this boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveTool {
    pub call_id: String,
    pub name: String,
    pub parent_call_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LivePhase {
    Idle,
    Working,
    Thinking,
    Responding,
    ToolRunning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveState {
    pub phase: LivePhase,
    pub active_tools: Vec<ActiveTool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProducerUnavailable {
    Resyncing,
    Overflow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProducerPayload {
    Snapshot { state: LiveState },
    Unavailable { reason: ProducerUnavailable },
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProducerEvidence {
    pub session_file: String,
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterProducer {
    pub version: u32,
    pub epoch: Option<String>,
    pub expected_generation: u64,
    pub producer_id: String,
    pub evidence: ProducerEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishActivity {
    pub version: u32,
    pub epoch: String,
    pub generation: u64,
    pub producer_id: String,
    pub session: String,
    pub sequence: u64,
    pub payload: ProducerPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityAck {
    pub version: u32,
    pub pane: String,
    pub epoch: String,
    pub generation: u64,
    pub producer_id: String,
    pub session: String,
    pub sequence: u64,
    pub heartbeat_ms: u64,
    pub ttl_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    UnsupportedHarness,
    SessionUnverified,
    NativeUnavailable,
    NoProducer,
    NoSnapshot,
    Expired,
    ProducerClosed,
    Resyncing,
    Overflow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "availability", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActivityDetail {
    Available {
        sequence: u64,
        received_at_ms: u64,
        valid_for_ms: u64,
        state: LiveState,
    },
    Unavailable {
        reason: UnavailableReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityProducer {
    pub generation: u64,
    pub id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityView {
    pub version: u32,
    pub pane: String,
    pub epoch: String,
    pub revision: u64,
    pub session: Option<String>,
    pub coarse_state: Option<String>,
    pub producer: ActivityProducer,
    pub detail: ActivityDetail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityCode {
    InvalidRequest,
    UnsupportedVersion,
    Unauthorized,
    AgentMissing,
    SessionUnverified,
    SessionChanged,
    StaleGeneration,
    SequenceConflict,
    ProducerClosed,
    UpstreamInvalid,
    UpstreamUnavailable,
    Capacity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityError {
    pub error: String,
    pub code: ActivityCode,
}

/// A change the agent stream pushes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentEvent {
    pub session: DetailSession,
    /// The Herdr event name, such as `pane.agent_status_changed`.
    pub kind: String,
    pub pane: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<BlockedPrompt>,
}
