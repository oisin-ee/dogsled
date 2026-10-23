//! Credential-safe preparation diagnostics. Peer text never becomes a message.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Prepare,
    Checkout,
    Launch,
    Reserve,
    Other,
}

impl Operation {
    pub fn from_args(args: &[String]) -> Self {
        match args.first().map(String::as_str) {
            Some("prepare") => Self::Prepare,
            Some("checkout") => Self::Checkout,
            Some("launch") => Self::Launch,
            Some("reserve") => Self::Reserve,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cause {
    BaseUnavailable,
    BranchExists,
    WorkspaceOccupied,
    CheckoutIdentityMismatch,
    CloneFailed,
    SetupFailed,
    /// The preparation failed and its outcome could not be recorded. The intent stays
    /// uncertain, so this cause says so instead of naming a preparation failure.
    OutcomeUnrecorded,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Occupancy {
    Path,
    Locked,
    MainCheckout,
    OpenWorkspace,
}

/// No origins, command output, prompts, or scripts are permitted in this context.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occupancy: Option<Occupancy>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub new_worktree_removed: bool,
}

impl Context {
    pub fn new(branch: Option<&str>, base: Option<&str>, workspace: Option<&str>) -> Self {
        Self {
            branch: branch.filter(|s| safe_ref(s)).map(str::to_owned),
            base: base.filter(|s| safe_ref(s)).map(str::to_owned),
            workspace: workspace.filter(|s| safe_path(s)).map(str::to_owned),
            occupancy: None,
            new_worktree_removed: false,
        }
    }

    pub fn occupied(mut self, occupancy: Occupancy) -> Self {
        self.occupancy = Some(occupancy);
        self
    }

    pub fn removed(mut self) -> Self {
        self.new_worktree_removed = true;
        self
    }

    fn valid(&self) -> bool {
        self.branch.as_deref().is_none_or(safe_ref)
            && self.base.as_deref().is_none_or(safe_ref)
            && self.workspace.as_deref().is_none_or(safe_path)
    }
}

fn safe_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.starts_with('-')
        && !value.contains("..")
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"/_-.~^".contains(&c))
}

fn safe_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value.contains("..")
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b" /_-.".contains(&c))
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Omission {
    #[default]
    UntrustedDetail,
    MissingEnvelope,
    InvalidEnvelope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub version: u8,
    pub operation: Operation,
    pub cause: Cause,
    pub context: Context,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default)]
    pub omission: Omission,
}

impl Diagnostic {
    pub fn new(operation: Operation, cause: Cause, context: Context) -> Self {
        Self {
            version: 1,
            operation,
            cause,
            context,
            status: None,
            host: None,
            request_id: None,
            omission: Omission::UntrustedDetail,
        }
    }

    pub fn request(mut self, request: Option<&str>) -> Self {
        self.request_id = request.filter(|s| safe_id(s)).map(str::to_owned);
        self
    }

    /// Operation, status, and correlation come from this invocation, never the peer.
    /// Invalid or unknown envelopes are discarded as a whole, including their context.
    pub fn remote(
        value: Option<&serde_json::Value>,
        args: &[String],
        status: Option<i32>,
        host: &str,
    ) -> Self {
        let operation = Operation::from_args(args);
        let request = args
            .windows(2)
            .find(|p| p[0] == "--request-id" || p[0] == "--id")
            .map(|p| p[1].clone())
            .or_else(|| {
                let input = args.windows(2).find(|p| p[0] == "--request")?;
                let launch: serde_json::Value = serde_json::from_str(&input[1]).ok()?;
                launch["id"]
                    .as_str()
                    .filter(|s| safe_id(s))
                    .map(str::to_owned)
            });
        let decoded = value.and_then(|v| serde_json::from_value::<Self>(v.clone()).ok());
        let valid = decoded.filter(|d| {
            d.version == 1
                && d.operation == operation
                && d.context.valid()
                && d.request_id.as_deref().is_none_or(safe_id)
                && match operation {
                    Operation::Prepare => matches!(
                        d.cause,
                        Cause::BaseUnavailable
                            | Cause::BranchExists
                            | Cause::WorkspaceOccupied
                            | Cause::SetupFailed
                            | Cause::OutcomeUnrecorded
                    ),
                    Operation::Checkout => matches!(
                        d.cause,
                        Cause::CheckoutIdentityMismatch | Cause::CloneFailed | Cause::SetupFailed
                    ),
                    _ => false,
                }
        });
        let mut diagnostic = valid.unwrap_or_else(|| {
            let mut d = Self::new(operation, Cause::Unavailable, Context::default());
            d.omission = if value.is_some() {
                Omission::InvalidEnvelope
            } else {
                Omission::MissingEnvelope
            };
            d
        });
        // Context at an untrusted boundary must be bound to this caller's inputs, not
        // merely printable. A peer can put a secret in an otherwise valid ref or path.
        if diagnostic.cause != Cause::Unavailable {
            diagnostic.omission = Omission::UntrustedDetail;
            let arg = |flag: &str| {
                args.windows(2)
                    .find(|p| p[0] == flag)
                    .map(|p| p[1].as_str())
            };
            let mut context = Context::new(arg("--branch"), arg("--base"), arg("--path"));
            context.occupancy = diagnostic.context.occupancy;
            context.new_worktree_removed = diagnostic.context.new_worktree_removed;
            diagnostic.context = context;
        }
        diagnostic.host = Some(host.to_owned());
        diagnostic.status = status;
        diagnostic.request_id = request.filter(|s| safe_id(s));
        diagnostic
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.cause {
            Cause::BaseUnavailable => {
                "requested base unavailable; provide a commit available on the selected host"
            }
            Cause::BranchExists => "branch exists; omit --base",
            Cause::WorkspaceOccupied => {
                "workspace occupied; inspect the selected host before resubmitting"
            }
            Cause::CheckoutIdentityMismatch => {
                "checkout identity mismatch; inspect the selected checkout"
            }
            Cause::CloneFailed => {
                "the peer could not clone the repository; check repository access on the selected host"
            }
            Cause::SetupFailed => {
                "repository setup command failed; inspect setup on the selected host"
            }
            Cause::OutcomeUnrecorded => {
                "the preparation failed and its cause was not recorded; \
                 inspect this host before resubmitting"
            }
            Cause::Unavailable => "remote command failed",
        };
        let code = match self.cause {
            Cause::BaseUnavailable => "base_unavailable",
            Cause::BranchExists => "branch_exists",
            Cause::WorkspaceOccupied => "workspace_occupied",
            Cause::CheckoutIdentityMismatch => "checkout_identity_mismatch",
            Cause::CloneFailed => "clone_failed",
            Cause::SetupFailed => "setup_failed",
            Cause::OutcomeUnrecorded => "outcome_unrecorded",
            Cause::Unavailable => "unavailable",
        };
        write!(f, "{:?}: {code}: ", self.operation)?;
        if self.cause == Cause::BranchExists
            && let Some(branch) = &self.context.branch
        {
            write!(f, "branch {branch} exists; omit --base")?;
        } else {
            write!(f, "{message}")?;
            if let Some(branch) = &self.context.branch {
                write!(f, "; branch {branch}")?;
            }
        }
        match self.context.occupancy {
            Some(Occupancy::MainCheckout) => write!(f, "; checked out in the main checkout")?,
            Some(Occupancy::Locked) => write!(f, "; locked; it is not an agent worktree")?,
            Some(Occupancy::OpenWorkspace) => {
                write!(f, "; a Herdr workspace already shows this worktree")?;
            }
            _ => (),
        }
        if self.context.new_worktree_removed {
            write!(f, "; the new worktree was removed")?;
        }
        if let Some(base) = &self.context.base {
            write!(f, "; base {base}")?;
        }
        if let Some(workspace) = &self.context.workspace {
            write!(f, "; workspace {workspace}")?;
        }
        if let Some(status) = self.status {
            write!(f, "; status {status}")?;
        }
        if let Some(request) = &self.request_id {
            write!(f, "; request {request}")?;
        }
        let reason = match self.omission {
            Omission::UntrustedDetail => "only validated cause and context are shown",
            Omission::MissingEnvelope => "peer supplied no diagnostic envelope",
            Omission::InvalidEnvelope => "peer diagnostic envelope is malformed or unknown",
        };
        write!(f, "; untrusted detail withheld: {reason}")
    }
}

/// Never format argv in a deadline: it can contain launch JSON or scripts.
pub fn shell_operation(program: &str) -> &'static str {
    match program {
        "ssh" => "ssh",
        "git" => "git",
        "herdr" => "herdr",
        "mise" => "mise",
        "sh" => "shell",
        _ => "command",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(cause: Cause) -> serde_json::Value {
        serde_json::json!({"version":1,"operation":"prepare","cause":cause,
            "context":{"branch":"worker","base":"main"}})
    }

    #[test]
    fn an_unrecorded_outcome_is_a_known_preparation_cause() {
        let args = vec![
            "prepare".to_owned(),
            "--branch".to_owned(),
            "worker".to_owned(),
            "--base".to_owned(),
            "main".to_owned(),
            "--request-id".to_owned(),
            "r-1".to_owned(),
        ];
        let diagnostic = Diagnostic::remote(
            Some(&envelope(Cause::OutcomeUnrecorded)),
            &args,
            Some(2),
            "netcup",
        );
        assert_eq!(diagnostic.cause, Cause::OutcomeUnrecorded);
        assert_eq!(diagnostic.host.as_deref(), Some("netcup"));
        assert_eq!(diagnostic.request_id.as_deref(), Some("r-1"));
        assert_eq!(diagnostic.status, Some(2));
        assert_eq!(diagnostic.context.branch.as_deref(), Some("worker"));
        let text = diagnostic.to_string();
        assert!(text.contains("outcome_unrecorded"), "{text}");
        assert!(text.contains("cause was not recorded"), "{text}");
        assert!(
            text.contains("inspect this host before resubmitting"),
            "{text}"
        );
    }

    #[test]
    fn an_unrecorded_outcome_is_not_accepted_for_another_operation() {
        let value = serde_json::json!({"version":1,"operation":"checkout",
            "cause":"outcome_unrecorded","context":{}});
        let args = vec!["checkout".to_owned()];
        let diagnostic = Diagnostic::remote(Some(&value), &args, Some(2), "netcup");
        assert_eq!(diagnostic.cause, Cause::Unavailable);
        assert_eq!(diagnostic.omission, Omission::InvalidEnvelope);
    }
}
