//! The stop gate: an owner about to end its turn hears once about its finished children with
//! branches ready to merge. A prompt line is easy to skip; a continued turn is not.
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

/// The findings of one cleanup pass about work that one agent created, as `id: detail` lines.
/// Findings about work that no agent created, or that another agent created, stay in
/// `dogsled status`; they never reach an agent that did not create the work. Repository
/// warnings and failures without a target are about the host, which no agent created.
#[derive(Debug, Default)]
pub struct Findings {
    pub failures: BTreeSet<String>,
    pub holds: BTreeSet<String>,
    pub landed_dirty: BTreeSet<String>,
}

pub fn findings(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    owner: &str,
    report: &crate::sweep::Report,
) -> Result<Findings> {
    let applied = &report.applied;
    if applied.failure_entries.is_empty()
        && applied.unlinked.is_empty()
        && applied.landed_dirty.is_empty()
    {
        return Ok(Findings::default());
    }
    let items = crate::items::list(ledger)?;
    let panes = crate::cleanup::current_panes(shell)?;
    let launches = crate::launch::requests(ledger)?;
    let custody = crate::custody::list(ledger)?;
    // A probe that cannot be read proves no session. The custody that names a session then
    // stays unproven, and the holder is no custodian of it.
    let session = match crate::probe::agent_list(shell) {
        Ok(agents) => agents
            .into_iter()
            .find(|agent| agent.pane == owner)
            .and_then(|agent| agent.agent_session),
        Err(_) => None,
    };
    let holder = Holder {
        pane: owner,
        session: session.as_deref(),
        records: &custody,
    };
    let recipient =
        |entry: &crate::cleanup::Entry| created(entry, &holder, &items, &panes, &launches);
    let failures = applied
        .failures
        .iter()
        .filter(|detail| {
            applied
                .failure_entries
                .iter()
                .any(|failure| &failure.detail == *detail && recipient(&failure.entry))
        })
        .cloned()
        .collect();
    let landed_dirty = applied
        .landed_dirty
        .iter()
        .filter(|entry| recipient(entry))
        .filter_map(|entry| {
            let tree = entry.tree.as_ref()?;
            Some(format!("{} (branch {}): its commits are already in the default branch. Dirty paths ({} total): {}", tree.path, tree.branch, entry.dirty_count.unwrap_or(entry.dirty_paths.len()), entry.dirty_paths.join(", ")))
        })
        .collect();
    Ok(Findings {
        failures,
        holds: entries(&applied.unlinked, &recipient),
        landed_dirty,
    })
}

pub fn notices(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    owner: &str,
) -> Result<Vec<Notice>> {
    let Some(report) = crate::sweep::latest(ledger)? else {
        return Ok(Vec::new());
    };
    let Findings {
        failures,
        holds,
        landed_dirty,
    } = findings(ledger, shell, owner, &report)?;
    let mut notices = Vec::new();
    if !failures.is_empty() {
        notices.push(Notice {
            key: "failures",
            signature: failures.iter().map(|s| (s.clone(), None, false)).collect(),
            text: format!("Cleanup pass at {} ms failed:\n{}\nRead `dogsled status` for the last pass. Hand the fix to a dogsled agent. Do not bypass cleanup safeguards.", report.finished_ms, failures.into_iter().collect::<Vec<_>>().join("\n")),
        });
    }
    if !holds.is_empty() {
        notices.push(Notice {
            key: "unlinked",
            signature: holds.iter().map(|s| (s.clone(), None, false)).collect(),
            text: format!("Cleanup holds worktrees that Git can no longer remove:\n{}\nThe owner decides what happens to each directory. Report it to the user; do not delete it yourself. Never use `git worktree remove --force`, `git clean`, or `rm -rf` on it, and do not edit {} by hand.", holds.into_iter().collect::<Vec<_>>().join("\n"), ledger.dir.join("worktrees.json").display()),
        });
    }
    if !landed_dirty.is_empty() {
        notices.push(Notice {
            key: "landed-dirty",
            signature: landed_dirty.iter().map(|s| (s.clone(), None, true)).collect(),
            text: format!("Landed worktrees still have dirty files:\n{}\nRead the dirty files. If they are wanted, commit them on a branch and land them. If the work is abandoned, ask the user before you discard it. Then run `dogsled close <pane>` or `dogsled prune worktrees`. Never use `git worktree remove --force`, `git clean`, or `rm -rf` on the checkout.", landed_dirty.into_iter().collect::<Vec<_>>().join("\n")),
        });
    }
    Ok(notices)
}

/// The entries addressed to this recipient, as `id: detail` lines.
fn entries(
    entries: &[crate::cleanup::Entry],
    recipient: &impl Fn(&crate::cleanup::Entry) -> bool,
) -> BTreeSet<String> {
    entries
        .iter()
        .filter(|entry| recipient(entry))
        .map(|e| format!("{}: {}", e.id, e.detail))
        .collect()
}

/// The agent that asks, and the custody records that say who answers for each item.
struct Holder<'a> {
    pane: &'a str,
    session: Option<&'a str>,
    records: &'a [crate::custody::Custody],
}

impl Holder<'_> {
    /// The agent answers for `item` now. The current custodian does, found by pane and by
    /// session, so a reused pane id never inherits the custody. A custody that names a
    /// session needs a live session that is readable and equal: a session nobody can read
    /// proves nothing, so the custody is not held then. An item with no custody record is
    /// still held by its origin owner.
    fn answers_for(&self, item: &crate::items::Item) -> bool {
        let custodian = crate::inbox::custodian_of(self.records, item);
        crate::inbox::holds(&custodian, self.pane, self.session)
    }

    /// The agent answers for the launch. The launch names the child it started, and the
    /// child's current custodian answers for it. A launch with no child record keeps its
    /// own owner.
    fn answers_for_launch(
        &self,
        launch: &crate::work::Launch,
        items: &[crate::items::Item],
    ) -> bool {
        match items.iter().find(|item| item.id == launch.id) {
            Some(item) => self.answers_for(item),
            None => launch.owner == self.pane,
        }
    }
}

/// An entry concerns only the agent that created its work: the custodian of a live local item
/// whose pane holds the entry, or the requester of a launch into the entry's worktree.
/// An agent that created nothing there hears nothing, whether Dogsled launched it or not.
fn created(
    entry: &crate::cleanup::Entry,
    holder: &Holder<'_>,
    items: &[crate::items::Item],
    panes: &[crate::probe::Pane],
    launches: &[crate::work::Launch],
) -> bool {
    let tree = entry
        .tree
        .as_ref()
        .map(|tree| tree.path.as_str())
        .or_else(|| (entry.class == "worktrees").then_some(entry.id.as_str()))
        .filter(|path| !path.is_empty());
    let holders: Vec<_> = match entry.class.as_str() {
        "panes" => panes
            .iter()
            .filter(|pane| pane.pane_id == entry.id)
            .collect(),
        "workspaces" => panes
            .iter()
            .filter(|pane| pane.workspace_id.as_deref() == Some(entry.id.as_str()))
            .collect(),
        _ => tree
            .map(|path| crate::worktree::holders(panes, path, None))
            .unwrap_or_default(),
    };
    let holds = items
        .iter()
        .filter(|item| item.closed_ms.is_none() && item.host == crate::hosts::LOCAL)
        .filter(|item| holder.answers_for(item))
        .any(|item| holders.iter().any(|pane| pane.pane_id == item.pane));
    let requested = tree.is_some_and(|path| {
        launches
            .iter()
            .filter(|launch| launch.host == crate::hosts::LOCAL)
            .filter(|launch| holder.answers_for_launch(launch, items))
            .any(|launch| crate::worktree::same(&launch.cwd, path))
    });
    holds || requested
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
            "Before you stop: your finished children have branches ready to merge: {names}. \
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
            owner_pane: None,
            title: None,
            cleanup: None,
            unseen: false,
            state: match group {
                Group::ToLand => "done",
                Group::Running => "working",
                Group::Attention => "blocked",
                Group::Waiting => "queued",
            }
            .into(),
            activity: String::new(),
            age_ms: 0,
            quiet_ms: None,
            ahead,
            dirty,
            repository: None,
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
            text.contains("branches ready to merge: web (+2), api (uncommitted)."),
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
    fn finishing_and_waiting_children_with_commits_do_not_trigger_the_gate() {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        for state in [
            crate::lifecycle::State::Finishing,
            crate::lifecycle::State::Waiting,
        ] {
            let mut child = row("busy", Group::Running, Some(3), false);
            child.state = crate::lifecycle::display(state).into();
            let mut working = board(vec![child]);
            let owner = state.as_str();
            assert_eq!(reminder(&ledger, &working, owner).unwrap(), None);
            working.rows[0].ahead = Some(4);
            working.rows[0].dirty = true;
            assert_eq!(reminder(&ledger, &working, owner).unwrap(), None);

            working
                .rows
                .push(row("ready", Group::ToLand, Some(2), false));
            let text = reminder(&ledger, &working, owner).unwrap().unwrap();
            assert!(
                text.contains("branches ready to merge: ready (+2)."),
                "{text}"
            );
            assert!(!text.contains("busy"), "{text}");
            // Changes to WORKING rows do not change the reminded set.
            working.rows[0].ahead = Some(5);
            assert_eq!(reminder(&ledger, &working, owner).unwrap(), None);
            working.rows[1].ahead = Some(3);
            assert!(reminder(&ledger, &working, owner).unwrap().is_some());
        }
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

    /// A child of `owner`.
    fn item(id: &str, owner: &str) -> crate::items::Item {
        crate::items::Item {
            id: id.into(),
            kind: "agent".into(),
            host: crate::hosts::LOCAL.into(),
            pane: "w1:p3".into(),
            owner_pane: owner.into(),
            name: Some(id.into()),
            created_ms: 1,
            closed_ms: None,
            ..crate::items::Item::default()
        }
    }

    fn custody(
        item: &crate::items::Item,
        pane: &str,
        session: Option<&str>,
    ) -> crate::custody::Custody {
        crate::custody::Custody {
            item: item.id.clone(),
            host: crate::hosts::LOCAL.into(),
            owner_pane: item.owner_pane.clone(),
            owner_session: None,
            custodian_pane: pane.into(),
            custodian_session: session.map(str::to_owned),
            revision: 1,
            claims: Vec::new(),
        }
    }

    /// A pane list that holds the child's pane, in a worktree. The agents in `w1:p1` and
    /// `w1:p2` run the sessions the names say.
    fn shell_in_tree(sessions: &[(&str, &str)]) -> crate::shell::Fake {
        let shell = crate::shell::Fake::new();
        shell.on(
            "herdr pane list",
            r#"{"result":{"panes":[{"pane_id":"w1:p3","cwd":"/t"}]}}"#,
        );
        let agents: Vec<serde_json::Value> = sessions
            .iter()
            .map(|(pane, session)| {
                serde_json::json!({"pane_id":pane,"agent":"pi",
                    "agent_session":{"kind":"path","value":session}})
            })
            .collect();
        shell.on(
            "herdr agent list",
            &serde_json::json!({"result":{"agents":agents}}).to_string(),
        );
        shell
    }

    /// The entry of a worktree the child's pane holds.
    fn worktree_entry() -> crate::cleanup::Entry {
        crate::cleanup::Entry {
            class: "worktrees".into(),
            id: "/t".into(),
            detail: "the worktree cannot be removed".into(),
            removable: true,
            tree: Some(crate::cleanup::Tree {
                repo: "/repo".into(),
                path: "/t".into(),
                branch: "feat/x".into(),
                head: "h".into(),
                identity: None,
            }),
            kind: None,
            dirty_paths: Vec::new(),
            dirty_count: None,
        }
    }

    /// The findings of one failed pass, for the agent that asks.
    fn findings_for(ledger: &Ledger, shell: &crate::shell::Fake, owner: &str) -> Findings {
        let report = crate::sweep::Report {
            ts_ms: 1,
            started_ms: 1,
            finished_ms: 2,
            applied: crate::cleanup::Applied {
                results: Vec::new(),
                failures: vec!["the worktree cannot be removed".into()],
                skipped: Vec::new(),
                failure_entries: vec![crate::cleanup::Failure {
                    entry: worktree_entry(),
                    detail: "the worktree cannot be removed".into(),
                }],
                repositories: Vec::new(),
                landed_dirty: Vec::new(),
                unlinked: Vec::new(),
            },
        };
        findings(ledger, shell, owner, &report).unwrap()
    }

    #[test]
    fn the_gate_finds_the_work_of_the_current_custodian() {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let child = item("child", "w1:p1");
        crate::items::record(&ledger, child.clone()).unwrap();
        // The custody moved to another pane: the origin owner hears nothing about it.
        ledger
            .write(
                "custody.json",
                &vec![custody(&child, "w1:p2", Some("custodian-session"))],
            )
            .unwrap();
        let shell = shell_in_tree(&[("w1:p1", "origin-session"), ("w1:p2", "custodian-session")]);
        assert!(
            findings_for(&ledger, &shell, "w1:p1").failures.is_empty(),
            "the origin owner must hear nothing"
        );
        assert_eq!(findings_for(&ledger, &shell, "w1:p2").failures.len(), 1);
        // Another session in the custodian's pane is another agent: it hears nothing.
        let reused = shell_in_tree(&[("w1:p1", "origin-session"), ("w1:p2", "another-session")]);
        assert!(
            findings_for(&ledger, &reused, "w1:p2").failures.is_empty(),
            "a reused pane id must not inherit the custody"
        );
        std::fs::remove_dir_all(&ledger.dir).unwrap();
    }

    #[test]
    fn the_gate_finds_the_work_of_the_origin_owner_without_a_custody_record() {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        crate::items::record(&ledger, item("child", "w1:p1")).unwrap();
        let shell = shell_in_tree(&[("w1:p1", "origin-session"), ("w1:p2", "other-session")]);
        // No custody record: the origin owner answers for the child.
        assert_eq!(findings_for(&ledger, &shell, "w1:p1").failures.len(), 1);
        assert!(
            findings_for(&ledger, &shell, "w1:p2").failures.is_empty(),
            "another pane must hear nothing"
        );
        std::fs::remove_dir_all(&ledger.dir).unwrap();
    }
}
