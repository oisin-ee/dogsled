//! Task validation is independent of the surrounding dependency graph.
use crate::{
    ContractDraft, MutationContext, Status, Task, TodoList, Unit, validate, validate_task,
};

fn task() -> Task {
    Task {
        content: "Build".to_owned(),
        description: Some("Recorded detail".to_owned()),
        status: Status::Pending,
        subtasks: vec![],
        unit: Some(Unit::new(
            &MutationContext {
                session_id: "session".to_owned(),
                mutation_id: "task-validation".to_owned(),
            },
            0,
            ContractDraft::default(),
        )),
    }
}

#[test]
fn task_validation_defers_dependencies_and_identity_uniqueness() {
    let mut task = task();
    task.unit.as_mut().unwrap().draft.depends_on = vec!["another-task".to_owned()];
    validate_task(&task).unwrap();
    assert!(
        validate(&TodoList {
            tasks: vec![task.clone()]
        })
        .unwrap_err()
        .to_string()
        .contains("UNIT_NOT_FOUND")
    );
    task.unit.as_mut().unwrap().draft.depends_on.clear();
    validate_task(&task).unwrap();
    assert!(
        validate(&TodoList {
            tasks: vec![
                task.clone(),
                Task {
                    content: "Other".to_owned(),
                    ..task
                }
            ]
        })
        .unwrap_err()
        .to_string()
        .contains("INVALID_IDENTITY")
    );
}

#[test]
fn task_validation_checks_labels_identity_scope_and_activation() {
    let original = task();
    for malformed in 0..4 {
        let mut changed = original.clone();
        match malformed {
            0 => changed.content.clear(),
            1 => changed.unit.as_mut().unwrap().reference.revision = 0,
            2 => changed.unit.as_mut().unwrap().draft.deliverable = Some(String::new()),
            _ => changed.unit.as_mut().unwrap().local_session_id = Some("session".to_owned()),
        }
        assert!(validate_task(&changed).is_err(), "case {malformed}");
        assert!(
            validate(&TodoList {
                tasks: vec![changed]
            })
            .is_err()
        );
    }
    validate_task(&original).unwrap();
}

#[test]
fn task_validation_rejects_a_binding_without_its_frozen_export() {
    let mut task = task();
    let unit = task.unit.as_mut().unwrap();
    unit.binding = Some(crate::TaskBinding {
        contract: unit.reference.clone(),
        execution: crate::ExecutionRef {
            executor: "dogsled".to_owned(),
            owner_id: "owner".to_owned(),
            request_id: "request".to_owned(),
        },
        export_digest: "digest".to_owned(),
    });
    assert!(
        validate_task(&task)
            .unwrap_err()
            .to_string()
            .contains("BINDING_MISMATCH: missing frozen export")
    );
}

#[test]
fn deserialization_checks_status_description_and_shape() {
    for value in [
        serde_json::json!({"content":"Build", "status":"lost"}),
        serde_json::json!({"content":"Build", "status":"pending", "description":17}),
        serde_json::json!({"content":"Build", "status":"pending", "unknown":true}),
    ] {
        serde_json::from_value::<Task>(value).unwrap_err();
    }
}
