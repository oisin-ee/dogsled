//! Model and thinking choices for Pi sessions and new leads.
//!
//! A live Pi session publishes its [`AgentSettings`]; the portal requests a
//! [`SettingsChange`]. Serve publishes one [`ModelCatalogue`] per host for new leads.

use serde::{Deserialize, Serialize};

/// Pi's thinking levels, in Pi's order. A model supports a subset; Pi clamps the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ThinkingLevel {
    /// The Pi flag and wire value, such as `xhigh`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// One model by Pi's identity: `provider/id`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRef {
    pub provider: String,
    pub id: String,
}

impl ModelRef {
    /// The `provider/id` form that Pi's `--model` flag accepts.
    #[must_use]
    pub fn pattern(&self) -> String {
        format!("{}/{}", self.provider, self.id)
    }
}

/// One selectable model and the thinking levels it supports, `[off]` without reasoning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelChoice {
    pub model: ModelRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub thinking: Vec<ThinkingLevel>,
}

/// A live Pi session's selection and the models it can select.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSettings {
    /// Absent while the session has no model.
    pub model: Option<ModelRef>,
    /// The effective level, after Pi clamps it to the model.
    pub thinking: ThinkingLevel,
    pub models: Vec<ModelChoice>,
}

/// A change the portal requests. An absent field stays as it is; at least one is set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsChange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingLevel>,
}

/// The models a new lead can start with on this host. A failed refresh keeps the last list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCatalogue {
    pub models: Vec<ModelChoice>,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_and_changes_use_pi_wire_names() {
        let choice: ModelChoice = serde_json::from_str(
            r#"{"model":{"provider":"openai","id":"gpt-5"},"name":"GPT-5","thinking":["off","xhigh","max"]}"#,
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(choice.model.pattern(), "openai/gpt-5");
        assert_eq!(
            choice.thinking,
            [ThinkingLevel::Off, ThinkingLevel::Xhigh, ThinkingLevel::Max]
        );
        let change = SettingsChange {
            model: None,
            thinking: Some(ThinkingLevel::High),
        };
        assert_eq!(
            serde_json::to_string(&change).unwrap_or_default(),
            r#"{"thinking":"high"}"#
        );
        assert!(serde_json::from_str::<SettingsChange>(r#"{"effort":"high"}"#).is_err());
        assert_eq!(ThinkingLevel::Xhigh.as_str(), "xhigh");
    }
}
