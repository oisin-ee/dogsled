//! Serialized facts and decisions. The session branch is the only protocol store.

use serde::{Deserialize, Serialize};

/// A concrete session model selection, including its thinking level.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(deny_unknown_fields)]
pub struct Selection {
    /// Provider-qualified model ID.
    pub model: String,
    /// pi thinking level.
    pub thinking: Thinking,
}

/// Thinking levels accepted by pi 1.1.0.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(rename_all = "lowercase")]
pub enum Thinking {
    /// No reasoning budget.
    Off,
    /// Minimum reasoning budget.
    Minimal,
    /// Low reasoning budget.
    Low,
    /// Default reasoning budget.
    #[default]
    Medium,
    /// High reasoning budget.
    High,
    /// Extra-high reasoning budget.
    Xhigh,
    /// Maximum reasoning budget.
    Max,
}

/// Validated effective config.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Arm new sessions.
    #[serde(default)]
    pub enabled: bool,
    /// Handoff destination. No automatic cheapest-model fallback.
    pub target: Option<Selection>,
}

/// Config and all file errors, for notifications and status.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct ConfigReport {
    /// Effective config, or defaults when validation fails.
    pub config: Config,
    /// File-specific errors.
    pub errors: Vec<String>,
}

/// The only persisted protocol states.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum State {
    /// Waiting for a gated implementation call.
    Arm {
        /// Selection at arm time.
        planner: Selection,
        /// Resolved handoff selection.
        target: Selection,
        /// Config at the last input, to detect live changes.
        enabled: bool,
    },
    /// Successful one-shot handoff.
    Handoff {
        /// Original planner.
        planner: Selection,
        /// Effective target after pi clamps thinking.
        target: Selection,
        /// Config at the last input.
        enabled: bool,
    },
    /// Canceled, consumed no-op, or never armed.
    Off {
        /// Config at the last input.
        enabled: bool,
    },
}

/// Normalized call evidence. Nested calls retain their own success status.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct Call {
    /// Registered tool name.
    pub name: String,
    /// False for errors and unfinished nested calls.
    pub success: bool,
}

/// Branch records used by the protocol, oldest first.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Record {
    /// A validated prewalk state entry.
    State { state: State },
    /// A prewalk hidden message.
    Prompt { id: String, name: String },
    /// One tool result, with nested calls before the outer result.
    Calls { calls: Vec<Call> },
    /// A persisted context removal.
    Scrub { id: String },
    /// A later explicit selection. It supersedes handoff restoration.
    Selection,
    /// A user message, used to distinguish new sessions from resumes.
    User,
}

/// Explicit startup flags. Values are read only on initial startup.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct Flags {
    /// --prewalk.
    pub prewalk: bool,
    /// --no-prewalk.
    pub no_prewalk: bool,
    /// --prewalk-into.
    pub into: Option<String>,
}

/// Event facts that affect the protocol.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Event {
    /// Session startup/replacement.
    Start { reason: String, flags: Flags },
    /// Live config re-read before input.
    Input,
    /// Completed assistant turn.
    Turn { calls: Vec<Call> },
    /// Final actionable boundary.
    Settle,
    /// Projection before the next LLM request.
    Context,
    /// /prewalk subcommand.
    Command { command: String },
}

/// All environment facts, without runtime authority.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct Facts {
    /// Current branch.
    pub branch: Vec<Record>,
    /// Previous session branch for /new.
    pub previous: Vec<Record>,
    /// Current selection, if a model is available.
    pub current: Option<Selection>,
    /// Current config and errors.
    pub report: ConfigReport,
    /// Whether the configured or flag target has credentials.
    pub target_available: bool,
    /// Credentialed model IDs.
    pub available: Vec<String>,
    /// Resolved configured default for restart.
    pub default: Option<Selection>,
    /// Is todo callable (directly or through codemode)?
    pub todo: bool,
    /// Current provider activation; absent preserves the legacy successful-todo rule.
    #[serde(default)]
    pub active_unit: Option<bool>,
    /// Todo source identity for status.
    pub todo_owner: String,
    /// The runtime event.
    pub event: Event,
}

/// Adapter actions, in execution order.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Action {
    /// Persist a state entry.
    Store { state: State },
    /// Append a hidden prompt.
    Prompt { name: String, content: String },
    /// Remove a planning nudge from projected context.
    Scrub { id: String },
    /// Select model and thinking. A failed selection aborts subsequent actions.
    Select { selection: Selection },
    /// Human-facing info.
    Notify { message: String },
}

/// One pure protocol result.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct Decision {
    /// Ordered effects.
    pub actions: Vec<Action>,
    /// Request one continuation at an actionable boundary.
    pub again: bool,
}

/// The branch projection boundary input.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct Branch {
    /// Oldest entry first.
    pub records: Vec<Record>,
}

/// Context projection policy and deferred persistent removals.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct Projection {
    /// Only the latest planning prompt can belong to a live arm.
    pub keep_plan: bool,
    /// Planning prompts outside the live cycle, not already removed.
    pub scrub: Vec<String>,
}
