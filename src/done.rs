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
    /// Settled: integrated into the default branch, or no local/origin branch remains.
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

/// The pane's unit evidence. An error stays an error: it never reads as a plain pane.
type Evidence = std::result::Result<crate::launch::Execution, String>;

fn evidence(ledger: &Ledger, mark: &Mark, agent: &Agent) -> Evidence {
    crate::launch::execution(ledger, mark.item.as_deref(), agent).map_err(|e| e.to_string())
}

/// `done` can end a plain pane's work. A unit execution ends only when its worker
/// acknowledged the sealed terminal boundary; until then its owner can still continue it.
fn closable(evidence: &Evidence) -> bool {
    matches!(
        evidence,
        Ok(crate::launch::Execution::Plain | crate::launch::Execution::Returned)
    )
}

/// A successful mark was made while working. Its first idle/done observation ends that turn,
/// even after restore with no `completion_seq`. Working/blocked/unknown never authorize close.
/// Once sealed it is terminal: a later turn cannot revoke it, but busy panes still never close.
/// An unreturned unit is not sealed. A seal written before this rule stays, and `terminal`
/// decides.
fn seal(mark: &mut Mark, agent: &Agent, evidence: &Evidence) {
    if mark.sealed.is_none() && waiting(agent) && closable(evidence) {
        mark.sealed = Some(agent.completion_seq.unwrap_or(0));
    }
}

fn terminal(mark: &Mark, evidence: &Evidence) -> bool {
    mark.sealed.is_some() && closable(evidence)
}

/// Who offers input to a marked pane.
#[derive(Debug, Clone, Copy)]
pub enum Input<'a> {
    /// The pane's own inbox reader claims records.
    Claim,
    /// A sender puts a record; `from` is the whole sender, because a pane ID is unique only
    /// on its own host.
    Put {
        from: Option<&'a crate::inbox::Sender>,
    },
    /// The local owner continues a unit execution whose launch record predates the owner
    /// host. `continue_legacy` is the only caller: its acceptance proves the original
    /// admission and the owner's live session under the launch lock.
    Legacy { from: &'a crate::inbox::Sender },
}

pub fn observe(ledger: &Ledger, agents: &mut [Agent]) -> Result<()> {
    ledger.with_lock("done", || {
        let before: Marks = ledger.read(FILE)?.unwrap_or_default();
        let mut marks = migrate(ledger, before.clone(), agents)?;
        let owners = owners(ledger)?;
        for agent in agents {
            let mut done = false;
            for mark in marks.values_mut().filter(|m| matches(m, agent)) {
                let evidence = evidence(ledger, mark, agent);
                seal(mark, agent, &evidence);
                done |= terminal(mark, &evidence)
                    && waiting(agent)
                    && !owners.contains_key(&agent.pane);
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
    let mut tip = None;
    for reference in [
        format!("refs/heads/{}", branch.name),
        format!("refs/remotes/origin/{}", branch.name),
    ] {
        let exists = shell.run(
            "git",
            &[
                "-C".into(),
                branch.repo.clone(),
                "show-ref".into(),
                "--verify".into(),
                "--quiet".into(),
                reference.clone(),
            ],
        )?;
        match exists.status {
            0 => {
                tip = Some(reference);
                break;
            }
            1 => (),
            _ => {
                return Err(Error::Invalid(format!(
                    "{}: cannot inspect branch {}: {}",
                    branch.repo,
                    branch.name,
                    exists.stderr.trim()
                )));
            }
        }
    }
    let Some(tip) = tip else {
        branch.landed = true;
        branch.ahead = 0;
        return Ok(());
    };
    let landed = crate::loss::Evidence::default_only(&branch.repo)
        .assess(shell, &tip, None)?
        .landed();
    let ahead = if landed {
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
    branch.landed = landed;
    branch.ahead = ahead;
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
            let mut updated = mark.clone();
            // Branch refresh can overlap a live inbox check. Never replace its terminal seal
            // with an older unsealed observation from the start of this handoff pass.
            updated.sealed = entry.get().sealed.or(updated.sealed);
            updated.closed_ms = entry.get().closed_ms.or(updated.closed_ms);
            entry.insert(updated);
            ledger.write(FILE, &marks)?;
        }
        Ok(())
    })
}

/// Serialize inbox acceptance with sealing. Only new work accepted while the marked turn is
/// still working cancels a plain pane's mark. After idle/done (or an earlier seal), reject all
/// messages, even if Herdr subsequently reports working. No post-turn claim can reopen a
/// finished child.
///
/// A unit execution that has not returned is not finished. Only its execution owner may
/// continue or cancel its mark, by putting a record. Every check runs before `accept`, so a
/// refusal leaves the mark and the inbox unchanged.
pub fn input<T>(
    ledger: &Ledger,
    agent: &Agent,
    input: Input<'_>,
    accept: impl FnOnce() -> Result<(T, bool)>,
) -> Result<T> {
    if matches!(input, Input::Legacy { .. }) {
        return Err(Error::Invalid(
            "legacy continuation needs its own guarded acceptance".into(),
        ));
    }
    guarded(ledger, agent, input, |_| accept())
}

/// Continue the one done mark of a legacy unit execution in `agent`'s pane. A mark must
/// belong to an unreturned launch with no owner host, and `from` must be its owner pane on
/// this host. `accept` gets that exact mark, or `None` when the pane has no mark, while the
/// done lock is held. It must prove the original admission and decide from its durable
/// receipt. It returns whether its acceptance consumed the mark: only then is that mark, and
/// no other, removed, and only if it is unchanged.
pub fn continue_legacy<T>(
    ledger: &Ledger,
    agent: &Agent,
    from: &crate::inbox::Sender,
    accept: impl FnOnce(Option<&Mark>) -> Result<(T, bool)>,
) -> Result<T> {
    guarded(ledger, agent, Input::Legacy { from }, accept)
}

fn guarded<T>(
    ledger: &Ledger,
    agent: &Agent,
    input: Input<'_>,
    accept: impl FnOnce(Option<&Mark>) -> Result<(T, bool)>,
) -> Result<T> {
    ledger.with_lock("done", || {
        let before: Marks = ledger.read(FILE)?.unwrap_or_default();
        let mut marks = migrate(ledger, before.clone(), std::slice::from_ref(agent))?;
        let mut refusal = None;
        let mut continued = false;
        let mut legacy = None;
        let matched = marks.values().filter(|m| matches(m, agent)).count();
        if matches!(input, Input::Legacy { .. }) && matched > 1 {
            return Err(Error::Invalid(format!(
                "{}: competing done marks; nothing was accepted",
                agent.pane
            )));
        }
        for (key, mark) in marks.iter_mut().filter(|(_, m)| matches(m, agent)) {
            let evidence = evidence(ledger, mark, agent);
            seal(mark, agent, &evidence);
            let name = mark
                .name
                .as_deref()
                .or(agent.name.as_deref())
                .unwrap_or(&agent.pane)
                .to_owned();
            refusal = if terminal(mark, &evidence) {
                Some(format!("{name} finished; start a new child for more work"))
            } else {
                match (evidence, input) {
                    // Legacy continuation exists only for a launch record that predates the
                    // owner host. Every other execution keeps the ordinary rules.
                    (
                        Ok(crate::launch::Execution::Unreturned {
                            owner,
                            host: None,
                        }),
                        Input::Legacy { from },
                    ) if from.pane.as_deref() == Some(owner.as_str())
                        && from.host == crate::inbox::hostname()
                        && mark.closed_ms.is_none() =>
                    {
                        legacy = Some((key.clone(), mark.clone()));
                        None
                    }
                    (Ok(crate::launch::Execution::Unreturned { host: Some(_), .. }), Input::Legacy { .. }) => Some(format!(
                        "{name}'s launch record names its owner host; continue it with dogsled send; nothing was accepted"
                    )),
                    (Ok(crate::launch::Execution::Plain), Input::Legacy { .. }) => Some(format!(
                        "{name} has no unit execution; continue it with dogsled send; nothing was accepted"
                    )),
                    (Ok(crate::launch::Execution::Plain), _) => None,
                    (Ok(crate::launch::Execution::Returned), _) => Some(format!(
                        "{name} returned its unit; start a new child for more work"
                    )),
                    (Err(error), _) => Some(format!(
                        "{name} called done, and its unit evidence is unavailable: {error}; nothing was accepted"
                    )),
                    // Pane and host must both match the stored originating owner. A launch
                    // record with no owner host proves nobody, so it refuses every sender.
                    (
                        Ok(crate::launch::Execution::Unreturned {
                            owner,
                            host: Some(host),
                        }),
                        Input::Put { from: Some(from) },
                    ) if from.pane.as_deref() == Some(owner.as_str()) && from.host == host => {
                        continued = true;
                        None
                    }
                    (Ok(crate::launch::Execution::Unreturned { owner, .. }), _) => Some(format!(
                        "{name} called done before its unit returned; only its owner {owner} can continue it; nothing was accepted"
                    )),
                }
            };
            if refusal.is_some() {
                break;
            }
        }
        if let Some(refusal) = refusal {
            if marks != before {
                ledger.write(FILE, &marks)?;
            }
            return Err(Error::Invalid(refusal));
        }
        // Legacy input without a refusal reaches here with its one mark, or with no matching
        // mark at all: its receipt decides whether that is a finished retry.
        let (result, received) = accept(legacy.as_ref().map(|(_, mark)| mark))?;
        if let Some((key, mark)) = &legacy {
            // The acceptance consumed this mark. Remove exactly it, and only if unchanged.
            if received && marks.get(key) == Some(mark) {
                marks.remove(key);
            }
        } else if received && (agent.state == "working" || continued) {
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
        if let Some(branch) = &mut mark.branch
            && !(mark.closed_ms.is_some() && branch.landed)
        {
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
        if !crate::launch::handoff_ready(ledger, &mark.pane)? {
            continue;
        }
        if mark.reply.is_none() {
            let reply = if let Some(text) = crate::launch::handoff_reply(ledger, &mark.pane)? {
                crate::sweep::Final {
                    seq: mark.sealed.unwrap_or(0),
                    text,
                }
            } else if let Some(reply) = crate::sweep::local_reply(shell, ledger, &mark.pane)? {
                reply
            } else {
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
