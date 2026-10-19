//! Host-owned agent launches. Persist the pane before starting, and reconcile that
//! exact terminal after an interrupted response; never allocate a second writer.

use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::shell::Shell;
use crate::work::{self, Action, Launch};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Serialize, Deserialize)]
struct Progress {
    request: Launch,
    pane: Option<String>,
    terminal: Option<String>,
    attempted: bool,
    confirmed: bool,
    result: Value,
    error: Option<String>,
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
        if let Some(result) = receipts.read::<Value>(&file)? {
            return Ok(result);
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
            start_or_reconcile(ledger, shell, &mut progress, &pane, &progress_file)?;
            progress.confirmed = true;
            receipts.write(&progress_file, &progress)?;
        }
        // Parent registration can fail after startup; the confirmed result survives it.
        crate::watchdog::register_parent(ledger, shell, &pane)?;
        receipts.write(&file, &progress.result)?;
        Ok(progress.result)
    })
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
    // Validate profile/argv before creating a pane. An allocating state without a
    // pane is deliberately not retried after a crash: its ownership is uncertain.
    let profile = profile.as_deref().map(crate::profiles::load).transpose()?;
    work::agent_args(kind, name, message, "task", profile.as_ref())?;
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
        terminal: None,
        attempted: false,
        confirmed: false,
        result,
        error: None,
    };
    receipts.write(file, &progress)?;
    let pane = work::pane(shell, request)?;
    progress.pane = Some(pane.clone());
    progress.result["pane"] = json!(pane);
    receipts.write(file, &progress)?;
    progress.terminal = Some(terminal(
        shell,
        progress
            .pane
            .as_deref()
            .ok_or_else(|| Error::Invalid("missing allocated pane".into()))?,
    )?);
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
    idle(shell, pane)?;
    let profile = profile.as_deref().map(crate::profiles::load).transpose()?;
    let task = work::task_file(ledger, &request.id, message)?;
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
        &task,
        profile.as_ref(),
    )?);
    progress.attempted = true;
    let receipts = ledger.child("launches");
    receipts.write(file, progress)?;
    if let Err(error) = shell.run_setup_ok("herdr", &args) {
        progress.error = Some(error.to_string());
        receipts.write(file, progress)?;
        return Err(error);
    }
    Ok(())
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
    if foreground
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
