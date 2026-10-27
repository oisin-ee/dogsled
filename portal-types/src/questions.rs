//! Wire types for the portal's questions: the form an agent asks, the pending
//! question in the store, and the answers the owner sends back.
//!
//! The form keeps the shape of pi-interview 0.13.0, so the model asks the way
//! it always did. Fields the schema does not name are kept as they came, so a
//! newer form still reaches the portal unchanged.

use serde::ser::SerializeMap;
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

/// The media of a question: what the form shows beside the question.
///
/// A media block is one object with a `type` and the fields of that type, the
/// same contract pi-interview writes. A block the schema does not name, or
/// does not fill, is kept as it came, so a newer form still reaches the
/// portal unchanged.
#[derive(Debug, Clone, PartialEq)]
pub enum Media {
    /// An image, with the source the browser loads and its alternative text.
    Image {
        /// The source the browser loads.
        src: String,
        /// The text that stands in for the image.
        alt: Option<String>,
        /// The line under the image.
        caption: Option<String>,
        /// Where the image sits against the question.
        position: MediaPosition,
    },
    /// A table, with its headers, its rows, and the rows it highlights.
    Table {
        /// The table itself.
        table: TableMedia,
        /// The line under the table.
        caption: Option<String>,
        /// Where the table sits against the question.
        position: MediaPosition,
    },
    /// A chart, with its data and its options.
    Chart {
        /// The chart itself.
        chart: ChartMedia,
        /// The line under the chart.
        caption: Option<String>,
        /// Where the chart sits against the question.
        position: MediaPosition,
    },
    /// A mermaid diagram, as its source.
    Mermaid {
        /// The source of the diagram.
        source: String,
        /// The line under the diagram.
        caption: Option<String>,
        /// Where the diagram sits against the question.
        position: MediaPosition,
    },
    /// HTML the form shows in a sandboxed frame.
    Html {
        /// The HTML itself.
        html: String,
        /// The line under the frame.
        caption: Option<String>,
        /// Where the frame sits against the question.
        position: MediaPosition,
    },
    /// A media block the schema does not name, kept as it came.
    Unknown(Value),
}

/// The table of a media block: its headers, its rows, and the rows it
/// highlights.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableMedia {
    /// The labels of the columns.
    #[serde(default, deserialize_with = "de_texts")]
    pub headers: Vec<String>,
    /// The rows of the table.
    #[serde(default, deserialize_with = "de_rows")]
    pub rows: Vec<Vec<String>>,
    /// The numbers of the rows the table highlights.
    #[serde(
        default,
        deserialize_with = "de_indices",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub highlights: Vec<usize>,
}

/// The chart of a media block: its data and its options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChartMedia {
    /// The values the chart shows.
    #[serde(default)]
    pub data: Value,
    /// The options the chart takes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Value>,
}

/// Where a media block sits against its question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MediaPosition {
    /// Above the question, which is the default.
    #[default]
    Above,
    /// Below the question.
    Below,
    /// Beside the question, in two columns.
    Side,
}

impl MediaPosition {
    /// The position a name means. A name the schema does not know is the
    /// default.
    fn from_name(name: &str) -> Self {
        match name {
            "below" => Self::Below,
            "side" => Self::Side,
            _ => Self::Above,
        }
    }

    /// The name of the position, as the class name the portal uses.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Above => "above",
            Self::Below => "below",
            Self::Side => "side",
        }
    }
}

impl std::fmt::Display for MediaPosition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.name())
    }
}

impl Serialize for MediaPosition {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match self {
            Self::Above => "above",
            Self::Below => "below",
            Self::Side => "side",
        })
    }
}

impl<'de> Deserialize<'de> for MediaPosition {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Ok(Self::from_name(&name))
    }
}

impl Media {
    /// The media of a value. A block the schema does not name, or does not
    /// fill, is kept as it came.
    fn from_value(value: Value) -> Self {
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let position = value
            .get("position")
            .and_then(Value::as_str)
            .map_or(MediaPosition::Above, MediaPosition::from_name);
        let caption = value
            .get("caption")
            .and_then(Value::as_str)
            .map(str::to_owned);
        match kind.as_str() {
            "image" => {
                let Some(src) = value.get("src").and_then(Value::as_str) else {
                    return Self::Unknown(value);
                };
                Self::Image {
                    src: src.to_owned(),
                    alt: value.get("alt").and_then(Value::as_str).map(str::to_owned),
                    caption,
                    position,
                }
            }
            "table" => {
                let Some(table) = value.get("table").and_then(Value::as_object) else {
                    return Self::Unknown(value);
                };
                Self::Table {
                    table: TableMedia {
                        headers: texts(table.get("headers")),
                        rows: rows_texts(table.get("rows")),
                        highlights: indices(table.get("highlights")),
                    },
                    caption,
                    position,
                }
            }
            "chart" => {
                let Some(chart) = value.get("chart").and_then(Value::as_object) else {
                    return Self::Unknown(value);
                };
                Self::Chart {
                    chart: ChartMedia {
                        data: chart.get("data").cloned().unwrap_or(Value::Null),
                        options: chart.get("options").cloned(),
                    },
                    caption,
                    position,
                }
            }
            "mermaid" => {
                let Some(source) = value.get("mermaid").and_then(Value::as_str) else {
                    return Self::Unknown(value);
                };
                Self::Mermaid {
                    source: source.to_owned(),
                    caption,
                    position,
                }
            }
            "html" => {
                let Some(html) = value.get("html").and_then(Value::as_str) else {
                    return Self::Unknown(value);
                };
                Self::Html {
                    html: html.to_owned(),
                    caption,
                    position,
                }
            }
            _ => Self::Unknown(value),
        }
    }

    /// Where the media sits against its question.
    #[must_use]
    pub fn position(&self) -> MediaPosition {
        match self {
            Self::Image { position, .. }
            | Self::Table { position, .. }
            | Self::Chart { position, .. }
            | Self::Mermaid { position, .. }
            | Self::Html { position, .. } => *position,
            Self::Unknown(value) => value
                .get("position")
                .and_then(Value::as_str)
                .map_or(MediaPosition::Above, MediaPosition::from_name),
        }
    }

    /// The name of the kind of the media, or the name the block carries when
    /// the schema does not know it.
    #[must_use]
    pub fn kind_name(&self) -> String {
        match self {
            Self::Image { .. } => "image".to_owned(),
            Self::Table { .. } => "table".to_owned(),
            Self::Chart { .. } => "chart".to_owned(),
            Self::Mermaid { .. } => "mermaid".to_owned(),
            Self::Html { .. } => "html".to_owned(),
            Self::Unknown(value) => value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned(),
        }
    }
}

impl Serialize for Media {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Unknown(value) => value.serialize(serializer),
            Self::Image {
                src,
                alt,
                caption,
                position,
            } => {
                let mut map = serializer.serialize_map(None)?;
                map.serialize_entry("type", "image")?;
                map.serialize_entry("src", src)?;
                write_optional(&mut map, "alt", alt.as_ref())?;
                write_optional(&mut map, "caption", caption.as_ref())?;
                write_position(&mut map, *position)?;
                map.end()
            }
            Self::Table {
                table,
                caption,
                position,
            } => {
                let mut map = serializer.serialize_map(None)?;
                map.serialize_entry("type", "table")?;
                map.serialize_entry("table", table)?;
                write_optional(&mut map, "caption", caption.as_ref())?;
                write_position(&mut map, *position)?;
                map.end()
            }
            Self::Chart {
                chart,
                caption,
                position,
            } => {
                let mut map = serializer.serialize_map(None)?;
                map.serialize_entry("type", "chart")?;
                map.serialize_entry("chart", chart)?;
                write_optional(&mut map, "caption", caption.as_ref())?;
                write_position(&mut map, *position)?;
                map.end()
            }
            Self::Mermaid {
                source,
                caption,
                position,
            } => {
                let mut map = serializer.serialize_map(None)?;
                map.serialize_entry("type", "mermaid")?;
                map.serialize_entry("mermaid", source)?;
                write_optional(&mut map, "caption", caption.as_ref())?;
                write_position(&mut map, *position)?;
                map.end()
            }
            Self::Html {
                html,
                caption,
                position,
            } => {
                let mut map = serializer.serialize_map(None)?;
                map.serialize_entry("type", "html")?;
                map.serialize_entry("html", html)?;
                write_optional(&mut map, "caption", caption.as_ref())?;
                write_position(&mut map, *position)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Media {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(Self::from_value(value))
    }
}

/// Write an optional field, or leave it out.
fn write_optional<S: SerializeMap>(
    map: &mut S,
    name: &str,
    value: Option<&String>,
) -> Result<(), S::Error> {
    if let Some(value) = value {
        map.serialize_entry(name, value)?;
    }
    Ok(())
}

/// Write the position of a media block. `above` is the default, so it is left
/// out.
fn write_position<S: SerializeMap>(map: &mut S, position: MediaPosition) -> Result<(), S::Error> {
    if position != MediaPosition::Above {
        map.serialize_entry("position", &position)?;
    }
    Ok(())
}

/// The text of a cell, whatever JSON value it holds.
fn cell_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// The texts of a list of cells.
fn texts(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|list| list.iter().map(cell_text).collect())
        .unwrap_or_default()
}

/// The texts of the rows of a table.
fn rows_texts(value: Option<&Value>) -> Vec<Vec<String>> {
    value
        .and_then(Value::as_array)
        .map(|rows| rows.iter().map(|row| texts(Some(row))).collect())
        .unwrap_or_default()
}

/// The numbers of the rows a table highlights.
fn indices(value: Option<&Value>) -> Vec<usize> {
    value
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|item| {
                    item.as_u64()
                        .and_then(|number| usize::try_from(number).ok())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Read the texts of a list of cells.
fn de_texts<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    Ok(texts(Some(&Value::deserialize(deserializer)?)))
}

/// Read the texts of the rows of a table.
fn de_rows<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Vec<String>>, D::Error> {
    Ok(rows_texts(Some(&Value::deserialize(deserializer)?)))
}

/// Read the numbers of the rows a table highlights.
fn de_indices<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<usize>, D::Error> {
    Ok(indices(Some(&Value::deserialize(deserializer)?)))
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
    /// The media the question shows beside its text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<Media>,
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

    /// A media block of each kind the schema names, and one it does not.
    const MEDIA: &str = r#"[
        {"type": "image", "src": "/api/attachments/a.png", "alt": "A", "caption": "Cap"},
        {"type": "table", "table": {"headers": ["A", "B"], "rows": [["1", "2"]], "highlights": [0]},
         "caption": "Cap"},
        {"type": "chart", "chart": {"data": {"x": [1, 2]}, "options": {"title": "T"}}},
        {"type": "mermaid", "mermaid": "graph LR\n  A --> B"},
        {"type": "html", "html": "<p>hi</p>"},
        {"type": "video", "url": "https://example.test/v"}
    ]"#;

    #[test]
    fn every_media_kind_round_trips() {
        let blocks: Vec<Media> = serde_json::from_str(MEDIA).expect("the media");
        assert_eq!(blocks.len(), 6);
        assert!(
            matches!(&blocks[0], Media::Image { src, alt, .. } if src == "/api/attachments/a.png" && alt.as_deref() == Some("A"))
        );
        assert!(
            matches!(&blocks[1], Media::Table { table, .. } if table.headers == ["A", "B"] && table.highlights == [0])
        );
        assert!(
            matches!(&blocks[2], Media::Chart { chart, .. } if chart.data["x"] == serde_json::json!([1, 2]))
        );
        assert!(
            matches!(&blocks[3], Media::Mermaid { source, .. } if source == "graph LR\n  A --> B")
        );
        assert!(matches!(&blocks[4], Media::Html { html, .. } if html == "<p>hi</p>"));
        let back = serde_json::to_string(&blocks).expect("the media again");
        let again: Vec<Media> = serde_json::from_str(&back).expect("the media again");
        assert_eq!(blocks, again);
    }

    #[test]
    fn a_media_kind_the_schema_does_not_name_is_kept() {
        let blocks: Vec<Media> = serde_json::from_str(MEDIA).expect("the media");
        let Media::Unknown(value) = &blocks[5] else {
            panic!("the unknown media is kept as it came");
        };
        assert_eq!(value["type"], serde_json::json!("video"));
        assert_eq!(blocks[5].kind_name(), "video");
    }

    #[test]
    fn the_position_of_a_media_block_is_read_and_kept() {
        let media: Media =
            serde_json::from_str(r#"{"type": "image", "src": "a.png", "position": "side"}"#)
                .expect("the media");
        assert_eq!(media.position(), MediaPosition::Side);
        let back = serde_json::to_string(&media).expect("the media again");
        assert!(back.contains(r#""position":"side""#), "{back}");
        // `above` is the default, so it is left out.
        let media: Media =
            serde_json::from_str(r#"{"type": "image", "src": "a.png"}"#).expect("the media");
        assert_eq!(media.position(), MediaPosition::Above);
        let back = serde_json::to_string(&media).expect("the media again");
        assert!(!back.contains("position"), "{back}");
    }

    #[test]
    fn a_media_block_the_schema_cannot_read_is_kept_as_it_came() {
        let media: Media = serde_json::from_str(r#"{"type": "image"}"#).expect("the media");
        let Media::Unknown(value) = media else {
            panic!("a media block without its source is kept as it came");
        };
        assert_eq!(value["type"], serde_json::json!("image"));
    }

    #[test]
    fn a_form_with_media_round_trips() {
        let form = format!(
            r#"{{"questions":[{{"id":"q1","type":"info","question":"Look","media":{MEDIA_BLOCK}}}]}}"#
        );
        let set: QuestionSet = serde_json::from_str(&form).expect("the form");
        assert!(matches!(set.questions[0].media, Some(Media::Table { .. })));
        let back = serde_json::to_string(&set).expect("the form again");
        let again: QuestionSet = serde_json::from_str(&back).expect("the form again");
        assert_eq!(set, again);
    }

    /// One media block, as a form carries it.
    const MEDIA_BLOCK: &str =
        r#"{"type": "table", "table": {"headers": ["A"], "rows": [["1"]], "highlights": [0]}}"#;

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
