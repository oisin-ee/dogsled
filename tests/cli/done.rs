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
    rig.put_items(&json!([{
        "id":"i1","kind":"agent","host":"local","pane":"w2:p1","owner_pane":"w1:p1",
        "name":"worker","created_ms":1,"closed_ms":null
    }]));
}

fn done(rig: &Rig) -> Output {
    rig.cmd(&["done"])
        .env("HERDR_PANE_ID", "w2:p1")
        .output()
        .unwrap()
}

#[test]
fn p3_dirty_done_still_refuses() {
    let (rig, tree) = close_rig("p3-dirty-done");
    worker(&rig, &tree, "working");
    fs::write(tree.join("draft"), b"unfinished\n").unwrap();
    let before = git(&tree, &["status", "--porcelain"]);
    let out = done(&rig);
    assert!(!out.status.success());
    assert!(!rig.dir.join("state/done.json").exists());
    assert!(!rig.dir.join("state/preservation-operations").exists());
    assert_eq!(git(&tree, &["for-each-ref", "refs/dogsled/recovery/"]), "");
    assert_eq!(before, git(&tree, &["status", "--porcelain"]));
    assert!(rig.items()[0]["reply"].is_null());
    assert!(tree.exists());
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
    assert!(rig.items()[0]["closed_ms"].is_u64());
}

/// The teardown ran after the handoff's loss check and left a commit that no ref holds. The
/// checkout, its lock, its panes, and its item stay: the removal checks the loss rule again.
#[test]
fn a_teardown_commit_in_the_handoff_keeps_the_checkout_its_lock_panes_and_item() {
    let (rig, tree) = close_rig("done-teardown-commit");
    worker(&rig, &tree, "working");
    fs::write(
        rig.dir.join("child.jsonl"),
        format!(
            "{}\n{}\n",
            json!({"type":"message","message":{"role":"user","content":"do it"}}),
            json!({"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"Done."}]}})
        ),
    )
    .unwrap();
    // A detached checkout has no branch to hold the teardown's commit.
    git(&tree, &["checkout", "--detach"]);
    let out = done(&rig);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The teardown leaves a commit that no ref holds, after the handoff checked the loss rule.
    fs::write(rig.dir.join("hook-commit"), "").unwrap();
    worker(&rig, &tree, "idle");
    let journal = String::from_utf8_lossy(&rig.run(&["serve", "--once", "--interval", "1"]).stderr)
        .into_owned();
    assert!(
        journal.contains("the teardown left unsaved work"),
        "{journal}"
    );
    assert!(
        !journal.contains("dogsled handoff: closed w2:p1"),
        "{journal}"
    );
    assert!(tree.exists());
    assert!(git(&rig.dir.join("home/repo"), &["branch", "--list", "feat/x"]).contains("feat/x"));
    let herdr = rig.log("herdr.log");
    assert!(
        !herdr.contains("workspace close") && !herdr.contains("pane close"),
        "{herdr}"
    );
    let repo = rig.dir.join("home/repo");
    assert!(git(&repo, &["worktree", "list", "--porcelain"]).contains("locked agent:worktree"));
    assert!(rig.items()[0]["closed_ms"].is_null());
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
fn unprovable_disappeared_child_does_not_block_the_next_handoff() {
    let (rig, tree) = close_rig("done-unprovable-first");
    worker(&rig, &tree, "idle");
    let mut items = rig.items();
    items.as_array_mut().unwrap().push(json!({
        "id":"missing","kind":"agent","host":"local","pane":"w9:p1",
        "owner_pane":"w1:p1","name":"disappeared","created_ms":1,"closed_ms":null
    }));
    rig.put_items(&items);
    // Herdr lost the child agent, but its non-repository shell pane remains.
    let mut panes = rig.read("panes.json");
    panes["result"]["panes"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "pane_id":"w9:p1","workspace_id":"w9","cwd":rig.dir
        }));
    rig.put("panes.json", &panes);
    rig.put(
        "state/done.json",
        &json!({
            "0-missing": {
                "pane":"w9:p1","item":"missing","after":9,"sealed":10,"marked_ms":1,
                "native_done":true,
                "reply":{"seq":10,"text":"unproven checkout","captured_ms":2,"done":true}
            },
            "term_child": {
                "pane":"w2:p1","after":9,"sealed":10,"marked_ms":1,"native_done":true,
                "reply":{"seq":10,"text":"sealed handoff","captured_ms":2,"done":true}
            }
        }),
    );
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let journal = String::from_utf8_lossy(&out.stderr);
    assert!(
        journal
            .contains("dogsled handoff: w9:p1: refuse: checkout has no unique saved launch owner"),
        "{journal}"
    );
    assert!(
        journal.contains("dogsled handoff: closed w2:p1"),
        "{journal}"
    );
    assert!(!tree.exists());
    let items = rig.items();
    assert!(items[0]["closed_ms"].is_u64(), "{items}");
    assert!(items[1]["closed_ms"].is_null(), "{items}");
    let marks = rig.read("state/done.json");
    let missing = marks
        .as_object()
        .unwrap()
        .values()
        .find(|m| m["pane"] == "w9:p1")
        .unwrap();
    assert!(missing["closed_ms"].is_null(), "{missing}");
}

#[test]
fn done_refuses_a_queued_ticket_before_writing() {
    let (rig, tree) = close_rig("done-queued");
    worker(&rig, &tree, "working");
    rig.put_tickets(&json!([{
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

/// The lifecycle state a host publishes for its agents. The host derives it from the
/// evidence it holds; a reader must never have to derive it again.
fn published_lifecycle(rig: &Rig, pane: &str) -> Value {
    let host = rig.read("state/host.json");
    host["probe"]["agent_panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["pane"] == pane)
        .cloned()
        .unwrap()
}

/// The host publishes `working` for an agent in a turn, and `idle` for a turn that ended
/// with no signal. Herdr's own `done` never becomes the final handoff.
#[test]
fn the_host_publishes_a_lifecycle_state_beside_the_raw_terminal_state() {
    let (rig, tree) = close_rig("done-lifecycle");
    worker(&rig, &tree, "working");
    rig.snapshot("state/host.json", now(), 0.0, 8);
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let published = published_lifecycle(&rig, "w2:p1");
    assert_eq!(published["state"], "working", "{published}");
    assert_eq!(published["lifecycle"], "working", "{published}");
    assert!(published["lifecycle_gap"].is_null(), "{published}");

    // A turn that ended with no signal is `stopped`. The board's own word stays `idle`.
    worker(&rig, &tree, "idle");
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let published = published_lifecycle(&rig, "w2:p1");
    assert_eq!(published["state"], "idle", "{published}");
    assert_eq!(published["lifecycle"], "stopped", "{published}");

    // Herdr's `done` means only that the turn ended. It is never the final handoff.
    worker(&rig, &tree, "done");
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let published = published_lifecycle(&rig, "w2:p1");
    assert_eq!(published["state"], "done", "{published}");
    assert_eq!(published["lifecycle"], "stopped", "{published}");
    assert_ne!(published["lifecycle"], "done", "{published}");
}

/// A child that called `done` and ended its turn, but whose final reply is not saved yet,
/// has an accepted handoff, not a recorded one. The host publishes `finishing`, and the raw
/// terminal state stays as it was.
#[test]
fn an_accepted_handoff_publishes_finishing_until_the_final_reply_is_saved() {
    let (rig, tree) = close_rig("done-handoff");
    worker(&rig, &tree, "idle");
    rig.done("term_child", "w2:p1", 10);
    rig.snapshot("state/host.json", now(), 0.0, 8);
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let published = published_lifecycle(&rig, "w2:p1");
    assert_eq!(published["state"], "idle", "{published}");
    assert_eq!(published["handoff"], "accepted", "{published}");
    assert_eq!(published["lifecycle"], "finishing", "{published}");
    assert_ne!(published["lifecycle"], "done", "{published}");
}

/// A child that called `done`, ended its turn and saved its final reply has a recorded
/// final handoff. The host publishes `done`, and the raw terminal state stays as it was.
#[test]
fn a_recorded_final_handoff_publishes_done_and_keeps_the_raw_terminal_state() {
    let (rig, tree) = close_rig("done-handoff-recorded");
    worker(&rig, &tree, "idle");
    rig.done("term_child", "w2:p1", 10);
    // The mark keeps the final reply of the sealed turn.
    rig.put(
        "state/done.json",
        &json!({"terminal": {
            "pane":"w2:p1","after":9,"sealed":10,"marked_ms":1,
            "reply":{"seq":10,"text":"the final reply","captured_ms":2,"done":true}
        }}),
    );
    rig.snapshot("state/host.json", now(), 0.0, 8);
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let published = published_lifecycle(&rig, "w2:p1");
    assert_eq!(published["state"], "idle", "{published}");
    assert_eq!(published["lifecycle"], "done", "{published}");
    assert_eq!(published["handoff"], "recorded", "{published}");

    // Closing an ordinary child must not turn its saved handoff into an expired unit.
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_number());
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let published = published_lifecycle(&rig, "w2:p1");
    assert_eq!(published["state"], "idle", "{published}");
    assert_eq!(published["lifecycle"], "done", "{published}");
    assert_eq!(published["handoff"], "recorded", "{published}");
}
