use super::*;

/// The repository's teardown: it logs where it ran and its placeholders, and fails while
/// `$RIG/hook-fails` exists.
const TEARDOWN: &str = r#"[worktree]
teardown = [
  "printf '%s %s %s %s\\n' \"$(pwd -P)\" {main} {branch} {path} >> \"$RIG/hook.log\"",
  "if test -f \"$RIG/hook-fails\"; then printf 'teardown failed: stack busy\\n' >&2; exit 1; fi",
]
"#;

/// A real repository with a `wrangle.toml` teardown and a locked linked worktree on `feat/x`.
/// Fake Herdr serves `panes.json` and `worktrees.json`, which shows the worktree in workspace w2.
fn close_rig(name: &str) -> (Rig, PathBuf) {
    let rig = Rig::new(name);
    let repo = rig.dir.join("home/repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    fs::write(repo.join("wrangle.toml"), TEARDOWN).unwrap();
    git(&repo, &["add", "wrangle.toml"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "--lock",
            "--reason",
            "agent:worktree",
            "-b",
            "feat/x",
            ".worktrees/x",
        ],
    );
    let tree = repo.join(".worktrees/x").canonicalize().unwrap();
    rig.put(
        "worktrees.json",
        &json!({"result":{"worktrees":[
            {"path":repo.canonicalize().unwrap(),"open_workspace_id":"w1","is_linked_worktree":false},
            {"path":tree,"open_workspace_id":"w2","is_linked_worktree":true}
        ]}}),
    );
    rig.put("workspaces.json", &json!({"result":{"workspaces":[]}}));
    rig.script(
        "herdr",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/herdr.log"
case "$*" in
  'machine list --json') cat "$RIG/machines.json" ;;
  'agent list') cat "$RIG/agents.json" ;;
  'pane list') cat "$RIG/panes.json" ;;
  'worktree list --cwd'*) cat "$RIG/worktrees.json" ;;
  'workspace list') cat "$RIG/workspaces.json" ;;
  'workspace close w2')
    if test -f "$RIG/panes-after.json"; then cp "$RIG/panes-after.json" "$RIG/panes.json"
    else printf '%s\n' '{"result":{"panes":[]}}' > "$RIG/panes.json"; fi
    sed 's/"w2"/null/' "$RIG/worktrees.json" > "$RIG/worktrees.next" && mv "$RIG/worktrees.next" "$RIG/worktrees.json"
    printf '{}\n' ;;
  'workspace close'*|'pane close'*) printf '{}\n' ;;
  'pane process-info'*) printf '%s\n' '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":10,"name":"zsh"}]}}}' ;;
  *) echo unexpected >&2; exit 2 ;;
esac
"#,
    );
    (rig, tree)
}

/// Move `main` past `feat/x`, so the branch is an ancestor of it: integrated.
fn advance_main(rig: &Rig) {
    git(
        &rig.dir.join("home/repo"),
        &["commit", "-q", "--allow-empty", "-m", "main moves on"],
    );
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

/// The teardown's log line: it ran in the worktree, with every placeholder substituted.
fn hook_log(rig: &Rig, tree: &Path) -> String {
    let main = rig.dir.join("home/repo").canonicalize().unwrap();
    format!(
        "{} {} feat/x {}\n",
        tree.display(),
        main.display(),
        tree.display()
    )
}

fn branch_exists(rig: &Rig, branch: &str) -> bool {
    !git(&rig.dir.join("home/repo"), &["branch", "--list", branch])
        .trim()
        .is_empty()
}

#[test]
fn close_runs_teardown_removes_the_worktree_and_deletes_an_integrated_branch() {
    let (rig, tree) = close_rig("close-worktree");
    advance_main(&rig);
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
    assert!(!branch_exists(&rig, "feat/x"));
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
fn failed_teardown_keeps_the_worktree_its_panes_lock_and_item() {
    let (rig, tree) = close_rig("close-hook-fails");
    advance_main(&rig);
    panes(&rig, "idle", &tree);
    item(&rig, "local");
    fs::write(rig.dir.join("hook-fails"), "").unwrap();
    let out = rig.run(&["close", "w2:p1"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("teardown command"), "{stderr}");
    assert!(stderr.contains("teardown failed: stack busy"), "{stderr}");
    assert!(tree.exists());
    assert!(branch_exists(&rig, "feat/x"));
    let herdr = rig.log("herdr.log");
    assert!(
        !herdr.contains("workspace close") && !herdr.contains("pane close"),
        "{herdr}"
    );
    let repo = rig.dir.join("home/repo");
    assert!(git(&repo, &["worktree", "list", "--porcelain"]).contains("locked agent:worktree"));
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_null());
}

#[test]
fn close_keeps_an_unmerged_branch_after_removing_its_worktree() {
    let (rig, tree) = close_rig("close-unmerged");
    fs::write(tree.join("work"), "unmerged\n").unwrap();
    git(&tree, &["add", "work"]);
    git(&tree, &["commit", "-q", "-m", "unmerged work"]);
    panes(&rig, "idle", &tree);
    let (code, closed) = rig.json(&["close", "w2:p1"]);
    assert_eq!(code, 0, "{closed}");
    assert_eq!(closed["worktree"]["branch_deleted"], false);
    assert!(!tree.exists());
    assert!(branch_exists(&rig, "feat/x"));
    // A branch still at main's tip has no work yet, so it is not integrated either.
    let (rig, tree) = close_rig("close-new-branch");
    panes(&rig, "idle", &tree);
    let (code, closed) = rig.json(&["close", "w2:p1"]);
    assert_eq!(code, 0, "{closed}");
    assert_eq!(closed["worktree"]["branch_deleted"], false);
    assert!(branch_exists(&rig, "feat/x"));
}

#[test]
fn close_shuts_the_anchor_after_its_last_child_only_when_idle() {
    let (rig, tree) = close_rig("close-anchor");
    let repo = rig.dir.join("home/repo").canonicalize().unwrap();
    fs::write(repo.join(".git/agent-worktree-anchor"), "w5\n").unwrap();
    let anchored = |rig: &Rig| {
        rig.put(
            "worktrees.json",
            &json!({"result":{"worktrees":[
                {"path":repo,"open_workspace_id":"w5","is_linked_worktree":false},
                {"path":tree,"open_workspace_id":"w2","is_linked_worktree":true}
            ]}}),
        );
    };
    anchored(&rig);
    let workspace = |agent: Value| {
        json!({"result":{"panes":[
            {"pane_id":"w2:p1","workspace_id":"w2","agent":"pi","agent_status":"done","cwd":tree},
            {"pane_id":"w5:p1","workspace_id":"w5","agent":agent,"cwd":repo}
        ]}})
    };
    rig.put(
        "workspaces.json",
        &json!({"result":{"workspaces":[{"workspace_id":"w5","focused":false,"pane_count":1}]}}),
    );
    // An agent in the anchor keeps it; the record stays for the next child's close.
    let after = |agent: Value| json!({"result":{"panes":[{"pane_id":"w5:p1","workspace_id":"w5","agent":agent,"cwd":repo}]}});
    rig.put("panes.json", &workspace(json!("pi")));
    rig.put("panes-after.json", &after(json!("pi")));
    let (code, closed) = rig.json(&["close", "w2:p1"]);
    assert_eq!(code, 0, "{closed}");
    assert_eq!(closed["worktree"].get("anchor_closed"), None);
    assert!(!rig.log("herdr.log").contains("workspace close w5"));
    assert!(repo.join(".git/agent-worktree-anchor").exists());
    // An idle anchor with one agentless pane closes after the last child.
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "--lock",
            "--reason",
            "agent:worktree",
            ".worktrees/x",
            "feat/x",
        ],
    );
    anchored(&rig);
    rig.put("panes.json", &workspace(Value::Null));
    rig.put("panes-after.json", &after(Value::Null));
    let (code, closed) = rig.json(&["close", "w2:p1"]);
    assert_eq!(code, 0, "{closed}");
    assert_eq!(closed["worktree"]["anchor_closed"], "w5");
    assert!(rig.log("herdr.log").contains("workspace close w5"));
    assert!(!repo.join(".git/agent-worktree-anchor").exists());
}

#[test]
fn prune_tries_every_entry_and_reports_each_failure() {
    let (rig, tree) = close_rig("prune-continues");
    let repo = rig.dir.join("home/repo").canonicalize().unwrap();
    // The feature is merged; its teardown fails. A stale registration comes after it.
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
    assert!(stdout.contains("failed: teardown command"), "{stdout}");
    // The command's own message.
    assert!(stdout.contains("teardown failed: stack busy"), "{stdout}");
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
    // The squashed branch is not an ancestor, but wrangle's integration check finds it applied.
    assert!(!branch_exists(&rig, "feat/x"));
    assert!(String::from_utf8_lossy(&out.stdout).contains("branch deleted"));
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

#[test]
fn doctor_reports_a_worktree_made_on_a_branch_that_already_had_commits() {
    let (rig, _) = close_rig("close-made-on-work");
    rig.config("[cleanup]\nrepositories = [\"~/repo\"]\n");
    let repo = rig.dir.join("home/repo");
    // feat/y gets a commit of its own, adding a file, in a worktree that is then removed.
    git(&repo, &["worktree", "add", "-q", "-b", "feat/y", "scratch"]);
    fs::write(repo.join("scratch/work.txt"), "work\n").unwrap();
    git(&repo.join("scratch"), &["add", "work.txt"]);
    git(&repo.join("scratch"), &["commit", "-q", "-m", "work"]);
    git(&repo, &["worktree", "remove", "scratch"]);
    git(&repo, &["worktree", "add", "-q", ".worktrees/y", "feat/y"]);
    fs::write(rig.dir.join("clock"), (now() + 2 * 86_400_000).to_string()).unwrap();
    let (_, plan) = rig.json(&["doctor"]);
    let entry = plan["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["tree"]["branch"] == "feat/y")
        .cloned()
        .unwrap();
    // Its HEAD never moved, but the branch holds a commit main does not have.
    assert!(
        entry["detail"]
            .as_str()
            .unwrap()
            .starts_with("branch feat/y has 1 commit not on main,"),
        "{entry}"
    );
    assert_eq!(entry["removable"], false);
}

#[test]
fn doctor_removes_a_worktree_with_no_commits_a_day_after_it_was_made() {
    let (rig, tree) = close_rig("close-empty");
    // The configured repository is how doctor finds it: nothing registered, no pane in it.
    rig.config("[cleanup]\nrepositories = [\"~/repo\", \"~/gone\"]\n");
    let found = |rig: &Rig| {
        let (_, plan) = rig.json(&["doctor"]);
        plan["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["class"] == "worktrees")
            .cloned()
    };
    // Main moves on, so feat/x is an ancestor of main. That is not a merge: a new worktree
    // whose HEAD never moved is ordinary work.
    git(
        &rig.dir.join("home/repo"),
        &["commit", "-q", "--allow-empty", "-m", "main moves on"],
    );
    assert_eq!(found(&rig), None);
    fs::write(rig.dir.join("clock"), (now() + 2 * 86_400_000).to_string()).unwrap();
    let entry = found(&rig).unwrap();
    assert_eq!(
        entry["detail"],
        "branch feat/x has no commits since its worktree was made 48h ago; clean and unused"
    );
    assert_eq!(entry["removable"], true);
    let (_, plan) = rig.json(&["doctor"]);
    assert!(
        plan["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["class"] == "repositories"
                && e["id"] == rig.dir.join("home/gone").display().to_string()
                && e["removable"] == false)
    );
    let out = rig.run(&["prune", "--yes", "worktrees"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(rig.log("hook.log"), hook_log(&rig, &tree));
    assert!(!tree.exists());
}

#[test]
fn doctor_finds_integrated_and_pushed_branches_without_a_worktree_and_prune_deletes_them() {
    let (rig, _) = close_rig("close-branches");
    let repo = rig.dir.join("home/repo");
    rig.put("state/repositories.json", &json!([repo]));
    let branch = |name: &str| {
        let path = rig.dir.join(format!("home/{name}"));
        git(
            &repo,
            &["worktree", "add", "-q", "-b", name, path.to_str().unwrap()],
        );
        fs::write(path.join(name), "work\n").unwrap();
        git(&path, &["add", name]);
        git(&path, &["commit", "-q", "-m", name]);
        git(&repo, &["worktree", "remove", path.to_str().unwrap()]);
    };
    for name in ["done", "local", "pushed"] {
        branch(name);
    }
    git(&repo, &["merge", "-q", "--squash", "done"]);
    git(&repo, &["commit", "-q", "-m", "done (#1)"]);
    git(
        &repo,
        &["update-ref", "refs/remotes/origin/pushed", "pushed"],
    );
    let (_, plan) = rig.json(&["doctor"]);
    let branches: Vec<_> = plan["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["class"] == "branches")
        .map(|e| {
            let id = e["id"].as_str().unwrap();
            (
                id.rsplit_once(' ').unwrap().1.to_owned(),
                e["detail"]
                    .as_str()
                    .unwrap()
                    .split(", the last")
                    .next()
                    .unwrap()
                    .to_owned(),
                e["removable"].as_bool().unwrap(),
            )
        })
        .collect();
    let done = git(&repo, &["rev-parse", "--short=10", "main"]);
    assert_eq!(
        branches,
        [
            (
                "done".into(),
                format!("applied to main by {}", done.trim()),
                true
            ),
            ("local".into(), "1 commit on no remote".into(), false),
            (
                "pushed".into(),
                "every commit is on a remote branch".into(),
                true
            ),
        ],
        "{plan}"
    );
    let out = rig.run(&["prune", "--yes", "branches"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let left = git(&repo, &["branch", "--format=%(refname:short)"]);
    assert_eq!(
        left.lines().collect::<Vec<_>>(),
        ["feat/x", "local", "main"]
    );
}

#[test]
fn doctor_lists_a_detached_worktree_and_prune_removes_it_once_a_remote_holds_its_head() {
    let (rig, _) = close_rig("close-detached");
    let repo = rig.dir.join("home/repo");
    rig.put("state/repositories.json", &json!([repo]));
    rig.put("panes.json", &json!({"result":{"panes":[]}}));
    git(
        &repo,
        &["worktree", "add", "-q", "--detach", ".worktrees/probe"],
    );
    let probe = repo.join(".worktrees/probe").canonicalize().unwrap();
    let head = git(&repo, &["rev-parse", "--short=10", "HEAD"]);
    let entry = |rig: &Rig| {
        let (_, plan) = rig.json(&["doctor"]);
        plan["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == json!(probe))
            .cloned()
            .unwrap_or_else(|| panic!("{plan}"))
    };
    let found = entry(&rig);
    assert_eq!(found["tree"]["branch"], "");
    assert!(
        found["detail"].as_str().unwrap().starts_with(&format!(
            "detached at {} with 1 commit on no remote",
            head.trim()
        )),
        "{found}"
    );
    assert_eq!(found["removable"], false);
    git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    fs::write(probe.join("scratch"), "x").unwrap();
    let found = entry(&rig);
    assert_eq!(
        found["detail"],
        format!(
            "detached at {}, which a remote branch holds; refused: uncommitted changes",
            head.trim()
        )
    );
    assert_eq!(found["removable"], false);
    fs::remove_file(probe.join("scratch")).unwrap();
    assert_eq!(entry(&rig)["removable"], true);
    // `close` removes branch worktrees; it points at prune for a detached one.
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[{"pane_id":"w3:p1","workspace_id":"w3","agent":"pi","agent_status":"idle","cwd":probe}]}}),
    );
    let out = rig.run(&["close", "w3:p1"]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("wrangle prune"));
    rig.put("panes.json", &json!({"result":{"panes":[]}}));
    let out = rig.run(&["prune", "--yes", "worktrees"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(
        stdout.contains(&format!("removed {}", probe.display())),
        "{stdout}"
    );
    assert!(!probe.exists());
    assert!(rig.log("hook.log").contains(&format!(
        "{} {}  {}",
        probe.display(),
        repo.canonicalize().unwrap().display(),
        probe.display()
    )));
}
