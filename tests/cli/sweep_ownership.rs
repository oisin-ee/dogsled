//! Automatic cleanup requires closed child ownership; explicit prune does not.
use super::close::git;
use super::*;

fn repository(name: &str) -> (Rig, PathBuf) {
    let (rig, repo) = place::place_rig_with(name, "[worktree]\n");
    let remote = rig.dir.join("remote.git");
    git(&repo, &["init", "-q", "--bare", remote.to_str().unwrap()]);
    git(
        &repo,
        &["remote", "set-url", "origin", remote.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    rig.put_state(&["state", "repositories", "--put"], &json!([repo]));
    (rig, repo)
}

fn quiet_host(rig: &Rig) {
    rig.put("panes.json", &json!({"result":{"panes":[]}}));
    rig.script(
        "herdr",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/herdr.log"
case "$*" in
  'machine list --json') cat "$RIG/machines.json" ;;
  'agent list') printf '{"result":{"agents":[]}}\n' ;;
  'pane list') cat "$RIG/panes.json" ;;
  'workspace list') printf '{"result":{"workspaces":[]}}\n' ;;
  'worktree list --cwd'*) printf '{"result":{"worktrees":[]}}\n' ;;
  *) echo unexpected >&2; exit 2 ;;
esac
"#,
    );
}

fn run(rig: &Rig, args: &[&str]) {
    let mut command = rig.start(args);
    let (_, stderr) = command.output();
    assert!(command.wait().success(), "{stderr}");
}

fn sweep(rig: &Rig) -> Value {
    rig.put_state(
        &["state", "sweep-request", "--put"],
        &json!("ownership-test"),
    );
    run(rig, &["serve", "--once", "--interval", "1"]);
    let report = rig.state(&["sweep"]);
    assert_eq!(report["failures"], json!([]), "{report}");
    report
}

fn prune_entry(rig: &Rig, class: &str, path: &Path) -> Value {
    let (code, plan) = rig.json(&["doctor"]);
    assert_eq!(code, 0, "{plan}");
    plan["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["class"] == class && entry["tree"]["path"] == json!(path))
        .unwrap_or_else(|| panic!("missing {class} {}: {plan}", path.display()))
        .clone()
}

/// Give cleanup fixtures the same identity-bearing registration, launch, and closed item
/// that a finished child leaves. An existing registration also supports the fake-Git rigs.
pub(super) fn record_closed_checkout(rig: &Rig, path: &Path, id: &str) {
    let mut registered = match rig.state(&["worktrees"]) {
        Value::Null => json!([]),
        registered => registered,
    };
    let captured = if let Some(tree) = registered
        .as_array()
        .unwrap()
        .iter()
        .find(|tree| tree["path"] == json!(path))
    {
        tree.clone()
    } else {
        let tree = prune_entry(rig, "worktrees", path)["tree"].clone();
        registered.as_array_mut().unwrap().push(tree.clone());
        rig.put_state(&["state", "worktrees", "--put"], &registered);
        tree
    };
    assert!(captured["identity"].is_object(), "{captured}");
    let pane = format!("closed:{id}");
    let mut items = rig.items();
    items
        .as_array_mut()
        .unwrap()
        .push(json!({"id":id,"kind":"agent","host":"local",
        "pane":pane,"owner_pane":"gone-owner","created_ms":now(),"closed_ms":now()}));
    rig.put_items(&items);
    let result = json!({"checkout":captured});
    rig.put_launch_progress(
        id,
        &json!({
            "request":{"id":id,"action":{"type":"agent","kind":"pi","name":id,"message":"go"},
                "host":"local","cwd":path,"pane":pane,"terminal":id,"owner":"gone-owner"},
            "pane":pane,"terminal":id,"attempted":true,"confirmed":true,"result":result,"error":null
        }),
    );
    rig.put_launch(id, &result);
}

pub(super) fn record_closed_branch(rig: &Rig, repo: &Path, branch: &str) {
    let mut items = rig.items();
    items
        .as_array_mut()
        .unwrap()
        .push(json!({"id":format!("branch-{branch}"),
        "kind":"agent","host":"local","pane":format!("closed:{branch}"),
        "owner_pane":"gone-owner","created_ms":now(),"closed_ms":now(),
        "branch":{"repo":repo,"name":branch,"ahead":0,"landed":false}}));
    rig.put_items(&items);
}

fn push_work(repo: &Path, tree: &Path, branch: &str) {
    fs::write(tree.join("work.txt"), "child work\n").unwrap();
    git(tree, &["add", "work.txt"]);
    git(tree, &["commit", "-q", "-m", "child work"]);
    git(repo, &["push", "-q", "origin", branch]);
}

fn child(name: &str, closed: bool) -> (Rig, PathBuf, PathBuf) {
    let (rig, repo) = repository(name);
    let (code, prepared) = rig.json(&[
        "host",
        "prepare",
        "--repo",
        repo.to_str().unwrap(),
        "--branch",
        "child",
        "--base",
        "main",
        "--request-id",
        "child-request",
    ]);
    assert_eq!(code, 0, "{prepared}");
    let tree = PathBuf::from(prepared["path"].as_str().unwrap());
    let request = json!({"id":"child-request", "validity":rig.job_validity(),
        "action":{"type":"agent","kind":"pi","name":"child","message":"work"},
        "host":"local", "cwd":tree, "pane":prepared["pane_id"],
        "terminal":prepared["terminal_id"], "workspace":prepared["workspace_id"],
        "workspace_name":"child", "owner":"w1:p1", "preparation_protocol":1,
        "preparation":prepared["binding"]});
    let (code, launched) = rig.json(&["host", "launch", "--request", &request.to_string()]);
    assert_eq!(code, 0, "{launched}");
    push_work(&repo, &tree, "child");
    rig.put_items(&json!([{"id":"child-request","kind":"agent","host":"local",
        "pane":prepared["pane_id"],"owner_pane":"w1:p1","created_ms":now(),
        "closed_ms":if closed { json!(now()) } else { Value::Null }}]));
    quiet_host(&rig);
    if !closed {
        // The item stays open, but no cwd or workspace associates the shell with its tree.
        rig.put(
            "panes.json",
            &json!({"result":{"panes":[{
                "pane_id":prepared["pane_id"], "terminal_id":prepared["terminal_id"]
            }]}}),
        );
    }
    (rig, repo, tree)
}

#[test]
fn lead_made_pushed_checkout_survives_sweep_but_explicit_prune_can_remove_it() {
    let (rig, repo) = repository("sweep-lead-checkout");
    let tree = repo.join(".worktrees/lead");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "lead",
            tree.to_str().unwrap(),
        ],
    );
    push_work(&repo, &tree, "lead");
    quiet_host(&rig);
    let before = prune_entry(&rig, "worktrees", &tree);
    assert_eq!(before["removable"], true, "{before}");
    // Discovery writes a generation marker even for an unmanaged checkout.
    assert!(before["tree"]["identity"].is_object(), "{before}");
    let report = sweep(&rig);
    assert!(tree.join(".git").exists(), "{report}");
    assert_ne!(git(&repo, &["branch", "--list", "lead"]).trim(), "");
    let after = prune_entry(&rig, "worktrees", &tree);
    assert_eq!(after["removable"], true, "{after}");
    run(&rig, &["prune", "--yes"]);
    assert!(!tree.exists());
    assert_eq!(git(&repo, &["branch", "--list", "lead"]).trim(), "");
}

#[test]
fn closed_child_pushed_checkout_is_still_swept() {
    let (rig, repo, tree) = child("sweep-closed-child", true);
    let report = sweep(&rig);
    assert!(!tree.exists(), "{report}");
    assert!(
        git(&repo, &["branch", "--list", "child"]).trim().is_empty(),
        "{report}"
    );
    assert!(
        report["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|result| { result.as_str().unwrap().contains("branch deleted") }),
        "{report}"
    );
}

#[test]
fn open_child_checkout_survives_without_a_pane_cwd_hold() {
    let (rig, repo, tree) = child("sweep-open-child", false);
    let report = sweep(&rig);
    assert!(tree.join(".git").exists(), "{report}");
    assert!(rig.items()[0]["closed_ms"].is_null());
    assert!(
        !git(&repo, &["branch", "--list", "child"]).trim().is_empty(),
        "{report}"
    );
}

#[test]
fn a_replacement_generation_cannot_inherit_a_closed_child_checkout() {
    let (rig, repo, tree) = child("sweep-replaced-child", true);
    let admin = PathBuf::from(git(&tree, &["rev-parse", "--absolute-git-dir"]).trim());
    fs::write(
        admin.join("dogsled-checkout-id"),
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n",
    )
    .unwrap();
    let report = sweep(&rig);
    assert!(tree.join(".git").exists(), "{report}");
    assert!(
        !git(&repo, &["branch", "--list", "child"]).trim().is_empty(),
        "{report}"
    );
}

#[test]
fn unrecorded_pushed_branch_survives_sweep_but_prune_can_remove_it() {
    let (rig, repo) = repository("sweep-lead-branch");
    git(&repo, &["branch", "lead"]);
    git(&repo, &["push", "-q", "origin", "lead"]);
    quiet_host(&rig);
    let report = sweep(&rig);
    assert!(
        !git(&repo, &["branch", "--list", "lead"]).trim().is_empty(),
        "{report}"
    );
    run(&rig, &["prune", "--yes"]);
    assert_eq!(git(&repo, &["branch", "--list", "lead"]).trim(), "");
}

#[test]
fn a_closed_childs_retained_pushed_branch_is_swept() {
    let (rig, repo, tree) = child("sweep-closed-branch", true);
    git(&repo, &["worktree", "unlock", tree.to_str().unwrap()]);
    git(&repo, &["worktree", "remove", tree.to_str().unwrap()]);
    let report = sweep(&rig);
    assert!(
        git(&repo, &["branch", "--list", "child"]).trim().is_empty(),
        "{report}"
    );
}

#[test]
fn an_open_childs_branch_is_kept_after_its_checkout_disappears() {
    let (rig, repo, tree) = child("sweep-open-branch", false);
    git(&repo, &["worktree", "unlock", tree.to_str().unwrap()]);
    git(&repo, &["worktree", "remove", tree.to_str().unwrap()]);
    let report = sweep(&rig);
    assert!(
        !git(&repo, &["branch", "--list", "child"]).trim().is_empty(),
        "{report}"
    );
}
