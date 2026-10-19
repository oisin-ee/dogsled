//! The stop gate: an owner about to end its turn hears once about its finished children with
//! work to land. A prompt line is easy to skip; a continued turn is not.
//!
//! Each owner's reminded sets live in `gate.json`. The gate speaks once per distinct set,
//! even when children change back to an earlier state. Closing all owner items clears that history.

use crate::board::{Board, Group};
use crate::error::Result;
use crate::ledger::Ledger;
use std::collections::{BTreeMap, BTreeSet};

const FILE: &str = "gate.json";

/// A cleanup reminder has its own history, independent of the owner's open children.
#[derive(Debug)]
pub struct Notice {
    key: &'static str,
    signature: Signature,
    text: String,
}

pub fn notices(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    owner: &str,
) -> Result<Vec<Notice>> {
    let Some(report) = crate::sweep::latest(ledger)? else {
        return Ok(Vec::new());
    };
    let mut notices = Vec::new();

    if !report.applied.failures.is_empty() {
        let failures: BTreeSet<_> = report.applied.failures.iter().cloned().collect();
        notices.push(Notice {
            key: "failures",
            signature: failures.iter().map(|s| (s.clone(), None, false)).collect(),
            text: format!("Cleanup pass at {} ms failed:\n{}\nRead `dogsled status` for the last pass. Hand the fix to a dogsled agent. Do not bypass cleanup safeguards.", report.finished_ms, failures.into_iter().collect::<Vec<_>>().join("\n")),
        });
    }
    if !report.applied.repositories.is_empty() {
        let warnings: BTreeSet<_> = report
            .applied
            .repositories
            .iter()
            .map(|e| format!("{}: {}", e.id, e.detail))
            .collect();
        notices.push(Notice {
            key: "repositories",
            signature: warnings.iter().map(|s| (s.clone(), None, false)).collect(),
            text: format!("Cleanup repository warnings:\n{}\nCheck the repository entries in {} and {}, or cleanup.repositories in the configuration. Hand the fix to a dogsled agent: `dogsled agent --profile general --name <name> --message-file <brief>`. Name the repository and its state file entry in the brief. Do not edit the state by hand.", warnings.into_iter().collect::<Vec<_>>().join("\n"), ledger.dir.join("repositories.json").display(), ledger.dir.join("worktrees.json").display()),
        });
    }
    if report.applied.landed_dirty.is_empty() {
        return Ok(notices);
    }
    let panes = crate::cleanup::current_panes(shell)?;
    let root = panes
        .iter()
        .find(|p| p.pane_id == owner)
        .and_then(|p| p.foreground_cwd.as_deref().or(p.cwd.as_deref()))
        .map(|path| crate::cleanup::main_root(shell, path))
        .transpose()?
        .flatten()
        .and_then(|root| std::fs::canonicalize(root).ok());
    let mut findings = BTreeSet::new();
    for entry in &report.applied.landed_dirty {
        let Some(tree) = &entry.tree else {
            continue;
        };
        let repo = std::fs::canonicalize(&tree.repo).ok();
        if root.is_none() || root != repo {
            continue;
        }
        findings.insert(format!("{} (branch {}): its commits are already in the default branch. Dirty paths ({} total): {}", tree.path, tree.branch, entry.dirty_count.unwrap_or(entry.dirty_paths.len()), entry.dirty_paths.join(", ")));
    }
    if !findings.is_empty() {
        notices.push(Notice {
            key: "landed-dirty",
            signature: findings.iter().map(|s| (s.clone(), None, true)).collect(),
            text: format!("Landed worktrees still have dirty files:\n{}\nRead the dirty files. If they are wanted, commit them on a branch and land them. If the work is abandoned, ask the user before you discard it. Then run `dogsled close <pane>` or `dogsled prune worktrees`. Never use `git worktree remove --force`, `git clean`, or `rm -rf` on the checkout.", findings.into_iter().collect::<Vec<_>>().join("\n")),
        });
    }
    Ok(notices)
}

/// The children to land, as one comparable value: each item with its commits and dirt.
type Signature = BTreeSet<(String, Option<u64>, bool)>;

fn signature(board: &Board) -> Signature {
    board
        .rows
        .iter()
        .filter(|r| r.group == Group::ToLand)
        .map(|r| (r.id.clone(), r.ahead, r.dirty))
        .collect()
}

/// Deliver a reminder, then record it. The same set never blocks twice, even across process
/// restarts. A failed output write leaves the set eligible for the next stop.
pub fn deliver(
    ledger: &Ledger,
    board: &Board,
    owner: &str,
    notices: &[Notice],
    emit: impl FnOnce(Option<&str>) -> Result<()>,
) -> Result<()> {
    ledger.with_lock("gate", || {
        let mut heard: BTreeMap<String, BTreeSet<Signature>> = ledger.read(FILE)?.unwrap_or_default();
        let current = signature(board);
        let mut messages = Vec::new();
        for notice in notices {
            let sets = heard.entry(format!("cleanup:{}:{owner}", notice.key)).or_default();
            if sets.insert(notice.signature.clone()) {
                messages.push(notice.text.clone());
            }
        }
        if board.rows.is_empty() { heard.remove(owner); }
        if let Some(names) = board.land_names()
            && heard.entry(owner.to_owned()).or_default().insert(current)
        {
            messages.push(format!(
            "Before you stop: your finished children still have work to land: {names}. \
             Review each child's result with `dogsled read`. \
             Ask children with uncommitted changes to finish and commit their work. \
             Integrate approved commits. Close a clean child only when you intend to drop its work. \
             Do not discard uncommitted changes or close a working child. \
             If work must wait, report the blocker or deferral to the user. \
             This reminder appears once for this set of children and commit/dirty states."
            ));
        }
        let text = messages.join("\n\n");
        emit((!text.is_empty()).then_some(text.as_str()))?;
        ledger.write(FILE, &heard)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Row;

    fn reminder(ledger: &Ledger, board: &Board, owner: &str) -> Result<Option<String>> {
        let mut text = None;
        deliver(ledger, board, owner, &[], |value| {
            text = value.map(str::to_owned);
            Ok(())
        })?;
        Ok(text)
    }

    fn row(id: &str, group: Group, ahead: Option<u64>, dirty: bool) -> Row {
        Row {
            group,
            kind: "agent".into(),
            id: id.into(),
            name: id.into(),
            what: "pi".into(),
            host: "local".into(),
            pane: None,
            state: "idle".into(),
            activity: String::new(),
            age_ms: 0,
            quiet_ms: None,
            ahead,
            dirty,
        }
    }

    fn board(rows: Vec<Row>) -> Board {
        Board {
            hosts: Vec::new(),
            rows,
        }
    }

    #[test]
    fn the_gate_names_unlanded_children_once_per_set_and_per_owner() {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let first = board(vec![
            row("web", Group::ToLand, Some(2), false),
            row("api", Group::ToLand, None, true),
            row("busy", Group::Running, Some(1), true),
        ]);
        let text = reminder(&ledger, &first, "w1:p1").unwrap().unwrap();
        assert!(
            text.contains("work to land: web (+2), api (uncommitted)."),
            "{text}"
        );
        assert!(!text.contains("busy"), "{text}");
        // The same children: the turn ends. Another owner hears about its own.
        assert_eq!(reminder(&ledger, &first, "w1:p1").unwrap(), None);
        assert!(reminder(&ledger, &first, "w2:p1").unwrap().is_some());
        // A new commit changes the set, so the owner hears again.
        let more = board(vec![
            row("web", Group::ToLand, Some(3), false),
            row("api", Group::ToLand, None, true),
        ]);
        assert!(reminder(&ledger, &more, "w1:p1").unwrap().is_some());
        // Returning to a previous set does not start a continuation loop.
        assert_eq!(reminder(&ledger, &first, "w1:p1").unwrap(), None);
        // Reordering rows is not a new set.
        let mut reversed = first.clone();
        reversed.rows.reverse();
        assert_eq!(reminder(&ledger, &reversed, "w1:p1").unwrap(), None);
        // A child resuming work does not reset deduplication.
        let working = board(vec![row("web", Group::Running, Some(3), false)]);
        assert_eq!(reminder(&ledger, &working, "w1:p1").unwrap(), None);
        assert_eq!(reminder(&ledger, &more, "w1:p1").unwrap(), None);
        // No open owner items forgets the set; new work is named again.
        assert_eq!(
            reminder(&ledger, &board(Vec::new()), "w1:p1").unwrap(),
            None
        );
        assert!(reminder(&ledger, &more, "w1:p1").unwrap().is_some());
        // A failed delivery must not consume the reminder.
        assert!(
            deliver(&ledger, &first, "failed", &[], |_| {
                Err(crate::error::Error::Invalid("write failed".into()))
            })
            .is_err()
        );
        assert!(reminder(&ledger, &first, "failed").unwrap().is_some());
        std::fs::remove_dir_all(&ledger.dir).unwrap();
    }

    #[test]
    fn hidden_children_still_change_the_signature() {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let mut many = board(
            (0..6)
                .map(|i| row(&format!("child-{i}"), Group::ToLand, None, true))
                .collect(),
        );
        let text = reminder(&ledger, &many, "owner").unwrap().unwrap();
        assert!(text.contains("and 1 more (see `dogsled board`)"));
        assert!(!text.contains("child-5"));
        many.rows[5].ahead = Some(1);
        assert!(reminder(&ledger, &many, "owner").unwrap().is_some());
        std::fs::remove_dir_all(&ledger.dir).unwrap();
    }
}
