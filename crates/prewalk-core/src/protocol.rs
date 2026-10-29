//! Branch replay and one-shot handoff rules.

use crate::types::{Action, Decision, Event, Facts, Flags, Projection, Record, Selection, State};

const PLAN: &str = include_str!("../prompts/plan.md");
const CONTINUE: &str = include_str!("../prompts/continue.md");
const CHECKLIST: &str = include_str!("../prompts/checklist.md");

struct Cycle<'a> {
    state: Option<&'a State>,
    tail: &'a [Record],
}

fn replay(branch: &[Record]) -> Cycle<'_> {
    let mut latest = None;
    let mut start = 0;
    for (index, entry) in branch.iter().enumerate().rev() {
        if let Record::State { state } = entry {
            if latest.is_some_and(|current| !same_cycle(current, state)) {
                break;
            }
            latest.get_or_insert(state);
            start = index + 1;
        }
    }
    Cycle {
        state: latest,
        tail: branch.get(start..).unwrap_or_default(),
    }
}

fn same_cycle(left: &State, right: &State) -> bool {
    match (left, right) {
        (
            State::Arm {
                planner: a,
                target: b,
                ..
            },
            State::Arm {
                planner: c,
                target: d,
                ..
            },
        )
        | (
            State::Handoff {
                planner: a,
                target: b,
                ..
            },
            State::Handoff {
                planner: c,
                target: d,
                ..
            },
        ) => a == c && b == d && enabled(left) != enabled(right),
        _ => false,
    }
}

const fn enabled(state: &State) -> bool {
    match state {
        State::Arm { enabled, .. } | State::Handoff { enabled, .. } | State::Off { enabled } => {
            *enabled
        }
    }
}

fn prompt(name: &str, content: &str) -> Action {
    Action::Prompt {
        name: name.into(),
        content: content.into(),
    }
}

fn nudge(tail: &[Record]) -> Option<&str> {
    tail.iter().find_map(|record| match record {
        Record::Prompt { id, name } if name == "prewalk-plan" => Some(id.as_str()),
        Record::State { .. }
        | Record::Prompt { .. }
        | Record::Calls { .. }
        | Record::Scrub { .. }
        | Record::Selection
        | Record::User => None,
    })
}

fn scrub(tail: &[Record]) -> Vec<Action> {
    nudge(tail)
        .map(|id| Action::Scrub { id: id.into() })
        .into_iter()
        .collect()
}

fn off(facts: &Facts, cycle: &Cycle<'_>) -> Decision {
    let mut actions = scrub(cycle.tail);
    actions.push(Action::Store {
        state: State::Off {
            enabled: facts.report.config.enabled,
        },
    });
    Decision {
        actions,
        again: false,
    }
}

fn notice(message: &str) -> Decision {
    Decision {
        actions: vec![Action::Notify {
            message: message.into(),
        }],
        again: false,
    }
}

fn target(facts: &Facts, flags: Option<&Flags>) -> Option<Selection> {
    let configured = facts.report.config.target.clone();
    if let Some(model) = flags.and_then(|flags| flags.into.as_ref()) {
        return Some(Selection {
            model: model.clone(),
            thinking: configured.map_or_else(Default::default, |selection| selection.thinking),
        });
    }
    configured
}

fn arm(facts: &Facts, flags: Option<&Flags>) -> Decision {
    let Some(target) = target(facts, flags) else {
        return notice("Prewalk: no target configured. Set target in prewalk.json.");
    };
    if !facts.target_available {
        return notice(
            "Prewalk: target has no usable model or credentials. Session stays unarmed.",
        );
    }
    let Some(planner) = facts.current.clone() else {
        return notice("Prewalk: no active planner model. Session stays unarmed.");
    };
    if planner == target {
        return off(facts, &replay(&facts.branch));
    }
    Decision {
        actions: vec![Action::Store {
            state: State::Arm {
                planner,
                target,
                enabled: facts.report.config.enabled,
            },
        }],
        again: false,
    }
}

fn start(facts: &Facts, reason: &str, flags: &Flags, cycle: &Cycle<'_>) -> Decision {
    if reason == "reload" {
        return Decision::default();
    }
    let explicit = flags.prewalk || flags.into.is_some();
    if reason == "startup" && flags.no_prewalk && explicit {
        let mut decision = off(facts, cycle);
        decision.actions.push(Action::Notify {
            message: "Prewalk: --no-prewalk conflicts with --prewalk and --prewalk-into.".into(),
        });
        return decision;
    }
    if reason == "new" {
        return new_session(facts);
    }
    if reason == "startup" && explicit {
        return arm(facts, Some(flags));
    }
    let resumed = reason != "startup"
        || facts
            .branch
            .iter()
            .any(|entry| matches!(entry, Record::User | Record::State { .. }));
    if resumed || flags.no_prewalk {
        return off(facts, cycle);
    }
    if facts.report.config.enabled {
        arm(facts, None)
    } else {
        off(facts, cycle)
    }
}

fn handoff(branch: &[Record]) -> Cycle<'_> {
    for (index, record) in branch.iter().enumerate().rev() {
        if let Record::State { state } = record {
            match state {
                State::Arm { .. } => break,
                State::Handoff { .. } => {
                    return Cycle {
                        state: Some(state),
                        tail: branch.get(index + 1..).unwrap_or_default(),
                    };
                }
                State::Off { .. } => {}
            }
        }
    }
    Cycle {
        state: None,
        tail: branch,
    }
}

fn new_session(facts: &Facts) -> Decision {
    let previous = handoff(&facts.previous);
    let mut decision = Decision::default();
    let mut fresh = facts.clone();
    if let Some(State::Handoff { planner, .. }) = previous.state
        && !previous
            .tail
            .iter()
            .any(|record| matches!(record, Record::Selection))
    {
        // /new picks its model afresh in createRuntime; use the outgoing
        // branch to decide restoration, never the new runtime selection.
        if facts.available.contains(&planner.model) {
            decision.actions.push(Action::Select {
                selection: planner.clone(),
            });
            fresh.current = Some(planner.clone());
        } else {
            decision.actions.push(Action::Notify {
                message: format!(
                    "Prewalk: cannot restore planner {}. Keeping the current model.",
                    planner.model
                ),
            });
        }
    }
    let next = if facts.report.config.enabled {
        arm(&fresh, None)
    } else {
        off(&fresh, &replay(&[]))
    };
    decision.actions.extend(next.actions);
    decision
}

fn gate(cycle: &Cycle<'_>, facts: &Facts) -> bool {
    if let Some(active) = facts.active_unit {
        return active;
    }
    let after_nudge = cycle.tail.iter().skip_while(|record| {
        !matches!(record,
        Record::Prompt { name, .. } if name == "prewalk-plan")
    });
    !facts.todo
        || after_nudge
            .filter_map(|record| match record {
                Record::Calls { calls } => Some(calls),
                Record::State { .. }
                | Record::Prompt { .. }
                | Record::Scrub { .. }
                | Record::Selection
                | Record::User => None,
            })
            .flatten()
            .any(|call| call.name == "todo" && call.success)
}

fn turn(facts: &Facts, calls: &[crate::types::Call], cycle: &Cycle<'_>) -> Decision {
    let Some(State::Arm {
        planner, target, ..
    }) = cycle.state
    else {
        return Decision::default();
    };
    if facts.current.as_ref() == Some(target) {
        return off(facts, cycle);
    }
    if nudge(cycle.tail).is_none() {
        return Decision {
            actions: vec![prompt("prewalk-plan", PLAN)],
            again: true,
        };
    }
    let mut open = gate(cycle, facts);
    for call in calls {
        open |= facts.active_unit.is_none() && call.name == "todo" && call.success;
        if open && (call.name == "edit" || call.name == "write") {
            let mut actions = vec![Action::Select {
                selection: target.clone(),
            }];
            actions.push(Action::Store {
                state: State::Handoff {
                    planner: planner.clone(),
                    target: target.clone(),
                    enabled: facts.report.config.enabled,
                },
            });
            actions.extend(scrub(cycle.tail));
            actions.push(prompt("prewalk-checklist", CHECKLIST));
            return Decision {
                actions,
                again: true,
            };
        }
    }
    Decision::default()
}

fn settle(cycle: &Cycle<'_>) -> Decision {
    if matches!(cycle.state, Some(State::Arm { .. }))
        && nudge(cycle.tail).is_some()
        && !cycle.tail.iter().any(|record| {
            matches!(record,
            Record::Prompt { name, .. } if name == "prewalk-continue")
        })
    {
        return Decision {
            actions: vec![prompt("prewalk-continue", CONTINUE)],
            again: true,
        };
    }
    Decision::default()
}

fn input(facts: &Facts, cycle: &Cycle<'_>) -> Decision {
    let current = facts.report.config.enabled;
    if cycle.state.map(enabled) == Some(current) {
        return Decision::default();
    }
    if current && !matches!(cycle.state, Some(State::Arm { .. })) {
        return arm(facts, None);
    }
    if !current && matches!(cycle.state, Some(State::Arm { .. })) {
        return off(facts, cycle);
    }
    // A live config change updates the existing entry type, preserving the cycle.
    let state = match cycle.state.cloned() {
        Some(State::Arm {
            planner, target, ..
        }) => State::Arm {
            planner,
            target,
            enabled: current,
        },
        Some(State::Handoff {
            planner, target, ..
        }) => State::Handoff {
            planner,
            target,
            enabled: current,
        },
        Some(State::Off { .. }) | None => State::Off { enabled: current },
    };
    Decision {
        actions: vec![Action::Store { state }],
        again: false,
    }
}

fn command(facts: &Facts, command: &str, cycle: &Cycle<'_>) -> Decision {
    match command {
        "off" => off(facts, cycle),
        "status" => status(facts, cycle),
        "" | "on" if matches!(cycle.state, Some(State::Arm { .. })) => {
            notice("Prewalk: already armed.")
        }
        "" | "on" => arm(facts, None),
        "restart" => restart(facts, cycle),
        _ => notice("Usage: /prewalk [on|off|restart|status]"),
    }
}

fn restart(facts: &Facts, cycle: &Cycle<'_>) -> Decision {
    if let Some(State::Arm { target, .. }) = cycle.state {
        return if Some(target) == facts.report.config.target.as_ref() {
            notice("Prewalk: matching arm retained.")
        } else {
            notice("Prewalk: restart refused because the armed target differs from config.")
        };
    }
    let Some(default) = &facts.default else {
        return notice("Prewalk: default model is unavailable.");
    };
    // Validate the target before any selection change.
    if !facts.target_available || facts.report.config.target.is_none() {
        return notice("Prewalk: target is unavailable. Active model stays unchanged.");
    }
    let mut fresh = facts.clone();
    fresh.current = Some(default.clone());
    let mut decision = arm(&fresh, None);
    decision.actions.insert(
        0,
        Action::Select {
            selection: default.clone(),
        },
    );
    decision
}

fn status(facts: &Facts, cycle: &Cycle<'_>) -> Decision {
    let state = match cycle.state {
        Some(State::Arm {
            planner, target, ..
        }) => format!("armed; planner {}; target {}", planner.model, target.model),
        Some(State::Handoff {
            planner, target, ..
        }) => format!(
            "handed off; planner {}; target {}",
            planner.model, target.model
        ),
        Some(State::Off { .. }) | None => "off".into(),
    };
    notice(&format!(
        "Prewalk: {state}; gate {}; todo owner {}; config errors: {}",
        if gate(cycle, facts) { "open" } else { "closed" },
        facts.todo_owner,
        facts.report.errors.join("; ")
    ))
}

/// Decides one transition using branch facts only.
#[must_use]
pub fn decide(facts: &Facts) -> Decision {
    let cycle = replay(&facts.branch);
    let mut decision = match &facts.event {
        Event::Start { reason, flags } => start(facts, reason, flags, &cycle),
        Event::Input => input(facts, &cycle),
        Event::Turn { calls } => turn(facts, calls, &cycle),
        Event::Settle => settle(&cycle),
        Event::Context => Decision::default(),
        Event::Command {
            command: subcommand,
        } => command(facts, subcommand, &cycle),
    };
    if matches!(facts.event, Event::Turn { .. } | Event::Settle) {
        for id in project(&facts.branch).scrub {
            if !decision
                .actions
                .iter()
                .any(|action| matches!(action, Action::Scrub { id: existing } if *existing == id))
            {
                decision.actions.push(Action::Scrub { id });
            }
        }
    }
    decision
}

/// Decides which planning steering belongs to the live branch cycle.
#[must_use]
pub fn project(branch: &[Record]) -> Projection {
    let cycle = replay(branch);
    let keep = matches!(cycle.state, Some(State::Arm { .. }))
        .then(|| nudge(cycle.tail))
        .flatten();
    let scrubbed: Vec<_> = branch
        .iter()
        .filter_map(|record| match record {
            Record::Scrub { id } => Some(id.as_str()),
            Record::State { .. }
            | Record::Prompt { .. }
            | Record::Calls { .. }
            | Record::Selection
            | Record::User => None,
        })
        .collect();
    let scrub = branch
        .iter()
        .filter_map(|record| match record {
            Record::Prompt { id, name }
                if name == "prewalk-plan"
                    && Some(id.as_str()) != keep
                    && !scrubbed.contains(&id.as_str()) =>
            {
                Some(id.clone())
            }
            Record::State { .. }
            | Record::Prompt { .. }
            | Record::Calls { .. }
            | Record::Scrub { .. }
            | Record::Selection
            | Record::User => None,
        })
        .collect();
    Projection {
        keep_plan: keep.is_some(),
        scrub,
    }
}
