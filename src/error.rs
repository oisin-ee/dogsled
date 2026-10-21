//! One error type for the whole binary. `main` maps it to an exit code.

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
    /// An agent launch failed after placement; admission retries it with backoff.
    #[error("launch failed: {0}")]
    Launch(String),
}

/// The refusals a peer reports by code. A caller maps a known code to its fixed message and
/// never shows the peer's own text, which can carry secrets. An unknown code stays hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
            Self::Startup { source, .. } => source.root(),
            error => error,
        }
    }

    #[must_use]
    pub fn exit_code(&self) -> ExitCode {
        match self.root() {
            Self::Full => ExitCode::from(EXIT_FULL),
            _ => ExitCode::from(EXIT_ERROR),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
