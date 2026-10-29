//! The importer consumes actual provider exports and ordinary worker operations.
use crate::*;

fn context(session: &str) -> MutationContext {
    MutationContext {
        session_id: session.into(),
        mutation_id: "mutation".into(),
    }
}

fn request(op: Operation, task: Option<String>) -> Request {
    Request {
        op,
        task,
        parent: None,
        items: None,
        contract: None,
        start: None,
        evidence: None,
        description: None,
    }
}

fn seed() -> Seed {
    let list = init(vec!["Build".into()]).unwrap();
    let mut update = request(Operation::Update, Some("Build".into()));
    update.contract = Some(ContractDraft {
        deliverable: Some("Ship importer".into()),
        scope: Some(Scope {
            repository_id: "repo".into(),
            owns: vec!["src/".into()],
            reads: vec![],
            generates: vec![],
        }),
        acceptance: Some(Acceptance::Review {
            artifact: "diff".into(),
            criterion: "approved".into(),
        }),
        depends_on: vec![],
        assignment: Some(Assignment::Worker {
            profile: "general".into(),
        }),
    });
    let mut list = apply(list, update, &context("lead")).unwrap().list;
    let unit = list.tasks[0].unit.as_mut().unwrap();
    unit.reference.contract_entry_id = "lead-entry".into();
    let input = ClaimInput {
        expected: unit.reference.clone(),
        execution: ExecutionRef {
            executor: "dogsled".into(),
            owner_id: "owner".into(),
            request_id: "request".into(),
        },
    };
    let claim = claim(list, &input, "lead").unwrap();
    Seed {
        version: 1,
        execution: input.execution,
        binding: claim.value.binding,
        exported: claim.value.exported,
    }
}

#[test]
fn imported_contract_activates_without_rewriting_worker_assignment() {
    let seed = seed();
    let imported = import_unit(TodoList::default(), seed.clone(), "worker", "repo").unwrap();
    assert!(imported.persist, "first import persists");
    assert_eq!(
        inspect(&imported.list, &seed.binding.contract.unit_id).unwrap(),
        seed.exported.contract
    );
    let active = active(&imported.list, "worker").unwrap().unwrap();
    assert!(active.seeded, "worker activation is seeded");
    assert_eq!(active.r#ref, seed.binding.contract);
    assert!(
        crate::active(&imported.list, "other").unwrap().is_none(),
        "another session cannot execute"
    );
    assert!(
        !import_unit(imported.list.clone(), seed.clone(), "worker", "repo")
            .unwrap()
            .persist,
        "exact replay does not append"
    );
    assert!(
        import_unit(imported.list, seed, "different", "repo").is_err(),
        "another local session cannot reimport"
    );
}

#[test]
fn ordinary_completion_seals_worker_and_replay_cannot_reset_progress() {
    for operation in [Operation::Done, Operation::Drop] {
        let seed = seed();
        let list = import_unit(TodoList::default(), seed.clone(), "worker", "repo")
            .unwrap()
            .list;
        let mut close = request(operation, Some(seed.binding.contract.unit_id.clone()));
        if matches!(operation, Operation::Done) {
            close.evidence = Some("reviewed diff".into());
        }
        let closed = apply(list, close, &context("worker")).unwrap().list;
        assert!(
            active(&closed, "worker").unwrap().is_none(),
            "closure revokes activation"
        );
        let receipt = imported(&closed, "worker").unwrap().unwrap();
        assert_eq!(receipt.digest, seed.exported.digest);
        assert_eq!(receipt.seal.unwrap().disposition, closed.tasks[0].status);
        let replay = import_unit(closed.clone(), seed, "worker", "repo").unwrap();
        assert!(!replay.persist, "sealed replay does not append");
        assert_eq!(replay.list, closed);
        for op in [
            Operation::Start,
            Operation::Update,
            Operation::Init,
            Operation::Append,
        ] {
            let mut input = request(op, Some(receipt.unit_ref.unit_id.clone()));
            if matches!(op, Operation::Init | Operation::Append) {
                input.task = None;
                input.items = Some(vec![Item::Text("Escape".into())]);
            }
            if matches!(op, Operation::Update) {
                input.contract = closed.tasks[0].unit.as_ref().map(|unit| unit.draft.clone());
            }
            assert!(
                apply(closed.clone(), input, &context("worker"))
                    .unwrap_err()
                    .to_string()
                    .contains("SEEDED_SCOPE_LOCKED"),
                "worker cannot alter or reopen its scope"
            );
        }
    }
}

#[test]
fn invalid_transport_and_scope_never_import() {
    let valid = seed();
    assert!(
        import_unit(TodoList::default(), valid.clone(), "worker", "other-repo").is_err(),
        "repository mismatch rejects"
    );
    assert!(
        import_unit(
            init(vec!["Existing".into()]).unwrap(),
            valid.clone(),
            "worker",
            "repo"
        )
        .is_err(),
        "import never replaces existing work"
    );
    for variant in 0..5 {
        let mut invalid = valid.clone();
        match variant {
            0 => invalid.version = 2,
            1 => invalid.execution.request_id = "different".into(),
            2 => invalid.exported.contract.deliverable = "tampered".into(),
            3 => invalid.binding.export_digest = "bad".into(),
            _ => invalid.binding.contract.contract_entry_id = "other".into(),
        }
        assert!(
            parse_seed(&serde_json::to_string(&invalid).unwrap()).is_err(),
            "invalid correlation rejects"
        );
    }
    let mut unknown = serde_json::to_value(valid).unwrap();
    unknown["pid"] = serde_json::json!(123);
    assert!(
        parse_seed(&unknown.to_string()).is_err(),
        "unknown transport fields reject"
    );
}

#[test]
fn prerequisites_are_evidence_not_dummy_completed_tasks() {
    let mut seed = seed();
    let mut contract = seed.exported.contract.clone();
    contract.depends_on = vec!["prerequisite".into()];
    seed.exported = TaskExport::create(
        contract,
        vec![Prerequisite {
            unit_id: "prerequisite".into(),
            completed_revision: 3,
        }],
    )
    .unwrap();
    seed.binding.export_digest.clone_from(&seed.exported.digest);
    let list = import_unit(TodoList::default(), seed.clone(), "worker", "repo")
        .unwrap()
        .list;
    assert_eq!(list.tasks.len(), 1);
    assert!(
        active(&list, "worker").unwrap().is_some(),
        "frozen prerequisite proof permits activation"
    );
    seed.exported = TaskExport::create(seed.exported.contract, vec![]).unwrap();
    seed.binding.export_digest.clone_from(&seed.exported.digest);
    assert!(
        import_unit(TodoList::default(), seed, "worker", "repo").is_err(),
        "missing prerequisite evidence rejects"
    );
}

#[test]
fn stored_import_cannot_change_contract_activation_or_seal() {
    let list = import_unit(TodoList::default(), seed(), "worker", "repo")
        .unwrap()
        .list;
    for variant in 0..4 {
        let mut invalid = list.clone();
        let unit = invalid.tasks[0].unit.as_mut().unwrap();
        match variant {
            0 => unit.local_session_id = Some("other".into()),
            1 => unit.reference.contract_entry_id = "local-not-origin".into(),
            2 => {
                unit.imported.as_mut().unwrap().seal = Some(Seal {
                    entry_id: "entry".into(),
                    disposition: Status::Pending,
                });
            }
            _ => invalid.tasks.push(invalid.tasks[0].clone()),
        }
        assert!(
            validate(&invalid).is_err(),
            "corrupted imported state rejects"
        );
    }
    let mut drop = request(Operation::Drop, None);
    assert!(
        apply(list.clone(), drop.clone(), &context("other")).is_err(),
        "another session cannot close import"
    );
    drop.task = Some("unknown".into());
    assert!(
        apply(list, drop, &context("worker")).is_err(),
        "unknown selector cannot close import"
    );
}
