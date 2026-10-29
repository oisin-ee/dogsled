//! Headline and description behavior, driven through the JSON request path.
use crate::{MutationContext, Request, TodoList, Unit, apply, sections, view, widget};
use serde_json::json;

fn context() -> MutationContext {
    MutationContext {
        session_id: "session".to_owned(),
        mutation_id: "mutation".to_owned(),
    }
}

fn parse(request: &serde_json::Value) -> Request {
    serde_json::from_value(request.clone()).unwrap()
}

fn run(list: TodoList, request: &serde_json::Value) -> Result<TodoList, crate::TodoError> {
    apply(list, parse(request), &context()).map(|change| change.list)
}

fn created() -> TodoList {
    run(
        TodoList::default(),
        &json!({"op": "init", "items": [
            {"content": "Ship", "description": " Why and how ", "subtasks": [
                {"content": "Build", "description": "Compile it"},
                "Test"
            ]},
            "Plain",
            {"content": "Blank", "description": "   "}
        ]}),
    )
    .unwrap()
}

fn id(unit: Option<&Unit>) -> String {
    unit.unwrap().reference.unit_id.clone()
}

#[test]
fn init_stores_descriptions_and_shorthand_has_none() {
    let list = created();
    assert_eq!(list.tasks[0].description.as_deref(), Some("Why and how"));
    assert_eq!(
        list.tasks[0].subtasks[0].description.as_deref(),
        Some("Compile it")
    );
    assert_eq!(list.tasks[0].subtasks[1].description, None, "string child");
    assert_eq!(list.tasks[1].description, None, "string outcome");
    assert_eq!(list.tasks[2].description, None, "blank normalises");
}

#[test]
fn append_stores_descriptions_on_new_leaves() {
    let list = created();
    let parent = id(list.tasks[0].unit.as_ref());
    let list = run(
        list,
        &json!({"op": "append", "parent": parent, "items": [
            {"content": "Extra", "description": "More detail"}
        ]}),
    )
    .unwrap();
    let extra = list.tasks[0].subtasks.last().unwrap();
    assert_eq!(extra.description.as_deref(), Some("More detail"));
}

#[test]
fn update_sets_and_clears_without_touching_content() {
    let list = created();
    let task = id(list.tasks[1].unit.as_ref());
    let set = run(
        list,
        &json!({"op": "update", "task": task, "description": " Added "}),
    )
    .unwrap();
    assert_eq!(set.tasks[1].description.as_deref(), Some("Added"));
    assert_eq!(set.tasks[1].content, "Plain");
    let kept = run(set.clone(), &json!({"op": "update", "task": task})).unwrap_err();
    assert_eq!(kept, crate::TodoError::InvalidRequest, "needs a change");
    let cleared = run(
        set,
        &json!({"op": "update", "task": task, "description": "  "}),
    )
    .unwrap();
    assert_eq!(cleared.tasks[1].description, None);
}

#[test]
fn update_edits_a_subtask_description() {
    let list = created();
    let child = id(list.tasks[0].subtasks[1].unit.as_ref());
    let list = run(
        list,
        &json!({"op": "update", "task": child, "description": "Run suite"}),
    )
    .unwrap();
    assert_eq!(
        list.tasks[0].subtasks[1].description.as_deref(),
        Some("Run suite")
    );
}

#[test]
fn legacy_state_without_description_loads_and_round_trips() {
    let saved = r#"{"tasks":[{"content":"Old","status":"pending","subtasks":[
        {"content":"Leaf","status":"completed"}]}]}"#;
    let list: TodoList = serde_json::from_str(saved).unwrap();
    assert_eq!(list.tasks[0].description, None);
    assert_eq!(list.tasks[0].subtasks[0].description, None);
    let text = serde_json::to_string(&list).unwrap();
    assert!(!text.contains("description"), "none is not serialised");
    let with = r#"{"tasks":[{"content":"A","description":"B","status":"pending"}]}"#;
    let list: TodoList = serde_json::from_str(with).unwrap();
    assert_eq!(list.tasks[0].description.as_deref(), Some("B"));
}

#[test]
fn sections_carry_descriptions_and_widget_ignores_them() {
    let list = created();
    let pending = &sections(&list)[1];
    assert_eq!(pending.items[0].description.as_deref(), Some("Why and how"));
    assert_eq!(
        pending.items[0].children[0].description.as_deref(),
        Some("Compile it")
    );
    let rows = widget(&list, true).unwrap();
    assert!(rows.iter().all(|row| !row.text.contains("Why and how")));
    assert!(rows.iter().all(|row| !row.text.contains("Compile it")));
}

#[test]
fn view_prints_descriptions_under_their_headline() {
    let text = view(&created());
    assert!(text.contains("[ ] Ship\n  Why and how\n"), "{text}");
    assert!(text.contains("  [ ] Build"), "{text}");
    let lines: Vec<_> = text.lines().collect();
    let build = lines.iter().position(|l| l.contains("[ ] Build")).unwrap();
    assert_eq!(lines[build + 1].trim(), "Compile it");
    assert!(lines[build + 1].starts_with("    "), "child detail indent");
}
