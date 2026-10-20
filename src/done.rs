//! A child's own word that its task is complete. An idle agent only waits for input: it may
//! have asked a question, or it may wait for its own children. Only a `done` mark lets dogsled
//! close an agent, or remove the checkout it works in, without being asked.
//!
//! The mark lives on the agent's host, keyed by its Herdr terminal. It covers the turn that
//! called `done`. The first completion after the mark seals it. A later turn, or an inbox record
//! delivered to the agent, ends it, so a child that gets more work must call `done` again.

use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::probe::Agent;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const FILE: &str = "done.json";

/// What dogsled tells each child it starts, as one line: Herdr refuses an argument with a
/// newline.
pub const CONTRACT: &str = "Dogsled started you as a child agent. When your whole task is \
complete, call the dogsled `done` tool (or run `dogsled done`) once, then end your turn with \
your final reply. Do not call `done` when you stop to ask a question, wait for input, or still \
own open children. Dogsled closes your pane and worktree only after `done`.";

/// One agent's `done`, for the terminal that holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    pub pane: String,
    /// Herdr's state counter when `done` ran. The completion of that turn has a higher number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<u64>,
    /// The completion of the turn that called `done`, once it is seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed: Option<u64>,
    pub marked_ms: u64,
}

/// Marks by Herdr terminal id.
type Marks = BTreeMap<String, Mark>;

fn waiting(agent: &Agent) -> bool {
    matches!(agent.state.as_str(), "idle" | "done")
}

/// `mark` against the agent now in its terminal. `None` ends it: a new turn started after the
/// sealed one.
fn judge(mark: &Mark, agent: &Agent) -> Option<Mark> {
    if matches!(agent.state.as_str(), "working" | "blocked") {
        return mark.sealed.is_none().then(|| mark.clone());
    }
    let Some(seq) = agent.completion_seq.filter(|_| waiting(agent)) else {
        return Some(mark.clone());
    };
    match mark.sealed {
        Some(sealed) => (seq == sealed).then(|| mark.clone()),
        None if mark.after.is_none_or(|after| seq > after) => Some(Mark {
            sealed: Some(seq),
            ..mark.clone()
        }),
        None => Some(mark.clone()),
    }
}

/// The panes that own open items or queued tickets on this host: they still wait for children.
fn owners(ledger: &Ledger) -> Result<BTreeMap<String, Vec<String>>> {
    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for item in crate::items::list(ledger)?
        .into_iter()
        .filter(|i| i.closed_ms.is_none())
    {
        owners
            .entry(item.owner_pane.clone())
            .or_default()
            .push(item.name.unwrap_or(item.pane));
    }
    for ticket in crate::work::queue(ledger)?
        .into_iter()
        .filter(|t| matches!(t.state.as_str(), "pending" | "admitting"))
    {
        owners
            .entry(ticket.request.owner.clone())
            .or_default()
            .push(format!("queued {}", ticket.id));
    }
    Ok(owners)
}

/// The marks that still hold, and the panes whose agents are done: sealed, still in that
/// completion, and owning no open work.
fn verdicts(ledger: &Ledger, marks: &Marks, agents: &[Agent]) -> Result<(Marks, BTreeSet<String>)> {
    let owners = owners(ledger)?;
    let mut kept = Marks::new();
    let mut done = BTreeSet::new();
    for (terminal, mark) in marks {
        let Some(agent) = agents
            .iter()
            .find(|a| a.terminal_id.as_deref() == Some(terminal.as_str()))
        else {
            continue;
        };
        let Some(mark) = judge(mark, agent) else {
            continue;
        };
        if mark.sealed.is_some()
            && waiting(agent)
            && agent.completion_seq == mark.sealed
            && !owners.contains_key(&agent.pane)
        {
            done.insert(agent.pane.clone());
        }
        kept.insert(terminal.clone(), mark);
    }
    Ok((kept, done))
}

/// The service's pass: seal and end marks, and say on each agent whether it is done.
pub fn observe(ledger: &Ledger, agents: &mut [Agent]) -> Result<()> {
    ledger.with_lock("done", || {
        let marks: Marks = ledger.read(FILE)?.unwrap_or_default();
        let (kept, done) = verdicts(ledger, &marks, agents)?;
        for agent in agents.iter_mut() {
            agent.done = done.contains(&agent.pane);
        }
        if kept != marks {
            ledger.write(FILE, &kept)?;
        }
        Ok(())
    })
}

/// The panes on this host whose agents are done, judged against `agents`, a fresh list.
pub fn finished(ledger: &Ledger, agents: &[Agent]) -> Result<BTreeSet<String>> {
    let marks: Marks = ledger.read(FILE)?.unwrap_or_default();
    Ok(verdicts(ledger, &marks, agents)?.1)
}

/// `dogsled done`: the agent in `pane` says its task is complete. Refused while it owns open
/// children or queued work, because their replies would reach no one.
pub fn mark(
    ledger: &Ledger,
    shell: &dyn crate::shell::Shell,
    pane: &str,
    now: u64,
) -> Result<Mark> {
    let agent = crate::inbox::local_agent(shell, pane)?;
    if waiting(&agent) {
        return Err(Error::Invalid(format!(
            "{pane} is {}; call done during the turn that completes your task",
            agent.state
        )));
    }
    let terminal = agent.terminal_id.clone().ok_or_else(|| {
        Error::Invalid(format!(
            "Herdr reports no terminal id for {pane}; upgrade Herdr"
        ))
    })?;
    if let Some(open) = owners(ledger)?.get(pane) {
        return Err(Error::Invalid(format!(
            "{pane} still owns open work: {}. Wait for each child's reply, land or close it, \
             then call done",
            open.join(", ")
        )));
    }
    // The agent calls `done` during its turn. The next completion after this state seals it.
    let mark = Mark {
        pane: pane.into(),
        after: agent.state_change_seq,
        sealed: None,
        marked_ms: now,
    };
    ledger.with_lock("done", || {
        let mut marks: Marks = ledger.read(FILE)?.unwrap_or_default();
        marks.insert(terminal, mark.clone());
        ledger.write(FILE, &marks)
    })?;
    Ok(mark)
}

/// End `terminal`'s mark: it received work after it called `done`.
pub fn clear(ledger: &Ledger, terminal: &str) -> Result<()> {
    ledger.with_lock("done", || {
        let mut marks: Marks = ledger.read(FILE)?.unwrap_or_default();
        if marks.remove(terminal).is_some() {
            ledger.write(FILE, &marks)?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(state: &str, completion: Option<u64>, change: u64) -> Agent {
        Agent {
            pane: "w2:p1".into(),
            agent: Some("pi".into()),
            state: state.into(),
            terminal_id: Some("term_child".into()),
            completion_seq: completion,
            state_change_seq: Some(change),
            ..Agent::default()
        }
    }

    fn ledger() -> Ledger {
        let ledger = Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        std::fs::create_dir_all(&ledger.dir).unwrap();
        ledger
    }

    fn put(ledger: &Ledger, mark: &Mark) {
        ledger
            .write(
                FILE,
                &Marks::from([("term_child".to_owned(), mark.clone())]),
            )
            .unwrap();
    }

    fn pass(ledger: &Ledger, agent: Agent) -> bool {
        let mut agents = [agent];
        observe(ledger, &mut agents).unwrap();
        agents[0].done
    }

    fn mark_at(after: u64) -> Mark {
        Mark {
            pane: "w2:p1".into(),
            after: Some(after),
            sealed: None,
            marked_ms: 1,
        }
    }

    #[test]
    fn an_idle_agent_without_a_mark_is_not_done() {
        let ledger = ledger();
        assert!(!pass(&ledger, agent("idle", Some(7), 7)));
        assert!(!pass(&ledger, agent("done", Some(7), 7)));
    }

    #[test]
    fn the_turn_that_called_done_seals_it_and_a_later_turn_ends_it() {
        let ledger = ledger();
        // `done` ran mid-turn, at state 10.
        put(&ledger, &mark_at(10));
        assert!(!pass(&ledger, agent("working", None, 10)));
        // That turn completes: the agent is done, and stays done while it waits.
        assert!(pass(&ledger, agent("idle", Some(12), 12)));
        assert!(pass(&ledger, agent("done", Some(12), 12)));
        // A new turn starts: the mark ends, and its completion is not done.
        assert!(!pass(&ledger, agent("working", None, 13)));
        assert!(!pass(&ledger, agent("idle", Some(15), 15)));
        assert_eq!(ledger.read::<Marks>(FILE).unwrap(), Some(Marks::new()));
    }

    #[test]
    fn a_question_after_done_is_a_new_completion_and_ends_the_mark() {
        let ledger = ledger();
        put(
            &ledger,
            &Mark {
                sealed: Some(12),
                ..mark_at(10)
            },
        );
        // A later completion the service never saw working: not the sealed one.
        assert!(!pass(&ledger, agent("idle", Some(20), 20)));
    }

    #[test]
    fn an_agent_that_owns_open_children_is_not_done() {
        let ledger = ledger();
        put(
            &ledger,
            &Mark {
                sealed: Some(12),
                ..mark_at(10)
            },
        );
        crate::items::record(
            &ledger,
            crate::items::Item {
                id: "c".into(),
                kind: "agent".into(),
                host: "local".into(),
                pane: "w3:p1".into(),
                owner_pane: "w2:p1".into(),
                ..crate::items::Item::default()
            },
        )
        .unwrap();
        assert!(!pass(&ledger, agent("idle", Some(12), 12)));
        crate::items::close(&ledger, "local", "w3:p1", 2).unwrap();
        assert!(pass(&ledger, agent("idle", Some(12), 12)));
    }

    #[test]
    fn a_new_terminal_in_the_same_pane_drops_the_mark() {
        let ledger = ledger();
        put(
            &ledger,
            &Mark {
                sealed: Some(12),
                ..mark_at(10)
            },
        );
        let mut other = agent("idle", Some(12), 12);
        other.terminal_id = Some("term_new".into());
        assert!(!pass(&ledger, other));
        assert_eq!(ledger.read::<Marks>(FILE).unwrap(), Some(Marks::new()));
    }
}
