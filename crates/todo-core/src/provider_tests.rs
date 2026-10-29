//! Provider policy through public operations, independent of a runtime executor.
use crate::*;

pub(crate) fn context() -> MutationContext {
    MutationContext {
        session_id: "session".to_owned(),
        mutation_id: "mutation".to_owned(),
    }
}
pub(crate) fn request(op: Operation, id: &str) -> Request {
    Request {
        op,
        items: None,
        task: Some(id.to_owned()),
        parent: None,
        contract: None,
        start: None,
        evidence: None,
        description: None,
    }
}
pub(crate) fn execution(id: &str) -> ExecutionRef {
    ExecutionRef {
        executor: "dogsled".to_owned(),
        owner_id: "owner".to_owned(),
        request_id: id.to_owned(),
    }
}
pub(crate) fn pending() -> TodoList {
    let mut list = init(vec!["Build".to_owned()]).unwrap();
    let mut update = request(Operation::Update, "Build");
    update.contract = Some(ContractDraft {
        deliverable: Some("Ship code".to_owned()),
        scope: Some(Scope {
            repository_id: "repo".to_owned(),
            owns: vec!["src/".to_owned()],
            reads: vec![],
            generates: vec![],
        }),
        acceptance: Some(Acceptance::Review {
            artifact: "diff".to_owned(),
            criterion: "reviewed".to_owned(),
        }),
        depends_on: vec![],
        assignment: Some(Assignment::Worker {
            profile: "general".to_owned(),
        }),
    });
    list = apply(list, update, &context()).unwrap().list;
    list.tasks[0]
        .unit
        .as_mut()
        .unwrap()
        .reference
        .contract_entry_id = "entry".to_owned();
    list
}
pub(crate) fn input(list: &TodoList, id: &str) -> ClaimInput {
    ClaimInput {
        expected: list.tasks[0].unit.as_ref().unwrap().reference.clone(),
        execution: execution(id),
    }
}
pub(crate) fn returned(binding: TaskBinding) -> Settlement {
    Settlement::Bound {
        version: 1,
        event_id: "returned-1".to_owned(),
        result: BoundResult::Returned {
            evidence: EvidenceRef {
                execution: binding.execution.clone(),
                result_id: "result".to_owned(),
                summary: "untrusted reply".to_owned(),
            },
        },
        binding,
    }
}

#[test]
fn exact_claim_replay_and_explicit_acceptance() {
    let list = pending();
    let input = input(&list, "first");
    let claimed = claim(list, &input, "session").unwrap();
    let frozen = claimed.list.tasks[0]
        .unit
        .as_ref()
        .unwrap()
        .frozen_export_json
        .clone();
    let replay = claim(claimed.list.clone(), &input, "session").unwrap();
    assert!(!replay.persist, "claim replay does not append");
    assert_eq!(
        frozen,
        replay.list.tasks[0]
            .unit
            .as_ref()
            .unwrap()
            .frozen_export_json,
        "exact frozen bytes"
    );
    assert!(
        claim(
            claimed.list.clone(),
            &ClaimInput {
                execution: execution("other"),
                ..input.clone()
            },
            "session"
        )
        .is_err(),
        "competing execution blocked"
    );
    let mut done = request(Operation::Done, &input.expected.unit_id);
    done.evidence = Some("reviewed diff".to_owned());
    let event = returned(claimed.value.binding);
    let released = settle(claimed.list, &event, "session").unwrap();
    assert_eq!(
        released.list.tasks[0].status,
        Status::InProgress,
        "settlement is not acceptance"
    );
    assert_eq!(
        active(&released.list, "session").unwrap(),
        None,
        "delegated result never activates lead"
    );
    assert!(
        claim(released.list.clone(), &input, "session").is_err(),
        "old execution cannot relaunch"
    );
    let complete = apply(released.list, done, &context()).unwrap();
    assert_eq!(
        complete.list.tasks[0].status,
        Status::Completed,
        "normal explicit review completes"
    );
}

#[test]
fn duplicate_event_cannot_clear_a_new_binding() {
    let list = pending();
    let first = claim(list.clone(), &input(&list, "first"), "session").unwrap();
    let event = returned(first.value.binding);
    let returned = settle(first.list, &event, "session").unwrap();
    let unit = returned.list.tasks[0].unit.as_ref().unwrap();
    let mut update = request(Operation::Update, &unit.reference.unit_id);
    update.contract = Some(unit.draft.clone());
    let mut revised = apply(returned.list, update, &context()).unwrap().list;
    revised.tasks[0]
        .unit
        .as_mut()
        .unwrap()
        .reference
        .contract_entry_id = "next-entry".to_owned();
    let next = claim(revised.clone(), &input(&revised, "next"), "session").unwrap();
    let duplicate = settle(next.list.clone(), &event, "session").unwrap();
    assert!(
        !duplicate.value.applied,
        "durable event identity deduplicates old revision"
    );
    assert_eq!(duplicate.list, next.list, "new binding untouched");
    let mut stale = event;
    if let Settlement::Bound { event_id, .. } = &mut stale {
        *event_id = "unknown-old".to_owned();
    }
    assert!(
        settle(next.list, &stale, "session").is_err(),
        "unseen stale revision rejected"
    );
}

#[test]
fn not_started_uses_origin_execution_without_digest() {
    let list = pending();
    let input = input(&list, "first");
    let event = Settlement::NotStarted {
        version: 1,
        event_id: "abandoned".to_owned(),
        origin: input.expected.clone(),
        execution: input.execution.clone(),
        result: UnstartedResult::NotStarted {
            reason: "never committed".to_owned(),
        },
    };
    assert!(
        !settle(list.clone(), &event, "session")
            .unwrap()
            .value
            .applied,
        "unclaimed origin is no-op"
    );
    let claimed = claim(list, &input, "session").unwrap();
    assert!(
        settle(claimed.list.clone(), &event, "other-session").is_err(),
        "inactive origin holds binding"
    );
    let released = settle(claimed.list, &event, "session").unwrap();
    assert_eq!(
        released.list.tasks[0].status,
        Status::Pending,
        "abandonment returns pending"
    );
    assert!(
        released.list.tasks[0]
            .unit
            .as_ref()
            .unwrap()
            .binding
            .is_none(),
        "matching hold released"
    );
    assert!(
        !settle(released.list, &event, "session")
            .unwrap()
            .value
            .applied,
        "duplicate no-op"
    );
}

#[test]
fn protected_calls_and_held_workers_cannot_be_replaced() {
    let list = pending();
    let claimed = claim(list.clone(), &input(&list, "first"), "session").unwrap();
    let id = &claimed.value.binding.contract.unit_id;
    assert!(
        apply(
            claimed.list.clone(),
            Request {
                task: None,
                ..request(Operation::Drop, id)
            },
            &context()
        )
        .is_err(),
        "untargeted drop cannot release a held unit"
    );
    let mut init = request(Operation::Init, id);
    init.task = None;
    init.items = Some(vec![Item::Text("Replacement".to_owned())]);
    assert!(
        apply(claimed.list.clone(), init, &context()).is_err(),
        "held init rejected"
    );
    assert!(
        protect(
            &claimed.list,
            &TodoList::default(),
            &[claimed.value.binding.contract]
        )
        .is_err(),
        "lease protects replacement"
    );
}

#[test]
fn shared_unicode_canonical_fixtures_are_byte_exact() {
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("../tests/fixtures/unit-exports.json")).unwrap();
    for fixture in fixtures.as_array().unwrap() {
        let canonical = fixture["canonical"].as_str().unwrap();
        let data: ExportData = serde_json::from_str(canonical).unwrap();
        assert_eq!(
            data.canonical().unwrap(),
            canonical,
            "fixed schema order and escaping"
        );
        assert_eq!(
            export_digest(canonical).unwrap(),
            fixture["digest"].as_str().unwrap(),
            "independently verified digest"
        );
    }
}

#[test]
fn held_child_blocks_parent_cascade_and_keeps_explicit_parent_review() {
    let list = pending();
    let child = list.tasks[0].clone();
    let mut parent = init(vec!["Outcome".to_owned()]).unwrap();
    parent.tasks[0].subtasks.push(Subtask {
        content: child.content,
        description: child.description,
        status: child.status,
        unit: child.unit,
    });
    let unit = parent.tasks[0].subtasks[0].unit.as_ref().unwrap();
    let input = ClaimInput {
        expected: unit.reference.clone(),
        execution: execution("child"),
    };
    let claimed = claim(parent, &input, "session").unwrap();
    assert!(
        apply(
            claimed.list.clone(),
            Request {
                task: None,
                ..request(Operation::Drop, "Outcome")
            },
            &MutationContext {
                mutation_id: "migrate".to_owned(),
                ..context()
            }
        )
        .is_err(),
        "untargeted drop cannot cascade into a held child"
    );
    let released = settle(claimed.list, &returned(claimed.value.binding), "session").unwrap();
    let mut done = request(Operation::Done, &input.expected.unit_id);
    done.evidence = Some("reviewed child".to_owned());
    let completed = apply(
        released.list,
        done,
        &MutationContext {
            session_id: "session".to_owned(),
            mutation_id: "review".to_owned(),
        },
    )
    .unwrap()
    .list;
    assert_eq!(
        completed.tasks[0].status,
        Status::InProgress,
        "parent needs explicit review"
    );
}

#[test]
fn strict_settlement_rejects_unknown_fields() {
    let list = pending();
    let input = input(&list, "first");
    let event = Settlement::NotStarted {
        version: 1,
        event_id: "event".to_owned(),
        origin: input.expected,
        execution: input.execution,
        result: UnstartedResult::NotStarted {
            reason: "not committed".to_owned(),
        },
    };
    let mut json = serde_json::to_value(event).unwrap();
    json["extra"] = serde_json::json!(true);
    assert!(
        parse_settlement(&json.to_string()).is_err(),
        "wire rejects unknown fields"
    );
}
