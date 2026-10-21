//! A durable, terminal handoff owned by the child's host. Idle without `done` is not consent.
//! Identity is launch/item + pane + harness session, never the restorable Herdr terminal.

use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::probe::Agent;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const FILE: &str = "done.json";

/// One line because Herdr refuses newlines in an argument.
pub const CONTRACT: &str = "Dogsled started you as a child agent. When your whole task is \
complete, call the dogsled `done` tool (or run `dogsled done`) once, then end your turn with \
your final reply. Do not call `done` when you ask a question, wait for input, or own open \
children. Commit your changes first. `done` hands your work back: Dogsled closes your pane \
and agent worktree when this turn ends, keeps your branch, and leaves landing to your owner. \
After that turn, messages cannot reopen you; more work needs a new child.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branch {
    pub repo: String,
    pub name: String,
    pub ahead: u64,
    pub landed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    pub pane: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<u64>,
    /// Legacy completion number, also used for reply deduplication. Zero means counters reset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed: Option<u64>,
    pub marked_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<Branch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<crate::items::Reply>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_ms: Option<u64>,
}

type Marks = BTreeMap<String, Mark>;

fn waiting(agent: &Agent) -> bool {
    matches!(agent.state.as_str(), "idle" | "done")
}

fn identity(ledger: &Ledger, pane: &str) -> Result<Option<String>> {
    if let Some(id) = crate::launch::identity(ledger, pane)? {
        return Ok(Some(id));
    }
    Ok(crate::items::list(ledger)?
        .into_iter()
        .find(|i| i.pane == pane && i.host == crate::hosts::LOCAL && i.closed_ms.is_none())
        .map(|i| i.id))
}

fn key(mark: &Mark) -> String {
    // JSON tuples avoid ambiguous concatenation, and the key is never a file path.
    serde_json::json!([mark.item, mark.pane, mark.session]).to_string()
}

fn matches(mark: &Mark, agent: &Agent) -> bool {
    mark.pane == agent.pane && mark.session == agent.agent_session
}

fn owners(ledger: &Ledger) -> Result<BTreeMap<String, Vec<String>>> {
    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for item in crate::items::list(ledger)?
        .into_iter()
        .filter(|i| i.closed_ms.is_none())
    {
        owners
            .entry(item.owner_pane)
            .or_default()
            .push(item.name.unwrap_or(item.pane));
    }
    for ticket in crate::work::queue(ledger)?
        .into_iter()
        .filter(|t| matches!(t.state.as_str(), "pending" | "admitting"))
    {
        owners
            .entry(ticket.request.owner)
            .or_default()
            .push(format!("queued {}", ticket.id));
    }
    Ok(owners)
}

/// Preserve unmatched legacy records until their pane is observable. Migration never deletes
/// a mark, even if its old terminal disappeared before the first read by this version.
fn migrate(ledger: &Ledger, marks: Marks, agents: &[Agent]) -> Result<Marks> {
    let mut migrated = Marks::new();
    for (old_key, mut mark) in marks {
        if old_key != key(&mark) {
            if let Some(agent) = agents.iter().find(|a| a.pane == mark.pane) {
                mark.session.clone_from(&agent.agent_session);
                mark.name.clone_from(&agent.name);
                mark.item = identity(ledger, &mark.pane)?;
            } else {
                migrated.insert(old_key, mark);
                continue;
            }
        }
        let new_key = key(&mark);
        if migrated.contains_key(&new_key) {
            migrated.insert(old_key, mark);
        } else {
            migrated.insert(new_key, mark);
        }
    }
    Ok(migrated)
}

/// A successful mark was made while working. Its first idle/done observation ends that turn,
/// even after restore with no `completion_seq`. Working/blocked/unknown never authorize close.
/// Once sealed it is terminal: a later turn cannot revoke it, but busy panes still never close.
fn seal(mark: &mut Mark, agent: &Agent) {
    if mark.sealed.is_none() && waiting(agent) {
        mark.sealed = Some(agent.completion_seq.unwrap_or(0));
    }
}

pub fn observe(ledger: &Ledger, agents: &mut [Agent]) -> Result<()> {
    ledger.with_lock("done", || {
        let before: Marks = ledger.read(FILE)?.unwrap_or_default();
        let mut marks = migrate(ledger, before.clone(), agents)?;
        let owners = owners(ledger)?;
        for agent in agents {
            let mut done = false;
            for mark in marks.values_mut().filter(|m| matches(m, agent)) {
                seal(mark, agent);
                done |=
                    mark.sealed.is_some() && waiting(agent) && !owners.contains_key(&agent.pane);
            }
            agent.done = done;
        }
        if before != marks {
            ledger.write(FILE, &marks)?;
        }
        Ok(())
    })
}

pub fn finished(ledger: &Ledger, agents: &[Agent]) -> Result<BTreeSet<String>> {
    let mut agents = agents.to_vec();
    observe(ledger, &mut agents)?;
    Ok(agents
        .into_iter()
        .filter(|a| a.done)
        .map(|a| a.pane)
        .collect())
}

/// Validate while the caller can still repair its work. No mark is written on a refusal.
pub fn mark(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    pane: &str,
    now: u64,
) -> Result<Mark> {
    let agent = crate::inbox::local_agent(shell, pane)?;
    if agent.state != "working" {
        return Err(Error::Invalid(format!(
            "{pane} is {}; call done during the turn that completes your task",
            agent.state
        )));
    }
    if let Some(open) = owners(ledger)?.get(pane) {
        return Err(Error::Invalid(format!(
            "{pane} still owns open work: {}. Finish or close each child and cancel queued work before calling done",
            open.join(", ")
        )));
    }
    let session = agent.agent_session.clone().ok_or_else(|| {
        Error::Invalid(format!(
            "{pane}: Herdr reports no agent session; cannot record a durable done"
        ))
    })?;
    let item = identity(ledger, pane)?;
    let branch = preflight(shell, &agent)?;
    let mark = Mark {
        pane: pane.into(),
        item,
        session: Some(session),
        name: agent.name,
        after: agent.state_change_seq,
        sealed: None,
        marked_ms: now,
        branch,
        reply: None,
        closed_ms: None,
    };
    ledger.with_lock("done", || {
        let mut marks: Marks = ledger.read(FILE)?.unwrap_or_default();
        if let Some(existing) = marks.get(&key(&mark))
            && existing.sealed.is_some()
        {
            return Err(Error::Invalid(format!(
                "{pane} finished; start a new child for more work"
            )));
        }
        marks.insert(key(&mark), mark.clone());
        ledger.write(FILE, &marks)
    })?;
    Ok(mark)
}

fn preflight(shell: &dyn crate::shell::Shell, agent: &Agent) -> Result<Option<Branch>> {
    let panes = if agent.cwd.is_none() {
        crate::cleanup::current_panes(shell)?
    } else {
        Vec::new()
    };
    let cwd = agent.cwd.as_deref().or_else(|| {
        panes
            .iter()
            .find(|p| p.pane_id == agent.pane)
            .and_then(|p| p.cwd.as_deref())
    });
    let Some(cwd) = cwd else { return Ok(None) };
    let Some(tree) = crate::worktree::linked(shell, cwd)? else {
        return Ok(None);
    };
    if crate::worktree::lock_reason(shell, &tree.checkout, &tree.path)?.as_deref()
        != Some(crate::worktree::AGENT_LOCK)
    {
        return Ok(None);
    }
    let dirty = crate::loss::dirty_paths(shell, &tree.path, false)?;
    if !dirty.is_empty() {
        return Err(Error::Invalid(format!(
            "{} has uncommitted or untracked changes: {}. Commit or preserve these changes before calling done",
            tree.path,
            dirty.join(", ")
        )));
    }
    let repo = tree.checkout.display().to_string();
    let evidence = crate::loss::Evidence::keeping_branch(&repo, &tree.branch);
    let head = shell.run_ok(
        "git",
        &[
            "-C".into(),
            tree.path.clone(),
            "rev-parse".into(),
            "HEAD".into(),
        ],
    )?;
    let loss = evidence.assess(shell, head.stdout.trim(), Some(&tree.path))?;
    if !loss.loses_nothing() {
        return Err(Error::Invalid(format!(
            "{}: {}. Preserve the work before calling done",
            tree.path,
            loss.detail()
        )));
    }
    let mut branch = Branch {
        repo,
        name: tree.branch,
        ahead: 0,
        landed: false,
    };
    refresh_branch(shell, &mut branch)?;
    Ok(Some(branch))
}

fn refresh_branch(shell: &dyn crate::shell::Shell, branch: &mut Branch) -> Result<()> {
    let tip = format!("refs/heads/{}", branch.name);
    branch.landed = crate::loss::Evidence::default_only(&branch.repo)
        .assess(shell, &tip, None)?
        .landed();
    branch.ahead = if branch.landed {
        0
    } else {
        let base = crate::integration::base(shell, &branch.repo)?;
        shell
            .run_ok(
                "git",
                &[
                    "-C".into(),
                    branch.repo.clone(),
                    "rev-list".into(),
                    "--count".into(),
                    format!("{base}..{tip}"),
                ],
            )?
            .stdout
            .trim()
            .parse()
            .map_err(|_| Error::Invalid(format!("{}: invalid branch commit count", branch.repo)))?
    };
    Ok(())
}

pub fn list(ledger: &Ledger) -> Result<Vec<Mark>> {
    Ok(ledger
        .read::<Marks>(FILE)?
        .unwrap_or_default()
        .into_values()
        .collect())
}

fn save(ledger: &Ledger, mark: &Mark) -> Result<()> {
    ledger.with_lock("done", || {
        let mut marks: Marks = ledger.read(FILE)?.unwrap_or_default();
        // A message during the same turn may have removed it. Never resurrect it.
        if let std::collections::btree_map::Entry::Occupied(mut entry) = marks.entry(key(mark)) {
            entry.insert(mark.clone());
            ledger.write(FILE, &marks)?;
        }
        Ok(())
    })
}

/// Serialize inbox acceptance with sealing. Only new work accepted while the marked turn is
/// still working cancels its mark. After idle/done (or an earlier seal), reject all messages,
/// even if Herdr subsequently reports working. No post-turn claim can reopen a finished child.
pub fn input<T>(
    ledger: &Ledger,
    agent: &Agent,
    accept: impl FnOnce() -> Result<(T, bool)>,
) -> Result<T> {
    ledger.with_lock("done", || {
        let before: Marks = ledger.read(FILE)?.unwrap_or_default();
        let mut marks = migrate(ledger, before.clone(), std::slice::from_ref(agent))?;
        for mark in marks.values_mut().filter(|m| matches(m, agent)) {
            seal(mark, agent);
            if mark.sealed.is_some() {
                let name = mark
                    .name
                    .as_deref()
                    .or(agent.name.as_deref())
                    .unwrap_or(&agent.pane)
                    .to_owned();
                ledger.write(FILE, &marks)?;
                return Err(Error::Invalid(format!(
                    "{name} finished; start a new child for more work"
                )));
            }
        }
        let (result, received) = accept()?;
        if received && agent.state == "working" {
            marks.retain(|_, m| !matches(m, agent));
        }
        if marks != before {
            ledger.write(FILE, &marks)?;
        }
        Ok(result)
    })
}

/// Child-host action lane, independent of fleet discovery, owner availability and sweeps.
/// Keep the result durably before closing; the owner's children lane imports it afterwards.
pub fn handoff(ledger: &Ledger, shell: &dyn crate::shell::Shell, now: u64) -> Result<()> {
    ledger.with_lock("handoff", || handoff_locked(ledger, shell, now))
}

fn handoff_locked(ledger: &Ledger, shell: &dyn crate::shell::Shell, now: u64) -> Result<()> {
    if list(ledger)?.is_empty() {
        return Ok(());
    }
    let mut agents = crate::probe::agent_list(shell)?;
    observe(ledger, &mut agents)?;
    for mut mark in list(ledger)? {
        if let Some(branch) = &mut mark.branch {
            if let Err(error) = refresh_branch(shell, branch) {
                eprintln!("dogsled handoff: branch {}: {error}", branch.name);
            }
            save(ledger, &mark)?;
        }
        if mark.closed_ms.is_some() {
            continue;
        }
        let Some(agent) = agents.iter().find(|a| matches(&mark, a) && a.done) else {
            continue;
        };
        if mark.reply.is_none() {
            let Some(reply) = crate::sweep::local_reply(shell, ledger, &mark.pane)? else {
                continue;
            };
            mark.reply = Some(crate::items::Reply {
                seq: reply.seq,
                result_id: None,
                text: reply.text,
                captured_ms: now,
                reported_ms: None,
                done: true,
            });
            save(ledger, &mark)?;
        }
        if let (Some(id), Some(reply)) = (&mark.item, &mark.reply)
            && !crate::items::list(ledger)?.iter().any(|i| {
                i.id == *id
                    && i.reply
                        .as_ref()
                        .is_some_and(|r| r.result_id.is_some() || (r.seq == reply.seq && r.done))
            })
        {
            crate::items::set_reply(ledger, id, reply.clone())?;
        }
        if mark.branch.is_none() {
            match preflight(shell, agent) {
                Ok(branch) => {
                    mark.branch = branch;
                    save(ledger, &mark)?;
                }
                Err(error) => {
                    eprintln!("dogsled handoff: {}: {error}", mark.pane);
                    continue;
                }
            }
        }
        match crate::close::local(
            shell,
            ledger,
            &mark.pane,
            &crate::close::Caller::from_env(),
            crate::close::Guard::Writer,
        ) {
            Ok(_) => {
                mark.closed_ms = Some(now);
                save(ledger, &mark)?;
                crate::items::close(ledger, crate::hosts::LOCAL, &mark.pane, now)?;
                eprintln!("dogsled handoff: closed {}", mark.pane);
            }
            Err(error) => eprintln!("dogsled handoff: {}: {error}", mark.pane),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
