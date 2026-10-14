use super::*;

/// A real repository with a locked, merged linked worktree. Fake Herdr serves `panes.json` and
/// shows the worktree in workspace w2; the configured remove hook logs its call and removes
/// through Git.
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
  'pane process-info'*) printf '%s\n' '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":10,"name":"zsh"}]}}}' ;;
  *) echo unexpected >&2; exit 2 ;;
esac
"#,
    );
    rig.config("[hooks]\nremove = [\"remove-hook\", \"{branch}\", \"{path}\"]\n");
    rig.script(
        "remove-hook",
        r#"#!/bin/sh
printf '%s %s\n' "$(pwd -P)" "$*" >> "$RIG/hook.log"
if test -f "$RIG/hook-fails"; then printf 'pre-remove hook failed\n\n[remove-hook] ERROR task failed\n' >&2; exit 1; fi
git worktree unlock "$2" && git worktree remove "$2" && { git branch -q -d "$1" 2>/dev/null || true; }
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

fn registered(rig: &Rig) -> Vec<Value> {
    rig.read("state/worktrees.json")
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["path"].clone())
        .collect()
}

fn hook_log(rig: &Rig, tree: &Path) -> String {
    let main = rig.dir.join("home/repo").canonicalize().unwrap();
    format!("{} feat/x {}\n", main.display(), tree.display())
}

#[test]
fn close_removes_workspace_then_worktree_through_the_remove_hook_and_ends_the_item() {
    let (rig, tree) = close_rig("close-worktree");
    panes(&rig, "done", &tree);
    item(&rig, "local");
    let repo = rig.dir.join("home/repo").canonicalize().unwrap();
    let other = json!({"repo":repo,"path":repo.join(".worktrees/other"),"branch":"other"});
    rig.put(
        "state/worktrees.json",
        &json!([{"repo":repo,"path":tree,"branch":"feat/x"}, other]),
    );
    let (code, closed) = rig.json(&["close", "w2:p1"]);
    assert_eq!(code, 0, "{closed}");
    assert_eq!(
        closed["worktree"],
        json!({"path":tree,"branch":"feat/x","workspace":"w2","branch_deleted":true})
    );
    assert_eq!(closed["pane_closed"], false);
    assert_eq!(rig.log("hook.log"), hook_log(&rig, &tree));
    let herdr = rig.log("herdr.log");
    assert!(herdr.contains("workspace close w2"));
    assert!(!herdr.contains("pane close"));
    assert!(!tree.exists());
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_u64());
    // Close forgets the removed worktree's registration, like prune.
    assert_eq!(registered(&rig), [other["path"].clone()]);
}

#[test]
fn close_finishes_a_child_tab_in_the_callers_workspace_but_not_the_callers_worktree() {
    let (rig, tree) = close_rig("close-own-workspace");
    rig.put(
        "worktrees.json",
        &json!({"result":{"worktrees":[{"path":tree,"open_workspace_id":null}]}}),
    );
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[
            {"pane_id":"w1:p1","workspace_id":"w1","agent":"pi","agent_status":"working","cwd":tree},
            {"pane_id":"w1:p2","workspace_id":"w1","agent":"pi","agent_status":"done","cwd":tree}
        ]}}),
    );
    let out = rig.run(&["close", "w1:p2"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("the caller works in"));
    assert_eq!(rig.log("hook.log"), "");
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[
            {"pane_id":"w1:p1","workspace_id":"w1","agent":"pi","agent_status":"working"},
            {"pane_id":"w1:p2","workspace_id":"w1","agent":"pi","agent_status":"done","cwd":tree}
        ]}}),
    );
    let (code, closed) = rig.json(&["close", "w1:p2"]);
    assert_eq!(code, 0, "{closed}");
    assert_eq!(closed["pane_closed"], true);
    assert!(rig.log("herdr.log").contains("pane close w1:p2"));
    assert!(!rig.log("herdr.log").contains("workspace close"));
    assert_eq!(rig.log("hook.log"), hook_log(&rig, &tree));
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
    assert_eq!(rig.log("hook.log"), "");
    assert!(!rig.log("herdr.log").contains("close"));
    assert!(tree.exists());
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_null());
}

#[test]
fn failed_remove_hook_reports_its_error_and_keeps_the_item_open() {
    let (rig, tree) = close_rig("close-hook-fails");
    panes(&rig, "idle", &tree);
    item(&rig, "local");
    fs::write(rig.dir.join("hook-fails"), "").unwrap();
    let out = rig.run(&["close", "w2:p1"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("pre-remove hook failed"));
    assert!(tree.exists());
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_null());
}

#[test]
fn prune_tries_every_entry_and_reports_each_failure() {
    let (rig, tree) = close_rig("prune-continues");
    let repo = rig.dir.join("home/repo").canonicalize().unwrap();
    // The feature is merged; the remove hook fails for it. A stale registration comes after it.
    git(&tree, &["commit", "-q", "--allow-empty", "-m", "feature"]);
    let ahead = git(
        &tree,
        &[
            "commit-tree",
            "-p",
            "feat/x",
            "-m",
            "after the feature",
            "feat/x^{tree}",
        ],
    );
    git(
        &repo,
        &["update-ref", "refs/remotes/origin/main", ahead.trim()],
    );
    let gone = repo.join(".worktrees/gone");
    rig.put(
        "state/worktrees.json",
        &json!([{"repo":repo,"path":tree,"branch":"feat/x"},{"repo":repo,"path":gone,"branch":"gone"}]),
    );
    rig.put("panes.json", &json!({"result":{"panes":[]}}));
    fs::write(rig.dir.join("hook-fails"), "").unwrap();
    let out = rig.run(&["prune", "--yes", "worktrees"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(2), "{stdout}");
    assert!(stdout.contains(&format!(
        "dropped the stale registration of {}",
        gone.display()
    )));
    assert!(
        stdout.contains("failed: remove hook failed for"),
        "{stdout}"
    );
    // The hook's own message, not only the runner's last line.
    assert!(stdout.contains("pre-remove hook failed"), "{stdout}");
    assert!(tree.exists());
    assert_eq!(registered(&rig), [json!(tree)]);
}

#[test]
fn doctor_lists_each_linked_worktree_once_and_never_the_main_checkout() {
    let (rig, tree) = close_rig("close-repo-identity");
    let repo = rig.dir.join("home/repo");
    // The remote default branch is ahead of the main checkout, and the feature is in it.
    git(&tree, &["commit", "-q", "--allow-empty", "-m", "feature"]);
    let ahead = git(
        &tree,
        &[
            "commit-tree",
            "-p",
            "feat/x",
            "-m",
            "after the feature",
            "feat/x^{tree}",
        ],
    );
    git(
        &repo,
        &["update-ref", "refs/remotes/origin/main", ahead.trim()],
    );
    git(
        &repo,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    // The same repository is registered under its path and through a symlink.
    let link = rig.dir.join("home/link");
    std::os::unix::fs::symlink(&repo, &link).unwrap();
    rig.put("state/repositories.json", &json!([repo, link]));
    rig.put("panes.json", &json!({"result":{"panes":[]}}));
    let (_, plan) = rig.json(&["doctor"]);
    let trees: Vec<_> = plan["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["class"] == "worktrees")
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(trees, [tree.display().to_string()], "{plan}");
}

#[test]
fn doctor_finds_a_squashed_branch_held_only_by_an_idle_shell_and_prune_removes_it() {
    let (rig, tree) = close_rig("close-squash");
    let repo = rig.dir.join("home/repo");
    fs::write(tree.join("feature"), "work\n").unwrap();
    git(&tree, &["add", "feature"]);
    git(&tree, &["commit", "-q", "-m", "feature"]);
    fs::write(repo.join("other"), "main\n").unwrap();
    git(&repo, &["add", "other"]);
    git(&repo, &["commit", "-q", "-m", "main moves"]);
    git(&repo, &["merge", "-q", "--squash", "feat/x"]);
    git(&repo, &["commit", "-q", "-m", "feature (#1)"]);
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[
            {"pane_id":"w2:p1","workspace_id":"w2","agent":"pi","agent_status":"done","cwd":tree},
            {"pane_id":"w2:p2","workspace_id":"w2","agent":null,"cwd":tree}
        ]}}),
    );
    let (_, plan) = rig.json(&["doctor"]);
    let entry = |plan: &Value| {
        plan["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["class"] == "worktrees")
            .cloned()
            .unwrap_or_else(|| panic!("{plan}"))
    };
    assert_eq!(entry(&plan)["removable"], false);
    assert!(
        entry(&plan)["detail"]
            .as_str()
            .unwrap()
            .contains("applied to main by")
    );
    // The child is closed by its owner's `wrangle close`; only its shell tab remains.
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[{"pane_id":"w2:p2","workspace_id":"w2","agent":null,"cwd":tree}]}}),
    );
    let (_, plan) = rig.json(&["doctor"]);
    assert_eq!(entry(&plan)["removable"], true, "{plan}");
    let out = rig.run(&["prune", "--yes", "worktrees"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(rig.log("herdr.log").contains("workspace close w2"));
    assert_eq!(rig.log("hook.log"), hook_log(&rig, &tree));
    assert!(!tree.exists());
    // The squashed branch is not an ancestor, so `git branch -d` keeps it.
    assert!(git(&repo, &["branch", "--list", "feat/x"]).contains("feat/x"));
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
    assert_eq!(rig.log("hook.log"), "");

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
