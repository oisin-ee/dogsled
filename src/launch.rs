//! Host-owned agent launches. Persist the pane before starting, and reconcile that
//! exact terminal after an interrupted response; never allocate a second writer.

use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::shell::Shell;
use crate::work::{self, Action, Launch};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

mod recovery;
pub use recovery::{PaneLaunch, Survey, close_unfinished, final_pane};
#[cfg(test)]
mod pane_tests;

fn with_pane<T>(ledger: &Ledger, pane: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let key = crate::unit_transport::digest_hex(pane.as_bytes());
    ledger.child("launch-panes").with_lock(&key, f)
}

#[derive(Debug, Serialize, Deserialize)]
struct Progress {
    request: Launch,
    pane: Option<String>,
    terminal: Option<String>,
    attempted: bool,
    confirmed: bool,
    result: Value,
    error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal_receipt: Option<crate::unit_reply::Receipt>,
}

/// Herdr's codes for an `agent start` it refused before typing into the pane. Any other
/// failure may come after the agent began, so its outcome stays unknown.
const REFUSALS: [&str; 7] = [
    "invalid_agent_name",
    "unsupported_agent_kind",
    "invalid_agent_argument",
    "invalid_agent_timeout",
    "agent_pane_not_found",
    "agent_pane_busy",
    "agent_name_taken",
];

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

pub fn agent(ledger: &Ledger, shell: &dyn Shell, request: &Launch) -> Result<Value> {
    let receipts = ledger.child("launches");
    receipts.with_lock(&request.id, || {
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
        let mut progress = match saved {
            Some(progress) => progress,
            None => allocate(&receipts, shell, request, &progress_file)?,
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
                    return Err(refused(&receipts, shell, &file, owned, error)?);
                }
                Ok(())
            })?;
            progress.confirmed = true;
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
    receipts: &Ledger,
    shell: &dyn Shell,
    file: &str,
    pane: Option<(&str, &str)>,
    error: Error,
) -> Result<Error> {
    let Error::Command { stderr, .. } = &error else {
        return Ok(error);
    };
    let code = serde_json::from_str::<Value>(stderr)
        .ok()
        .and_then(|v| v.pointer("/error/code")?.as_str().map(str::to_owned));
    if !code.is_some_and(|code| REFUSALS.contains(&code.as_str())) {
        return Ok(error);
    }
    let failed = error.to_string();
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
    receipts: &Ledger,
    shell: &dyn Shell,
    request: &Launch,
    file: &str,
) -> Result<Progress> {
    let Action::Agent {
        kind,
        name,
        message,
        profile,
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
    // Refuse a launch line over Herdr's limit here, before any pane exists.
    work::agent_args(
        kind,
        name,
        message,
        &seed_args(request, receipts),
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
    if let Some(profile) = &profile {
        result["profile"] = json!(profile.name);
        for (key, value) in [("model", &profile.model), ("role", &profile.role)] {
            if let Some(value) = value {
                result[key] = json!(value);
            }
        }
    }
    let mut progress = Progress {
        request: request.clone(),
        pane: None,
        terminal: request.terminal.clone(),
        attempted: false,
        confirmed: false,
        result,
        error: None,
        terminal_receipt: None,
    };
    receipts.write(file, &progress)?;
    let (pane, terminal) = work::pane(shell, request)?;
    progress.pane = Some(pane.clone());
    progress.result["pane"] = json!(pane);
    progress.terminal = terminal;
    receipts.write(file, &progress)?;
    Ok(progress)
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
        return Err(Error::Launch(format!(
            "launch {}: no matching agent in {pane}; startup outcome is unknown; inspect before resubmitting; {}",
            request.id,
            progress
                .error
                .as_deref()
                .unwrap_or("startup response was interrupted")
        )));
    }
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
        message,
        &seed_flags,
        &files,
        profile.as_ref(),
    )?);
    progress.attempted = true;
    receipts.write(file, progress)?;
    if let Err(error) = shell.run_setup_ok("herdr", &args) {
        progress.error = Some(error.to_string());
        receipts.write(file, progress)?;
        return Err(error);
    }
    Ok(())
}

fn validate_seed<'a>(
    request: &Launch,
    seed: &'a crate::unit_transport::Seed,
    profile: Option<&crate::profiles::Profile>,
) -> Result<&'a str> {
    let expected = seed.validate(&request.id, &request.owner)?;
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

/// Peer refresh reads immutable execution evidence, even after its pane closes.
/// A final native refusal repairs only this request's preparation, never its pane.
pub fn unit_receipt(
    ledger: &Ledger,
    execution: &crate::unit_transport::ExecutionRef,
) -> Result<Option<crate::unit_reply::HostEvidence>> {
    use crate::unit_reply::{HostEvidence, Refusal, RefusalKind};
    crate::jobs::validate_id(&execution.request_id)?;
    let receipts = ledger.child("launches");
    receipts.with_lock(&execution.request_id, || {
        let Some(progress) =
            receipts.read::<Progress>(&format!("{}.progress.json", execution.request_id))?
        else {
            return Ok(None);
        };
        let seed = progress
            .request
            .unit
            .as_ref()
            .ok_or_else(|| crate::unit_transport::conflict("host seed missing"))?;
        seed.validate(&progress.request.id, &progress.request.owner)?;
        if seed.execution != *execution {
            return Err(crate::unit_transport::conflict("host execution mismatch"));
        }
        let final_receipt = receipts.read::<Value>(&format!("{}.json", execution.request_id))?;
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
                || !progress.attempted
                || progress.confirmed
                || progress.terminal_receipt.is_some()
                || progress.error.as_deref() != Some(failed)
                || progress.pane.as_deref().is_none_or(str::is_empty)
                || progress.terminal.as_deref().is_none_or(str::is_empty)
                || receipts
                    .read::<crate::unit_transport::Seed>(&format!(
                        "{}.unit.json",
                        execution.request_id
                    ))?
                    .as_ref()
                    != Some(seed)
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
            crate::prepare::used(ledger, &progress.request.cwd, Some(&execution.request_id))?;
            crate::unit_request::crash_point("refusal-hold-released");
            return Ok(Some(HostEvidence::Refused(Box::new(evidence))));
        }
        Ok(progress
            .terminal_receipt
            .map(|receipt| HostEvidence::Returned(Box::new(receipt))))
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
