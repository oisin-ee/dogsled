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

/// A pane is unique only within its fleet host.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AgentAddress {
    pub host: String,
    pub pane: String,
}

impl AgentAddress {
    #[must_use]
    pub fn local(pane: impl Into<String>) -> Self {
        Self {
            host: "local".into(),
            pane: pane.into(),
        }
    }
}

/// One agent in the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentView {
    pub host: String,
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
    Context,
    Tool,
}

/// What the recorded timestamp measures, not an inferred display time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeMeaning {
    /// The harness message timestamp. Pi assistant times mark request start,
    /// not response completion.
    Message,
    Recorded,
    Created,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timestamp {
    pub ms: u64,
    pub meaning: TimeMeaning,
}

/// Provenance supplied by the transcript, never recovered from message prose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    Harness {
        harness: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
    },
    Custom {
        custom_type: String,
    },
    Dogsled {
        sender: String,
        delivery_kind: String,
    },
}

/// Historical evidence only. An unmatched call is not a live running tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Unknown,
    Completed,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSummary {
    pub call_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Completed means a result was recorded, not that its payload claims success.
    pub status: ToolStatus,
}

/// One chronological record: message text, attributed context, or a tool summary.
/// Tool records have empty text; arguments, output, and reasoning are never included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub id: String,
    pub role: Role,
    pub text: String,
    pub source: Source,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<ToolSummary>,
}

/// An agent's turns from its transcript, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    pub pane: String,
    pub session: String,
    pub turns: Vec<Turn>,
    /// At least one read or record bound omitted history.
    pub truncated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn agent_event_without_revisions_deserializes() -> Result<(), serde_json::Error> {
        let wire = json!({
            "session":{"availability":"unavailable"},
            "kind":"pane.agent_status_changed", "pane":"agent-pane"
        });
        let event: AgentEvent = serde_json::from_value(wire.clone())?;
        assert_eq!(event.revisions, None);
        assert_eq!(serde_json::to_value(event)?, wire);
        Ok(())
    }

    #[test]
    fn conversation_metadata_and_tool_evidence_round_trip() -> Result<(), serde_json::Error> {
        let wire = json!({
            "pane":"agent-pane", "session":"session-key", "truncated":true,
            "turns":[
                {"id":"native-message","role":"assistant","text":"reply",
                 "source":{"kind":"harness","harness":"pi","model":"recorded-model","provider":"recorded-provider"},
                 "timestamp":{"ms":42,"meaning":"message"}},
                {"id":"delivery","role":"context","text":"report",
                 "source":{"kind":"dogsled","sender":"worker","delivery_kind":"message"},
                 "timestamp":{"ms":40,"meaning":"created"}},
                {"id":"tool-entry","role":"tool","text":"",
                 "source":{"kind":"harness","harness":"claude"},
                 "timestamp":{"ms":43,"meaning":"recorded"},
                 "tool":{"call_id":"tool-id","name":"read","status":"unknown"}},
                {"id":"custom","role":"context","text":"context",
                 "source":{"kind":"custom","custom_type":"extension-note"}}
            ]
        });
        let conversation: Conversation = serde_json::from_value(wire.clone())?;
        assert!(conversation.truncated);
        assert_eq!(serde_json::to_value(conversation)?, wire);
        Ok(())
    }
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

/// One accessible, non-archived GitHub repository. Forks are included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoChoice {
    pub slug: String,
    pub description: Option<String>,
    pub pushed_at: Option<String>,
    pub private: bool,
}

/// The published catalogue, newest push first. Failed refreshes retain the last list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoList {
    pub repos: Vec<RepoChoice>,
    pub error: Option<String>,
}

/// A lead that started and accepted its first prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedLead {
    pub host: String,
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

/// Independent revisions of an agent's conversation and screen content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetailRevisions {
    pub conversation: u64,
    pub screen: u64,
}

/// A change the agent stream pushes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentEvent {
    /// Absent when the peer does not supply part revisions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revisions: Option<DetailRevisions>,
    pub session: DetailSession,
    /// The Herdr event name, such as `pane.agent_status_changed`.
    pub kind: String,
    pub pane: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<BlockedPrompt>,
}
