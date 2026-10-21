//! Final launch shells share recovery and cleanup ownership checks. Receipts stay unchanged.

use super::{Progress, Receipt, idle, terminal, with_pane};
use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::shell::Shell;

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;

pub(super) fn candidates(receipts: &Ledger, pane: Option<&str>) -> Result<Vec<(String, Progress)>> {
    let entries = match std::fs::read_dir(&receipts.dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut found = Vec::new();
    for entry in entries {
        let name = entry?.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".progress.json") else {
            continue;
        };
        let Some(progress) = receipts.read::<Progress>(&name)? else {
            continue;
        };
        if pane.is_none_or(|pane| progress.pane.as_deref() == Some(pane)) {
            crate::jobs::validate_id(id)?;
            found.push((id.to_owned(), progress));
        }
    }
    Ok(found)
}

fn unfinished(receipts: &Ledger, id: &str, progress: &Progress) -> Result<bool> {
    let receipt = receipts.read::<Receipt>(&format!("{id}.json"))?;
    Ok(matches!(receipt, Some(Receipt::Refused { .. }))
        || (receipt.is_none() && !progress.confirmed && progress.terminal_receipt.is_none()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneLaunch {
    Unrecorded,
    Started,
    Final,
    Unresolved,
}

/// Final launches must still own the idle shell. Successful records prevent recordless
/// age-based cleanup; unresolved ownership also prevents closing by another shell rule.
pub fn final_pane(ledger: &Ledger, shell: &dyn Shell, pane: &str) -> Result<PaneLaunch> {
    let receipts = ledger.child("launches");
    let found = candidates(&receipts, Some(pane))?;
    if found.is_empty() {
        return Ok(PaneLaunch::Unrecorded);
    }
    let [(id, progress)] = found.as_slice() else {
        return Ok(PaneLaunch::Unresolved);
    };
    if matches!(
        receipts.read::<Receipt>(&format!("{id}.json"))?,
        Some(Receipt::Started(_))
    ) {
        return Ok(PaneLaunch::Started);
    }
    Ok(
        if unfinished(&receipts, id, progress)?
            && progress.request.id == *id
            && verify_items(ledger, pane, progress).is_ok()
            && verify_shell(shell, pane, progress).is_ok()
        {
            PaneLaunch::Final
        } else {
            PaneLaunch::Unresolved
        },
    )
}

/// Same-request work is serialized by its receipt lock; different Dogsled requests that
/// target this pane share the launch-panes lock. Never acquire another request lock inside
/// either lock. External terminal mutations still require fresh identity and busy checks.
/// No receipt, preparation, branch or checkout is changed by this operation.
pub fn close_unfinished(ledger: &Ledger, shell: &dyn Shell, pane: &str) -> Result<bool> {
    let receipts = ledger.child("launches");
    let found = candidates(&receipts, Some(pane))?;
    let mut selected = None;
    for (id, progress) in &found {
        if unfinished(&receipts, id, progress)? {
            selected = Some((id, progress));
        }
    }
    let Some((id, original)) = selected else {
        for (id, _) in &found {
            if receipts.read::<Receipt>(&format!("{id}.json"))?.is_none() {
                return Err(Error::Invalid(format!(
                    "{pane}: unresolved launch is not eligible for recovery; nothing was closed"
                )));
            }
        }
        return Ok(false);
    };
    if found.len() != 1 {
        return Err(Error::Invalid(format!(
            "{pane}: competing launch records; nothing was closed"
        )));
    }
    close_selected(ledger, shell, pane, id, original)
}

fn close_selected(
    ledger: &Ledger,
    shell: &dyn Shell,
    pane: &str,
    id: &str,
    original: &Progress,
) -> Result<bool> {
    let receipts = ledger.child("launches");
    receipts.with_lock(id, || {
        with_pane(ledger, pane, || {
            let found = candidates(&receipts, Some(pane))?;
            let [(current_id, progress)] = found.as_slice() else {
                return Err(Error::Invalid(format!("{pane}: launch ownership changed; nothing was closed")));
            };
            if current_id != id
                || progress.request != original.request
                || progress.request.id != id
                || progress.terminal != original.terminal
                || !unfinished(&receipts, id, progress)?
            {
                return Err(Error::Invalid(format!("{pane}: launch changed; nothing was closed")));
            }
            verify_items(ledger, pane, progress)?;
            verify_shell(shell, pane, progress)?;
            shell.run_ok("herdr", &["pane".into(), "close".into(), pane.into()])?;
            eprintln!("{pane}: closed final launch pane; checkout retained at {}; launch evidence unchanged", progress.request.cwd);
            Ok(true)
        })
    })
}

fn verify_items(ledger: &Ledger, pane: &str, progress: &Progress) -> Result<()> {
    if crate::items::list(ledger)?.iter().any(|item| {
        item.closed_ms.is_none()
            && item.host == crate::hosts::LOCAL
            && item.pane == pane
            && item.id != progress.request.id
    }) {
        return Err(Error::Invalid(format!(
            "{pane}: another item owns the pane; nothing was closed"
        )));
    }
    Ok(())
}

fn verify_shell(shell: &dyn Shell, pane: &str, progress: &Progress) -> Result<()> {
    let expected = progress
        .terminal
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Error::Invalid(format!("{pane}: no saved terminal identity")))?;
    if terminal(shell, pane)? != expected {
        return Err(Error::Invalid(format!("{pane}: launch terminal changed")));
    }
    let panes = crate::cleanup::current_panes(shell)?;
    let current = panes
        .iter()
        .find(|p| p.pane_id == pane)
        .ok_or_else(|| Error::Invalid(format!("{pane}: pane disappeared")))?;
    if current.focused
        || std::env::var("HERDR_PANE_ID").ok().as_deref() == Some(pane)
        || current.agent.is_some()
        || !current
            .cwd
            .as_deref()
            .is_some_and(|cwd| crate::worktree::same(cwd, &progress.request.cwd))
        || crate::probe::agent_list(shell)?
            .iter()
            .any(|agent| agent.pane == pane)
    {
        return Err(Error::Invalid(format!(
            "{pane}: no longer the launch's shell"
        )));
    }
    idle(shell, pane)?;
    let info = crate::probe::process_info(shell, pane)?
        .ok_or_else(|| Error::Invalid(format!("{pane}: shell evidence disappeared")))?;
    let shell_pid = info["shell_pid"].as_u64();
    if shell_pid.is_none()
        || info["foreground_process_group_id"].as_u64() != shell_pid
        || info["foreground_processes"]
            .as_array()
            .is_none_or(|processes| {
                processes.len() != 1 || processes[0]["pid"].as_u64() != shell_pid
            })
    {
        return Err(Error::Invalid(format!(
            "{pane}: cannot prove an idle shell foreground"
        )));
    }
    if terminal(shell, pane)? != expected {
        return Err(Error::Invalid(format!("{pane}: launch terminal changed")));
    }
    Ok(())
}
