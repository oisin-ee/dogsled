use super::*;

fn snapshot(pane: &str) -> Value {
    json!({"pane":pane,"session":"session","branch":"branch","revision":1,"current":"Work",
    "sections":[
        {"status":"in_progress","count":1,"items":[{"status":"in_progress","text":"Task","description":"Details","unit":"unit","review":true,"children":[{"status":"pending","text":"Child","unit":null}]}]},
        {"status":"pending","count":0,"items":[]},
        {"status":"completed","count":0,"items":[]},
        {"status":"abandoned","count":0,"items":[]}
    ]})
}

#[test]
fn todo_publication_uses_typed_rows_and_clears_without_files() {
    let rig = Rig::new("typed-todo");
    let todo = snapshot("pane");
    rig.put_state(&["todo-publish", "--pane", "pane"], &todo);
    assert_eq!(rig.state(&["todo", "pane"]), todo);
    let mut next = todo;
    next["revision"] = json!(2);
    next["sections"][0]["items"][0]["children"] = json!([]);
    rig.put_state(&["todo-publish", "--pane", "pane"], &next);
    assert_eq!(rig.state(&["todo", "pane"]), next);
    assert!(
        rig.run(&["todo-publish", "--pane", "pane", "--clear"])
            .status
            .success()
    );
    assert!(rig.state(&["todo", "pane"]).is_null());
}

#[test]
fn malformed_todo_publication_preserves_the_previous_rows() {
    let rig = Rig::new("bad-typed-todo");
    let todo = snapshot("pane");
    rig.put_state(&["todo-publish", "--pane", "pane"], &todo);
    let mut invalid = todo.clone();
    invalid["pane"] = json!("other");
    let mut child = rig
        .cmd(&["todo-publish", "--pane", "pane"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(
        &mut child.stdin.take().unwrap(),
        invalid.to_string().as_bytes(),
    )
    .unwrap();
    assert!(!child.wait_with_output().unwrap().status.success());
    assert_eq!(rig.state(&["todo", "pane"]), todo);
}
