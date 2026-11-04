//! Explicit disposal of retained child branches and bounded missing-mark reconciliation.

use crate::done::Branch;
use crate::error::{Error, Result};
use crate::hosts::Fleet;
use crate::items::{self, Item};
use crate::ledger::{Ledger, now_ms};
use crate::shell::Shell;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Dropped {
    pub branch: String,
    pub discarded: u64,
}

impl Dropped {
    pub fn line(&self) -> String {
        format!(
            "dropped {} ({} unmerged commits discarded)\n",
            self.branch, self.discarded
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checked {
    pub item: String,
    pub branch: Branch,
}

fn validate(item: &Item) -> Result<&Branch> {
    if item.kind != "agent" || item.closed_ms.is_none() {
        return Err(Error::Invalid(format!(
            "{} is not a closed agent item",
            item.id
        )));
    }
    item.branch
        .as_ref()
        .ok_or_else(|| Error::Invalid(format!("{} has no retained branch", item.id)))
}

/// Authorization belongs to the originating item, before host transport.
pub fn drop(fleet: &Fleet<'_>, id: &str, host: Option<&str>) -> Result<Dropped> {
    let item = items::list(fleet.ledger)?
        .into_iter()
        .find(|i| i.id == id)
        .ok_or_else(|| Error::Invalid(format!("unknown item {id}")))?;
    validate(&item)?;
    let caller = crate::inbox::own_pane(None)?;
    if caller != item.owner_pane && !orphaned(fleet, &item)? {
        return Err(Error::Invalid(format!(
            "only owner {} can drop {id} while it holds the child",
            item.owner_pane
        )));
    }
    let target = fleet.find(host.unwrap_or(&item.host))?;
    if target.id != item.host {
        return Err(Error::Invalid(format!(
            "{id} belongs to host {}",
            item.host
        )));
    }
    let dropped = if target.is_local() {
        local(fleet.shell, fleet.ledger, &item)?
    } else {
        fleet.remote_setup(
            target,
            &[
                "drop".into(),
                "--item".into(),
                serde_json::to_string(&item)?,
            ],
        )?
    };
    items::dropped(fleet.ledger, id, now_ms())?;
    Ok(dropped)
}

/// Any session may drop an orphan: the origin's custody evidence proves its custodian is
/// gone, by the rule the sweep uses. Unreadable or unprovable evidence keeps the owner rule.
fn orphaned(fleet: &Fleet<'_>, item: &Item) -> Result<bool> {
    let custody = match crate::custody::get(fleet.ledger, &item.id)? {
        Some(custody) => custody,
        None => crate::custody::ensure(fleet.ledger, item)?,
    };
    let (panes, session) = crate::custody::evidence(fleet.shell, &custody.custodian_pane);
    Ok(crate::custody::classify(&custody, &panes, &session) == crate::custody::Status::Orphaned)
}

fn git(shell: &dyn Shell, repo: &str, args: &[&str]) -> Result<crate::shell::Output> {
    shell.run(
        "git",
        &std::iter::once("-C".to_owned())
            .chain(std::iter::once(repo.to_owned()))
            .chain(args.iter().map(|s| (*s).to_owned()))
            .collect::<Vec<_>>(),
    )
}

fn git_ok(shell: &dyn Shell, repo: &str, args: &[&str]) -> Result<String> {
    let out = git(shell, repo, args)?;
    if !out.ok() {
        return Err(Error::Invalid(format!(
            "{repo}: git {}: {}",
            args.join(" "),
            out.stderr.trim()
        )));
    }
    Ok(out.stdout)
}

/// The host trusts the authorized origin request, as host close does.
pub fn local(shell: &dyn Shell, ledger: &Ledger, item: &Item) -> Result<Dropped> {
    let mut branch = validate(item)?.clone();
    if item.dropped_ms.is_some() {
        return Ok(Dropped {
            branch: branch.name,
            discarded: 0,
        });
    }
    git_ok(
        shell,
        &branch.repo,
        &["check-ref-format", "--branch", &branch.name],
    )?;
    let reference = format!("refs/heads/{}", branch.name);
    let trees = git_ok(shell, &branch.repo, &["worktree", "list", "--porcelain"])?;
    if trees
        .lines()
        .any(|line| line == format!("branch {reference}"))
    {
        return Err(Error::Invalid(format!(
            "branch {} is checked out in a worktree",
            branch.name
        )));
    }
    // Count against the origin as it is now, as close and done do: a stale default branch
    // would report landed commits as discarded.
    if let Some(warning) = crate::loss::Evidence::fetch(shell, &branch.repo).warning {
        eprintln!("{warning}");
    }
    crate::done::refresh_branch(shell, &mut branch)?;
    let discarded = branch.ahead;
    let exists = git(
        shell,
        &branch.repo,
        &["show-ref", "--verify", "--quiet", &reference],
    )?;
    match exists.status {
        0 => {
            // C1: adopt the shared repository lock when close and prune implement it.
            // Git's branch guard still refuses a concurrent checkout in any worktree.
            let deleted = git(shell, &branch.repo, &["branch", "-D", &branch.name])?;
            if !deleted.ok()
                && git(
                    shell,
                    &branch.repo,
                    &["show-ref", "--verify", "--quiet", &reference],
                )?
                .status
                    != 1
            {
                return Err(Error::Invalid(format!(
                    "cannot delete branch {} (it may be checked out): {}",
                    branch.name,
                    deleted.stderr.trim()
                )));
            }
        }
        1 => (),
        _ => {
            return Err(Error::Invalid(format!(
                "cannot inspect branch {}: {}",
                branch.name,
                exists.stderr.trim()
            )));
        }
    }
    delete_origin(shell, &branch)?;
    crate::done::dropped(ledger, item, now_ms())?;
    Ok(Dropped {
        branch: branch.name,
        discarded,
    })
}

/// Remove the remote branch only after local deletion passed Git's worktree guard.
fn delete_origin(shell: &dyn Shell, branch: &Branch) -> Result<()> {
    let reference = format!("refs/heads/{}", branch.name);
    let remotes = git_ok(shell, &branch.repo, &["remote"])?;
    if remotes.lines().any(|name| name == "origin") {
        let remote = git(
            shell,
            &branch.repo,
            &["ls-remote", "--exit-code", "origin", &reference],
        )?;
        match remote.status {
            0 => {
                let deleted = git(
                    shell,
                    &branch.repo,
                    &["push", "origin", "--delete", &branch.name],
                )?;
                if !deleted.ok()
                    && git(
                        shell,
                        &branch.repo,
                        &["ls-remote", "--exit-code", "origin", &reference],
                    )?
                    .status
                        != 2
                {
                    return Err(Error::Invalid(format!(
                        "cannot delete origin branch {}: {}",
                        branch.name,
                        deleted.stderr.trim()
                    )));
                }
            }
            2 => (),
            _ => {
                return Err(Error::Invalid(format!(
                    "cannot inspect origin branch {}: {}",
                    branch.name,
                    remote.stderr.trim()
                )));
            }
        }
    }
    // An already absent remote can still leave a stale tracking ref locally.
    git_ok(
        shell,
        &branch.repo,
        &[
            "update-ref",
            "-d",
            &format!("refs/remotes/origin/{}", branch.name),
        ],
    )?;
    Ok(())
}
