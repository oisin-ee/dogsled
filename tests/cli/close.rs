use super::*;

/// The repository's teardown: it logs where it ran and its placeholders, and fails while
/// `$RIG/hook-fails` exists.
const TEARDOWN: &str = r#"[worktree]
teardown = [
  "printf '%s %s %s %s\\n' \"$(pwd -P)\" {main} {branch} {path} >> \"$RIG/hook.log\"",
  "if test -f \"$RIG/hook-fails\"; then printf 'teardown failed: stack busy\\n' >&2; exit 1; fi",
  "if test -f \"$RIG/hook-commit\"; then printf 'teardown work\\n' > teardown-work.txt && git add teardown-work.txt && git -c user.name=t -c user.email=t@t commit -q -m 'teardown work'; fi",
  "if test -f \"$RIG/hook-dirt\"; then printf 'teardown dirt\\n' > teardown-dirt.txt; fi",
]
"#;

/// A real repository with a `dogsled.toml` teardown and a locked linked worktree on `feat/x`.
/// Fake Herdr serves `panes.json` and `worktrees.json`, which shows the worktree in workspace w2.
pub(super) fn close_rig(name: &str) -> (Rig, PathBuf) {
    let rig = Rig::new(name);
    let repo = rig.dir.join("home/repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    fs::write(repo.join("dogsled.toml"), TEARDOWN).unwrap();
    git(&repo, &["add", "dogsled.toml"]);
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
  'pane process-info'*)
    if test -f "$RIG/process-info.json"; then cat "$RIG/process-info.json"
    else printf '%s\n' '{"result":{"process_info":{"shell_pid":10,"foreground_process_group_id":10,"foreground_processes":[{"pid":10,"name":"zsh","argv":["zsh"]}]}}}'; fi ;;
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

pub(super) fn git(dir: &Path, args: &[&str]) -> String {
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
pub(super) fn hook_log(rig: &Rig, tree: &Path) -> String {
    let main = rig.dir.join("home/repo").canonicalize().unwrap();
    format!(
        "{} {} feat/x {}\n",
        tree.display(),
        main.display(),
        tree.display()
    )
}

pub(super) fn branch_exists(rig: &Rig, branch: &str) -> bool {
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
    // The caller's agent shares the checkout: only the child's pane closes.
    let (code, closed) = rig.json(&["close", "w1:p2"]);
    assert_eq!(code, 0, "{closed}");
    assert_eq!(closed["worktree"], Value::Null);
    assert!(rig.log("herdr.log").contains("pane close w1:p2"));
    assert_eq!(rig.log("hook.log"), "");
    assert!(tree.exists());
    fs::write(rig.dir.join("herdr.log"), "").unwrap();
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
fn a_child_in_an_unlocked_worktree_loses_its_pane_and_never_the_checkout() {
    // The checkout belongs to someone else, such as the owner; the child only ran in it.
    let (rig, tree) = close_rig("close-unlocked");
    let repo = rig.dir.join("home/repo");
    git(&repo, &["worktree", "unlock", tree.to_str().unwrap()]);
    fs::write(tree.join("draft"), "the owner's work").unwrap();
    item(&rig, "local");
    panes(&rig, "idle", &tree);
    let out = rig.run(&["close", "w2:p1", "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let closed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(closed["worktree"], Value::Null);
    assert!(rig.log("herdr.log").contains("pane close w2:p1"));
    assert!(!rig.log("herdr.log").contains("workspace close"));
    assert_eq!(rig.log("hook.log"), "");
    assert_eq!(
        fs::read_to_string(tree.join("draft")).unwrap(),
        "the owner's work"
    );
    assert!(git(&repo, &["worktree", "list"]).contains(".worktrees/x"));
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
    let herdr = rig.log("herdr.log");
    assert!(!herdr.contains("pane close") && !herdr.contains("workspace close"));
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

/// The teardown ran after the loss check and left a commit that no ref holds. The checkout,
/// its lock, its panes, and its item stay: the removal checks the loss rule again.
#[test]
fn a_teardown_commit_no_ref_holds_keeps_the_checkout_its_lock_panes_and_item() {
    let (rig, tree) = close_rig("close-teardown-commit");
    advance_main(&rig);
    panes(&rig, "idle", &tree);
    item(&rig, "local");
    fs::write(rig.dir.join("hook-commit"), "").unwrap();
    let out = rig.run(&["close", "w2:p1"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("the teardown left unsaved work"),
        "{stderr}"
    );
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

/// The teardown ran after the loss check and left a file that no ref holds. The checkout,
/// its lock, its panes, and its item stay: the removal checks the loss rule again.
#[test]
fn a_teardown_file_no_ref_holds_keeps_the_checkout_its_lock_panes_and_item() {
    let (rig, tree) = close_rig("close-teardown-dirt");
    advance_main(&rig);
    panes(&rig, "idle", &tree);
    item(&rig, "local");
    fs::write(rig.dir.join("hook-dirt"), "").unwrap();
    let out = rig.run(&["close", "w2:p1"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("the teardown left unsaved work"),
        "{stderr}"
    );
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

/// A launch receipt with no result, as an interrupted or failed launch leaves it.
pub(super) fn unfinished_launch(rig: &Rig, id: &str, tree: &Path, pane: &str) {
    fs::create_dir_all(rig.dir.join("state/launches")).unwrap();
    rig.put(
        &format!("state/launches/{id}.progress.json"),
        &json!({"request":{"id":id,"action":{"type":"agent","kind":"pi","name":"worker","message":"go"},
            "host":"local","cwd":tree,"pane":null,"workspace":"w2","workspace_name":"x","owner":"w1:p1"},
            "pane":pane,"terminal":"term_gone","attempted":true,"confirmed":false,
            "result":{"host":"local","name":"worker","pane":pane},"error":"startup response lost"}),
    );
}

#[test]
fn host_stopped_orphan_flag_cannot_override_launch_progress() {
    for progress in ["allocated", "attempted", "ambiguous"] {
        let (rig, tree) = close_rig(&format!("close-stopped-launch-{progress}"));
        panes(&rig, "idle", &tree);
        rig.put(
            "agents.json",
            &json!({"result":{"agents":[{
                "pane_id":"w2:p1","terminal_id":"term_child","agent":"pi","agent_status":"idle",
                "agent_session":{"kind":"id","value":"child-session"},"state_change_seq":1
            }]}}),
        );
        unfinished_launch(&rig, "pending", &tree, "w2:p1");
        if progress == "allocated" {
            let mut record = rig.read("state/launches/pending.progress.json");
            record["attempted"] = json!(false);
            rig.put("state/launches/pending.progress.json", &record);
        } else if progress == "ambiguous" {
            unfinished_launch(&rig, "competing", &tree, "w2:p1");
        }
        let episode = json!({"pane":"w2:p1","agent_session":"child-session",
            "turn":{"state_change_seq":1,"state_since_ms":null}});
        let out = rig.run(&[
            "host",
            "close",
            "w2:p1",
            "--stopped-orphan",
            &episode.to_string(),
        ]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{progress}: {stderr}");
        assert!(
            stderr.contains("stopped orphan session or turn changed"),
            "{progress}: {stderr}"
        );
        assert!(tree.exists(), "{progress}");
        let log = rig.log("herdr.log");
        assert!(
            !log.contains("workspace close") && !log.contains("pane close"),
            "{progress}: {log}"
        );
        assert!(rig.log("hook.log").is_empty(), "{progress}");
    }
}

#[test]
fn host_stopped_orphan_close_fails_closed_when_lifecycle_facts_cannot_be_read() {
    for source in ["launch", "snapshot", "done", "probe"] {
        let (rig, tree) = close_rig(&format!("close-stopped-facts-{source}"));
        panes(&rig, "idle", &tree);
        rig.put(
            "agents.json",
            &json!({"result":{"agents":[{
                "pane_id":"w2:p1","terminal_id":"term_child","agent":"pi","agent_status":"idle",
                "agent_session":{"kind":"id","value":"child-session"},"state_change_seq":1
            }]}}),
        );
        match source {
            "launch" => {
                fs::create_dir_all(rig.dir.join("state/launches")).unwrap();
                fs::write(rig.dir.join("state/launches/bad.progress.json"), "{").unwrap();
            }
            "snapshot" => fs::write(rig.dir.join("state/host.json"), "{").unwrap(),
            "done" => fs::write(rig.dir.join("state/done.json"), "{").unwrap(),
            "probe" => fs::write(rig.dir.join("agents.json"), "{").unwrap(),
            _ => unreachable!(),
        }
        let episode = json!({"pane":"w2:p1","agent_session":"child-session",
            "turn":{"state_change_seq":1,"state_since_ms":null}});
        let out = rig.run(&[
            "host",
            "close",
            "w2:p1",
            "--stopped-orphan",
            &episode.to_string(),
        ]);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{source}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(tree.exists(), "{source}");
        let log = rig.log("herdr.log");
        assert!(
            !log.contains("workspace close") && !log.contains("pane close"),
            "{source}: {log}"
        );
        assert!(rig.log("hook.log").is_empty(), "{source}");
    }
}

#[test]
fn an_unfinished_launch_holds_its_checkout_only_with_the_matching_agent() {
    let (rig, tree) = close_rig("close-unfinished-launch");
    advance_main(&rig);
    panes(&rig, "idle", &tree);
    item(&rig, "local");
    // Startup has no receipt yet, but its matching agent still runs in w2:p2.
    unfinished_launch(&rig, "live", &tree, "w2:p2");
    rig.put("agents.json", &json!({"result":{"agents":[{
        "pane_id":"w2:p2","terminal_id":"term_gone","name":"worker","agent":"pi","agent_status":"idle"
    }]}}));
    let out = rig.run(&["close", "w2:p1"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("pending preparation or launch"), "{stderr}");
    assert!(tree.exists());
    // Its agent is gone: an empty shell cannot finish startup and no longer holds the path.
    rig.put("agents.json", &json!({"result":{"agents":[]}}));
    let (code, closed) = rig.json(&["close", "w2:p1"]);
    assert_eq!(code, 0, "{closed}");
    assert_eq!(closed["worktree"]["branch_deleted"], true);
    assert!(!tree.exists());
}

#[test]
fn a_settled_preparation_no_longer_holds_its_landed_checkout() {
    let (rig, tree) = close_rig("close-settled-preparation");
    advance_main(&rig);
    panes(&rig, "idle", &tree);
    item(&rig, "local");
    let repo = rig.dir.join("home/repo");
    let hold = |id: Option<&str>| {
        let mut hold = json!({"repo":repo,"path":tree,"branch":"feat/x","head":""});
        if let Some(id) = id {
            hold["request_id"] = json!(id);
        }
        hold
    };
    // Each request settled without releasing its hold: Herdr refused the start, Herdr refused
    // the prepared workspace, and a legacy path-only record's checkout launched.
    unfinished_launch(&rig, "refused", &tree, "w9:p1");
    rig.put(
        "state/launches/refused.json",
        &json!({"failed":"herdr agent start: agent_pane_busy"}),
    );
    unfinished_launch(&rig, "started", &tree, "w9:p2");
    rig.put(
        "state/launches/started.json",
        &json!({"host":"local","name":"worker","pane":"w9:p2"}),
    );
    fs::create_dir_all(rig.dir.join("state/admissions")).unwrap();
    rig.put(
        "state/admissions/failed.json",
        &json!({"request":{},"host":{"id":"local","label":"local"},"launch":null,
            "error":"Herdr workspace w2 already shows the checkout; a child may own it"}),
    );
    // A launch with no final receipt whose own terminal still runs in its pane is unresolved,
    // and its preparation still holds.
    unfinished_launch(&rig, "live", &tree, "w9:p3");
    let mut listed = rig.read("panes.json");
    listed["result"]["panes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"pane_id":"w9:p3","workspace_id":"w9","terminal_id":"term_gone"}));
    rig.put("panes.json", &listed);
    let holds = json!([
        hold(Some("refused")),
        hold(Some("failed")),
        hold(None),
        hold(Some("live"))
    ]);
    rig.put("state/preparations.json", &holds);
    let out = rig.run(&["close", "w2:p1"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("pending preparation or launch"), "{stderr}");
    assert!(tree.exists());
    // Once that launch is final too, nothing holds the landed checkout. Settled records are
    // ignored as evidence, never edited.
    rig.put(
        "state/launches/live.json",
        &json!({"host":"local","name":"worker","pane":"w9:p3"}),
    );
    let (code, closed) = rig.json(&["close", "w2:p1"]);
    assert_eq!(code, 0, "{closed}");
    assert_eq!(closed["worktree"]["branch_deleted"], true);
    assert!(!tree.exists());
    assert_eq!(rig.read("state/preparations.json"), holds);
}

#[test]
fn explicit_close_retires_only_the_unfinished_launch_pane_without_inventing_a_receipt() {
    let (rig, tree) = close_rig("close-own-unfinished-launch");
    advance_main(&rig);
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[
            {"pane_id":"w2:p2","terminal_id":"term_gone","workspace_id":"w2","cwd":tree}
        ]}}),
    );
    unfinished_launch(&rig, "own", &tree, "w2:p2");
    let before = rig.read("state/launches/own.progress.json");
    let holds = json!([{"repo":rig.dir.join("home/repo"),"path":tree,"branch":"feat/x","head":""}]);
    rig.put("state/preparations.json", &holds);
    rig.put(
        "state/items.json",
        &json!([{
            "id":"remote-item","kind":"agent","host":"remote","pane":"w2:p2",
            "owner_pane":"w1:p1","created_ms":1
        }]),
    );
    let (code, result) = rig.json(&["close", "w2:p2", "--host", "local"]);
    assert_eq!(code, 0, "{result}");
    assert_eq!(result["pane_closed"], true);
    assert!(result["worktree"].is_null());
    assert_eq!(rig.read("state/preparations.json"), holds);
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_null());
    assert!(tree.exists());
    assert!(branch_exists(&rig, "feat/x"));
    assert!(!rig.dir.join("hook.log").exists());
    assert_eq!(rig.read("state/launches/own.progress.json"), before);
    assert!(!rig.dir.join("state/launches/own.json").exists());
    assert!(!rig.log("herdr.log").contains("agent start"));
    rig.put("panes.json", &json!({"result":{"panes":[]}}));
    let request = before["request"].to_string();
    let (code, _) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "1",
        "--request",
        &request,
    ]);
    assert_eq!(code, 2);
    assert_eq!(rig.read("state/launches/own.progress.json"), before);
    assert!(!rig.log("herdr.log").contains("agent start"));
    assert!(!rig.log("herdr.log").contains("tab create"));
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[{
            "pane_id":"w2:p2","terminal_id":"replacement","cwd":tree
        }]}}),
    );
    assert_eq!(
        rig.json(&[
            "host",
            "launch",
            "--job-validity",
            "1",
            "--request",
            &request
        ])
        .0,
        2
    );
    assert_eq!(rig.read("state/launches/own.progress.json"), before);
    assert!(!rig.log("herdr.log").contains("agent start"));
}

#[test]
fn interrupted_launch_recovery_refuses_ambiguous_or_changed_ownership() {
    for conflict in [
        "other-launch",
        "terminal",
        "agent",
        "item",
        "focused",
        "automatic",
        "missing-process",
        "foreign-pgid",
        "malformed",
        "close-fails",
    ] {
        let (rig, tree) = close_rig(&format!("close-launch-{conflict}"));
        let mut pane =
            json!({"pane_id":"w2:p2","terminal_id":"term_gone","workspace_id":"w2","cwd":tree});
        unfinished_launch(&rig, "own", &tree, "w2:p2");
        match conflict {
            "other-launch" => unfinished_launch(&rig, "other", &tree, "w2:p2"),
            "terminal" => pane["terminal_id"] = json!("replacement"),
            "agent" => {
                pane["agent"] = json!("pi");
                pane["agent_status"] = json!("idle");
            }
            "item" => rig.put("state/items.json", &json!([{
                "id":"different","kind":"agent","host":"local","pane":"w2:p2",
                "owner_pane":"w1:p1","created_ms":1
            }])),
            "missing-process" => rig.put("process-info.json", &json!({"result":{"process_info":{}}})),
            "foreign-pgid" => rig.put("process-info.json", &json!({"result":{"process_info":{
                "shell_pid":10,"foreground_process_group_id":99,"foreground_processes":[{"pid":10,"name":"zsh","argv":["zsh"]}]
            }}})),
            "malformed" => fs::write(rig.dir.join("state/launches/bad.progress.json"), "{").unwrap(),
            "close-fails" => {
                let script = fs::read_to_string(rig.dir.join("bin/herdr")).unwrap();
                rig.script("herdr", &script.replace("'workspace close'*|'pane close'*) printf '{}\\n' ;;", "'pane close'*) exit 2 ;;\n  'workspace close'*) printf '{}\\n' ;;"));
            }
            "focused" => pane["focused"] = json!(true),
            _ => (),
        }
        rig.put("panes.json", &json!({"result":{"panes":[pane]}}));
        let before = rig.read("state/launches/own.progress.json");
        let args = if conflict == "automatic" {
            vec!["host", "close", "w2:p2", "--pushed"]
        } else {
            vec!["close", "w2:p2"]
        };
        let out = rig.run(&args);
        assert_eq!(out.status.code(), Some(2), "{conflict}: {out:?}");
        assert!(tree.exists());
        if conflict != "close-fails" {
            assert!(!rig.log("herdr.log").contains("pane close"));
        }
        assert!(!rig.log("herdr.log").contains("workspace close"));
        assert_eq!(rig.read("state/launches/own.progress.json"), before);
    }
}

#[test]
fn final_unattempted_launch_recovers_but_confirmed_unreceipted_launch_cannot_fall_through() {
    for confirmed in [false, true] {
        let (rig, tree) = close_rig("close-unresolved-plain-pane");
        let repo = rig.dir.join("home/repo").canonicalize().unwrap();
        rig.put(
            "panes.json",
            &json!({"result":{"panes":[{
                "pane_id":"w2:p2","terminal_id":"term_gone","workspace_id":"w2","cwd":repo
            }]}}),
        );
        unfinished_launch(&rig, "own", &repo, "w2:p2");
        let mut progress = rig.read("state/launches/own.progress.json");
        progress["attempted"] = json!(confirmed);
        progress["confirmed"] = json!(confirmed);
        rig.put("state/launches/own.progress.json", &progress);
        let out = rig.run(&["close", "w2:p2"]);
        assert_eq!(out.status.success(), !confirmed, "{out:?}");
        assert_eq!(rig.log("herdr.log").contains("pane close"), !confirmed);
        assert!(tree.exists());
        assert_eq!(rig.read("state/launches/own.progress.json"), progress);
        assert!(!rig.dir.join("state/launches/own.json").exists());
    }
}

#[test]
fn close_keeps_local_only_work_and_deletes_a_branch_at_the_default_tip() {
    let (rig, tree) = close_rig("close-unmerged");
    fs::write(tree.join("work"), "unmerged\n").unwrap();
    git(&tree, &["add", "work"]);
    git(&tree, &["commit", "-q", "-m", "unmerged work"]);
    panes(&rig, "idle", &tree);
    let out = rig.run(&["close", "w2:p1"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("commits are not held"));
    assert!(tree.exists());
    assert!(branch_exists(&rig, "feat/x"));
    // A branch at the default tip loses nothing without a preparation hold.
    let (rig, tree) = close_rig("close-new-branch");
    panes(&rig, "idle", &tree);
    let (code, closed) = rig.json(&["close", "w2:p1"]);
    assert_eq!(code, 0, "{closed}");
    assert_eq!(closed["worktree"]["branch_deleted"], true);
    assert!(!branch_exists(&rig, "feat/x"));
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
    git(&repo, &["branch", "feat/x"]);
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
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{"pane_id":"w2:p1","agent":"pi","agent_status":"done",
            "terminal_id":"term_child","completion_seq":2,"state_change_seq":2}]}}),
    );
    rig.done("term_child", "w2:p1", 2);
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
    assert_eq!(entry(&plan)["removable"], true);
    assert!(
        entry(&plan)["detail"]
            .as_str()
            .unwrap()
            .contains("applied to main by")
    );
    // The child is closed by its owner's `dogsled close`; only its shell tab remains.
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
    // The squashed branch is not an ancestor, but dogsled's integration check finds it applied.
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
            .contains("commits are not held"),
        "{entry}"
    );
    assert_eq!(entry["removable"], false);
}

#[test]
fn doctor_removes_a_worktree_with_no_commits_without_an_age_rule() {
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
    assert_eq!(found(&rig).unwrap()["removable"], true);
    fs::write(rig.dir.join("clock"), (now() + 2 * 86_400_000).to_string()).unwrap();
    let entry = found(&rig).unwrap();
    assert_eq!(entry["detail"], "branch feat/x merged into main; no holds");
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
            (
                "local".into(),
                "commits are not held by the default branch or a refreshed remote ref".into(),
                false
            ),
            (
                "pushed".into(),
                "every commit is on a refreshed remote ref".into(),
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

/// A locked detached worktree with one commit, as an agent checkout whose branch is gone. It
/// has no branch to land, so its commits must be held by the default branch or a remote.
fn detached_rig(name: &str) -> (Rig, PathBuf) {
    let (rig, _) = close_rig(name);
    let repo = rig.dir.join("home/repo");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "--lock",
            "--reason",
            "agent:worktree",
            "--detach",
            ".worktrees/detached",
        ],
    );
    let detached = repo.join(".worktrees/detached").canonicalize().unwrap();
    fs::write(detached.join("work"), "detached work").unwrap();
    git(&detached, &["add", "work"]);
    git(&detached, &["commit", "-q", "-m", "detached work"]);
    // Herdr shows the detached checkout in its own workspace, as any agent checkout.
    let mut worktrees = rig.read("worktrees.json");
    worktrees["result"]["worktrees"]
        .as_array_mut()
        .unwrap()
        .push(json!({"path":detached,"open_workspace_id":"w3","is_linked_worktree":true}));
    rig.put("worktrees.json", &worktrees);
    (rig, detached)
}

/// The detached checkout's own commit, which no ref holds until a test says so.
fn detached_head(rig: &Rig) -> String {
    git(
        &rig.dir.join("home/repo/.worktrees/detached"),
        &["rev-parse", "HEAD"],
    )
    .trim()
    .to_string()
}

/// A pane that works in `tree`, as the child of a close.
fn detached_panes(rig: &Rig, tree: &Path) {
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[
            {"pane_id":"w1:p1","workspace_id":"w1","agent":"pi","agent_status":"working"},
            {"pane_id":"w3:p1","workspace_id":"w3","agent":"pi","agent_status":"idle","cwd":tree}
        ]}}),
    );
}

#[test]
fn close_removes_a_clean_detached_checkout_without_inventing_a_branch() {
    let (rig, tree) = detached_rig("close-detached-native");
    let repo = rig.dir.join("home/repo");
    // The default branch holds the detached commit, so nothing is lost with the checkout.
    git(
        &repo,
        &["merge", "-q", "--ff-only", detached_head(&rig).trim()],
    );
    detached_panes(&rig, &tree);
    item(&rig, "local");
    let before = git(&repo, &["branch", "--format=%(refname:short)"]);
    let (code, closed) = rig.json(&["close", "w3:p1"]);
    assert_eq!(code, 0, "{closed}");
    // The checkout is gone, and no branch was named for it or deleted.
    assert!(!tree.exists());
    assert_eq!(
        closed["worktree"]["path"],
        json!(tree.display().to_string())
    );
    assert_eq!(closed["worktree"]["branch"], "");
    assert_eq!(closed["worktree"]["branch_deleted"], false);
    assert_eq!(
        git(&repo, &["branch", "--format=%(refname:short)"]),
        before,
        "close must not invent or delete a branch for a detached checkout"
    );
    // The teardown ran in the checkout, with an empty branch placeholder.
    let main = repo.canonicalize().unwrap().display().to_string();
    assert_eq!(
        rig.log("hook.log"),
        format!("{} {}  {}\n", tree.display(), main, tree.display())
    );
    assert!(rig.log("herdr.log").contains("workspace close w3"));
}

#[test]
fn close_removes_a_detached_checkout_a_remote_holds_once_its_owner_is_gone() {
    let (rig, tree) = detached_rig("close-detached-remote");
    let repo = rig.dir.join("home/repo");
    // A refreshed remote ref holds the commit, so nothing is lost with the checkout.
    git(
        &repo,
        &[
            "update-ref",
            "refs/remotes/origin/detached",
            detached_head(&rig).trim(),
        ],
    );
    detached_panes(&rig, &tree);
    // No item: no owner holds the checkout, so the close takes it.
    let (code, closed) = rig.json(&["close", "w3:p1"]);
    assert_eq!(code, 0, "{closed}");
    assert!(!tree.exists());
    assert_eq!(closed["worktree"]["branch_deleted"], false);
    assert_eq!(
        git(&repo, &["branch", "--format=%(refname:short)"]),
        "feat/x\nmain\n"
    );
}

#[test]
fn close_refuses_a_detached_checkout_whose_commits_no_ref_holds() {
    let (rig, tree) = detached_rig("close-detached-unheld");
    // A commit no ref holds: its work would be lost with the checkout.
    fs::write(tree.join("more"), "more work").unwrap();
    git(&tree, &["add", "more"]);
    git(&tree, &["commit", "-q", "-m", "more work"]);
    detached_panes(&rig, &tree);
    item(&rig, "local");
    let out = rig.run(&["close", "w3:p1"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("commits are not held"), "{stderr}");
    assert!(tree.exists());
    assert!(!rig.log("herdr.log").contains("workspace close"));
    assert!(!rig.log("herdr.log").contains("pane close"));
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_null());
    // The checkout is untouched: its commit is still there.
    assert!(tree.join("more").exists());
}

#[test]
fn close_refuses_a_dirty_detached_checkout_before_it_removes_anything() {
    let (rig, tree) = detached_rig("close-detached-dirty");
    detached_panes(&rig, &tree);
    item(&rig, "local");
    fs::write(tree.join("scratch"), "untracked").unwrap();
    let out = rig.run(&["close", "w3:p1"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("uncommitted"), "{stderr}");
    assert!(tree.exists());
    assert!(tree.join("scratch").exists());
    assert!(!rig.log("herdr.log").contains("workspace close"));
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_null());
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
    fs::write(probe.join("work"), "detached work").unwrap();
    git(&probe, &["add", "work"]);
    git(&probe, &["commit", "-q", "-m", "detached work"]);
    let head = git(&probe, &["rev-parse", "--short=10", "HEAD"]);
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
        found["detail"]
            .as_str()
            .unwrap()
            .starts_with(&format!("detached at {} commits are not held", head.trim())),
        "{found}"
    );
    assert_eq!(found["removable"], false);
    git(
        &repo,
        &["update-ref", "refs/remotes/origin/feature", head.trim()],
    );
    fs::write(probe.join("scratch"), "x").unwrap();
    let found = entry(&rig);
    assert_eq!(
        found["detail"],
        format!(
            "detached at {} every commit is on a refreshed remote ref; uncommitted changes (1 paths): scratch; no holds",
            head.trim()
        )
    );
    assert_eq!(found["removable"], false);
    fs::remove_file(probe.join("scratch")).unwrap();
    assert_eq!(entry(&rig)["removable"], true);
    // `close` removes an agent worktree. This probe is unlocked, so it is someone's own
    // checkout: the pane closes and the checkout stays.
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[{"pane_id":"w3:p1","workspace_id":"w3","agent":"pi","agent_status":"idle","cwd":probe}]}}),
    );
    let out = rig.run(&["close", "w3:p1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(probe.exists());
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
