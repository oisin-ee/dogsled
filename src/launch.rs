//! Host-owned agent launches. Persist the pane before starting, and reconcile that
//! exact terminal after an interrupted response; never allocate a second writer.

use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::shell::Shell;
use crate::work::{self, Action, Launch};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

mod recovery;
pub use recovery::{
    PaneLaunch, Survey, close_final, close_unfinished, final_pane, launched, requests,
};
#[cfg(test)]
mod diagnostic_tests;
#[cfg(test)]
mod pane_tests;

fn with_pane<T>(ledger: &Ledger, pane: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let key = crate::unit_transport::digest_hex(pane.as_bytes());
    ledger.child("launch-panes").with_lock(&key, f)
}

#[derive(Debug, Serialize, Deserialize)]
struct Progress {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    finished_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    closed_ms: Option<u64>,
    request: Launch,
    pane: Option<String>,
    terminal: Option<String>,
    attempted: bool,
    confirmed: bool,
    result: Value,
    error: Option<String>,
    /// Definitive local rejection, persisted before allocation or any dispatch attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preflight_refusal: Option<PreflightRefusal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal_receipt: Option<crate::unit_reply::Receipt>,
    /// The host's proof that the confirmed worker ended without a sealed reply. Written once
    /// by `unit_receipt` under the launch lock; `terminal_ack` refuses after it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stopped: Option<crate::unit_reply::Stopped>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PreflightRefusal {
    LineBudget { bytes: usize },
    Validity { detail: String },
}

impl Progress {
    fn preflight_failure(&self) -> Result<Option<&str>> {
        if let Some(PreflightRefusal::Validity { detail }) = &self.preflight_refusal {
            if self.attempted
                || self.confirmed
                || self.terminal_receipt.is_some()
                || self.stopped.is_some()
                || self.error.as_ref() != Some(detail)
            {
                return Err(crate::worktree::refuse(
                    "inconsistent validity refusal provenance",
                ));
            }
            return Ok(Some(detail));
        }
        let Some(PreflightRefusal::LineBudget { bytes }) = self.preflight_refusal else {
            return Ok(None);
        };
        let Action::Agent { kind, .. } = &self.request.action else {
            return Err(crate::worktree::refuse(
                "preflight refusal is not an agent launch",
            ));
        };
        let expected = work::line_budget_error(kind, bytes).to_string();
        if bytes <= work::LINE_BUDGET
            || self.attempted
            || self.confirmed
            || self.terminal_receipt.is_some()
            || self.stopped.is_some()
            || self.pane != self.request.pane
            || self.terminal != self.request.terminal
            || self.error.as_deref() != Some(expected.as_str())
        {
            return Err(crate::worktree::refuse(
                "inconsistent preflight refusal provenance",
            ));
        }
        Ok(self.error.as_deref())
    }
}

/// R1 asks the launch owner, never startup confirmation, for completion evidence.
pub(crate) fn retention_completion(ledger: &Ledger, id: &str) -> Result<Option<u64>> {
    crate::jobs::validate_id(id)?;
    let Some(progress) = ledger
        .child("launches")
        .read::<Progress>(&format!("{id}.progress.json"))?
    else {
        return Ok(None);
    };
    if progress.request.id != id {
        return Err(crate::worktree::refuse(
            "launch completion identity changed",
        ));
    }
    if let Some(stopped) = &progress.stopped
        && (stopped.launch != progress.request
            || !progress.confirmed
            || !progress.attempted
            || progress.terminal_receipt.is_some())
    {
        return Err(crate::worktree::refuse(
            "stopped completion provenance changed",
        ));
    }
    if let Some(receipt) = &progress.terminal_receipt {
        let seed = progress
            .request
            .unit
            .as_ref()
            .ok_or_else(|| crate::worktree::refuse("terminal seed missing"))?;
        crate::unit_transport::validate_seed(seed, id, &progress.request.owner)?;
        if !progress.confirmed
            || !progress.attempted
            || receipt.ack.execution != seed.execution
            || receipt.ack.unit_ref != seed.binding.contract
            || receipt.ack.digest != seed.exported.digest
        {
            return Err(crate::worktree::refuse(
                "terminal completion provenance changed",
            ));
        }
    }
    let terminal = progress.preflight_failure()?.is_some()
        || progress.stopped.is_some()
        || progress.terminal_receipt.is_some()
        || (progress.confirmed && progress.closed_ms.is_some())
        || refused_before_start(ledger, &progress.request)?;
    crate::jobs::completion_time(
        ledger,
        progress.finished_ms,
        progress.request.validity.map(|v| v.created_ms),
        terminal,
    )
}

pub(crate) fn execution_closed(ledger: &Ledger, item: &crate::items::Item) -> Result<()> {
    let receipts = ledger.child("launches");
    let Some(closed) = item.closed_ms.filter(|_| item.resource_closed_ms.is_none()) else {
        return Ok(());
    };
    receipts.with_lock(&item.id, || {
        let file = format!("{}.progress.json", item.id);
        let Some(mut progress) = receipts.read::<Progress>(&file)? else {
            return Ok(());
        };
        if progress.request.id != item.id
            || progress.request.host != item.host
            || progress.pane.as_ref() != Some(&item.pane)
            || !progress.confirmed
        {
            return Ok(());
        }
        if progress.closed_ms.is_some_and(|saved| saved != closed) {
            return Err(crate::worktree::refuse(
                "closed execution provenance changed",
            ));
        }
        if progress.closed_ms.is_none() {
            progress.closed_ms = Some(closed);
            if progress.finished_ms.is_none() {
                progress.finished_ms = Some(closed);
            }
            receipts.write(&file, &progress)?;
        }
        Ok(())
    })
}

/// The reason a unit settles `stopped`.
const STOPPED_REASON: &str = "worker pane closed without a final reply";

/// Herdr's codes for an `agent start` it refused before typing into the pane. Any other
/// failure may come after the agent began, so its outcome stays unknown. `agent_pane_busy` is
/// not final: the shell may still be starting, so the start is retried on the same pane.
const REFUSALS: [&str; 6] = [
    "invalid_agent_name",
    "unsupported_agent_kind",
    "invalid_agent_argument",
    "invalid_agent_timeout",
    "agent_pane_not_found",
    "agent_name_taken",
];

/// How long a busy pane may keep its shell initializing before the launch gives up.
const BUSY_WINDOW: std::time::Duration = std::time::Duration::from_secs(20);
const BUSY_FIRST_PAUSE: std::time::Duration = std::time::Duration::from_millis(250);
const BUSY_MAX_PAUSE: std::time::Duration = std::time::Duration::from_secs(2);

fn busy_window() -> std::time::Duration {
    // Test rigs only. Production always waits `BUSY_WINDOW`.
    std::env::var("DOGSLED_TEST_BUSY_WINDOW_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map_or(BUSY_WINDOW, std::time::Duration::from_millis)
}

fn is_busy(error: &Error) -> bool {
    let Error::Command { stderr, .. } = error.root() else {
        return false;
    };
    serde_json::from_str::<Value>(stderr)
        .ok()
        .and_then(|v| v.pointer("/error/code")?.as_str().map(str::to_owned))
        .as_deref()
        == Some("agent_pane_busy")
}

/// Run `agent start`, repeating it on the same pane while Herdr reports the shell busy. Herdr
/// typed nothing into a busy pane, so a repeat cannot start the agent twice. A pane id can be
/// reused by another terminal during a pause, so every repeat first proves that the pane still
/// runs the launch's saved `terminal`.
fn start_while_busy(
    ledger: &Ledger,
    shell: &dyn Shell,
    args: &[String],
    pane: &str,
    terminal: &str,
    window: std::time::Duration,
    request: &Launch,
) -> Result<()> {
    let began = std::time::Instant::now();
    let mut pause = BUSY_FIRST_PAUSE;
    let mut retry = false;
    loop {
        // Before invocation, expiry proves no start was issued. A deadline returned by the
        // invocation itself is different: the agent may already have begun.
        let remaining = ledger
            .budget
            .with_limit("agent start", crate::budget::SETUP_LIMIT)
            .and_then(|left| shell.budget().with_limit("agent start", left))
            .map_err(|_| {
                Error::Launch(format!(
                    "{pane} was still not an available shell when the command deadline passed after {:.1}s; the pane stays open",
                    began.elapsed().as_secs_f32()
                ))
            })?;
        if retry {
            // Every earlier start was refused as busy, so nothing was issued yet.
            same_terminal(shell, pane, terminal).map_err(|error| {
                Error::Launch(format!(
                    "{pane} was busy and its launch terminal could not be revalidated before a retry; refusing to take it over: {error}"
                ))
            })?;
        }
        work::check_launch_validity(ledger, request)?;
        retry = true;
        match shell.run_setup_ok("herdr", args) {
            Err(error) if is_busy(&error) => {
                let elapsed = began.elapsed();
                let Some(left) = window.checked_sub(elapsed).filter(|d| !d.is_zero()) else {
                    return Err(Error::Launch(format!(
                        "{pane} was still not an available shell after {:.1}s; the pane stays open: {error}",
                        elapsed.as_secs_f32()
                    )));
                };
                // Include time spent in the refused command in the shared deadline.
                let remaining = ledger
                    .budget
                    .with_limit("agent start", remaining)
                    .and_then(|left| shell.budget().with_limit("agent start", left))
                    .unwrap_or_default();
                std::thread::sleep(pause.min(left).min(remaining));
                pause = (pause * 2).min(BUSY_MAX_PAUSE);
            }
            other => return other.map(drop),
        }
    }
}

/// A finished launch: its result, or the refusal that ended it.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum Receipt {
    Refused { failed: String },
    Started(Value),
}

/// Hold only an allocating launch or an attempted start with its matching live agent.
/// A stranded empty shell cannot finish startup and does not hold the checkout.
fn pending(progress: &Progress, agents: &[crate::probe::Agent]) -> bool {
    progress.pane.as_ref().is_none_or(|pane| {
        progress.attempted
            && agents.iter().any(|agent| {
                let Action::Agent { name, kind, .. } = &progress.request.action else {
                    return false;
                };
                agent.pane == *pane
                    && agent.name.as_ref() == Some(name)
                    && agent.agent.as_ref() == Some(kind)
                    && progress.terminal.is_some()
                    && agent.terminal_id == progress.terminal
            })
    })
}

/// Find the durable launch identity for a pane, independent of Herdr's terminal ids.
pub fn identity(ledger: &Ledger, pane: &str) -> Result<Option<String>> {
    let found = recovery::candidates(&ledger.child("launches"), Some(pane))?;
    match found.as_slice() {
        [] => Ok(None),
        [(id, _)] => Ok(Some(id.clone())),
        _ => Err(Error::Invalid(format!("{pane}: competing launch records"))),
    }
}

/// How far this host's launch for `pane` has progressed. The lifecycle owner reads this to
/// tell startup in progress from a launch that never reached its pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchProgress {
    /// No launch record names this pane.
    None,
    /// The launch is recorded, and its pane is not allocated yet.
    Allocating,
    /// The pane is allocated, and no start has been attempted yet.
    Allocated,
    /// A start was attempted. The outcome may still be unknown.
    Attempted,
    /// The start is confirmed: the pane runs the launched agent.
    Confirmed,
    /// More than one launch record names this pane. The records contradict each other, so
    /// nothing is proved: not a launch, and not the absence of one.
    Ambiguous,
}

impl LaunchProgress {
    /// The name the launch result and the owner's item publish.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Allocating => "allocating",
            Self::Allocated => "allocated",
            Self::Attempted => "attempted",
            Self::Confirmed => "confirmed",
            Self::Ambiguous => "ambiguous",
        }
    }
}

/// The launch progress this host recorded for `pane`.
pub fn progress(ledger: &Ledger, pane: &str) -> Result<LaunchProgress> {
    let found = recovery::candidates(&ledger.child("launches"), Some(pane))?;
    Ok(match found.as_slice() {
        [(_, progress)] => {
            if progress.confirmed {
                LaunchProgress::Confirmed
            } else if progress.attempted {
                LaunchProgress::Attempted
            } else if progress.pane.is_some() {
                LaunchProgress::Allocated
            } else {
                LaunchProgress::Allocating
            }
        }
        // No record names this pane: no launch is proved.
        [] => LaunchProgress::None,
        // Competing records contradict each other. That is not the absence of a launch, so
        // it gets its own name and never reads as "nothing was launched".
        [_, _, ..] => LaunchProgress::Ambiguous,
    })
}

/// A seeded worker's end-of-turn hook must persist its immutable terminal evidence before
/// its process closes. This is host-local and does not wait for the owner's acknowledgement.
pub fn handoff_ready(ledger: &Ledger, pane: &str) -> Result<bool> {
    Ok(recovery::candidates(&ledger.child("launches"), Some(pane))?
        .iter()
        .all(|(_, progress)| {
            progress.request.unit.is_none() || progress.terminal_receipt.is_some()
        }))
}

/// What this host's launch evidence says about the unit execution in a pane that called `done`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Execution {
    /// No unit execution was launched in the pane.
    Plain,
    /// The unit has not returned. Only `owner` may continue it. `host` is the originating
    /// owner's hostname; a launch record without one cannot prove any sender is its owner.
    Unreturned { owner: String, host: Option<String> },
    /// The worker acknowledged its sealed terminal boundary.
    Returned,
}

/// Bind `agent` and its done mark's launch `item` to exactly one seeded launch, or to none.
/// Missing, unreadable, competing, or mismatched evidence is an error, never `Plain`.
pub fn execution(
    ledger: &Ledger,
    item: Option<&str>,
    agent: &crate::probe::Agent,
) -> Result<Execution> {
    let receipts = ledger.child("launches");
    let seeded: Vec<_> = recovery::candidates(&receipts, Some(&agent.pane))?
        .into_iter()
        .filter(|(_, progress)| progress.request.unit.is_some())
        .collect();
    let [(id, progress)] = seeded.as_slice() else {
        if !seeded.is_empty() {
            return Err(crate::unit_transport::conflict(&format!(
                "{}: competing unit launches",
                agent.pane
            )));
        }
        if let Some(item) = item {
            crate::jobs::validate_id(item)?;
            match std::fs::symlink_metadata(receipts.dir.join(format!("{item}.unit.json"))) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
                Ok(_) => {
                    return Err(crate::unit_transport::conflict(&format!(
                        "unit launch {item} has a seed but no launch record for {}",
                        agent.pane
                    )));
                }
            }
        }
        return Ok(Execution::Plain);
    };
    let seed = progress
        .request
        .unit
        .as_ref()
        .ok_or_else(|| crate::unit_transport::conflict("host seed missing"))?;
    crate::unit_transport::validate_seed(seed, id, &progress.request.owner)?;
    let Action::Agent { name, .. } = &progress.request.action else {
        return Err(crate::unit_transport::conflict("not an agent launch"));
    };
    if item != Some(id.as_str())
        || progress.terminal.is_none()
        || agent.terminal_id != progress.terminal
        || agent.agent.as_deref() != Some("pi")
        || agent.name.as_deref() != Some(name.as_str())
    {
        return Err(crate::unit_transport::conflict(&format!(
            "{}: done mark, terminal, or agent does not match unit launch {id}",
            agent.pane
        )));
    }
    if receipts
        .read::<crate::unit_transport::Seed>(&format!("{id}.unit.json"))?
        .as_ref()
        != Some(seed)
    {
        return Err(crate::unit_transport::conflict(&format!(
            "unit launch {id}: durable seed is missing or changed"
        )));
    }
    match &progress.terminal_receipt {
        None => Ok(Execution::Unreturned {
            owner: seed.execution.owner_id.clone(),
            host: progress.request.owner_host.clone(),
        }),
        Some(receipt) if receipt.ack.execution == seed.execution => Ok(Execution::Returned),
        Some(_) => Err(crate::unit_transport::conflict(&format!(
            "unit launch {id}: terminal receipt names another execution"
        ))),
    }
}

/// Hold launch `id`'s lock while `f` decides on input to its legacy unreturned execution in
/// `agent`'s pane. `f` gets the exact saved launch request, as stored. Under the lock the launch
/// must still bind the pane, be started and confirmed, have no owner host, and have no
/// terminal receipt, so a terminal acknowledgement cannot pass between check and input.
pub fn with_legacy_unreturned<T>(
    ledger: &Ledger,
    id: &str,
    agent: &crate::probe::Agent,
    f: impl FnOnce(&Value) -> Result<T>,
) -> Result<T> {
    crate::jobs::validate_id(id)?;
    let receipts = ledger.child("launches");
    receipts.with_lock(id, || {
        if !matches!(
            execution(ledger, Some(id), agent)?,
            Execution::Unreturned { host: None, .. }
        ) {
            return Err(crate::unit_transport::conflict(&format!(
                "unit launch {id} is not an unreturned launch without an owner host"
            )));
        }
        let saved = receipts
            .read::<Value>(&format!("{id}.progress.json"))?
            .ok_or_else(|| crate::unit_transport::conflict("host launch record missing"))?;
        let progress: Progress = serde_json::from_value(saved.clone())?;
        let started = match receipts.read::<Receipt>(&format!("{id}.json"))? {
            Some(Receipt::Started(result)) => result == progress.result,
            _ => false,
        };
        if !started
            || !progress.attempted
            || !progress.confirmed
            || progress.terminal_receipt.is_some()
            || progress.request.id != id
            || progress.pane.as_deref() != Some(agent.pane.as_str())
            || progress
                .request
                .pane
                .as_deref()
                .is_some_and(|p| p != agent.pane)
            || saved["request"].get("owner_host").is_some()
        {
            return Err(crate::unit_transport::conflict(&format!(
                "unit launch {id} is not a started, confirmed, unreturned legacy launch"
            )));
        }
        f(&saved["request"])
    })
}

/// A seeded result uses the acknowledged transcript branch, not its newest message.
pub fn handoff_reply(ledger: &Ledger, pane: &str) -> Result<Option<String>> {
    Ok(recovery::candidates(&ledger.child("launches"), Some(pane))?
        .into_iter()
        .find_map(|(_, progress)| progress.terminal_receipt.map(|receipt| receipt.text)))
}

/// Replay a final outcome without acquiring repository protection or changing host state.
pub fn replay(ledger: &Ledger, request: &Launch) -> Result<Option<Value>> {
    let receipts = ledger.child("launches");
    let saved = receipts.read::<Progress>(&format!("{}.progress.json", request.id))?;
    if saved.as_ref().is_some_and(|p| p.request != *request) {
        return Err(Error::Invalid(format!(
            "launch {} changed after it was recorded",
            request.id
        )));
    }
    match receipts.read::<Receipt>(&format!("{}.json", request.id))? {
        Some(Receipt::Started(result)) => Ok(Some(result)),
        Some(Receipt::Refused { failed }) => Err(Error::Invalid(failed)),
        None => {
            if let Some(progress) = saved
                && let Some(failed) = progress.preflight_failure()?
            {
                return Err(Error::Invalid(failed.into()));
            }
            Ok(None)
        }
    }
}

/// Whether a launch record exists, and the checkout identity that record captured.
pub fn checkout_for_request(
    ledger: &Ledger,
    request: &Launch,
) -> Result<(bool, Option<crate::cleanup::Tree>)> {
    let saved = ledger
        .child("launches")
        .read::<Progress>(&format!("{}.progress.json", request.id))?;
    let Some(saved) = saved else {
        return Ok((false, None));
    };
    if saved.request != *request {
        return Err(crate::worktree::refuse("launch request changed"));
    }
    Ok((
        true,
        saved
            .result
            .get("checkout")
            .map(|tree| serde_json::from_value(tree.clone()).map_err(crate::error::Error::from))
            .transpose()?,
    ))
}

/// Ownership refusal is local to one checkout; evidence I/O failures remain pass-wide.
#[derive(Debug, thiserror::Error)]
pub enum CheckoutOwnerError {
    #[error("refuse: {0}; nothing was closed")]
    Refused(&'static str),
    #[error(transparent)]
    Evidence(#[from] Error),
}

impl From<CheckoutOwnerError> for Error {
    fn from(error: CheckoutOwnerError) -> Self {
        match error {
            CheckoutOwnerError::Refused(reason) => crate::worktree::refuse(reason),
            CheckoutOwnerError::Evidence(error) => error,
        }
    }
}

/// A reused pane or a replacement item cannot inherit a removed launch's checkout.
pub fn validate_checkout_owner(
    ledger: &Ledger,
    pane: &str,
    request: Option<&str>,
    terminal: Option<&str>,
) -> std::result::Result<(), CheckoutOwnerError> {
    let captured: Vec<_> = recovery::candidates(&ledger.child("launches"), Some(pane))?
        .into_iter()
        .filter(|(_, progress)| progress.result.get("checkout").is_some())
        .collect();
    let [(id, progress)] = captured.as_slice() else {
        return Err(CheckoutOwnerError::Refused(
            "checkout has no unique saved launch owner",
        ));
    };
    if request.is_some_and(|request| request != id)
        || terminal.is_some_and(|terminal| progress.terminal.as_deref() != Some(terminal))
        || crate::items::list(ledger)?.iter().any(|item| {
            item.host == crate::hosts::LOCAL
                && item.pane == pane
                && item.closed_ms.is_none()
                && item.id != *id
        })
    {
        return Err(CheckoutOwnerError::Refused(
            "pane or item belongs to another checkout owner",
        ));
    }
    Ok(())
}

/// Saved administrative identity survives checkout and preparation retirement.
pub fn checkout_for_pane(ledger: &Ledger, pane: &str) -> Result<Option<crate::cleanup::Tree>> {
    let candidates = recovery::candidates(&ledger.child("launches"), Some(pane))?;
    let mut captured = None;
    for (_, progress) in candidates {
        if let Some(value) = progress.result.get("checkout") {
            let tree: crate::cleanup::Tree = serde_json::from_value(value.clone())?;
            if tree
                .identity
                .as_ref()
                .is_some_and(|identity| identity.common == identity.gitdir)
            {
                continue;
            }
            if captured.as_ref().is_some_and(|old| old != &tree) {
                return Err(crate::worktree::refuse(
                    "pane has conflicting checkout identities",
                ));
            }
            captured = Some(tree);
        }
    }
    Ok(captured)
}

#[cfg(test)]
fn agent(ledger: &Ledger, shell: &dyn Shell, request: &Launch) -> Result<Value> {
    agent_guarded(ledger, shell, request, None)
}

pub fn agent_guarded(
    ledger: &Ledger,
    shell: &dyn Shell,
    request: &Launch,
    checkout: Option<&crate::cleanup::Tree>,
) -> Result<Value> {
    let receipts = ledger.child("launches");
    receipts.with_lock(&request.id, || {
        retention_completion(ledger, &request.id)?;
        let file = format!("{}.json", request.id);
        let progress_file = format!("{}.progress.json", request.id);
        let saved = receipts.read::<Progress>(&progress_file)?;
        if saved.as_ref().is_some_and(|p| p.request != *request) {
            return Err(Error::Invalid(format!(
                "launch {} changed after it was recorded",
                request.id
            )));
        }
        match receipts.read::<Receipt>(&file)? {
            Some(Receipt::Started(result)) => return Ok(result),
            Some(Receipt::Refused { failed }) => return Err(Error::Invalid(failed)),
            None => (),
        }
        if let Some(progress) = &saved
            && let Some(failed) = progress.preflight_failure()?
        {
            receipts.write(
                &file,
                &Receipt::Refused {
                    failed: failed.into(),
                },
            )?;
            return Err(Error::Invalid(failed.into()));
        }
        let mut progress = if let Some(progress) = saved {
            progress
        } else {
            work::check_launch_validity(ledger, request)?;
            allocate(ledger, shell, request, &progress_file, checkout)?
        };
        let pane = progress.pane.clone().ok_or_else(|| {
            Error::Invalid(format!(
                "launch {} stopped while creating its pane; inspect the host before resubmitting",
                request.id
            ))
        })?;
        // A confirmed start must not run again, even if the agent finished and exited.
        if !progress.confirmed {
            with_pane(ledger, &pane, || {
                if let Err(error) =
                    start_or_reconcile(ledger, shell, &mut progress, &pane, &progress_file)
                {
                    let owned = progress
                        .terminal
                        .as_deref()
                        .map(|terminal| (pane.as_str(), terminal));
                    return Err(refused(ledger, shell, &file, owned, error)?);
                }
                Ok(())
            })?;
            progress.confirmed = true;
            // Stamp after native start (or matching-agent recovery), on the pane's host.
            // Persist with confirmation so retries return the same evidence to the owner.
            progress.result["launch_confirmed_ms"] = json!(crate::ledger::now_ms());
            receipts.write(&progress_file, &progress)?;
        }
        // Parent registration can fail after startup; the confirmed result survives it.
        crate::watchdog::register_parent(ledger, shell, &pane)?;
        receipts.write(&file, &progress.result)?;
        Ok(progress.result)
    })
}

/// A start that Herdr refused is final: no agent ran, so record the refusal as the receipt and
/// close only its still-owned idle shell, whether allocated or prepared. Unknown or occupied panes
/// survive the final refusal. Any other error passes through unchanged.
fn refused(
    ledger: &Ledger,
    shell: &dyn Shell,
    file: &str,
    pane: Option<(&str, &str)>,
    error: Error,
) -> Result<Error> {
    let Error::Command { stderr, .. } = error.root() else {
        return Ok(error);
    };
    let code = serde_json::from_str::<Value>(stderr)
        .ok()
        .and_then(|v| v.pointer("/error/code")?.as_str().map(str::to_owned));
    if !code.is_some_and(|code| REFUSALS.contains(&code.as_str())) {
        return Ok(error);
    }
    let receipts = ledger.child("launches");
    let failed = error.to_string();
    let progress_file = file.replace(".json", ".progress.json");
    if let Some(mut progress) = receipts.read::<Progress>(&progress_file)? {
        crate::jobs::finish_once(
            ledger,
            &mut progress.finished_ms,
            progress.request.validity.map(|v| v.created_ms),
        )?;
        receipts.write(&progress_file, &progress)?;
    }
    receipts.write(
        file,
        &Receipt::Refused {
            failed: failed.clone(),
        },
    )?;
    let closed = pane.map_or(Ok(()), |(pane, expected)| {
        close_refused_pane(shell, pane, expected)
    });
    Ok(Error::Invalid(match closed {
        Ok(()) => failed,
        Err(close) => format!("{failed}; its pane stays open: {close}"),
    }))
}

/// Typed local preflight and final native refusals prove that no agent was started.
/// The caller holds preparation protection, and the final check holds the launch lock.
pub(crate) fn refused_before_start(ledger: &Ledger, request: &Launch) -> Result<bool> {
    let receipts = ledger.child("launches");
    let receipt = receipts.read::<Receipt>(&format!("{}.json", request.id))?;
    let Some(progress) = receipts.read::<Progress>(&format!("{}.progress.json", request.id))?
    else {
        return if matches!(receipt, Some(Receipt::Refused { .. })) {
            Err(crate::worktree::refuse("refusal has no launch provenance"))
        } else {
            Ok(false)
        };
    };
    let preflight = progress.preflight_failure()?;
    let failed = match &receipt {
        Some(Receipt::Refused { failed }) => failed.as_str(),
        None if preflight.is_some() => preflight.unwrap_or_default(),
        _ => return Ok(false),
    };
    let preflight = preflight.is_some();
    if progress.request != *request
        || (!preflight && !progress.attempted)
        || progress.confirmed
        || progress.terminal_receipt.is_some()
        || progress.error.as_deref() != Some(failed)
        || (!preflight && progress.pane.as_deref().is_none_or(str::is_empty))
        || (!preflight && progress.terminal.as_deref().is_none_or(str::is_empty))
    {
        return Err(crate::worktree::refuse(
            "refusal has inconsistent launch provenance",
        ));
    }
    Ok(true)
}

fn close_refused_pane(shell: &dyn Shell, pane: &str, expected: &str) -> Result<()> {
    if std::env::var("HERDR_PANE_ID").ok().as_deref() == Some(pane)
        || crate::cleanup::current_panes(shell)?
            .iter()
            .any(|p| p.pane_id == pane && p.focused)
    {
        return Err(Error::Invalid(format!("{pane}: caller or focused pane")));
    }
    if terminal(shell, pane)? != expected {
        return Err(Error::Invalid(format!("{pane}: launch terminal changed")));
    }
    if crate::probe::agent_list(shell)?
        .iter()
        .any(|agent| agent.pane == pane)
    {
        return Err(Error::Invalid(format!(
            "{pane}: an agent now owns the pane"
        )));
    }
    idle(shell, pane)?;
    // Inspection must not authorize closing a replacement terminal.
    if terminal(shell, pane)? != expected {
        return Err(Error::Invalid(format!("{pane}: launch terminal changed")));
    }
    shell.run_ok("herdr", &["pane".into(), "close".into(), pane.into()])?;
    Ok(())
}

fn allocate(
    ledger: &Ledger,
    shell: &dyn Shell,
    request: &Launch,
    file: &str,
    checkout: Option<&crate::cleanup::Tree>,
) -> Result<Progress> {
    let receipts = ledger.child("launches");
    let Action::Agent {
        kind,
        name,
        profile,
        ..
    } = &request.action
    else {
        return Err(Error::Invalid("not an agent launch".into()));
    };
    if request.pane.is_some() && request.terminal.as_deref().is_none_or(str::is_empty) {
        return Err(Error::Invalid(format!(
            "launch {}: prepared pane has no terminal identity; refusing to take it over",
            request.id
        )));
    }
    // Validate profile/argv before creating a pane. An allocating state without a
    // pane is deliberately not retried after a crash: its ownership is uncertain.
    let profile = profile.as_deref().map(crate::profiles::load).transpose()?;
    // Encode first so a budget refusal has typed, durable proof of no dispatch.
    let argv = work::agent_argv(
        kind,
        name,
        &seed_args(request, &receipts),
        &work::Files::at(&receipts.dir, &request.id),
        profile.as_ref(),
    )?;
    if let Some(seed) = &request.unit {
        validate_seed(request, seed, profile.as_ref())?;
        let sessions = receipts.dir.join(format!("{}.sessions", request.id));
        if sessions.exists() {
            return Err(crate::unit_transport::conflict(
                "fresh unit session directory already exists",
            ));
        }
    }
    let mut result = json!({"host":request.host,"name":name});
    if let Some(checkout) = checkout {
        result["checkout"] = serde_json::to_value(checkout)?;
    }
    if let Some(profile) = &profile {
        result["profile"] = json!(profile.name);
        for (key, value) in [("model", &profile.model), ("role", &profile.role)] {
            if let Some(value) = value {
                result[key] = json!(value);
            }
        }
    }
    let mut progress = Progress {
        finished_ms: None,
        closed_ms: None,
        request: request.clone(),
        pane: None,
        terminal: request.terminal.clone(),
        attempted: false,
        confirmed: false,
        result,
        error: None,
        preflight_refusal: None,
        terminal_receipt: None,
        stopped: None,
    };
    let bytes = work::line_len(kind, &argv);
    if bytes > work::LINE_BUDGET {
        let error = work::line_budget_error(kind, bytes);
        progress.pane.clone_from(&request.pane);
        progress.error = Some(error.to_string());
        progress.preflight_refusal = Some(PreflightRefusal::LineBudget { bytes });
        crate::jobs::finish_once(
            ledger,
            &mut progress.finished_ms,
            request.validity.map(|v| v.created_ms),
        )?;
        receipts.write(file, &progress)?;
        crate::unit_request::crash_point("preflight-refusal-persisted");
        receipts.write(
            &format!("{}.json", request.id),
            &Receipt::Refused {
                failed: error.to_string(),
            },
        )?;
        return Err(error);
    }
    work::check_launch_validity(ledger, request)?;
    receipts.write(file, &progress)?;
    let (pane, terminal) = work::pane(shell, request)?;
    progress.pane = Some(pane.clone());
    progress.result["pane"] = json!(pane);
    progress.terminal = terminal;
    receipts.write(file, &progress)?;
    Ok(progress)
}

fn validity_refused(
    ledger: &Ledger,
    progress: &mut Progress,
    file: &str,
    error: &Error,
) -> Result<()> {
    let receipts = ledger.child("launches");
    progress.attempted = false;
    progress.error = Some(error.to_string());
    progress.preflight_refusal = Some(PreflightRefusal::Validity {
        detail: error.to_string(),
    });
    crate::jobs::finish_once(
        ledger,
        &mut progress.finished_ms,
        progress.request.validity.map(|v| v.created_ms),
    )?;
    receipts.write(file, progress)?;
    receipts.write(
        &format!("{}.json", progress.request.id),
        &Receipt::Refused {
            failed: error.to_string(),
        },
    )
}

fn finish_progress(ledger: &Ledger, progress: &mut Progress) -> Result<()> {
    crate::jobs::finish_once(
        ledger,
        &mut progress.finished_ms,
        progress.request.validity.map(|v| v.created_ms),
    )?;
    ledger
        .child("launches")
        .write(&format!("{}.progress.json", progress.request.id), progress)
}

fn start_or_reconcile(
    ledger: &Ledger,
    shell: &dyn Shell,
    progress: &mut Progress,
    pane: &str,
    file: &str,
) -> Result<()> {
    let request = &progress.request;
    let Action::Agent {
        kind,
        name,
        message,
        profile,
    } = &request.action
    else {
        return Err(Error::Invalid("not an agent launch".into()));
    };
    let current = terminal(shell, pane)?;
    if progress.terminal.as_deref() != Some(current.as_str()) {
        return Err(Error::Invalid(format!(
            "launch {}: terminal for {pane} changed or was not recorded; refusing to take it over",
            request.id
        )));
    }
    let existing = crate::probe::agent_list(shell)?
        .into_iter()
        .find(|a| a.pane == pane);
    if let Some(existing) = existing {
        if !progress.attempted
            || existing.name.as_deref() != Some(name)
            || existing.agent.as_deref() != Some(kind)
            || existing.terminal_id.as_deref() != Some(current.as_str())
        {
            return Err(Error::Invalid(format!(
                "launch {}: {pane} has another agent; refusing to take it over",
                request.id
            )));
        }
        return Ok(());
    }
    if progress.attempted {
        // An idle shell could mean the task ran and exited, not that it never ran.
        let previous = progress
            .error
            .as_deref()
            .unwrap_or("startup response was interrupted");
        // A replay may find diagnostics that were unavailable at the original deadline.
        let detail = match startup_output(shell, pane) {
            Some(output) if !previous.contains(&output) => {
                format!("{previous}; startup output from {pane}:\n{output}")
            }
            _ => previous.to_owned(),
        };
        let error = Error::Launch(format!(
            "launch {}: no matching agent in {pane}; startup outcome is unknown; inspect before resubmitting; {detail}",
            request.id,
        ));
        progress.error = Some(detail);
        ledger.child("launches").write(file, progress)?;
        return Err(error);
    }
    work::check_launch_validity(ledger, request)?;
    // Native agent.start owns shell readiness and its atomic busy refusal. An eager
    // process snapshot here rejects startup before that supported boundary can run.
    let profile = profile.as_deref().map(crate::profiles::load).transpose()?;
    let receipts = ledger.child("launches");
    let files = work::Files::write(&receipts.dir, &request.id, message)?;
    let seed_flags = seed_flags(ledger, request, profile.as_ref())?;
    let mut args = vec![
        "agent".into(),
        "start".into(),
        name.clone(),
        "--kind".into(),
        kind.clone(),
        "--pane".into(),
        pane.into(),
        "--".into(),
    ];
    args.extend(work::agent_args(
        kind,
        name,
        &seed_flags,
        &files,
        profile.as_ref(),
    )?);
    progress.attempted = true;
    receipts.write(file, progress)?;
    if let Err(error) =
        start_while_busy(ledger, shell, &args, pane, &current, busy_window(), request)
    {
        if matches!(error.root(), Error::Refused { .. }) {
            validity_refused(ledger, progress, file, &error)?;
            return Err(error);
        }
        if matches!(error, Error::Launch(_)) {
            // Herdr never typed into the pane, so the launch may be tried again.
            progress.attempted = false;
            progress.error = Some(error.to_string());
            receipts.write(file, progress)?;
            return Err(error);
        }
        let error = startup_error(shell, pane, error);
        progress.error = Some(error.to_string());
        receipts.write(file, progress)?;
        return Err(error);
    }
    Ok(())
}

fn startup_error(shell: &dyn Shell, pane: &str, error: Error) -> Error {
    match startup_output(shell, pane) {
        Some(output) => Error::Startup {
            source: Box::new(error),
            pane: pane.into(),
            output,
        },
        None => error,
    }
}

/// Read startup diagnostics from the terminal even when no agent was recognized.
/// A missing pane or failed read must not replace the original launch failure.
fn startup_output(shell: &dyn Shell, pane: &str) -> Option<String> {
    let output = shell
        .run_ok(
            "herdr",
            &[
                "pane".into(),
                "read".into(),
                pane.into(),
                "--source".into(),
                "recent-unwrapped".into(),
                "--lines".into(),
                "120".into(),
                "--format".into(),
                "text".into(),
            ],
        )
        .ok()?;
    let text = crate::activity::plain(&output.stdout);
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn validate_seed<'a>(
    request: &Launch,
    seed: &'a crate::unit_transport::Seed,
    profile: Option<&crate::profiles::Profile>,
) -> Result<&'a str> {
    let expected = crate::unit_transport::validate_seed(seed, &request.id, &request.owner)?;
    let Action::Agent {
        kind,
        profile: selected,
        name,
        message,
    } = &request.action
    else {
        return Err(crate::unit_transport::conflict("seed requires an agent"));
    };
    if kind != "pi"
        || selected.as_deref() != Some(expected)
        || *message != seed.exported.brief
        || *name != crate::unit_transport::agent_name(&seed.binding.contract, &seed.execution)?
    {
        return Err(crate::unit_transport::conflict(
            "seed requires its assigned Pi profile",
        ));
    }
    if profile.is_none_or(|p| {
        p.pi_args.iter().any(|arg| {
            matches!(
                arg.split('=').next().unwrap_or(arg),
                "-c" | "--continue"
                    | "-r"
                    | "--resume"
                    | "--session"
                    | "--session-id"
                    | "--session-dir"
                    | "--fork"
                    | "--no-session"
                    | "--todo-unit-file"
                    | "--"
            )
        })
    }) {
        return Err(Error::Invalid("PROFILE_INVALID: managed units require fresh persisted sessions; profile session/seed flags are incompatible".into()));
    }
    Ok(expected)
}

fn seed_flags(
    ledger: &Ledger,
    request: &Launch,
    profile: Option<&crate::profiles::Profile>,
) -> Result<Vec<String>> {
    let Some(seed) = &request.unit else {
        return Ok(Vec::new());
    };
    validate_seed(request, seed, profile)?;
    let receipts = ledger.child("launches");
    let file = format!("{}.unit.json", request.id);
    match receipts.read::<crate::unit_transport::Seed>(&file)? {
        Some(saved) if saved != *seed => {
            return Err(crate::unit_transport::conflict("durable seed changed"));
        }
        Some(_) => (),
        None => receipts.write(&file, seed)?,
    }
    let sessions = receipts.dir.join(format!("{}.sessions", request.id));
    std::fs::create_dir_all(&sessions)?;
    if std::fs::read_dir(&sessions)?.next().is_some() {
        return Err(crate::unit_transport::conflict(
            "fresh unit session directory is not empty",
        ));
    }
    Ok(seed_args(request, &receipts))
}

/// The Pi flags that bind a unit launch to its seed and fresh session directory. Pure: the
/// files and directory they name are made by `seed_flags`.
fn seed_args(request: &Launch, receipts: &Ledger) -> Vec<String> {
    if request.unit.is_none() {
        return Vec::new();
    }
    vec![
        "--todo-unit-file".into(),
        receipts
            .dir
            .join(format!("{}.unit.json", request.id))
            .display()
            .to_string(),
        "--session-dir".into(),
        receipts
            .dir
            .join(format!("{}.sessions", request.id))
            .display()
            .to_string(),
        "--session-id".into(),
        request.id.clone(),
    ]
}

/// Worker acknowledgement is host-owned, serialized with launch and kept with its receipt.
pub fn terminal_ack(
    ledger: &Ledger,
    shell: &dyn Shell,
    ack: &crate::unit_transport::TerminalAck,
) -> Result<crate::unit_reply::Receipt> {
    crate::jobs::validate_id(&ack.execution.request_id)?;
    let receipts = ledger.child("launches");
    receipts.with_lock(&ack.execution.request_id, || {
        let file = format!("{}.progress.json", ack.execution.request_id);
        let mut progress = receipts
            .read::<Progress>(&file)?
            .ok_or_else(|| crate::unit_transport::conflict("unknown host launch"))?;
        if receipts
            .read::<Value>(&format!("{}.json", ack.execution.request_id))?
            .is_some_and(|value| value.get("failed").is_some())
        {
            return Err(crate::unit_transport::conflict(
                "refused launch cannot acknowledge a return",
            ));
        }
        if progress.stopped.is_some() {
            return Err(crate::unit_transport::conflict(
                "stopped launch cannot acknowledge a return",
            ));
        }
        let pane = progress
            .pane
            .as_deref()
            .ok_or_else(|| crate::unit_transport::conflict("launch has no pane"))?;
        if std::env::var("HERDR_PANE_ID").ok().as_deref() != Some(pane) {
            return Err(crate::unit_transport::conflict(
                "terminal acknowledgement requires the worker pane",
            ));
        }
        if terminal(shell, pane)?.as_str() != progress.terminal.as_deref().unwrap_or_default() {
            return Err(crate::unit_transport::conflict("launch terminal changed"));
        }
        if let Some(saved) = &progress.terminal_receipt {
            if saved.ack != *ack {
                return Err(crate::unit_transport::conflict(
                    "terminal acknowledgement changed",
                ));
            }
            return Ok(saved.clone());
        }
        if !progress.attempted {
            return Err(crate::unit_transport::conflict("worker was not launched"));
        }
        let seed = progress
            .request
            .unit
            .as_ref()
            .ok_or_else(|| crate::unit_transport::conflict("not a seeded launch"))?;
        let agent = crate::probe::agent_list(shell)?
            .into_iter()
            .find(|a| a.pane == pane)
            .ok_or_else(|| crate::unit_transport::conflict("worker session unavailable"))?;
        let Action::Agent { name, .. } = &progress.request.action else {
            return Err(crate::unit_transport::conflict("not an agent launch"));
        };
        if agent.agent.as_deref() != Some("pi")
            || agent.terminal_id != progress.terminal
            || agent.name.as_deref() != Some(name.as_str())
        {
            return Err(crate::unit_transport::conflict(
                "worker session identity changed",
            ));
        }
        let path = agent
            .transcript
            .as_deref()
            .ok_or_else(|| crate::unit_transport::conflict("worker transcript unavailable"))?;
        let receipt = crate::unit_reply::capture(ledger, &progress.request, seed, ack, path)?;
        progress.terminal_receipt = Some(receipt.clone());
        receipts.write(&file, &progress)?;
        Ok(receipt)
    })
}

/// One execution's answer in a batched host read. Exactly one of `evidence`, `invalid`
/// and `failed` describes the outcome; all empty means the host has no evidence yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiptEntry {
    pub request_id: String,
    pub evidence: Option<crate::unit_reply::HostEvidence>,
    /// The host rejected this request's evidence; the text is dogsled's own rejection.
    pub invalid: Option<String>,
    /// The host could not read this request's evidence, such as a storage failure.
    pub failed: Option<String>,
}

/// The input of `host unit-receipts`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionsInput {
    pub executions: Vec<crate::unit_transport::ExecutionRef>,
}

/// Peer refresh reads every execution on this host in one call. Each execution is
/// read alone: one bad entry never fails its neighbours.
pub fn unit_receipts(
    ledger: &Ledger,
    shell: &dyn Shell,
    executions: &[crate::unit_transport::ExecutionRef],
) -> Vec<ReceiptEntry> {
    executions
        .iter()
        .map(|execution| {
            let mut entry = ReceiptEntry {
                request_id: execution.request_id.clone(),
                evidence: None,
                invalid: None,
                failed: None,
            };
            match unit_receipt(ledger, shell, execution) {
                Ok(evidence) => entry.evidence = evidence,
                Err(Error::Invalid(reason)) => entry.invalid = Some(reason),
                Err(error) => entry.failed = Some(error.to_string()),
            }
            entry
        })
        .collect()
}

/// Peer refresh reads immutable execution evidence, even after its pane closes.
/// A final native refusal repairs only this request's preparation, never its pane.
///
/// This host owns the proof that a confirmed worker stopped without a sealed reply. Panes
/// leave by `dogsled close`, by a crash, or by a tab closed by hand, so the proof is taken
/// here, on the next read after the pane left, under the launch lock that also orders
/// `terminal_ack`. One successful pane snapshot must show the saved pane missing or running
/// another terminal. A failed or unparsable snapshot is an error, never a stop.
pub fn unit_receipt(
    ledger: &Ledger,
    shell: &dyn Shell,
    execution: &crate::unit_transport::ExecutionRef,
) -> Result<Option<crate::unit_reply::HostEvidence>> {
    crate::jobs::validate_id(&execution.request_id)?;
    let evidence = ledger
        .child("launches")
        .with_lock(&execution.request_id, || {
            unit_receipt_locked(ledger, shell, execution)
        })?;
    if let Some(crate::unit_reply::HostEvidence::Refused(refusal)) = &evidence {
        crate::prepare::disposition::refused(ledger, shell, execution, &refusal.launch)?;
    }
    Ok(evidence)
}

pub(crate) fn unit_receipt_locked(
    ledger: &Ledger,
    shell: &dyn Shell,
    execution: &crate::unit_transport::ExecutionRef,
) -> Result<Option<crate::unit_reply::HostEvidence>> {
    use crate::unit_reply::{HostEvidence, Refusal, RefusalKind, Stopped, StoppedKind};
    let receipts = ledger.child("launches");
    (|| {
        let Some(progress) =
            receipts.read::<Progress>(&format!("{}.progress.json", execution.request_id))?
        else {
            if receipts
                .read::<Value>(&format!("{}.json", execution.request_id))?
                .is_some_and(|value| value.get("failed").is_some())
            {
                return Err(crate::unit_transport::conflict(
                    "host refusal has no launch provenance",
                ));
            }
            return Ok(None);
        };
        let seed = progress
            .request
            .unit
            .as_ref()
            .ok_or_else(|| crate::unit_transport::conflict("host seed missing"))?;
        crate::unit_transport::validate_seed(seed, &progress.request.id, &progress.request.owner)?;
        if seed.execution != *execution {
            return Err(crate::unit_transport::conflict("host execution mismatch"));
        }
        let preflight = progress.preflight_failure()?;
        let receipt_file = format!("{}.json", execution.request_id);
        let mut final_receipt = receipts.read::<Value>(&receipt_file)?;
        if final_receipt.is_none()
            && let Some(failed) = preflight
        {
            let value = json!({"failed": failed});
            receipts.write(&receipt_file, &value)?;
            final_receipt = Some(value);
        }
        if let Some(value) = final_receipt
            .as_ref()
            .filter(|value| value.get("failed").is_some())
        {
            let failed = value
                .get("failed")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .ok_or_else(|| crate::unit_transport::conflict("malformed host refusal"))?;
            if value.as_object().is_none_or(|object| object.len() != 1)
                || (preflight.is_none() && !progress.attempted)
                || progress.confirmed
                || progress.terminal_receipt.is_some()
                || progress.error.as_deref() != Some(failed)
                || (preflight.is_none() && progress.pane.as_deref().is_none_or(str::is_empty))
                || (preflight.is_none() && progress.terminal.as_deref().is_none_or(str::is_empty))
                || (preflight.is_none()
                    && receipts
                        .read::<crate::unit_transport::Seed>(&format!(
                            "{}.unit.json",
                            execution.request_id
                        ))?
                        .as_ref()
                        != Some(seed))
            {
                return Err(crate::unit_transport::conflict(
                    "host refusal lacks consistent launch provenance",
                ));
            }
            // Do not validate the label against today's naming algorithm: old committed
            // actions and their refusal remain immutable evidence, not a new launch.
            let evidence = Refusal {
                kind: RefusalKind::Refused,
                launch: progress.request.clone(),
                failed: failed.to_owned(),
            };
            return Ok(Some(HostEvidence::Refused(Box::new(evidence))));
        }
        if let Some(receipt) = &progress.terminal_receipt {
            return Ok(Some(HostEvidence::Returned(Box::new(receipt.clone()))));
        }
        // Idempotent: a saved stop is returned again without another probe.
        if let Some(stopped) = &progress.stopped {
            return Ok(Some(HostEvidence::Stopped(Box::new(stopped.clone()))));
        }
        if final_receipt.is_some_and(|value| value.get("failed").is_some())
            || !progress.attempted
            || !progress.confirmed
        {
            return Ok(None);
        }
        let (panes, terminals) = pane_proof(shell)?;
        if !recovery::lost_pane(&progress, &panes, &terminals) {
            return Ok(None);
        }
        let stopped = Stopped {
            kind: StoppedKind::Stopped,
            launch: progress.request.clone(),
            reason: STOPPED_REASON.into(),
        };
        let mut progress = progress;
        progress.stopped = Some(stopped.clone());
        finish_progress(ledger, &mut progress)?;
        Ok(Some(HostEvidence::Stopped(Box::new(stopped))))
    })()
}

/// One pane snapshot for the stop proof. Output that does not parse is a parse error from
/// Herdr, kept apart from local storage errors; a failed or timed-out command passes through.
fn pane_proof(
    shell: &dyn Shell,
) -> Result<(
    Vec<crate::probe::Pane>,
    std::collections::BTreeMap<String, String>,
)> {
    crate::cleanup::pane_snapshot(shell).map_err(|error| match error {
        Error::Json(_) | Error::Invalid(_) => Error::Parse {
            origin: "herdr pane list".into(),
            detail: error.to_string(),
        },
        other => other,
    })
}

fn idle(shell: &dyn Shell, pane: &str) -> Result<()> {
    let info = crate::probe::process_info(shell, pane)?
        .ok_or_else(|| Error::Invalid(format!("pane {pane} disappeared before startup")))?;
    let shell_pid = info["shell_pid"]
        .as_u64()
        .ok_or_else(|| Error::Invalid(format!("{pane}: no shell process identity")))?;
    let foreground = info["foreground_processes"]
        .as_array()
        .ok_or_else(|| Error::Invalid(format!("{pane}: no foreground process information")))?;
    if foreground.is_empty()
        || foreground
            .iter()
            .any(|p| p["pid"].as_u64() != Some(shell_pid))
    {
        return Err(Error::Invalid(format!(
            "{pane} is busy; refusing to start another process"
        )));
    }
    Ok(())
}

fn same_terminal(shell: &dyn Shell, pane: &str, expected: &str) -> Result<()> {
    let current = terminal(shell, pane)?;
    if current != expected {
        return Err(Error::Invalid(format!("{pane}: launch terminal changed")));
    }
    Ok(())
}

fn terminal(shell: &dyn Shell, pane: &str) -> Result<String> {
    let out = shell.run_ok("herdr", &["pane".into(), "list".into()])?;
    let value: Value = serde_json::from_str(&out.stdout)?;
    value
        .pointer("/result/panes")
        .and_then(Value::as_array)
        .and_then(|panes| panes.iter().find(|p| p["pane_id"] == pane))
        .and_then(|p| p["terminal_id"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| Error::Invalid(format!("{pane}: cannot establish launch terminal identity")))
}

#[cfg(test)]
mod receipt_tests {
    use super::*;
    use crate::unit_transport::ExecutionRef;

    fn execution(request_id: &str) -> ExecutionRef {
        ExecutionRef {
            executor: "dogsled".into(),
            owner_id: "owner".into(),
            request_id: request_id.into(),
        }
    }

    /// Competing launch records contradict each other. They prove neither a launch nor its
    /// absence, so they get their own name and never read as "nothing was launched".
    #[test]
    fn competing_launch_records_are_ambiguous_not_missing() {
        let dir = std::env::temp_dir().join(crate::ledger::new_id());
        let ledger = Ledger::new(dir.clone());
        let receipts = ledger.child("launches");
        for id in ["first", "second"] {
            receipts
                .write(
                    &format!("{id}.progress.json"),
                    &json!({
                        "request":{"id":id,"action":{"type":"agent","kind":"pi","name":"worker",
                            "message":"go"},"host":"local","cwd":"/checkout","pane":"w2:p2",
                            "terminal":"term_shell","owner":"w1:p1"},
                        "pane":"w2:p2","terminal":"term_shell","attempted":true,"confirmed":true,
                        "result":{},"error":null
                    }),
                )
                .unwrap();
        }
        assert_eq!(
            progress(&ledger, "w2:p2").unwrap(),
            LaunchProgress::Ambiguous
        );
        // One record names the pane: the launch is proved.
        std::fs::remove_file(receipts.dir.join("second.progress.json")).unwrap();
        assert_eq!(
            progress(&ledger, "w2:p2").unwrap(),
            LaunchProgress::Confirmed
        );
        // No record names the pane: no launch is proved.
        std::fs::remove_file(receipts.dir.join("first.progress.json")).unwrap();
        assert_eq!(progress(&ledger, "w2:p2").unwrap(), LaunchProgress::None);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn unit_receipts_answers_each_execution_without_failing_neighbours() {
        let dir = std::env::temp_dir().join(crate::ledger::new_id());
        let ledger = Ledger::new(dir.clone());
        // A progress path that is a directory cannot be read: a storage failure.
        std::fs::create_dir_all(dir.join("launches/r-broken.progress.json")).unwrap();
        let executions = [
            execution("r-first"),
            execution("../escape"),
            execution("r-broken"),
            execution("r-last"),
        ];
        let entries = unit_receipts(&ledger, &crate::shell::Fake::new(), &executions);
        let ids: Vec<_> = entries.iter().map(|e| e.request_id.as_str()).collect();
        assert_eq!(ids, ["r-first", "../escape", "r-broken", "r-last"]);
        for quiet in [&entries[0], &entries[3]] {
            assert_eq!(
                (&quiet.evidence, &quiet.invalid, &quiet.failed),
                (&None, &None, &None)
            );
        }
        assert_eq!(entries[1].invalid.as_deref(), Some("invalid job id"));
        assert!(entries[1].evidence.is_none() && entries[1].failed.is_none());
        assert!(entries[2].failed.is_some(), "{:?}", entries[2]);
        assert!(entries[2].evidence.is_none() && entries[2].invalid.is_none());
        let wire = serde_json::to_value(&entries[1]).unwrap();
        assert_eq!(
            wire,
            json!({"requestId":"../escape","evidence":null,"invalid":"invalid job id","failed":null})
        );
        assert_eq!(
            serde_json::from_value::<ReceiptEntry>(wire).unwrap(),
            entries[1]
        );
        std::fs::remove_dir_all(dir).ok();
    }
}
