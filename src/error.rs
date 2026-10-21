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
    #[error("ssh {target}: {detail}")]
    Ssh {
        target: String,
        detail: String,
        /// The safe category of the failure; status shows this and never `detail`.
        failure: crate::hosts::Failure,
    },
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
