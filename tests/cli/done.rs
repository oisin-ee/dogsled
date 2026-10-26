use super::close::{close_rig, git};
use super::*;

fn worker(rig: &Rig, tree: &Path, state: &str) {
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{
            "pane_id":"w2:p1","agent":"pi","agent_status":state,"name":"worker",
            "cwd":tree,"terminal_id":"term_child","state_change_seq":10,
            "agent_session":{"kind":"path","value":rig.dir.join("child.jsonl")}
        }]}}),
    );
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[{
            "pane_id":"w2:p1","workspace_id":"w2","agent":"pi","agent_status":state,"cwd":tree
        }]}}),
    );
    rig.put(
        "state/items.json",
        &json!([{
            "id":"i1","kind":"agent","host":"local","pane":"w2:p1","owner_pane":"w1:p1",
            "name":"worker","created_ms":1,"closed_ms":null
        }]),
    );
}

fn done(rig: &Rig) -> Output {
    rig.cmd(&["done"])
        .env("HERDR_PANE_ID", "w2:p1")
        .output()
        .unwrap()
}

#[test]
fn done_refuses_uncommitted_untracked_deleted_and_unheld_work_without_a_mark() {
    let (rig, tree) = close_rig("done-dirt");
    worker(&rig, &tree, "working");
    fs::write(tree.join("scratch"), "untracked").unwrap();
    let out = done(&rig);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("Commit or preserve"));
    assert!(!rig.dir.join("state/done.json").exists());
    fs::remove_file(tree.join("scratch")).unwrap();
    fs::write(tree.join("dogsled.toml"), "changed").unwrap();
    assert!(!done(&rig).status.success());
    assert!(!rig.dir.join("state/done.json").exists());
    fs::remove_file(tree.join("dogsled.toml")).unwrap();
    assert!(!done(&rig).status.success());
    assert!(!rig.dir.join("state/done.json").exists());
    git(&tree, &["checkout", "HEAD", "--", "dogsled.toml"]);
    git(&tree, &["checkout", "--detach"]);
    // A detached checkout has no branch to land, so a commit no ref holds is unheld work.
    fs::write(tree.join("detached"), "work no ref holds\n").unwrap();
    git(&tree, &["add", "detached"]);
    git(&tree, &["commit", "-q", "-m", "detached work"]);
    let out = done(&rig);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not held"), "{stderr}");
    assert!(stderr.contains("Preserve the work"), "{stderr}");
    assert!(!rig.dir.join("state/done.json").exists());
}

/// A child in a detached checkout whose commit the default branch holds. Its handoff removes
/// the checkout, names no branch, and leaves the branch list alone.
#[test]
fn done_hands_off_a_held_detached_checkout_without_inventing_a_branch() {
    let (rig, tree) = close_rig("done-detached");
    worker(&rig, &tree, "working");
    // A transcript, so the handoff can capture the child's final reply.
    fs::write(
        rig.dir.join("child.jsonl"),
        format!(
            "{}\n{}\n",
            json!({"type":"message","message":{"role":"user","content":"do it"}}),
            json!({"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"Done."}]}})
        ),
    )
    .unwrap();
    git(&tree, &["checkout", "--detach"]);
    let out = done(&rig);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The mark records a detached checkout: no branch, and no branch to land.
    let marks = rig.read("state/done.json");
    let mark = marks.as_object().unwrap().values().next().unwrap();
    assert_eq!(mark["detached"], true, "{mark}");
    assert!(mark["branch"].is_null(), "{mark}");
    // The handoff removes the checkout and deletes no branch.
    let repo = rig.dir.join("home/repo");
    let branches = git(&repo, &["branch", "--format=%(refname:short)"]);
    worker(&rig, &tree, "idle");
    let journal = String::from_utf8_lossy(&rig.run(&["serve", "--once", "--interval", "1"]).stderr)
        .into_owned();
    assert!(
        journal.contains("dogsled handoff: closed w2:p1"),
        "{journal}"
    );
    assert!(!tree.exists());
    assert_eq!(
        git(&repo, &["branch", "--format=%(refname:short)"]),
        branches,
        "the handoff must not invent or delete a branch for a detached checkout"
    );
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_u64());
}

#[test]
fn inbox_after_done_turn_is_rejected_but_during_turn_cancels_done() {
    let (rig, tree) = close_rig("done-inbox");
    worker(&rig, &tree, "working");
    rig.snapshot("state/host.json", now(), 0.0, 8);
    let mut host = rig.read("state/host.json");
    host["probe"]["agent_panes"] = json!([{"pane":"w2:p1","agent":"pi","state":"working","name":"worker","terminal_id":"term_child"}]);
    rig.put("state/host.json", &host);
    fs::create_dir_all(rig.dir.join("state/inbox")).unwrap();
    rig.put(
        "state/inbox/term_child.reader.json",
        &json!({"pane":"w2:p1","harness":"pi","registered_ms":1}),
    );
    assert!(done(&rig).status.success());
    let (code, _) = rig.json(&["send", "worker", "--message", "more work during this turn"]);
    assert_eq!(code, 0);
    assert_eq!(rig.read("state/done.json"), json!({}));
    // The existing pending record also cancels a new mark if the child claims it mid-turn.
    assert!(done(&rig).status.success());
    let out = rig
        .cmd(&["inbox", "claim", "--harness", "pi"])
        .env("HERDR_PANE_ID", "w2:p1")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(rig.read("state/done.json"), json!({}));
    assert!(done(&rig).status.success());
    worker(&rig, &tree, "idle");
    // The published snapshot is deliberately stale-working; live state must win.
    let (code, refused) = rig.json(&["send", "worker", "--message", "too late"]);
    assert_eq!(code, 2);
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("worker finished; start a new child for more work")
    );
    assert_eq!(
        rig.read("state/inbox/term_child.json")
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let out = rig
        .cmd(&["inbox", "claim", "--harness", "pi"])
        .env("HERDR_PANE_ID", "w2:p1")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(rig.read("state/done.json").as_object().unwrap().len(), 1);
}

#[test]
fn done_refuses_a_queued_ticket_before_writing() {
    let (rig, tree) = close_rig("done-queued");
    worker(&rig, &tree, "working");
    rig.put("state/queue.json", &json!([{
        "id":"q","state":"pending","created_ms":1,"waiting":false,"waiting_until_ms":null,"notified":false,"result":null,"error":null,
        "request":{"id":"q","action":{"type":"place"},"host":null,"branch":null,"base":null,"cwd":tree,"repository":null,"workspace":null,"workspace_name":null,"owner":"w2:p1"}
    }]));
    let out = done(&rig);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("queued q"));
    assert!(!rig.dir.join("state/done.json").exists());
}

#[test]
fn automatic_close_keeps_primary_with_linked_agent_then_closes_after_link_disappears() {
    let (rig, _) = close_rig("done-group");
    let repo = rig.dir.join("home/repo");
    worker(&rig, &repo, "idle");
    rig.done("term_child", "w2:p1", 10);
    rig.put("workspaces.json", &json!({"result":{"workspaces":[
        {"workspace_id":"w2","label":"primary","worktree":{"repo_root":repo,"repo_key":"repo","is_linked_worktree":false}},
        {"workspace_id":"w3","label":"linked","worktree":{"repo_root":repo,"repo_key":"repo","is_linked_worktree":true}}
    ]}}));
    let out = rig.run(&["host", "close", "w2:p1", "--landed", "--writer"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("closing its pane would close linked workspaces w3")
    );
    assert!(!rig.log("herdr.log").contains("pane close"));
    rig.put("workspaces.json", &json!({"result":{"workspaces":[]}}));
    let out = rig.run(&["host", "close", "w2:p1", "--landed", "--writer"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(rig.log("herdr.log").contains("pane close w2:p1"));
}

#[test]
fn handoff_closes_only_its_pane_when_a_busy_agent_shares_the_checkout() {
    let (rig, tree) = close_rig("done-shared");
    worker(&rig, &tree, "idle");
    rig.done("term_child", "w2:p1", 10);
    let mut panes = rig.read("panes.json");
    panes["result"]["panes"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "pane_id":"w2:p2","workspace_id":"w2","agent":"pi","agent_status":"working","cwd":tree
        }));
    rig.put("panes.json", &panes);
    let out = rig.run(&["host", "close", "w2:p1", "--landed", "--writer"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(rig.log("herdr.log").contains("pane close w2:p1"));
    assert!(!rig.log("herdr.log").contains("workspace close"));
    assert!(tree.exists());
}
