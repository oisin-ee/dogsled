//! Autonomy: which agents decide without their owner, and the decisions they recorded.
//!
//! A review's approval is also called a decision (`reviews`). The types here are the
//! record an agent writes with `dogsled decide`, never a review's outcome.

use serde::{Deserialize, Serialize};

/// The longest `title` of a decision, in bytes. A title is one line.
pub const TITLE_BYTES: usize = 200;
/// The longest `decision`, `why`, `question` or `alternatives` of a decision, in bytes.
pub const TEXT_BYTES: usize = 2000;

/// Why an agent is autonomous. The first cause that holds is the one reported: the fleet
/// switch, then the agent's own switch, then an autonomous owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyCause {
    Fleet,
    Agent,
    Owner,
}

/// The fleet's autonomy in one portal snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutonomyView {
    /// Labels of the hosts that hold the fleet switch on. Empty means the switch is off.
    pub fleet: Vec<String>,
    /// Labels of the reachable hosts whose release has no autonomy.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsupported: Vec<String>,
}

/// How many decisions an agent's page lists. A change of either field means that the list
/// changed, also when a host dropped an old decision for a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionMark {
    pub count: u32,
    pub newest_ms: u64,
}

/// The body of both switch routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwitchBody {
    pub on: bool,
}

/// What led to a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionSource {
    /// The agent settled a question that Dogsled refused or released.
    Question,
    /// Dogsled approved the agent's plan. `review` names the stored review.
    Plan,
    /// The agent recorded the decision unprompted.
    Agent,
}

/// What an agent gives `dogsled decide` and the `decide` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecideBody {
    pub title: String,
    pub decision: String,
    pub why: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alternatives: Option<String>,
}

impl DecideBody {
    /// Why the body cannot be recorded, or nothing when it can.
    #[must_use]
    pub fn refusal(&self) -> Option<String> {
        let title = self.title.trim();
        if title.is_empty() || title.len() > TITLE_BYTES || title.contains('\n') {
            return Some(format!(
                "title must be one line of 1 to {TITLE_BYTES} bytes"
            ));
        }
        let required = [("decision", &self.decision), ("why", &self.why)];
        let optional = [
            ("question", &self.question),
            ("alternatives", &self.alternatives),
        ];
        required
            .into_iter()
            .map(|(name, text)| (name, text.as_str(), true))
            .chain(
                optional
                    .into_iter()
                    .filter_map(|(name, text)| Some((name, text.as_deref()?, false))),
            )
            .find_map(|(name, text, required)| {
                let empty = text.trim().is_empty();
                ((required && empty) || text.len() > TEXT_BYTES).then(|| {
                    if empty {
                        format!("{name} must not be empty")
                    } else {
                        format!("{name} must be at most {TEXT_BYTES} bytes")
                    }
                })
            })
    }
}

/// One recorded decision, as the portal shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionView {
    pub id: String,
    /// The label of the host that recorded the decision.
    pub host: String,
    pub pane: String,
    /// The agent's name when Dogsled started it; a lead the owner started has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The hostname and pane of the agent's owner when it decided. A lead has neither.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_pane: Option<String>,
    pub created_ms: u64,
    pub source: DecisionSource,
    pub title: String,
    pub decision: String,
    pub why: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alternatives: Option<String>,
    /// The review that holds the approved plan, for a `plan` decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
}

/// The decisions of one agent and of the agents it owns, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionList {
    pub decisions: Vec<DecisionView>,
    /// Hosts whose decisions could not be read. Absent when every host answered.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<crate::inbox::HostGap>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> DecideBody {
        DecideBody {
            title: "Keep the CSV column order of v1".into(),
            decision: "Keep the v1 order. Add new columns last.".into(),
            why: "Existing dashboards read columns by position.".into(),
            question: None,
            alternatives: None,
        }
    }

    #[test]
    fn a_decision_needs_a_one_line_title_and_bounded_text() {
        assert_eq!(body().refusal(), None);
        for title in ["", "  ", "two\nlines", &"t".repeat(TITLE_BYTES + 1)] {
            let refused = DecideBody {
                title: title.into(),
                ..body()
            };
            assert_eq!(
                refused.refusal().as_deref(),
                Some("title must be one line of 1 to 200 bytes")
            );
        }
        let empty = DecideBody {
            why: " ".into(),
            ..body()
        };
        assert_eq!(empty.refusal().as_deref(), Some("why must not be empty"));
        let long = DecideBody {
            question: Some("q".repeat(TEXT_BYTES + 1)),
            ..body()
        };
        assert_eq!(
            long.refusal().as_deref(),
            Some("question must be at most 2000 bytes")
        );
        // An optional field can be absent, but a given one is still bounded, not required.
        let blank = DecideBody {
            alternatives: Some(String::new()),
            ..body()
        };
        assert_eq!(blank.refusal(), None);
    }

    #[test]
    fn a_lead_decision_omits_its_absent_fields() -> Result<(), serde_json::Error> {
        let decision = DecisionView {
            id: "d1".into(),
            host: "netcup".into(),
            pane: "w1:p1".into(),
            name: None,
            owner_host: None,
            owner_pane: None,
            created_ms: 7,
            source: DecisionSource::Plan,
            title: "Approved its own plan: plans/export.md".into(),
            decision: "Approved".into(),
            why: "Autonomous mode".into(),
            question: None,
            alternatives: None,
            review: Some("r1".into()),
        };
        let value = serde_json::to_value(&decision)?;
        assert_eq!(
            value,
            serde_json::json!({
                "id": "d1", "host": "netcup", "pane": "w1:p1", "created_ms": 7,
                "source": "plan", "title": "Approved its own plan: plans/export.md",
                "decision": "Approved", "why": "Autonomous mode", "review": "r1"
            })
        );
        assert_eq!(serde_json::from_value::<DecisionView>(value)?, decision);
        Ok(())
    }
}
