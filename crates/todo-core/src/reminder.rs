//! Completion policy derived only from branch facts.

use serde::{Deserialize, Serialize};

use crate::TodoList;

/// Default from oh-my-pi e7fe318, `packages/coding-agent/src/tools/settings.ts`,
/// `cfgTodoRemindersMax` (`todo.remindersMax`).
/// <https://github.com/can1357/oh-my-pi/blob/e7fe318/packages/coding-agent/src/tools/settings.ts>
const REMINDERS_MAX: usize = 3;

/// Relevant facts in branch order, including proposed boundary entries.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(tag = "kind", content = "list", rename_all = "snake_case")]
pub enum ReminderFact {
    User,
    Snapshot(TodoList),
    Reminder,
}

/// Branch facts supplied to the reminder boundary.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct ReminderHistory {
    pub facts: Vec<ReminderFact>,
}

/// Returns the next reminder, if open tasks and the per-prompt budget allow it.
///
/// Repeated snapshots do not count as progress. A new user prompt resets both
/// the budget and the progress guard, including after resume or tree navigation.
#[must_use]
pub fn completion_reminder(facts: &[ReminderFact]) -> Option<String> {
    let mut latest = TodoList::default();
    let mut reminders = 0;
    let mut awaiting_progress = false;
    for fact in facts {
        match fact {
            ReminderFact::User => {
                reminders = 0;
                awaiting_progress = false;
            }
            ReminderFact::Snapshot(list) => {
                if *list != latest {
                    awaiting_progress = false;
                }
                latest = list.clone();
            }
            ReminderFact::Reminder => {
                reminders += 1;
                awaiting_progress = true;
            }
        }
    }
    if awaiting_progress || reminders >= REMINDERS_MAX {
        return None;
    }
    let open: Vec<String> = latest
        .tasks
        .iter()
        .filter(|task| task.status.is_open())
        .flat_map(|task| {
            let review = if task.ready_for_review() {
                " · Ready for review: complete after explicit integration review"
            } else {
                ""
            };
            std::iter::once(format!("{}{review}", task.content)).chain(
                task.subtasks
                    .iter()
                    .filter(|child| child.status.is_open())
                    .map(|child| format!("{} → {}", task.content, child.content)),
            )
        })
        .collect();
    if open.is_empty() {
        return None;
    }
    Some(format!(
        "<system-reminder>\nYou stopped with {} incomplete todo item(s):\n- {}\n\nContinue working on these tasks or mark them complete if finished.\n(Reminder {}/{REMINDERS_MAX})\n</system-reminder>",
        open.len(),
        open.join("\n- "),
        reminders + 1,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Status, init};

    fn snapshot(items: &[&str]) -> ReminderFact {
        ReminderFact::Snapshot(init(items.iter().map(|item| (*item).to_owned()).collect()).unwrap())
    }

    #[test]
    fn reminds_only_for_open_work() {
        assert_eq!(completion_reminder(&[]), None, "empty branch");
        let text = completion_reminder(&[ReminderFact::User, snapshot(&["a", "b"])]).unwrap();
        assert!(text.contains("- a\n- b"), "lists every open task");
        assert!(text.contains("Reminder 1/3"), "omp default cap");
        let closed = TodoList {
            tasks: vec![
                crate::Task {
                    content: "done".to_owned(),
                    description: None,
                    status: Status::Completed,
                    subtasks: vec![],
                    unit: None,
                },
                crate::Task {
                    content: "dropped".to_owned(),
                    description: None,
                    status: Status::Abandoned,
                    subtasks: vec![],
                    unit: None,
                },
            ],
        };
        assert_eq!(
            completion_reminder(&[ReminderFact::Snapshot(closed)]),
            None,
            "closed tasks excluded"
        );
    }

    #[test]
    fn requires_a_change_after_the_last_reminder() {
        let mut facts = vec![ReminderFact::User, snapshot(&["a"]), ReminderFact::Reminder];
        assert_eq!(completion_reminder(&facts), None, "no action");
        facts.push(snapshot(&["a"]));
        assert_eq!(
            completion_reminder(&facts),
            None,
            "idempotent writes are not progress"
        );
        facts.push(snapshot(&["a", "b"]));
        assert!(
            completion_reminder(&facts)
                .unwrap()
                .contains("Reminder 2/3"),
            "change re-arms"
        );
    }

    #[test]
    fn budget_survives_replay_and_resets_at_user_message() {
        let mut facts = vec![ReminderFact::User];
        for task in ["a", "b", "c"] {
            facts.push(snapshot(&[task]));
            assert!(completion_reminder(&facts).is_some(), "within budget");
            facts.push(ReminderFact::Reminder);
        }
        facts.push(snapshot(&["d"]));
        assert_eq!(
            completion_reminder(&facts),
            None,
            "three reminders exhausted"
        );
        facts.push(ReminderFact::User);
        assert!(
            completion_reminder(&facts)
                .unwrap()
                .contains("Reminder 1/3"),
            "new prompt"
        );
    }
}
