//! One error type for the whole binary. `main` maps it to an exit code.

use serde::{Deserialize, Serialize};
use std::process::ExitCode;

/// Exit code when every eligible host is at capacity. Not an error for callers
/// that queue; the JSON body carries the detail.
pub const EXIT_FULL: u8 = 1;
/// Exit code for every other failure.
pub const EXIT_ERROR: u8 = 2;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("config {path}: {source}")]
    Config {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("{program} {args}: exit {status}: {stderr}")]
    Command {
        program: String,
        args: String,
        status: String,
        stderr: String,
    },
    /// An ssh read failed. The error holds only a safe category: remote stderr and remote
    /// error text can carry secrets, and an error reaches tickets, handoffs, logs, and JSON.
    /// The raw text never enters this type; see `hosts::record_ssh_detail`.
    #[error("ssh {target}: {}", .refusal.map_or_else(|| failure.to_string(), |r| r.message().to_owned()))]
    Ssh {
        target: String,
        failure: crate::hosts::Failure,
        /// Set when the peer answered with one of dogsled's own refusal codes.
        refusal: Option<Refusal>,
    },
    /// A refusal dogsled itself raises. `detail` is for this host's own output; a peer sends
    /// only `code`, and the caller shows the code's fixed message.
    #[error("{detail}")]
    Refused { code: Refusal, detail: String },
    #[error("unexpected output from {origin}: {detail}")]
    Parse { origin: String, detail: String },
    #[error("unknown host {0}; see `dogsled status`")]
    UnknownMachine(String),
    #[error("every eligible host is full")]
    Full,
    #[error("{0}")]
    Invalid(String),
    #[error("deadline reached: {0}")]
    Deadline(String),
    /// Terminal diagnostics retain the original error classification and payload.
    #[error("{source}; startup output from {pane}:\n{output}")]
    Startup {
        #[source]
        source: Box<Error>,
        pane: String,
        output: String,
    },
    /// Safe diagnostics wrap the original classification, never replacing it.
    #[error("{}{diagnostic}", diagnostic_prefix(source))]
    Diagnostic {
        #[source]
        source: Box<Error>,
        diagnostic: Box<crate::diagnostic::Diagnostic>,
    },
    /// An agent launch failed after placement; admission retries it with backoff.
    #[error("launch failed: {0}")]
    Launch(String),
}

/// The refusals a peer reports by code. A caller maps a known code to its fixed message and
/// never shows the peer's own text, which can carry secrets. An unknown code stays hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Refusal {
    /// The target agent has no dogsled inbox reader, so a message would never be delivered.
    NoInboxReader,
    /// The peer could not clone the repository.
    CloneFailed,
}

impl Refusal {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::NoInboxReader => "NO_INBOX_READER",
            Self::CloneFailed => "CLONE_FAILED",
        }
    }

    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        [Self::NoInboxReader, Self::CloneFailed]
            .into_iter()
            .find(|r| r.code() == code)
    }

    /// The fixed, safe message for this refusal.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::NoInboxReader => {
                "the target has no dogsled inbox reader, so nothing would deliver the message; nothing was sent"
            }
            Self::CloneFailed => "the peer could not clone the repository",
        }
    }
}

impl Error {
    /// Classify a failure without discarding its startup diagnostics.
    pub fn root(&self) -> &Self {
        match self {
            Self::Startup { source, .. } | Self::Diagnostic { source, .. } => source.root(),
            error => error,
        }
    }

    pub fn with_diagnostic(self, diagnostic: crate::diagnostic::Diagnostic) -> Self {
        Self::Diagnostic {
            source: Box::new(self),
            diagnostic: Box::new(diagnostic),
        }
    }

    pub fn correlate(mut self, request: Option<&str>) -> Self {
        if let Self::Diagnostic { diagnostic, .. } = &mut self {
            **diagnostic = (**diagnostic).clone().request(request);
        }
        self
    }

    pub fn on_host(mut self, host: &str) -> Self {
        if let Self::Diagnostic { diagnostic, .. } = &mut self {
            diagnostic.host = Some(host.into());
        }
        self
    }

    pub fn diagnostic(&self) -> Option<&crate::diagnostic::Diagnostic> {
        match self {
            Self::Diagnostic { diagnostic, .. } => Some(diagnostic),
            Self::Startup { source, .. } => source.diagnostic(),
            _ => None,
        }
    }

    /// The safe record of a returned failure. It keeps the typed cause, the correlation, and
    /// the classification, and never the original error text.
    pub fn retained(
        &self,
        operation: crate::diagnostic::Operation,
        request_id: Option<&str>,
    ) -> RetainedFailure {
        let (kind, transport, refusal) = match self.root() {
            Error::Deadline(_) => (RetainedKind::Deadline, None, None),
            Error::Refused { code, .. } => (RetainedKind::Refused, None, Some(*code)),
            Error::Ssh {
                failure, refusal, ..
            } => (RetainedKind::Transport, Some(*failure), *refusal),
            _ => (RetainedKind::Other, None, None),
        };
        RetainedFailure {
            operation,
            diagnostic: self.diagnostic().cloned(),
            kind,
            transport,
            refusal,
            request_id: self
                .diagnostic()
                .and_then(|diagnostic| diagnostic.request_id.clone())
                .or_else(|| request_id.map(str::to_owned)),
            detail_withheld: true,
        }
    }

    /// Structured tool errors retain text for older consumers.
    pub fn tool_message(&self) -> String {
        self.diagnostic().map_or_else(
            || self.to_string(),
            |diagnostic| {
                serde_json::json!({"error":self.to_string(),"diagnostic":diagnostic}).to_string()
            },
        )
    }

    #[must_use]
    pub fn exit_code(&self) -> ExitCode {
        match self.root() {
            Self::Full => ExitCode::from(EXIT_FULL),
            _ => ExitCode::from(EXIT_ERROR),
        }
    }
}

/// The safe classification of a failed operation, kept without its original text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetainedKind {
    /// The operation reached its deadline.
    Deadline,
    /// The peer refused the operation with a known code.
    Refused,
    /// The transport failed in a known category.
    Transport,
    /// The failure has no further safe classification.
    Other,
}

/// The safe record of a returned failure. It keeps the typed cause, the correlation, and the
/// classification, and never the original error text: no command argv, no output, no
/// prompt, no script, and no origin that can carry a credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetainedFailure {
    pub operation: crate::diagnostic::Operation,
    /// The typed cause and context, when the failure carried them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<crate::diagnostic::Diagnostic>,
    pub kind: RetainedKind,
    /// The transport category, for a transport failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<crate::hosts::Failure>,
    /// The refusal code, for a refusal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<Refusal>,
    /// The request this failure belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// The original error text is not retained. This marker says so.
    #[serde(default)]
    pub detail_withheld: bool,
}

impl RetainedFailure {
    /// Rebuild the failure from its safe record. The rebuilt error keeps its classification
    /// and its typed cause, and never shows the withheld text.
    pub fn restore(&self) -> Error {
        let restored = match self.kind {
            RetainedKind::Deadline => Error::Deadline("deadline reached".into()),
            RetainedKind::Refused => {
                let code = self.refusal.unwrap_or(Refusal::CloneFailed);
                Error::Refused {
                    code,
                    detail: code.message().to_owned(),
                }
            }
            RetainedKind::Transport => Error::Ssh {
                target: self
                    .diagnostic
                    .as_ref()
                    .and_then(|diagnostic| diagnostic.host.clone())
                    .unwrap_or_else(|| "unknown host".into()),
                failure: self.transport.unwrap_or(crate::hosts::Failure::Other),
                refusal: self.refusal,
            },
            RetainedKind::Other => {
                // Without a typed cause, the record still names the operation and the
                // request it belongs to. The original text stays withheld.
                let mut message = format!("{:?} failed; untrusted detail withheld", self.operation);
                if let Some(request) = &self.request_id {
                    use std::fmt::Write as _;
                    let _ = write!(message, "; request {request}");
                }
                Error::Invalid(message)
            }
        };
        match &self.diagnostic {
            Some(diagnostic) => restored.with_diagnostic(diagnostic.clone()),
            None => restored,
        }
    }
}

fn diagnostic_prefix(source: &Error) -> String {
    match source.root() {
        Error::Ssh { .. } | Error::Deadline(_) | Error::Parse { .. } => format!("{source}; "),
        _ => String::new(),
    }
}

pub type Result<T> = std::result::Result<T, Error>;
