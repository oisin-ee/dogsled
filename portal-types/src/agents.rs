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

/// Exact final record of an idle turn, qualified by the conversation session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyStamp {
    pub session: String,
    pub id: String,
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
    /// Model label from the execution host's current transcript sample.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<ReplyStamp>,
    /// How far the Dogsled launch that owns the pane got: `allocating`, `allocated`,
    /// `attempted`, `confirmed` or `ambiguous`. A pane no launch started, such as a lead's,
    /// has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<String>,
    /// Why the agent decides without its owner. Absent while it asks its owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomous: Option<crate::autonomy::AutonomyCause>,
    /// The recorded decisions of the agent and of the agents it owns. Absent when none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decisions: Option<crate::autonomy::DecisionMark>,
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
    Thinking,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<String>,
    /// The bounded tail of the recorded result, up to 20 lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_lines: Option<u32>,
}

/// How an agent said its turn ends, with the `end_turn` tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndTurnKind {
    /// The turn ends with this message, and nothing else.
    Reply,
    /// The whole task is complete.
    Done,
    /// The agent needs an answer from its owner.
    Ask,
    /// The agent cannot continue.
    Blocked,
    /// The agent waits for its own open children.
    Waiting,
}

impl EndTurnKind {
    pub const ALL: [Self; 5] = [
        Self::Reply,
        Self::Done,
        Self::Ask,
        Self::Blocked,
        Self::Waiting,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reply => "reply",
            Self::Done => "done",
            Self::Ask => "ask",
            Self::Blocked => "blocked",
            Self::Waiting => "waiting",
        }
    }

    /// The label the portal shows on a pinned message. A plain reply has none.
    #[must_use]
    pub fn label(self) -> Option<&'static str> {
        match self {
            Self::Reply => None,
            Self::Done => Some("Done"),
            Self::Ask => Some("Question"),
            Self::Blocked => Some("Blocked"),
            Self::Waiting => Some("Waiting"),
        }
    }
}

impl std::str::FromStr for EndTurnKind {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == text)
            .ok_or_else(|| {
                format!("unknown end_turn kind {text:?}; use reply, done, ask, blocked or waiting")
            })
    }
}

/// The tool names that end a turn: Pi's native tool, and the dogsled MCP tool under any
/// server prefix, such as `mcp__dogsled__end_turn` or Claude's plugin prefix.
#[must_use]
pub fn is_end_turn_tool(name: &str) -> bool {
    name == "end_turn" || (name.starts_with("mcp__") && name.ends_with("dogsled__end_turn"))
}

/// One chronological record: message text, thinking, attributed context, or tool detail.
/// Tool records have empty text; their bounded call and result are in `tool`. An `end_turn`
/// call is a tool record whose text is the message it ends the turn with.
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
    /// Set on an `end_turn` call record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_turn: Option<EndTurnKind>,
}

impl Turn {
    /// The kind of the end-of-turn message this record pins: an `end_turn` call with a
    /// recorded result that is not an error. A refused call pins nothing.
    #[must_use]
    pub fn pinned(&self) -> Option<EndTurnKind> {
        self.end_turn.filter(|_| {
            self.tool
                .as_ref()
                .is_some_and(|tool| tool.status == ToolStatus::Completed)
        })
    }

    /// A displayed system message that started another run: an extension message such as
    /// a todo reminder, or a dogsled delivery.
    #[must_use]
    pub fn is_nudge(&self) -> bool {
        self.role == Role::Context
            && matches!(self.source, Source::Custom { .. } | Source::Dogsled { .. })
    }
}

/// Transcript clock for the latest user turn. Running clocks use the viewer's wall clock;
/// finished clocks stop at the latest dated non-user record in that turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnClock {
    Running { started_ms: u64 },
    Finished { elapsed_ms: u64 },
}

impl TurnClock {
    #[must_use]
    pub fn from_history(turns: &[Turn], phase: LivePhase) -> Option<Self> {
        let (user, work) = current_turn(turns).split_first()?;
        let started_ms = user
            .timestamp
            .as_ref()
            .filter(|_| user.role == Role::User)?
            .ms;
        if phase == LivePhase::Idle {
            Some(Self::Finished {
                elapsed_ms: latest_record_time(work)?.checked_sub(started_ms)?,
            })
        } else {
            Some(Self::Running { started_ms })
        }
    }

    #[must_use]
    pub fn seconds(self, now_ms: u64) -> Option<u64> {
        Some(
            match self {
                Self::Running { started_ms } => now_ms.checked_sub(started_ms)?,
                Self::Finished { elapsed_ms } => elapsed_ms,
            } / 1000,
        )
    }
}

#[must_use]
pub fn current_turn(turns: &[Turn]) -> &[Turn] {
    &turns[turns
        .iter()
        .rposition(|turn| turn.role == Role::User)
        .unwrap_or(0)..]
}

/// The record that holds an agent's final reply in `turns` (oldest first): the newest
/// pinned `end_turn` since the newest owner message. Without one, the newest record that is
/// not context, when it is an assistant message. The host computes the same record from the
/// raw transcript for the reply stamp.
#[must_use]
pub fn reply_edge(turns: &[Turn]) -> Option<&Turn> {
    current_turn(turns)
        .iter()
        .rev()
        .find(|turn| turn.pinned().is_some())
        .or_else(|| {
            turns
                .iter()
                .rev()
                .find(|turn| turn.role != Role::Context)
                .filter(|turn| turn.role == Role::Assistant)
        })
}

#[must_use]
pub fn latest_record_time(turns: &[Turn]) -> Option<u64> {
    turns
        .iter()
        .filter(|turn| turn.role != Role::User)
        .filter_map(|turn| turn.timestamp.as_ref().map(|time| time.ms))
        .max()
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
    fn prompt_without_reply_deserializes() -> Result<(), serde_json::Error> {
        let body: PromptBody = serde_json::from_value(json!({"text":"go on"}))?;
        assert_eq!(body.text, "go on");
        assert!(!body.reply);
        Ok(())
    }

    #[test]
    fn agent_event_without_revisions_deserializes() -> Result<(), serde_json::Error> {
        let wire = json!({
            "session":{"availability":"unavailable"},
            "kind":"pane.agent_status_changed", "pane":"agent-pane"
        });
        let event: AgentEvent = serde_json::from_value(wire.clone())?;
        assert_eq!(event.revisions, None);
        assert_eq!(event.pending, None);
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
    /// the text answers the agent's blocked prompt
    #[serde(default)]
    pub reply: bool,
}

/// Body of the lead creation request: a registered repository and the first prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateLeadBody {
    pub repo: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<crate::models::ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<crate::models::ThinkingLevel>,
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

/// One executing tool with a bounded display line derived from its arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveTool {
    pub call_id: String,
    pub name: String,
    pub parent_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LivePhase {
    Idle,
    Working,
    Thinking,
    Responding,
    ToolRunning,
    /// Pi is summarising the session to fit its context window.
    Compacting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveState {
    pub phase: LivePhase,
    pub active_tools: Vec<ActiveTool>,
    /// Bounded thinking text from the in-flight assistant message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
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

/// Independent revisions of an agent's conversation, screen and files content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetailRevisions {
    pub conversation: u64,
    pub screen: u64,
    #[serde(default)]
    pub files: u64,
}

/// Parts whose content is not ready to read yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingParts {
    pub conversation: bool,
    pub screen: bool,
    pub files: bool,
}

/// A change the agent stream pushes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentEvent {
    /// Absent when the peer does not supply part revisions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revisions: Option<DetailRevisions>,
    /// Absent when the peer does not supply pending-part metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<PendingParts>,
    pub session: DetailSession,
    /// The Herdr event name, such as `pane.agent_status_changed`.
    pub kind: String,
    pub pane: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<BlockedPrompt>,
}
