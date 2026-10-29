use crate::types::{
    Action, Call, Config, ConfigReport, Event, Facts, Flags, Record, Selection, State, Thinking,
};
use crate::{configure, decide, project};

fn selection(model: &str) -> Selection {
    Selection {
        model: model.into(),
        thinking: Thinking::Medium,
    }
}

fn facts(event: Event) -> Facts {
    Facts {
        branch: Vec::new(),
        previous: Vec::new(),
        current: Some(selection("test/planner")),
        report: ConfigReport {
            config: Config {
                enabled: false,
                target: Some(selection("test/worker")),
            },
            errors: Vec::new(),
        },
        target_available: true,
        available: vec!["test/planner".into(), "test/worker".into()],
        default: Some(selection("test/default")),
        todo: true,
        active_unit: None,
        todo_owner: "test".into(),
        event,
    }
}

fn armed() -> Vec<Record> {
    vec![Record::State {
        state: State::Arm {
            planner: selection("test/planner"),
            target: selection("test/worker"),
            enabled: false,
        },
    }]
}

fn nudged() -> Vec<Record> {
    let mut branch = armed();
    branch.push(Record::Prompt {
        id: "nudge".into(),
        name: "prewalk-plan".into(),
    });
    branch
}

fn call(name: &str, success: bool) -> Call {
    Call {
        name: name.into(),
        success,
    }
}

#[test]
fn activation_opens_gate_without_synthetic_todo_and_keeps_post_edit_switch() {
    let mut input = facts(Event::Turn {
        calls: vec![call("read", true)],
    });
    input.branch = nudged();
    input.active_unit = Some(true);
    assert!(
        decide(&input).actions.is_empty(),
        "activation alone never switches model"
    );
    input.event = Event::Turn {
        calls: vec![call("edit", true)],
    };
    assert!(
        matches!(decide(&input).actions.first(), Some(Action::Select { .. })),
        "first edit triggers the existing handoff"
    );
    input.active_unit = Some(false);
    input.event = Event::Turn {
        calls: vec![call("todo", true), call("edit", true)],
    };
    assert!(
        decide(&input).actions.is_empty(),
        "a successful view is not validated activation"
    );
    input.active_unit = None;
    assert!(
        matches!(decide(&input).actions.first(), Some(Action::Select { .. })),
        "missing provider preserves legacy evidence"
    );
}

#[test]
fn config_replaces_project_keys_and_validates_each_document() {
    let result = configure(
        Some(r#"{"enabled":true,"target":{"model":"a/b","thinking":"high"}}"#),
        Some(r#"{"target":{"model":"c/d","thinking":"low"}}"#),
    );
    assert!(result.errors.is_empty(), "valid config");
    assert!(result.config.enabled, "user enabled is retained");
    assert_eq!(
        result.config.target.unwrap(),
        Selection {
            model: "c/d".into(),
            thinking: Thinking::Low
        }
    );
    for source in [
        "[]",
        "null",
        "{",
        r#"{"enabled":"yes"}"#,
        r#"{"extra":1}"#,
        r#"{"target":{"model":"bad","thinking":"medium"}}"#,
        r#"{"target":{"model":"a/b","thinking":"auto"}}"#,
    ] {
        assert!(
            !configure(Some(source), None).errors.is_empty(),
            "reject {source}"
        );
    }
}

#[test]
fn nudge_precedes_gate_and_failed_edit_triggers_after_successful_todo() {
    let mut input = facts(Event::Turn {
        calls: vec![call("todo", true), call("edit", true)],
    });
    input.branch = armed();
    let result = decide(&input);
    assert!(
        matches!(result.actions.as_slice(), [Action::Prompt { name, .. }] if name == "prewalk-plan"),
        "nudge first"
    );
    input.branch = nudged();
    input.event = Event::Turn {
        calls: vec![call("todo", false), call("edit", true)],
    };
    assert!(
        decide(&input).actions.is_empty(),
        "failed todo keeps gate closed"
    );
    input.event = Event::Turn {
        calls: vec![call("todo", true), call("edit", false)],
    };
    let result = decide(&input);
    assert!(
        matches!(result.actions.as_slice(), [Action::Select { .. }, Action::Store { state: State::Handoff { .. } }, Action::Scrub { id }, Action::Prompt { name, .. }] if id == "nudge" && name == "prewalk-checklist"),
        "ordered handoff"
    );
}

#[test]
fn gate_survives_later_turns_but_not_a_new_arm() {
    let mut input = facts(Event::Turn {
        calls: vec![call("write", false)],
    });
    input.branch = nudged();
    input.branch.push(Record::Calls {
        calls: vec![call("todo", true)],
    });
    assert!(decide(&input).again, "gate persists");
    input.branch.extend(armed());
    assert!(
        matches!(decide(&input).actions.first(), Some(Action::Prompt { .. })),
        "fresh arm gets fresh nudge"
    );
}

#[test]
fn continuation_fires_once_and_absent_todo_opens_gate() {
    let mut input = facts(Event::Settle);
    input.branch = nudged();
    assert!(decide(&input).again, "one continuation");
    input.branch.push(Record::Prompt {
        id: "continued".into(),
        name: "prewalk-continue".into(),
    });
    assert!(!decide(&input).again, "not repeated");
    input.todo = false;
    input.event = Event::Turn {
        calls: vec![call("write", true)],
    };
    assert!(decide(&input).again, "no todo gate");
}

#[test]
fn resume_cancels_unless_startup_flag_arms() {
    let mut input = facts(Event::Start {
        reason: "resume".into(),
        flags: Flags::default(),
    });
    input.branch = nudged();
    assert!(
        matches!(
            decide(&input).actions.last(),
            Some(Action::Store {
                state: State::Off { .. }
            })
        ),
        "resume disarms"
    );
    input.event = Event::Start {
        reason: "startup".into(),
        flags: Flags {
            prewalk: true,
            ..Flags::default()
        },
    };
    assert!(
        matches!(
            decide(&input).actions.first(),
            Some(Action::Store {
                state: State::Arm { .. }
            })
        ),
        "explicit startup arms"
    );
}

#[test]
fn no_prewalk_conflict_disarms_and_warns() {
    let input = facts(Event::Start {
        reason: "startup".into(),
        flags: Flags {
            prewalk: true,
            no_prewalk: true,
            into: None,
        },
    });
    assert!(
        matches!(decide(&input).actions.last(), Some(Action::Notify { .. })),
        "conflicting flags warn"
    );
}

#[test]
fn new_restores_only_without_a_later_selection() {
    let mut input = facts(Event::Start {
        reason: "new".into(),
        flags: Flags::default(),
    });
    input.previous = vec![Record::State {
        state: State::Handoff {
            planner: selection("test/planner"),
            target: selection("test/worker"),
            enabled: false,
        },
    }];
    input.report.config.enabled = true;
    input.current = Some(selection("test/default"));
    let result = decide(&input);
    assert!(
        matches!(result.actions.first(), Some(Action::Select { selection: value }) if value.model == "test/planner"),
        "restore from old session, not fresh runtime"
    );
    input.previous.push(Record::State {
        state: State::Off { enabled: false },
    });
    assert!(
        matches!(decide(&input).actions.first(), Some(Action::Select { .. })),
        "off after handoff does not replace the model"
    );
    input.previous.push(Record::Selection);
    assert!(
        !decide(&input)
            .actions
            .iter()
            .any(|action| matches!(action, Action::Select { .. })),
        "later selection owns model"
    );
    input.previous.pop();
    input.available.clear();
    assert!(
        matches!(decide(&input).actions.first(), Some(Action::Notify { .. })),
        "missing planner warns and still arms"
    );
}

#[test]
fn off_projects_immediately_and_persists_scrub_once_at_boundary() {
    let mut input = facts(Event::Settle);
    input.branch = nudged();
    input.branch.push(Record::State {
        state: State::Off { enabled: false },
    });
    assert!(!project(&input.branch).keep_plan, "off removes nudge now");
    assert!(
        matches!(decide(&input).actions.as_slice(), [Action::Scrub { id }] if id == "nudge"),
        "persist scrub"
    );
    input.branch.push(Record::Scrub { id: "nudge".into() });
    assert!(decide(&input).actions.is_empty(), "no duplicate scrub");
}

#[test]
fn live_config_and_restart_keep_their_owners() {
    let mut input = facts(Event::Input);
    input.branch = nudged();
    input.report.config.enabled = true;
    let result = decide(&input);
    assert!(
        matches!(
            result.actions.as_slice(),
            [Action::Store {
                state: State::Arm { enabled: true, .. }
            }]
        ),
        "live enable preserves existing arm"
    );
    input.event = Event::Command {
        command: "restart".into(),
    };
    input.report.config.target = Some(selection("other/target"));
    assert!(
        matches!(decide(&input).actions.as_slice(), [Action::Notify { .. }]),
        "mismatch rejects before selection"
    );
    input.branch.clear();
    input.report.config.target = Some(selection("test/worker"));
    assert!(
        matches!(decide(&input).actions.first(), Some(Action::Select { selection: value }) if value.model == "test/default"),
        "restart uses saved default"
    );
}

#[test]
fn on_arms_like_the_bare_command() {
    for command in ["", "on"] {
        let mut input = facts(Event::Command {
            command: command.into(),
        });
        assert!(
            matches!(
                decide(&input).actions.as_slice(),
                [Action::Store {
                    state: State::Arm { .. }
                }]
            ),
            "/prewalk {command} arms"
        );
        input.branch = armed();
        assert!(
            matches!(decide(&input).actions.as_slice(), [Action::Notify { message }] if message == "Prewalk: already armed."),
            "/prewalk {command} keeps an existing arm"
        );
    }
}
