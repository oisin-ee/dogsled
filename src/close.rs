//! Close a finished child: the panes and Herdr workspace that hold its worktree, the worktree
//! (after the repository's `teardown`), its branch when integrated, and its pane.

use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::worktree::{self, AGENT_LOCK, refuse};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    pub path: String,
    pub branch: String,
    /// The Herdr workspace that showed the worktree, closed before removal.
    pub workspace: Option<String>,
    /// The branch is deleted under the shared loss rule.
    pub branch_deleted: bool,
    /// The main checkout's anchor workspace, closed after its last child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_closed: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Closed {
    pub pane: String,
    /// The linked worktree that was removed, if the pane was in one.
    pub worktree: Option<Worktree>,
    /// True when dogsled closed the pane itself, because no workspace close covered it.
    pub pane_closed: bool,
    /// Retained stopped-orphan work, measured before the checkout is removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_branch: Option<crate::done::Branch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<crate::preservation::Recovery>,
}

/// The caller's own pane, workspace, and directory. Over SSH all are empty.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caller {
    pub pane: Option<String>,
    pub workspace: Option<String>,
    pub cwd: Option<String>,
}

impl Caller {
    pub fn from_env() -> Self {
        Self {
            pane: std::env::var("HERDR_PANE_ID").ok(),
            workspace: std::env::var("HERDR_WORKSPACE_ID").ok(),
            cwd: std::env::current_dir()
                .ok()
                .map(|d| d.display().to_string()),
        }
    }
}

/// Close context carried over host transport. Every variant uses the same loss rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guard<'a> {
    /// Explicit close; inspect local item ownership.
    Clean,
    /// An orphan handoff; retain its branch.
    Pushed,
    /// Host-verified stopped-orphan permission for this target pane only.
    StoppedOrphan { episode: &'a StoppedOrphan },
    /// A finished reader handoff; retain its branch.
    Reader,
    /// A finished writer handoff; retain its branch.
    Writer,
}

impl Guard<'_> {
    fn preservation(self) -> Option<crate::preservation::CloseMode> {
        match self {
            Self::Clean => Some(crate::preservation::CloseMode::Owner),
            Self::StoppedOrphan { episode } => {
                Some(crate::preservation::CloseMode::StoppedOrphan {
                    episode: episode.clone(),
                })
            }
            Self::Pushed | Self::Reader | Self::Writer => None,
        }
    }

    /// The flags of `host close` for this guard.
    pub fn flags(self) -> &'static [&'static str] {
        match self {
            Self::Clean => &[],
            Self::Pushed | Self::StoppedOrphan { .. } => &["--pushed"],
            Self::Reader => &["--landed"],
            Self::Writer => &["--landed", "--writer"],
        }
    }

    pub fn from_flags(pushed: bool, landed: bool, writer: bool) -> Self {
        match (pushed, landed, writer) {
            (_, true, true) => Self::Writer,
            (_, true, false) => Self::Reader,
            (true, false, _) => Self::Pushed,
            _ => Self::Clean,
        }
    }
}

/// Session and turn evidence for the stopped-orphan-only close permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoppedOrphan {
    pane: String,
    agent_session: String,
    turn: crate::done::Turn,
}

impl StoppedOrphan {
    pub fn pane(&self) -> &str {
        &self.pane
    }

    pub fn recorded(ledger: &Ledger, item: &str, agent: &crate::probe::Agent) -> Result<Self> {
        #[derive(Default, Deserialize)]
        struct Episodes {
            #[serde(default)]
            stopped: std::collections::BTreeMap<String, StoppedOrphan>,
        }
        let mut episodes: Episodes = ledger.read("watchdog.json")?.unwrap_or_default();
        let episode = episodes
            .stopped
            .remove(item)
            .ok_or_else(|| refuse("stopped orphan has no recorded episode"))?;
        episode.validate(agent)?;
        Ok(episode)
    }

    fn verify_live(
        &self,
        shell: &dyn crate::shell::Shell,
        ledger: &Ledger,
        pane: &str,
    ) -> Result<()> {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        let probe = crate::node::observed_probe(ledger, shell, &home)?;
        let agent = probe
            .agent_panes
            .iter()
            .find(|agent| agent.pane == pane)
            .ok_or_else(|| refuse("stopped orphan agent disappeared"))?;
        self.validate(agent)
    }

    fn validate(&self, agent: &crate::probe::Agent) -> Result<()> {
        let turn = crate::done::Turn::of_agent(agent);
        let axes = [
            self.turn.state_change_seq.zip(turn.state_change_seq),
            self.turn.state_since_ms.zip(turn.state_since_ms),
        ];
        if self.pane != agent.pane
            || agent.agent_session.as_deref() != Some(self.agent_session.as_str())
            || !axes.iter().any(Option::is_some)
            || !axes.iter().flatten().all(|(old, now)| old == now)
            || crate::lifecycle::of_agent(agent, crate::lifecycle::Host::Up).state
                != crate::lifecycle::State::Stopped
        {
            return Err(refuse("stopped orphan session or turn changed"));
        }
        Ok(())
    }
}

/// Every refusal happens before anything changes. A child tab in the caller's workspace is
/// closed; the caller's own pane, and a worktree the caller works in or shows, are refused.
/// Only an agent worktree (locked `agent:worktree`) is removed; a child in any other checkout
/// loses only its pane.
/// `guard` adds the checks that make an automatic close lose nothing.
pub fn local(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    pane: &str,
    caller: &Caller,
    guard: Guard,
) -> Result<Closed> {
    with_local(shell, ledger, pane, caller, guard, Ok)
}

/// Keep repository protection through the caller's matching-record reconciliation.
pub fn with_local<T>(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    pane: &str,
    caller: &Caller,
    mode: Guard,
    reconcile: impl FnOnce(Closed) -> Result<T>,
) -> Result<T> {
    with_request(
        shell,
        ledger,
        pane,
        caller,
        Options {
            guard: mode,
            request: None,
        },
        reconcile,
    )
}

#[derive(Debug, Clone, Copy)]
pub struct Options<'a> {
    pub guard: Guard<'a>,
    pub request: Option<&'a crate::preservation::CloseRequest>,
}

#[derive(Clone, Copy)]
struct CloseContext<'a> {
    mode: Guard<'a>,
    request: Option<&'a crate::preservation::CloseRequest>,
    existing: Option<&'a crate::preservation::Operation>,
    terminal: Option<&'a str>,
    directory: Option<&'a worktree::Pin>,
}

/// A stable origin item identity makes host result replay independent of the old path.
pub fn with_request<T>(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    pane: &str,
    caller: &Caller,
    options: Options<'_>,
    reconcile: impl FnOnce(Closed) -> Result<T>,
) -> Result<T> {
    let Options {
        guard: mode,
        request,
    } = options;
    if caller.pane.as_deref() == Some(pane) {
        return Err(refuse("cannot close the caller pane"));
    }
    let existing = if let Some(mode) = mode.preservation() {
        let existing = crate::preservation::operation(ledger, pane, request)?;
        if let Some(operation) = &existing {
            if operation.expectation.mode != mode {
                return Err(operation.refusal("close mode or stopped episode changed"));
            }
            if matches!(
                operation.stage,
                crate::preservation::Stage::Removed | crate::preservation::Stage::Finished
            ) {
                validate_replay(shell, ledger, operation)?;
                let closed = operation
                    .result
                    .clone()
                    .ok_or_else(|| operation.refusal("removed checkout has no saved result"))?;
                let repo = &operation.expectation.tree.repo;
                return worktree::with_lifecycle(shell, ledger, Path::new(repo), |_| {
                    let mut operation = crate::preservation::operation(ledger, pane, request)?
                        .ok_or_else(|| refuse("preservation operation disappeared"))?;
                    validate_replay(shell, ledger, &operation)?;
                    operation.finish(ledger, &closed)?;
                    reconcile(closed)
                });
            }
        }
        existing
    } else {
        None
    };
    with_expected(
        shell,
        ledger,
        pane,
        caller,
        options,
        existing.as_ref(),
        reconcile,
    )
}

struct CapturedClose {
    panes: Vec<crate::probe::Pane>,
    terminals: std::collections::BTreeMap<String, String>,
    owners: Vec<crate::items::Item>,
    saved: Option<crate::cleanup::Tree>,
    observation: Option<worktree::CloseObservation>,
    planned: Option<crate::cleanup::Tree>,
}

fn capture_expected(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    pane: &str,
    existing: Option<&crate::preservation::Operation>,
) -> Result<CapturedClose> {
    let (panes, terminals) = crate::cleanup::pane_snapshot(shell)?;
    let owners = local_owners(ledger, pane)?;
    let target = panes.iter().find(|p| p.pane_id == pane);
    let cwd = target.and_then(|p| p.cwd.as_deref());
    let capture = cwd
        .map(|cwd| worktree::capture_close(shell, cwd))
        .transpose()?
        .flatten();
    let saved = crate::launch::checkout_for_pane(ledger, pane)?;
    let observation = capture
        .map(|capture| capture.bind_saved(saved.as_ref()))
        .transpose()?;
    let live = match &observation {
        Some(worktree::CloseObservation::Live { tree, .. }) => Some(tree),
        Some(worktree::CloseObservation::Missing) | None => None,
    };
    let planned = if let Some(operation) = existing {
        if operation.expectation.terminal != terminals.get(pane).cloned()
            || operation.expectation.owners
                != owners
                    .iter()
                    .map(crate::preservation::CloseRequest::of)
                    .collect::<Vec<_>>()
        {
            return Err(operation.refusal("pane terminal or item changed"));
        }
        Some(operation.expectation.tree.clone())
    } else {
        close_expectation(saved.as_ref(), live)
    };
    Ok(CapturedClose {
        panes,
        terminals,
        owners,
        saved,
        observation,
        planned,
    })
}

fn with_expected<T>(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    pane: &str,
    caller: &Caller,
    options: Options<'_>,
    existing: Option<&crate::preservation::Operation>,
    reconcile: impl FnOnce(Closed) -> Result<T>,
) -> Result<T> {
    let Options {
        guard: mode,
        request,
    } = options;
    let CapturedClose {
        panes,
        terminals,
        owners,
        saved,
        observation,
        planned,
    } = capture_expected(shell, ledger, pane, existing)?;
    let (live, directory) = match &observation {
        Some(worktree::CloseObservation::Live { tree, pin }) => (Some(tree), pin.as_ref()),
        Some(worktree::CloseObservation::Missing) | None => (None, None),
    };
    let Some(planned) = planned else {
        let target = validate_target(shell, &panes, pane, caller)?;
        return reconcile(remove_target(
            shell,
            ledger,
            target,
            &panes,
            caller,
            None,
            CloseContext {
                mode,
                request,
                existing,
                terminal: terminals.get(pane).map(String::as_str),
                directory: None,
            },
        )?);
    };
    let repo = planned.repo.clone();
    worktree::with_lifecycle(shell, ledger, Path::new(&repo), |lifecycle| {
        if let Some(closed) = locked_replay(shell, ledger, pane, options)? {
            return reconcile(closed);
        }
        let current_owners = local_owners(ledger, pane)?;
        if owner_identities(&current_owners) != owner_identities(&owners) {
            return Err(refuse("checkout item ownership changed"));
        }
        if let (Some(saved), Some(live)) = (&saved, live)
            && !live.matches_checkout(saved)
        {
            return Err(refuse("pane moved to another checkout"));
        }
        let absent = worktree::verified_absent(lifecycle, shell, &planned)
            || (saved.is_none()
                && live.is_some_and(|tree| {
                    worktree::verified_absent_unmarked(lifecycle, shell, tree)
                }));
        if !absent && matches!(observation, Some(worktree::CloseObservation::Missing)) {
            return Err(refuse("checkout reappeared"));
        }
        let planned = if absent {
            planned
        } else {
            establish_close_identity(lifecycle, shell, planned, live, directory, saved.is_none())?
        };
        if absent || worktree::verified_absent(lifecycle, shell, &planned) {
            return reconcile(close_absent(
                shell,
                ledger,
                AbsentClose {
                    pane,
                    caller,
                    planned,
                    terminal: terminals.get(pane).map(String::as_str),
                    owners: &owners,
                    saved: saved.is_some(),
                },
            )?);
        }
        // Every removal needs an observation and its original directory handles.
        let live = live.ok_or_else(|| refuse("checkout identity is unreadable"))?;
        let directory = directory.ok_or_else(|| refuse("checkout identity is unreadable"))?;
        directory.validate(live)?;
        let panes = crate::cleanup::current_panes(shell)?;
        let target = validate_target(shell, &panes, pane, caller)?;
        directory.validate(live)?;
        reconcile(remove_target(
            shell,
            ledger,
            target,
            &panes,
            caller,
            Some((lifecycle, &planned)),
            CloseContext {
                mode,
                request,
                existing,
                terminal: terminals.get(pane).map(String::as_str),
                directory: Some(directory),
            },
        )?)
    })
}

fn owner_identities(owners: &[crate::items::Item]) -> Vec<crate::preservation::CloseRequest> {
    owners
        .iter()
        .map(crate::preservation::CloseRequest::of)
        .collect()
}

fn locked_replay(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    pane: &str,
    options: Options<'_>,
) -> Result<Option<Closed>> {
    let Some(mode) = options.guard.preservation() else {
        return Ok(None);
    };
    let Some(mut operation) = crate::preservation::operation(ledger, pane, options.request)? else {
        return Ok(None);
    };
    operation.check_retry(&mode)?;
    if !matches!(
        operation.stage,
        crate::preservation::Stage::Removed | crate::preservation::Stage::Finished
    ) {
        return Ok(None);
    }
    validate_replay(shell, ledger, &operation)?;
    let closed = operation
        .result
        .clone()
        .ok_or_else(|| operation.refusal("missing saved close result"))?;
    operation.finish(ledger, &closed)?;
    Ok(Some(closed))
}

struct AbsentClose<'a> {
    pane: &'a str,
    caller: &'a Caller,
    planned: crate::cleanup::Tree,
    terminal: Option<&'a str>,
    owners: &'a [crate::items::Item],
    saved: bool,
}

fn close_absent(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    close: AbsentClose<'_>,
) -> Result<Closed> {
    let AbsentClose {
        pane,
        caller,
        planned,
        terminal,
        owners,
        saved,
    } = close;
    validate_absent_target(shell, ledger, pane, caller, &planned, terminal, owners)?;
    if saved {
        crate::launch::validate_checkout_owner(ledger, pane, None, terminal)?;
    }
    crate::cleanup::forget_matching(ledger, &planned)?;
    Ok(Closed {
        pane: pane.into(),
        worktree: Some(Worktree {
            path: planned.path,
            branch: planned.branch,
            workspace: None,
            branch_deleted: false,
            anchor_closed: None,
        }),
        pane_closed: false,
        retained_branch: None,
        recovery: None,
    })
}

/// Recheck the original terminal, ownership, lifecycle, and all external holds before clearing content.
pub(crate) fn validate_preservation(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    operation: &crate::preservation::Operation,
    tree: &worktree::Linked,
) -> Result<()> {
    validate_replay(shell, ledger, operation)?;
    let expected = &operation.expectation;
    let (panes, _) = crate::cleanup::pane_snapshot(shell)?;
    let target = validate_target(shell, &panes, &expected.pane, &expected.caller)?;
    let mode = match &expected.mode {
        crate::preservation::CloseMode::Owner => Guard::Clean,
        crate::preservation::CloseMode::StoppedOrphan { episode } => {
            Guard::StoppedOrphan { episode }
        }
    };
    validate_completion(shell, ledger, target, mode)?;
    let evidence = crate::loss::Evidence::default_only(&expected.tree.repo);
    let loss = evidence.assess(shell, &tree.head, None)?;
    crate::cleanup::close_holds(shell, ledger, tree, &expected.caller, mode, &loss)
}

fn validate_replay(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    operation: &crate::preservation::Operation,
) -> Result<()> {
    let expected = &operation.expectation;
    let (panes, terminals) = crate::cleanup::pane_snapshot(shell)?;
    if panes.iter().any(|p| p.pane_id == expected.pane)
        && (expected.terminal.is_none()
            || terminals.get(&expected.pane) != expected.terminal.as_ref())
    {
        return Err(operation.refusal("pane terminal changed; saved result only"));
    }
    if local_owners(ledger, &expected.pane)?.iter().any(|item| {
        !expected
            .owners
            .contains(&crate::preservation::CloseRequest::of(item))
    }) {
        return Err(operation.refusal("item ownership changed; saved result only"));
    }
    let recovery = operation.recovery()?;
    let saved = crate::preservation::show(shell, Path::new(&recovery.repository), &recovery.id)
        .map_err(|e| operation.refusal(e))?;
    if saved.commit != recovery.pinned_oid {
        return Err(operation.refusal("pinned recovery changed"));
    }
    Ok(())
}

/// Add only an unmarked checkout's generation; never replace the request's HEAD.
fn establish_close_identity(
    lifecycle: &worktree::Guard,
    shell: &dyn crate::shell::Shell,
    planned: crate::cleanup::Tree,
    live: Option<&worktree::Linked>,
    pin: Option<&worktree::Pin>,
    unsaved: bool,
) -> Result<crate::cleanup::Tree> {
    if unsaved && planned.identity.is_none() {
        return Ok(lifecycle
            .discover_unmarked(
                shell,
                live.ok_or_else(|| refuse("checkout identity is unreadable"))?,
                pin.ok_or_else(|| refuse("checkout identity is unreadable"))?,
            )?
            .tree());
    }
    // Saved identity stays launch-bound; HEAD is the close request's observation.
    if let Some(now) = lifecycle.observe(shell, &planned.path)? {
        match planned.drift(&now.tree()) {
            crate::cleanup::Drift::Identity => return Err(refuse("worktree identity changed")),
            crate::cleanup::Drift::HeadMoved => return Err(refuse("HEAD moved before close")),
            crate::cleanup::Drift::Same => (),
        }
    }
    Ok(planned)
}

/// Retain launch identity, but capture HEAD at the start of this close request.
fn close_expectation(
    saved: Option<&crate::cleanup::Tree>,
    live: Option<&worktree::Linked>,
) -> Option<crate::cleanup::Tree> {
    let mut planned = saved
        .cloned()
        .or_else(|| live.map(worktree::Linked::tree))?;
    if let Some(live) = live {
        planned.head.clone_from(&live.head);
    }
    Some(planned)
}

fn local_owners(ledger: &Ledger, pane: &str) -> Result<Vec<crate::items::Item>> {
    Ok(crate::items::list(ledger)?
        .into_iter()
        .filter(|item| {
            item.host == crate::hosts::LOCAL && item.pane == pane && item.closed_ms.is_none()
        })
        .collect())
}

/// Absence never authorizes reconciliation of a replacement pane or another item.
fn validate_absent_target(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    pane: &str,
    caller: &Caller,
    tree: &crate::cleanup::Tree,
    terminal: Option<&str>,
    owners: &[crate::items::Item],
) -> Result<()> {
    let (panes, terminals) = crate::cleanup::pane_snapshot(shell)?;
    let items = crate::items::list(ledger)?;
    let new_owner = items.iter().any(|item| {
        item.host == crate::hosts::LOCAL
            && item.pane == pane
            && item.closed_ms.is_none()
            && !owners.iter().any(|owner| owner.id == item.id)
    });
    let lost_owner = owners.iter().any(|owner| {
        items
            .iter()
            .find(|item| item.id == owner.id)
            .is_none_or(|item| {
                item.host != owner.host
                    || item.pane != owner.pane
                    || item.owner_pane != owner.owner_pane
                    || item.owner_session != owner.owner_session
                    || item.created_ms != owner.created_ms
            })
    });
    // Only the same captured item becoming closed may leave the open owner set.
    if new_owner || lost_owner {
        return Err(refuse("checkout item ownership changed"));
    }
    if panes.iter().any(|target| target.pane_id == pane) {
        let target = validate_target(shell, &panes, pane, caller)?;
        if terminal.is_none()
            || terminal != terminals.get(pane).map(String::as_str)
            || target.cwd.as_deref().is_none_or(|cwd| {
                !worktree::resolved(Path::new(cwd))
                    .starts_with(worktree::resolved(Path::new(&tree.path)))
            })
        {
            return Err(refuse("removed checkout no longer owns the pane"));
        }
    }
    Ok(())
}

/// Repeat the captured-resource check before entering any close effect.
fn validate_checkout(
    shell: &dyn crate::shell::Shell,
    target: &crate::probe::Pane,
    expected: &crate::cleanup::Tree,
) -> Result<worktree::Linked> {
    let now = worktree::linked_or_detached(shell, &expected.path)?
        .ok_or_else(|| refuse("checkout identity is unreadable"))?;
    if expected.identity.is_none()
        || now.identity != expected.identity
        || now.branch != expected.branch
        || !worktree::same(&now.path, &expected.path)
    {
        return Err(refuse("worktree identity changed"));
    }
    if now.head != expected.head {
        return Err(refuse("HEAD moved before close"));
    }
    worktree::validate_pane_checkout(shell, target, &now)?;
    Ok(now)
}

/// Validate pane safety before selecting checkout removal or pane-only closure.
fn validate_target<'a>(
    shell: &dyn crate::shell::Shell,
    panes: &'a [crate::probe::Pane],
    pane: &str,
    caller: &Caller,
) -> Result<&'a crate::probe::Pane> {
    let target = panes
        .iter()
        .find(|p| p.pane_id == pane)
        .ok_or_else(|| Error::Invalid(format!("pane {pane} does not exist on this host")))?;
    if caller.pane.as_deref() == Some(pane) {
        return Err(refuse(&format!("{pane} is the caller's own pane")));
    }
    if let Some(reason) = worktree::busy(shell, target)? {
        return Err(refuse(&format!(
            "{pane} is busy ({reason}); close only an idle or done child"
        )));
    }
    if let Some(reason) = crate::cleanup::close_group_hold(shell, panes, pane)? {
        return Err(refuse(&reason));
    }
    Ok(target)
}

fn pane_only(shell: &dyn crate::shell::Shell, pane: &str) -> Result<Closed> {
    shell.run_ok("herdr", &["pane".into(), "close".into(), pane.into()])?;
    Ok(Closed {
        pane: pane.into(),
        worktree: None,
        pane_closed: true,
        retained_branch: None,
        recovery: None,
    })
}

fn remove_target(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    target: &crate::probe::Pane,
    panes: &[crate::probe::Pane],
    caller: &Caller,
    lifecycle: Option<(&worktree::Guard, &crate::cleanup::Tree)>,
    context: CloseContext<'_>,
) -> Result<Closed> {
    let guard = context.mode;
    let pane = target.pane_id.as_str();
    let validated = lifecycle
        .map(|(_, expected)| validate_checkout(shell, target, expected))
        .transpose()?;
    // An explicit recovery may end a stranded shell without removing its checkout or
    // assigning an outcome to the interrupted launch. Automatic cleanup cannot do this.
    if guard == Guard::Clean && crate::launch::close_unfinished(ledger, shell, pane)? {
        return Ok(Closed {
            pane: pane.into(),
            worktree: None,
            pane_closed: true,
            retained_branch: None,
            recovery: None,
        });
    }
    validate_completion(shell, ledger, target, guard)?;
    let tree = match validated {
        Some(tree) => Some(tree),
        None => target
            .cwd
            .as_deref()
            .map(|cwd| worktree::linked_or_detached(shell, cwd))
            .transpose()?
            .flatten(),
    };
    // Dogsled locks every worktree it makes. An unlocked one is someone's own checkout that
    // the child only ran in, such as its owner's: closing the child closes its pane alone.
    let lock = match &tree {
        Some(tree) => worktree::lock_reason(shell, &tree.checkout, &tree.path)?,
        None => None,
    };
    let automatic = guard != Guard::Clean;
    let Some(tree) =
        tree.filter(|_| lock.is_some() && (!automatic || lock.as_deref() == Some(AGENT_LOCK)))
    else {
        return pane_only(shell, pane);
    };
    if lock.as_deref() == Some(AGENT_LOCK) {
        crate::cleanup::remember(ledger, tree.tree())?;
    }
    let workspace = worktree::workspace(shell, &tree.checkout, &tree.path)?;
    let holders = worktree::holders(panes, &tree.path, workspace.as_deref());
    // Another agent, open item, or busy pane still uses the checkout. Even an explicit close
    // releases only its own pane: closing the others would close agents nobody asked to close.
    if shared(shell, ledger, pane, &holders)? {
        if guard.preservation().is_some()
            && !crate::loss::dirty_paths(shell, &tree.path, false)?.is_empty()
        {
            return Err(refuse(
                "shared checkout ownership; preservation cannot release another holder",
            ));
        }
        return pane_only(shell, pane);
    }
    outside(&tree, caller)?;
    let evidence = close_evidence(shell, ledger, &tree, caller, guard)?;
    let retained_branch = retained_branch(shell, &tree, guard)?;
    if let Some(lock) = lock
        && lock != AGENT_LOCK
    {
        return Err(refuse(&format!(
            "{} is locked ({lock}); it is not an agent worktree",
            tree.path
        )));
    }
    if workspace.is_some() && workspace == caller.workspace {
        return Err(refuse(&format!(
            "workspace {} is the caller's own",
            workspace.as_deref().unwrap_or_default()
        )));
    }
    for other in holders.iter().filter(|p| p.pane_id != pane) {
        if caller.pane.as_deref() == Some(other.pane_id.as_str()) {
            return Err(refuse(&format!("the caller works in {}", tree.path)));
        }
        if let Some(reason) = worktree::busy(shell, other)? {
            return Err(refuse(&format!(
                "{} also uses {} and is busy ({reason})",
                other.pane_id, tree.path
            )));
        }
    }
    // The target is a holder through its cwd, so removal closes it unless its workspace does.
    let pane_closed = workspace.is_none() || target.workspace_id != workspace;
    let plan = RemovalPlan {
        tree,
        workspace,
        holders,
        pane: pane.into(),
        pane_closed,
        retained_branch,
        evidence,
        lifecycle: lifecycle
            .ok_or_else(|| refuse("repository protection missing"))?
            .0,
    };
    execute_removal(shell, ledger, caller, context, plan)
}

fn close_evidence(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    tree: &worktree::Linked,
    caller: &Caller,
    guard: Guard<'_>,
) -> Result<crate::loss::Evidence> {
    if guard.preservation().is_none() {
        return unsaved(shell, ledger, tree, caller, guard);
    }
    let evidence = if guard == Guard::Clean || tree.branch.is_empty() {
        crate::loss::Evidence::fetch(shell, &tree.checkout.display().to_string())
    } else {
        crate::loss::Evidence::keeping_branch(&tree.checkout.display().to_string(), &tree.branch)
    };
    let loss = evidence.assess(shell, &tree.head, None)?;
    crate::cleanup::close_holds(shell, ledger, tree, caller, guard, &loss)?;
    Ok(evidence)
}

struct RemovalPlan<'a> {
    tree: worktree::Linked,
    workspace: Option<String>,
    holders: Vec<&'a crate::probe::Pane>,
    pane: String,
    pane_closed: bool,
    retained_branch: Option<crate::done::Branch>,
    evidence: crate::loss::Evidence,
    lifecycle: &'a worktree::Guard,
}

fn execute_removal(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    caller: &Caller,
    context: CloseContext<'_>,
    plan: RemovalPlan<'_>,
) -> Result<Closed> {
    let RemovalPlan {
        tree,
        workspace,
        holders,
        pane,
        pane_closed,
        retained_branch,
        evidence,
        lifecycle,
    } = plan;
    let pane = pane.as_str();
    let mut operation = begin_preservation(shell, ledger, &tree, pane, caller, context)?;
    if let Some(operation) = &mut operation {
        operation.capture_initial(shell, ledger)?;
        operation.result = Some(Closed {
            pane: pane.into(),
            worktree: Some(Worktree {
                path: tree.path.clone(),
                branch: tree.branch.clone(),
                workspace: workspace.clone(),
                branch_deleted: false,
                anchor_closed: None,
            }),
            pane_closed,
            retained_branch: retained_branch.clone(),
            recovery: Some(operation.recovery()?),
        });
        operation.save(ledger)?;
    }
    let evidence = if tree.branch.is_empty() {
        operation
            .as_ref()
            .and_then(|o| o.initial.as_ref())
            .map_or(evidence, |s| {
                crate::loss::Evidence::keeping_recovery(&tree.checkout.display().to_string(), s)
            })
    } else {
        evidence
    };
    let removal = worktree::Removal {
        guard: lifecycle,
        workspace: workspace.as_deref(),
        holders: &holders,
        caller_workspace: caller.workspace.as_deref(),
        evidence: &evidence,
    };
    let removed = if let Some(operation) = &mut operation {
        worktree::remove_preserved(
            shell,
            ledger,
            &tree,
            removal,
            operation,
            context
                .directory
                .ok_or_else(|| refuse("original directory protection missing"))?,
        )
        .map_err(|e| operation.refusal(e))?
    } else {
        worktree::remove(shell, ledger, &tree, removal)?
    };
    let closed = Closed {
        pane: pane.into(),
        worktree: Some(Worktree {
            path: tree.path,
            branch: tree.branch,
            workspace,
            branch_deleted: removed.branch_deleted,
            anchor_closed: removed.anchor_closed,
        }),
        pane_closed,
        retained_branch: operation.as_ref().map_or(retained_branch, |o| {
            o.result.as_ref().and_then(|r| r.retained_branch.clone())
        }),
        recovery: operation
            .as_ref()
            .map(crate::preservation::Operation::recovery)
            .transpose()?,
    };
    if let Some(operation) = &mut operation {
        operation.finish(ledger, &closed)?;
    }
    Ok(closed)
}

fn begin_preservation(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    tree: &worktree::Linked,
    pane: &str,
    caller: &Caller,
    context: CloseContext<'_>,
) -> Result<Option<crate::preservation::Operation>> {
    let CloseContext {
        mode: guard,
        request,
        existing,
        terminal,
        ..
    } = context;
    Ok(if let Some(mode) = guard.preservation() {
        let (_, terminals) = crate::cleanup::pane_snapshot(shell)?;
        if terminal.is_none() || terminal != terminals.get(pane).map(String::as_str) {
            return Err(refuse(
                "pane terminal is unreadable or changed before preservation",
            ));
        }
        let owners = local_owners(ledger, pane)?
            .iter()
            .map(crate::preservation::CloseRequest::of)
            .collect();
        let expectation = crate::preservation::CloseExpectation {
            pane: pane.into(),
            terminal: terminals.get(pane).cloned(),
            request: request.cloned(),
            launch: crate::launch::identity(ledger, pane)?,
            owners,
            tree: tree.tree(),
            mode,
            caller: caller.clone(),
        };
        if let Some(existing) = existing
            && existing.expectation != expectation
        {
            return Err(existing.refusal("preservation close expectation changed"));
        }
        Some(crate::preservation::Operation::begin(ledger, expectation)?)
    } else {
        None
    })
}

/// Only the explicit orphan path can replace native done with correlated stopped evidence.
fn validate_completion(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    target: &crate::probe::Pane,
    guard: Guard,
) -> Result<()> {
    let pane = target.pane_id.as_str();
    if let Guard::StoppedOrphan { episode } = guard {
        episode.verify_live(shell, ledger, pane)?;
    } else if guard != Guard::Clean
        && target.agent.is_some()
        && !crate::done::finished(ledger, &crate::probe::agent_list(shell)?)?.contains(pane)
    {
        return Err(refuse(&format!(
            "{pane} has not called done; it waits for input"
        )));
    }
    Ok(())
}

/// Snapshot the branch while its checkout and repository protection still exist.
fn retained_branch(
    shell: &dyn crate::shell::Shell,
    tree: &worktree::Linked,
    guard: Guard,
) -> Result<Option<crate::done::Branch>> {
    if !matches!(guard, Guard::Clean | Guard::StoppedOrphan { .. }) || tree.branch.is_empty() {
        return Ok(None);
    }
    let mut branch = crate::done::Branch {
        repo: tree.checkout.display().to_string(),
        name: tree.branch.clone(),
        ahead: 0,
        landed: false,
    };
    crate::done::refresh_branch(shell, &mut branch)?;
    Ok((!branch.landed).then_some(branch))
}

/// A shared checkout stays, but its finished child can release its own pane.
fn shared(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    pane: &str,
    holders: &[&crate::probe::Pane],
) -> Result<bool> {
    let items = crate::items::list(ledger)?;
    for other in holders.iter().filter(|p| p.pane_id != pane) {
        if other.agent.is_some()
            || items
                .iter()
                .any(|i| i.closed_ms.is_none() && i.pane == other.pane_id)
            || worktree::busy(shell, other)?.is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The same loss rule, with the local branch retained for automatic handoff.
fn unsaved(
    shell: &dyn crate::shell::Shell,
    ledger: &Ledger,
    tree: &worktree::Linked,
    caller: &Caller,
    guard: Guard,
) -> Result<crate::loss::Evidence> {
    let repo = tree.checkout.display().to_string();
    // A detached checkout has no branch. No branch is invented for it, so its commits must
    // be held by the default branch or a refreshed remote ref, exactly as for a branch.
    let detached = tree.branch.is_empty();
    let evidence = if guard == Guard::Clean {
        crate::loss::Evidence::fetch(shell, &repo)
    } else {
        let dirty = crate::loss::dirty_paths(shell, &tree.path, false)?;
        if !dirty.is_empty() {
            return Err(refuse(&format!(
                "{}: uncommitted changes: {}",
                tree.path,
                dirty.join(", ")
            )));
        }
        if detached {
            crate::loss::Evidence::fetch(shell, &repo)
        } else {
            crate::loss::Evidence::keeping_branch(&repo, &tree.branch)
        }
    };
    if let Some(warning) = &evidence.warning {
        eprintln!("{warning}");
    }
    let tip = if detached {
        worktree::head_sha(shell, &tree.path)?
    } else {
        format!("refs/heads/{}", tree.branch)
    };
    let loss = evidence.assess(shell, &tip, Some(&tree.path))?;
    if !loss.loses_nothing() {
        return Err(refuse(&format!("{}: {}", tree.path, loss.detail())));
    }
    crate::cleanup::close_holds(shell, ledger, tree, caller, guard, &loss)?;
    Ok(evidence)
}

/// Refuse a worktree outside the repository's worktree directory, or one the caller is in.
fn outside(tree: &worktree::Linked, caller: &Caller) -> Result<()> {
    let settings = crate::repository::load(&tree.checkout)?;
    let home = tree.checkout.join(&settings.worktree.dir);
    let path = worktree::resolved(Path::new(&tree.path));
    if !path.starts_with(worktree::resolved(&home)) {
        return Err(refuse(&format!(
            "{} is not under {}",
            tree.path,
            home.display()
        )));
    }
    if caller
        .cwd
        .as_deref()
        .is_some_and(|cwd| worktree::resolved(Path::new(cwd)).starts_with(&path))
    {
        return Err(refuse(&format!(
            "the caller is inside {}; run from the main checkout",
            tree.path
        )));
    }
    Ok(())
}
