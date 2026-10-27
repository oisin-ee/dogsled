//! Wire types for the portal's questions: the form an agent asks, the pending
//! question in the store, and the answers the owner sends back.
//!
//! The form keeps the shape of pi-interview 0.13.0, so the model asks the way
//! it always did. Fields the schema does not name are kept as they came, so a
//! newer form still reaches the portal unchanged.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// The kind of a question. A kind the schema does not name is kept as it came.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionKind {
    Single,
    Multi,
    Text,
    Image,
    Info,
    Other(String),
}

impl QuestionKind {
    /// The kind a name means, or the name itself when it means none.
    fn from_name(name: &str) -> Self {
        match name {
            "single" => Self::Single,
            "multi" => Self::Multi,
            "text" => Self::Text,
            "image" => Self::Image,
            "info" => Self::Info,
            other => Self::Other(other.to_owned()),
        }
    }

    /// The name of the kind.
    fn name(&self) -> &str {
        match self {
            Self::Single => "single",
            Self::Multi => "multi",
            Self::Text => "text",
            Self::Image => "image",
            Self::Info => "info",
            Self::Other(name) => name,
        }
    }
}

impl Serialize for QuestionKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for QuestionKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Ok(Self::from_name(&name))
    }
}

/// One question of a form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: QuestionKind,
    pub question: String,
    /// The choices of a single or multi question. Each choice is a string or
    /// an object with a label and its content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conviction: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// The code or the text shown above the choices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<Value>,
    /// The fields the schema does not name.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The form an agent asks: the questions, and the title and the description
/// that frame them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionSet {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub questions: Vec<Question>,
    /// The fields the schema does not name.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A question that waits for an answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    pub id: String,
    pub pane: String,
    pub session: String,
    pub cwd: String,
    pub created_ms: u64,
    pub questions: QuestionSet,
}

/// The answers of one question, as the portal keeps them. The answers are the
/// responses of the form, so their shape belongs to the form and not to the
/// portal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub id: String,
    pub answers: Value,
}

/// One answer of one question, as the portal sends and returns it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseItem {
    pub id: String,
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<String>>,
}

/// The answers of a question, as the API sends and accepts them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Responses {
    pub id: String,
    pub responses: Vec<ResponseItem>,
}

/// The body of the request that asks a question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AskBody {
    pub questions: QuestionSet,
    #[serde(default)]
    pub pane: String,
    #[serde(default)]
    pub session: String,
    #[serde(default)]
    pub cwd: String,
}

/// The body of the request that answers a question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnswerBody {
    pub responses: Vec<ResponseItem>,
}

/// The id of a question the portal made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingId {
    pub id: String,
}

/// The pending questions, oldest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingList {
    pub pending: Vec<Pending>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A form of the shape pi-interview writes.
    const FORM: &str = r#"{
        "title": "Choice",
        "description": "one question",
        "questions": [
            {"id": "q1", "type": "single", "question": "Which one?", "options": ["A", "B"],
             "recommended": "B", "conviction": "strong", "weight": "critical"},
            {"id": "q2", "type": "text", "question": "Why?"}
        ],
        "unknown": {"kept": true}
    }"#;

    #[test]
    fn a_form_round_trips_with_its_unknown_fields() {
        let set: QuestionSet = serde_json::from_str(FORM).expect("the form");
        assert_eq!(set.title.as_deref(), Some("Choice"));
        assert_eq!(set.questions.len(), 2);
        assert_eq!(set.questions[0].kind, QuestionKind::Single);
        assert_eq!(set.questions[1].kind, QuestionKind::Text);
        assert_eq!(set.extra["unknown"]["kept"], serde_json::json!(true));
        let back = serde_json::to_string(&set).expect("the form again");
        let again: QuestionSet = serde_json::from_str(&back).expect("the form again");
        assert_eq!(set, again);
    }

    #[test]
    fn a_kind_the_schema_does_not_name_is_kept() {
        let set: QuestionSet =
            serde_json::from_str(r#"{"questions":[{"id":"q1","type":"select","question":"?"}]}"#)
                .expect("the form");
        assert_eq!(
            set.questions[0].kind,
            QuestionKind::Other("select".to_owned())
        );
    }

    #[test]
    fn the_answers_keep_their_own_shape() {
        let body: AnswerBody = serde_json::from_str(
            r#"{"responses":[{"id":"q1","value":"B"},{"id":"q2","value":["a","b"],"attachments":["/tmp/a.png"]}]}"#,
        )
        .expect("the answers");
        assert_eq!(body.responses.len(), 2);
        assert_eq!(body.responses[0].value, serde_json::json!("B"));
        assert_eq!(
            body.responses[1].attachments.as_deref(),
            Some(["/tmp/a.png".to_owned()].as_slice())
        );
        let back = serde_json::to_string(&body).expect("the answers again");
        let again: AnswerBody = serde_json::from_str(&back).expect("the answers again");
        assert_eq!(body, again);
    }
}
