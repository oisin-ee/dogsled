//! Wire types for the portal's plan reviews: a plan an agent submits, the
//! revisions of it, the owner's annotations, and the decision that goes back
//! to the agent's session.

use serde::{Deserialize, Serialize};

/// One plan under review, with everything the owner added to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub id: String,
    /// The Herdr pane of the submitting agent.
    pub pane: String,
    /// The Pi session that submitted the plan and receives the decision.
    pub session: String,
    pub cwd: String,
    /// The plan file's path, as the agent named it.
    pub path: String,
    /// The submitted revisions, oldest first. Revision numbers start at 1.
    pub revisions: Vec<Revision>,
    /// The draft annotations. They all point at the latest revision.
    pub annotations: Vec<Annotation>,
    /// The comment on the plan as a whole.
    pub general: String,
    /// The owner's decision. A review with a decision is closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
}

/// One submitted version of the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision {
    pub n: u32,
    pub content: String,
    pub submitted_ms: u64,
}

/// A comment on a line range of one revision. Lines count from 1 and both ends
/// are included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Annotation {
    pub id: String,
    pub revision: u32,
    pub start_line: usize,
    pub end_line: usize,
    /// The text of the lines the comment is about.
    pub quote: String,
    pub comment: String,
    /// Absolute paths of attachment images, as `POST /api/attachments` returned
    /// them.
    #[serde(default)]
    pub images: Vec<String>,
}

/// What the owner decided.
///
/// The record of a decision keeps its text, so the portal can show it again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Decision {
    Approved {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notes: Option<String>,
    },
    ChangesRequested {
        message: String,
    },
}

impl Decision {
    /// The name of the decision, as the event carries it.
    #[must_use]
    pub const fn kind(&self) -> DecisionKind {
        match self {
            Self::Approved { .. } => DecisionKind::Approved,
            Self::ChangesRequested { .. } => DecisionKind::ChangesRequested,
        }
    }
}

/// The name of a decision.
///
/// The Pi client reads the name of the decision as a plain string, so the
/// event carries the name and not the whole record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Approved,
    ChangesRequested,
}

/// How a line differs between two revisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Same,
    Added,
    Removed,
}

/// One line of a diff. `old` and `new` are 1-based line numbers; an added line
/// has no `old` and a removed line has no `new`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineChange {
    pub kind: ChangeKind,
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub text: String,
}

/// The line diff between two revisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffView {
    pub from: u32,
    pub to: u32,
    pub changes: Vec<LineChange>,
}

/// Body of `POST /api/reviews`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitBody {
    pub path: String,
    pub content: String,
    pub pane: String,
    pub session: String,
    pub cwd: String,
}

/// Answer of `POST /api/reviews`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submitted {
    pub id: String,
    pub revision: u32,
}

/// One row of `GET /api/reviews`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSummary {
    pub id: String,
    pub pane: String,
    pub session: String,
    pub path: String,
    pub revisions: usize,
    pub annotations: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
}

/// Answer of `GET /api/reviews`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewList {
    pub reviews: Vec<ReviewSummary>,
}

/// Body of `PUT /api/reviews/{id}/annotations`: the whole draft list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnotationsBody {
    pub annotations: Vec<Annotation>,
}

/// Body of `PUT /api/reviews/{id}/general`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneralBody {
    pub general: String,
}

/// Body of `POST /api/reviews/{id}/approve`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApproveBody {
    #[serde(default)]
    pub notes: Option<String>,
}

/// Answer of `POST /api/attachments`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// The absolute path of the stored image.
    pub path: String,
}

/// The event `GET /api/sessions/{session}/decisions` pushes, named `decision`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionEvent {
    pub review_id: String,
    pub path: String,
    /// The name of the decision: `approved` or `changes_requested`.
    pub decision: DecisionKind,
    /// The text the agent receives: the notes of an approval, or the built
    /// change request.
    pub message: String,
    /// The latest revision of the plan.
    pub plan_content: String,
}
