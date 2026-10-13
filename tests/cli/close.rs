use super::*;

/// A real repository with a locked, merged linked worktree. Fake Herdr serves `panes.json` and
/// shows the worktree in workspace w2; fake `wt` logs its call and removes through Git.
fn close_rig(name: &str) -> (Rig, PathBuf) {
    let rig = Rig::new(name);
    let repo = rig.dir.join("home/repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    git(
        &repo,
        &["worktree", "add", "-q", "-b", "feat/x", ".worktrees/x"],
    );
    let tree = repo.join(".worktrees/x").canonicalize().unwrap();
    git(
        &repo,
        &[
            "worktree",
            "lock",
            "--reason",
            "agent:worktree",
            tree.to_str().unwrap(),
        ],
    );
    rig.put(
        "worktrees.json",
        &json!({"result":{"worktrees":[
            {"path":repo.canonicalize().unwrap(),"open_workspace_id":"w1"},
            {"path":tree,"open_workspace_id":"w2"}
        ]}}),
    );
    rig.script(
        "herdr",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/herdr.log"
case "$*" in
  'machine list --json') cat "$RIG/machines.json" ;;
  'agent list') cat "$RIG/agents.json" ;;
  'pane list') cat "$RIG/panes.json" ;;
  'worktree list --cwd'*) cat "$RIG/worktrees.json" ;;
  'workspace close w2') printf '%s\n' '{"result":{"panes":[]}}' > "$RIG/panes.json"; printf '{}\n' ;;
  'pane close'*) printf '{}\n' ;;
  *) echo unexpected >&2; exit 2 ;;
esac
"#,
    );
    rig.script(
        "wt",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/wt.log"
if test -f "$RIG/wt-fails"; then echo 'pre-remove hook failed' >&2; exit 1; fi
git -C "$2" worktree remove "$(git -C "$2" worktree list --porcelain | awk '/^worktree .*\.worktrees/ {print substr($0, 10)}')" &&
  git -C "$2" branch -q -d "$5"
"#,
    );
    (rig, tree)
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = isolated("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn panes(rig: &Rig, status: &str, cwd: &Path) {
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[
            {"pane_id":"w1:p1","workspace_id":"w1","agent":"pi","agent_status":"working"},
            {"pane_id":"w2:p1","workspace_id":"w2","agent":"pi","agent_status":status,"cwd":cwd},
            {"pane_id":"w2:p2","workspace_id":"w2","agent":null,"agent_status":"unknown","cwd":cwd}
        ]}}),
    );
}

fn item(rig: &Rig, host: &str) {
    rig.put(
        "state/items.json",
        &json!([{"id":"i1","kind":"agent","host":host,"pane":"w2:p1","owner_pane":"w1:p1",
            "name":"worker","created_ms":1,"closed_ms":null}]),
    );
}

#[test]
fn close_removes_workspace_then_worktree_through_wt_and_ends_the_item() {
    let (rig, tree) = close_rig("close-worktree");
    panes(&rig, "done", &tree);
    item(&rig, "local");
    let (code, closed) = rig.json(&["close", "w2:p1"]);
    assert_eq!(code, 0, "{closed}");
    assert_eq!(
        closed["worktree"],
        json!({"path":tree,"branch":"feat/x","workspace":"w2","branch_deleted":true})
    );
    assert_eq!(closed["pane_closed"], false);
    let main = rig.dir.join("home/repo").canonicalize().unwrap();
    assert_eq!(
        rig.log("wt.log"),
        format!("-C {} remove --foreground feat/x\n", main.display())
    );
    let herdr = rig.log("herdr.log");
    assert!(herdr.contains("workspace close w2"));
    assert!(!herdr.contains("pane close"));
    assert!(!tree.exists());
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_u64());
}

#[test]
fn close_refuses_active_dirty_foreign_locked_or_own_work_without_changes() {
    let (rig, tree) = close_rig("close-refuse");
    item(&rig, "local");
    for status in ["working", "blocked", "unknown"] {
        panes(&rig, status, &tree);
        let out = rig.run(&["close", "w2:p1"]);
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains(status));
    }
    panes(&rig, "idle", &tree);
    assert!(!rig.run(&["close", "w1:p1"]).status.success());
    fs::write(tree.join("note"), "x").unwrap();
    let out = rig.run(&["close", "w2:p1"]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("uncommitted"));
    fs::remove_file(tree.join("note")).unwrap();
    let repo = rig.dir.join("home/repo");
    git(&repo, &["worktree", "unlock", tree.to_str().unwrap()]);
    git(
        &repo,
        &[
            "worktree",
            "lock",
            "--reason",
            "mine",
            tree.to_str().unwrap(),
        ],
    );
    let out = rig.run(&["close", "w2:p1"]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("locked (mine)"));
    assert_eq!(rig.log("wt.log"), "");
    assert!(!rig.log("herdr.log").contains("close"));
    assert!(tree.exists());
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_null());
}

#[test]
fn failed_wt_remove_relocks_the_worktree_and_keeps_the_item_open() {
    let (rig, tree) = close_rig("close-wt-fails");
    panes(&rig, "idle", &tree);
    item(&rig, "local");
    fs::write(rig.dir.join("wt-fails"), "").unwrap();
    let out = rig.run(&["close", "w2:p1"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("pre-remove hook failed"));
    let list = git(
        &rig.dir.join("home/repo"),
        &["worktree", "list", "--porcelain"],
    );
    assert!(list.contains("locked agent:worktree"), "{list}");
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_null());
}

#[test]
fn close_shuts_a_pane_outside_any_worktree_and_routes_remote_items_over_ssh() {
    let (rig, _) = close_rig("close-pane");
    panes(&rig, "idle", &rig.dir.join("home"));
    let out = rig.run(&["close", "w2:p1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(rig.log("herdr.log").contains("pane close w2:p1"));
    assert_eq!(rig.log("wt.log"), "");

    rig.peer();
    item(&rig, "8103");
    rig.script(
        "ssh",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/ssh.log"
case "$*" in
  *'host close w2:p1'*) printf '%s\n' '{"pane":"w2:p1","worktree":null,"pane_closed":true}' ;;
  *) echo unexpected >&2; exit 2 ;;
esac
"#,
    );
    let (code, closed) = rig.json(&["close", "w2:p1"]);
    assert_eq!(code, 0, "{closed}");
    assert!(rig.log("ssh.log").contains("netcup-dev"));
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_u64());
}
