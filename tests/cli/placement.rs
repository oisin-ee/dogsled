//! Work in a repository opens where that repository is shown, never in the caller's
//! unrelated workspace. Work outside any repository keeps the caller's workspace.

use super::close::git;
use super::*;

/// A repository at `home/repo` with a subdirectory and a linked worktree at `home/tree`.
fn repository(rig: &Rig) -> (PathBuf, PathBuf) {
    let repo = rig.dir.join("home/repo");
    fs::create_dir_all(repo.join("sub")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let tree = rig.dir.join("home/tree");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            tree.to_str().unwrap(),
        ],
    );
    (repo, tree)
}

/// Herdr's view of the repository: which checkout each open workspace shows.
fn shown(rig: &Rig, repo: &Path, main: Option<&str>, tree: &Path, linked: Option<&str>) {
    rig.put(
        "worktrees.json",
        &json!({"result":{"worktrees":[
            {"path":repo,"open_workspace_id":main,"is_linked_worktree":false},
            {"path":tree,"open_workspace_id":linked,"is_linked_worktree":true}
        ]}}),
    );
}

/// Launch a job in `cwd` as a caller in workspace `w1`, named `caller`. A remote launch
/// carries only the caller's workspace name.
fn launch(rig: &Rig, cwd: &Path, remote: bool) -> String {
    let request = json!({"id":"placed","action":{"type":"job","cmd":["true"],"timeout_ms":null,"idle_timeout_ms":null,
        "validity":{"version":1,"created_ms":rig.now_ms()}},
        "host":"local","cwd":cwd,"pane":null,"workspace":(!remote).then_some("w1"),
        "workspace_name":"caller","owner":"w1:p1"});
    let (code, value) = rig.json(&[
        "host",
        "launch",
        "--job-validity",
        "2",
        "--request",
        &request.to_string(),
    ]);
    assert_eq!(code, 0, "{value}");
    rig.log("herdr.log")
        .lines()
        .find(|line| line.starts_with("tab create") || line.starts_with("workspace create"))
        .unwrap()
        .to_owned()
}

#[test]
fn repository_work_opens_in_the_main_checkouts_workspace() {
    let rig = Rig::new("placement-main");
    let (repo, tree) = repository(&rig);
    shown(&rig, &repo, Some("w5"), &tree, None);
    let cwd = repo.join("sub");
    assert_eq!(
        launch(&rig, &cwd, false),
        format!(
            "tab create --workspace w5 --cwd {} --no-focus",
            cwd.display()
        )
    );
}

#[test]
fn worktree_work_opens_in_the_worktrees_own_workspace() {
    let rig = Rig::new("placement-tree");
    let (repo, tree) = repository(&rig);
    shown(&rig, &repo, Some("w5"), &tree, Some("w6"));
    assert_eq!(
        launch(&rig, &tree, false),
        format!(
            "tab create --workspace w6 --cwd {} --no-focus",
            tree.display()
        )
    );
}

#[test]
fn worktree_without_a_workspace_uses_the_main_checkouts() {
    let rig = Rig::new("placement-tree-main");
    let (repo, tree) = repository(&rig);
    shown(&rig, &repo, Some("w5"), &tree, None);
    assert_eq!(
        launch(&rig, &tree, false),
        format!(
            "tab create --workspace w5 --cwd {} --no-focus",
            tree.display()
        )
    );
}

#[test]
fn repository_with_no_workspace_gets_its_own_not_the_callers_name() {
    let rig = Rig::new("placement-open");
    let (repo, tree) = repository(&rig);
    shown(&rig, &repo, None, &tree, None);
    // A remote launch knows only the caller's workspace name; the repository rule wins.
    assert_eq!(
        launch(&rig, &repo, true),
        format!("workspace create --cwd {} --no-focus", repo.display())
    );
}

#[test]
fn work_outside_a_repository_keeps_the_callers_workspace() {
    let rig = Rig::new("placement-plain");
    let cwd = rig.dir.join("home");
    assert_eq!(
        launch(&rig, &cwd, false),
        format!(
            "tab create --workspace w1 --cwd {} --no-focus",
            cwd.display()
        )
    );
    assert!(!rig.log("herdr.log").contains("worktree list"));
}
