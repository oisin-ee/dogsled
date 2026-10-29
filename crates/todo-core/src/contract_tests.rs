//! Domain behavior exercised through the same request entry point as WASM.
use crate::{
    Acceptance, Assignment, ContractDraft, DraftItem, Item, MutationContext, Operation, Request,
    Scope, Status, TodoList, apply, init,
};

fn context() -> MutationContext {
    MutationContext {
        session_id: "session".to_owned(),
        mutation_id: "mutation".to_owned(),
    }
}
fn request(op: Operation) -> Request {
    Request {
        op,
        items: None,
        task: None,
        parent: None,
        contract: None,
        start: None,
        evidence: None,
        description: None,
    }
}
fn draft() -> ContractDraft {
    ContractDraft {
        deliverable: Some("Deliver contracts".to_owned()),
        scope: Some(Scope {
            repository_id: "repo".to_owned(),
            owns: vec!["src/".to_owned()],
            reads: vec![],
            generates: vec![],
        }),
        acceptance: Some(Acceptance::Command {
            cwd: ".".to_owned(),
            argv: vec!["test".to_owned()],
            expected: "pass".to_owned(),
        }),
        depends_on: vec![],
        assignment: Some(Assignment::Self_),
    }
}
fn create(start: bool) -> TodoList {
    let mut req = request(Operation::Init);
    req.items = Some(vec![Item::Draft(Box::new(DraftItem {
        content: "Build".to_owned(),
        description: None,
        key: Some("build".to_owned()),
        contract: draft(),
    }))]);
    if start {
        req.start = Some("build".to_owned());
    }
    apply(TodoList::default(), req, &context()).unwrap().list
}
fn id(list: &TodoList) -> String {
    list.tasks[0]
        .unit
        .as_ref()
        .unwrap()
        .reference
        .unit_id
        .clone()
}

#[test]
fn atomic_creation_and_evidence_completion() {
    let list = create(true);
    assert_eq!(
        list.tasks[0].status,
        Status::InProgress,
        "atomic activation"
    );
    let mut done = request(Operation::Done);
    done.task = Some(id(&list));
    assert!(
        apply(list.clone(), done.clone(), &context()).is_err(),
        "evidence required"
    );
    done.evidence = Some("test passed".to_owned());
    let closed = apply(list, done, &context()).unwrap().list;
    assert_eq!(
        closed.tasks[0].status,
        Status::Completed,
        "acceptance recorded"
    );
    assert_eq!(
        closed.tasks[0].unit.as_ref().unwrap().local_session_id,
        None,
        "activation ends"
    );
}

#[test]
fn drafts_and_worker_assignments_cannot_start() {
    let list = create(false);
    let mut update = request(Operation::Update);
    update.task = Some(id(&list));
    let mut worker = draft();
    worker.assignment = Some(Assignment::Worker {
        profile: "general".to_owned(),
    });
    update.contract = Some(worker);
    let list = apply(list, update, &context()).unwrap().list;
    let mut start = request(Operation::Start);
    start.task = Some(id(&list));
    assert!(
        apply(list, start, &context())
            .unwrap_err()
            .to_string()
            .contains("WRONG_ASSIGNMENT"),
        "worker not local"
    );
    let legacy = init(vec!["Legacy".to_owned()]).unwrap();
    let mut start = request(Operation::Start);
    start.task = Some("Legacy".to_owned());
    assert!(
        apply(legacy, start, &context()).is_err(),
        "legacy draft cannot execute"
    );
}

#[test]
fn revisions_freeze_and_closed_work_never_reopens() {
    let list = create(true);
    let mut update = request(Operation::Update);
    update.task = Some(id(&list));
    update.contract = Some(draft());
    assert!(
        apply(list.clone(), update, &context()).is_err(),
        "executing contract frozen"
    );
    let mut drop = request(Operation::Drop);
    drop.task = Some(id(&list));
    let list = apply(list, drop, &context()).unwrap().list;
    let mut start = request(Operation::Start);
    start.task = Some(id(&list));
    assert!(
        apply(list, start, &context()).is_err(),
        "closed revision stays closed"
    );
}

#[test]
fn failed_create_start_preserves_the_previous_list() {
    let list = create(false);
    let mut req = request(Operation::Init);
    req.items = Some(vec![Item::Text("Draft".to_owned())]);
    req.start = Some("Draft".to_owned());
    assert!(
        apply(list.clone(), req, &context()).is_err(),
        "bad selector rejected atomically"
    );
    assert_eq!(
        list.tasks[0].content, "Build",
        "original input remains available"
    );
}
