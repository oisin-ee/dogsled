//! The owning lead closes any open leaf with a short note, releasing worker holds.
use crate::provider_tests::{context, execution, input, pending, request, returned};
use crate::*;

fn done(id: &str, note: Option<&str>) -> Request {
    Request {
        evidence: note.map(str::to_owned),
        ..request(Operation::Done, id)
    }
}
fn claimed() -> (TodoList, ClaimResult, String) {
    let list = pending();
    let claim_input = input(&list, "first");
    let id = claim_input.expected.unit_id.clone();
    let result = claim(list, &claim_input, "session").unwrap();
    (result.list, result.value, id)
}
fn not_started(list: &TodoList, id: &str) -> Settlement {
    Settlement::NotStarted {
        version: 1,
        event_id: format!("not-started-{id}"),
        origin: list.tasks[0].unit.as_ref().unwrap().reference.clone(),
        execution: execution(id),
        result: UnstartedResult::NotStarted {
            reason: "gone".to_owned(),
        },
    }
}
fn stopped(binding: TaskBinding) -> Settlement {
    Settlement::Bound {
        version: 1,
        event_id: "stopped-1".to_owned(),
        result: BoundResult::Stopped {
            evidence: None,
            reason: "killed".to_owned(),
        },
        binding,
    }
}

#[test]
fn done_with_a_note_completes_a_leaf_that_is_not_active() {
    let list = pending();
    let id = list.tasks[0]
        .unit
        .as_ref()
        .unwrap()
        .reference
        .unit_id
        .clone();
    let closed = apply(list.clone(), done(&id, Some("merged")), &context())
        .unwrap()
        .list;
    assert_eq!(closed.tasks[0].status, Status::Completed, "closed");
    assert_eq!(
        closed.tasks[0]
            .unit
            .as_ref()
            .unwrap()
            .acceptance_evidence
            .as_deref(),
        Some("merged"),
        "note recorded"
    );
    for note in [None, Some("  ")] {
        assert!(
            apply(list.clone(), done(&id, note), &context()).is_err(),
            "done needs a non-empty note"
        );
    }
}

#[test]
fn done_on_a_bound_leaf_releases_it_and_frees_its_paths() {
    let (list, value, id) = claimed();
    let closed = apply(list, done(&id, Some("merged long ago")), &context())
        .unwrap()
        .list;
    let unit = closed.tasks[0].unit.clone().unwrap();
    assert_eq!(closed.tasks[0].status, Status::Completed, "completed");
    assert!(unit.binding.is_none(), "binding cleared");
    assert_eq!(unit.released, vec![value.binding.execution], "recorded");
    let mut append = request(Operation::Append, "");
    append.task = None;
    append.items = Some(vec![Item::Draft(Box::new(DraftItem {
        content: "Next".to_owned(),
        description: None,
        key: Some("next".to_owned()),
        contract: unit.draft.clone(),
    }))]);
    append.start = None;
    let appended = apply(closed, append, &context()).unwrap().list;
    let next = appended.tasks[1]
        .unit
        .as_ref()
        .unwrap()
        .reference
        .unit_id
        .clone();
    let mut contract = unit.draft;
    contract.assignment = Some(Assignment::Self_);
    let mut update = request(Operation::Update, &next);
    update.contract = Some(contract);
    let updated = apply(appended, update, &context()).unwrap().list;
    apply(updated, request(Operation::Start, &next), &context())
        .expect("old paths no longer conflict");
}

#[test]
fn drop_releases_a_bound_leaf_and_a_bound_child() {
    let (list, value, id) = claimed();
    let dropped = apply(list, request(Operation::Drop, &id), &context())
        .unwrap()
        .list;
    let unit = dropped.tasks[0].unit.as_ref().unwrap();
    assert_eq!(dropped.tasks[0].status, Status::Abandoned, "abandoned");
    assert!(unit.binding.is_none(), "binding cleared");
    assert_eq!(unit.released, vec![value.binding.execution], "recorded");

    let list = pending();
    let child = list.tasks[0].clone();
    let mut parent = init(vec!["Outcome".to_owned()]).unwrap();
    parent.tasks[0].subtasks.push(Subtask {
        content: child.content,
        description: child.description,
        status: child.status,
        unit: child.unit,
    });
    let claim_input = ClaimInput {
        expected: parent.tasks[0].subtasks[0]
            .unit
            .as_ref()
            .unwrap()
            .reference
            .clone(),
        execution: execution("child"),
    };
    let held = claim(parent, &claim_input, "session").unwrap().list;
    let migrate = MutationContext {
        mutation_id: "migrate".to_owned(),
        ..context()
    };
    let dropped = apply(held, request(Operation::Drop, "Outcome"), &migrate)
        .unwrap()
        .list;
    let child = &dropped.tasks[0].subtasks[0];
    assert_eq!(child.status, Status::Abandoned, "child abandoned");
    assert!(
        child.unit.as_ref().unwrap().binding.is_none(),
        "child released"
    );
    assert_eq!(child.unit.as_ref().unwrap().released.len(), 1, "recorded");
}

#[test]
fn late_settlements_for_a_released_execution_are_no_ops() {
    let (list, value, id) = claimed();
    let closed = apply(list, done(&id, Some("merged")), &context())
        .unwrap()
        .list;
    for event in [
        returned(value.binding.clone()),
        stopped(value.binding.clone()),
        not_started(&closed, "first"),
    ] {
        let late = settle(closed.clone(), &event, "session").unwrap();
        assert!(!late.value.applied, "released settlement not applied");
        assert!(!late.persist, "nothing to persist");
        assert_eq!(late.list, closed, "state unchanged");
    }
    let mut other = value.binding;
    other.execution = execution("other");
    assert!(
        settle(closed, &returned(other), "session").is_err(),
        "another execution still fails"
    );
}

#[test]
fn other_changes_to_a_bound_leaf_stay_blocked() {
    let (list, _, id) = claimed();
    let mut update = request(Operation::Update, &id);
    update.contract = list.tasks[0].unit.as_ref().map(|unit| unit.draft.clone());
    for blocked in [
        update,
        Request {
            task: None,
            ..request(Operation::Drop, &id)
        },
    ] {
        let message = apply(list.clone(), blocked, &context())
            .unwrap_err()
            .to_string();
        assert!(message.contains("UNIT_ALREADY_BOUND"), "{message}");
    }
}
