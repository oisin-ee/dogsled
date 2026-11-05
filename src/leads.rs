//! Ownerless lead identity and launch state, independent of child work and retention.

use crate::checkout::Repository;
use crate::jobs::Validity;
use crate::probe::Agent;
use serde::{Deserialize, Serialize};

mod checkout;
mod routing;

/// Intent is saved before each external effect; its outcome advances the phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Recorded,
    Allocating,
    Allocated,
    Starting,
    Started,
    Prompting,
    Ready,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Record {
    // Immutable launch intent. These fields never change during an update.
    pub id: String,
    pub name: String,
    pub repository: Repository,
    pub prompt: String,
    pub validity: Validity,
    pub created_ms: u64,
    // Observed launch outcomes and explicit closure, not child completion.
    pub phase: Phase,
    pub pane: Option<String>,
    pub terminal: Option<String>,
    pub workspace: Option<String>,
    pub updated_ms: u64,
    pub error: Option<String>,
    pub close_requested_ms: Option<u64>,
    pub closed_ms: Option<u64>,
}

/// A name or pane alone never proves a lead's identity for capacity exemption.
pub(crate) fn matches_agent(record: &Record, agent: &Agent) -> bool {
    record.closed_ms.is_none()
        && record.pane.as_deref() == Some(agent.pane.as_str())
        && matches!(
            (record.terminal.as_deref(), agent.terminal_id.as_deref()),
            (Some(saved), Some(observed)) if !saved.is_empty() && saved == observed
        )
}

/// Missing identity is unknown and must not authorize automatic destruction.
/// Only a positively different terminal proves that this pane has been reused.
pub(crate) fn protects_pane(record: &Record, pane: &str, terminal: Option<&str>) -> bool {
    record.closed_ms.is_none()
        && record.pane.as_deref() == Some(pane)
        && !matches!(
            (record.terminal.as_deref(), terminal),
            (Some(saved), Some(observed))
                if !saved.is_empty() && !observed.is_empty() && saved != observed
        )
}

/// What a lead launch needs. It never reads the caller's pane or workspace.
pub(crate) struct CreateContext<'a> {
    pub shell: &'a dyn crate::shell::Shell,
    pub ledger: &'a crate::ledger::Ledger,
    /// Reloaded for the fleet's remote command and capacity policy.
    pub config_path: &'a std::path::Path,
    pub home: &'a str,
    pub herdr: Option<&'a crate::herdr_socket::Client>,
}

/// A lead that started and accepted its first prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Started {
    pub host: String,
    pub id: String,
    pub name: String,
    pub pane: String,
    pub workspace: Option<String>,
}

/// Which side of the launch a failure is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailureKind {
    /// The selector, prompt, or checkout is wrong. Nothing was recorded.
    Invalid,
    /// Herdr cannot take a prompt here. Nothing was recorded.
    Unavailable,
    /// The launch was recorded and an effect failed. The pane stays as it is.
    Launch,
}

/// A failed launch keeps what is known: its record, pane, and the phase that failed.
#[derive(Debug)]
pub(crate) struct Failure {
    pub kind: FailureKind,
    pub id: Option<String>,
    pub pane: Option<String>,
    pub phase: Option<Phase>,
    pub error: Box<crate::error::Error>,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.error)?;
        if let Some(id) = &self.id {
            write!(f, "; lead request {id}")?;
        }
        if let Some(pane) = &self.pane {
            write!(f, ", pane {pane}")?;
        }
        if let Some(phase) = self.phase {
            write!(
                f,
                ", phase {}",
                crate::rows::name(&phase).unwrap_or_default()
            )?;
        }
        Ok(())
    }
}

impl From<Failure> for crate::error::Error {
    fn from(failure: Failure) -> Self {
        match failure.kind {
            FailureKind::Invalid | FailureKind::Unavailable if failure.id.is_none() => {
                *failure.error
            }
            _ => Self::Launch(failure.to_string()),
        }
    }
}

/// A request ID short enough that `lead-<id>` is a Herdr agent name: at most 32 characters
/// of `[a-z0-9_-]`, starting with a letter.
pub(crate) fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    format!(
        "r-{}-{}-{}",
        base36(crate::ledger::now_ms()),
        base36(u64::from(std::process::id())),
        base36(SEQUENCE.fetch_add(1, Ordering::Relaxed))
    )
}

fn base36(mut value: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut digits = Vec::new();
    loop {
        digits.push(DIGITS[usize::try_from(value % 36).unwrap_or_default()]);
        value /= 36;
        if value == 0 {
            break;
        }
    }
    digits.reverse();
    String::from_utf8(digits).unwrap_or_default()
}

fn agent_name(id: &str) -> crate::error::Result<String> {
    let name = format!("lead-{id}");
    if name.len() > 32
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        return Err(crate::error::Error::Invalid(format!(
            "{name} is not a Herdr agent name"
        )));
    }
    Ok(name)
}

/// Start one ownerless Pi lead on the roomiest reachable fleet host.
/// Intent is saved before every effect. A failure after the record keeps the pane and
/// never retries an uncertain start or prompt.
pub(crate) fn create(
    context: &CreateContext<'_>,
    selector: &str,
    prompt: &str,
) -> Result<Started, Failure> {
    routing::create(context, selector, prompt)
}

fn failure(kind: FailureKind, error: crate::error::Error) -> Failure {
    Failure {
        kind,
        id: None,
        pane: None,
        phase: None,
        error: Box::new(error),
    }
}

fn start(context: &CreateContext<'_>, delivery: &routing::Delivery) -> Result<Started, Failure> {
    let unavailable = |error| failure(FailureKind::Unavailable, error);
    let discovered;
    let herdr = if let Some(herdr) = context.herdr {
        herdr
    } else {
        discovered = crate::herdr_socket::Client::discover().map_err(unavailable)?;
        &discovered
    };
    herdr.agent_list().map_err(unavailable)?;
    let context = &CreateContext {
        herdr: Some(herdr),
        ..*context
    };
    let repository = checkout::ensure(context, &delivery.repo)
        .map_err(|error| failure(FailureKind::Launch, error))?;
    let id = delivery.id.clone();
    let name = agent_name(&id).map_err(|error| failure(FailureKind::Invalid, error))?;
    let validity = delivery.validity;
    let now = crate::ledger::now_ms();
    let mut record = Record {
        id,
        name,
        repository,
        prompt: delivery.prompt.clone(),
        validity,
        created_ms: now,
        phase: Phase::Recorded,
        pane: None,
        terminal: None,
        workspace: None,
        updated_ms: now,
        error: None,
        close_requested_ms: None,
        closed_ms: None,
    };
    crate::lead_store::insert(context.ledger, &record).map_err(|error| Failure {
        kind: FailureKind::Launch,
        id: Some(record.id.clone()),
        pane: None,
        phase: None,
        error: Box::new(error),
    })?;
    launch(context, &mut record).map_err(|error| {
        let error = match record.pane.as_deref() {
            Some(pane) if record.phase == Phase::Starting => {
                crate::launch::startup_error(context.shell, pane, error)
            }
            _ => error,
        };
        let message = error.to_string();
        // The original error is the answer; a failed note must not replace it.
        let _ = crate::lead_store::update(context.ledger, &record.id, |saved| {
            saved.error = Some(message);
            saved.updated_ms = crate::ledger::now_ms();
            Ok(())
        });
        Failure {
            kind: FailureKind::Launch,
            id: Some(record.id.clone()),
            pane: record.pane.clone(),
            phase: Some(record.phase),
            error: Box::new(error),
        }
    })?;
    Ok(Started {
        host: delivery.host.clone(),
        id: record.id,
        name: record.name,
        pane: record.pane.unwrap_or_default(),
        workspace: record.workspace,
    })
}

/// Execute a peer delivery without entering placement again.
pub(crate) fn peer(
    context: &CreateContext<'_>,
    input: &str,
) -> crate::error::Result<serde_json::Value> {
    routing::peer(context, input)
}

/// Advance one saved phase per effect. `record` mirrors what is saved.
fn launch(context: &CreateContext<'_>, record: &mut Record) -> crate::error::Result<()> {
    let ledger = context.ledger;
    let advance = |record: &mut Record, phase: Phase| -> crate::error::Result<()> {
        record.phase = phase;
        record.updated_ms = crate::ledger::now_ms();
        let saved = record.clone();
        crate::lead_store::update(ledger, &record.id, |stored| {
            *stored = saved;
            Ok(())
        })
    };
    advance(record, Phase::Allocating)?;
    let (pane, created) = crate::work::repository_pane(context.shell, &record.repository.path)?;
    record.pane = Some(pane.clone());
    let (panes, terminals) = crate::cleanup::pane_snapshot(context.shell)?;
    record.terminal = created.or_else(|| terminals.get(&pane).cloned());
    record.workspace = panes
        .iter()
        .find(|listed| listed.pane_id == pane)
        .and_then(|listed| listed.workspace_id.clone());
    advance(record, Phase::Allocated)?;
    let terminal = record.terminal.clone().ok_or_else(|| {
        crate::error::Error::Invalid(format!("{pane}: cannot establish its terminal identity"))
    })?;
    advance(record, Phase::Starting)?;
    let args = [
        "agent",
        "start",
        &record.name,
        "--kind",
        "pi",
        "--pane",
        &pane,
    ]
    .map(str::to_owned);
    let validity = record.validity;
    crate::launch::start_native(ledger, context.shell, &args, &pane, &terminal, &|| {
        crate::jobs::check_validity(ledger, Some(&validity))
    })?;
    advance(record, Phase::Started)?;
    advance(record, Phase::Prompting)?;
    context
        .herdr
        .ok_or_else(|| {
            crate::error::Error::Invalid("this host has no Herdr prompt connection".into())
        })?
        .agent_prompt(&pane, &record.prompt)?;
    advance(record, Phase::Ready)
}

/// Whether any open lead may own this pane. Automatic paths leave such a pane alone.
pub(crate) fn protected(leads: &[Record], pane: &str, terminal: Option<&str>) -> bool {
    leads.iter().any(|lead| protects_pane(lead, pane, terminal))
}

/// The open lead that may own `pane`, read fresh for a destructive or terminal decision.
pub(crate) fn on_pane(
    ledger: &crate::ledger::Ledger,
    pane: &str,
    terminal: Option<&str>,
) -> crate::error::Result<Option<Record>> {
    Ok(crate::lead_store::list(ledger)?
        .into_iter()
        .find(|lead| protects_pane(lead, pane, terminal)))
}

/// The open lead an explicit command names, by its generated name or its pane.
pub(crate) fn resolve_local(
    ledger: &crate::ledger::Ledger,
    target: &str,
) -> crate::error::Result<Option<Record>> {
    Ok(crate::lead_store::list(ledger)?.into_iter().find(|lead| {
        lead.closed_ms.is_none() && (lead.name == target || lead.pane.as_deref() == Some(target))
    }))
}

/// A lead has no completion contract: it never signals done, ask, or blocked.
pub(crate) fn refuse_signal(pane: &str, signal: &str) -> crate::error::Error {
    crate::error::Error::Invalid(format!(
        "{pane} is a lead; leads have no {signal}. Close it explicitly with dogsled close"
    ))
}

#[cfg(test)]
mod tests;
