use super::*;

/// The repository's setup: it logs where it ran and the branch, and its second command fails
/// while `$RIG/setup-fails` exists.
const SETUP: &str = r#"[worktree]
setup = [
  "printf '%s %s\\n' \"$(pwd -P)\" {branch} >> \"$RIG/setup.log\"",
  "if test -f \"$RIG/setup-fails\"; then printf 'setup failed: no network\\n' >&2; exit 1; fi",
]
"#;

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

/// A real repository with a `dogsled.toml` setup, and a fake Herdr whose `worktree open`
/// opens the main checkout's workspace w7 as the anchor on first use.
pub(super) fn place_rig(name: &str) -> (Rig, PathBuf) {
    let rig = Rig::new(name);
    let repo = rig.dir.join("home/repo space");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/repo.git",
        ],
    );
    fs::write(repo.join("dogsled.toml"), SETUP).unwrap();
    git(&repo, &["add", "dogsled.toml"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    rig.snapshot("state/host.json", now(), 0.0, 8);
    rig.script(
        "herdr",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/herdr.log"
case "$*" in
  'machine list --json') cat "$RIG/machines.json" ;;
  'workspace list') printf '%s\n' '{"result":{"workspaces":[{"workspace_id":"w1","label":"caller"}]}}' ;;
  'agent list') cat "$RIG/agents.json" ;;
  'pane list') printf '%s\n' '{"result":{"panes":[{"pane_id":"w8:p1","terminal_id":"term_tree"}]}}' ;;
  'pane process-info'*) printf '%s\n' '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":10,"name":"zsh"}]}}}' ;;
  'agent start'*)
    if test -f "$RIG/start-fails"; then echo 'original startup failed' >&2; exit 1; fi
    printf '{}\n' ;;
  'worktree list --cwd'*)
    if test -f "$RIG/anchor"; then main='"w7"'; else main=null; fi
    printf '{"result":{"worktrees":[{"path":"%s","open_workspace_id":%s,"is_linked_worktree":false}]}}\n' "$4" "$main" ;;
  'worktree open'*)
    while test "$#" -gt 0; do if test "$1" = --path; then path="$2"; fi; shift; done
    touch "$RIG/anchor"
    if test -f "$RIG/already-open"; then open=true; else open=false; fi
    printf '{"result":{"already_open":%s,"workspace":{"workspace_id":"w8"},"root_pane":{"pane_id":"w8:p1","terminal_id":"term_tree"},"worktree":{"path":"%s"}}}\n' "$open" "$path" ;;
  *) echo unexpected >&2; exit 2 ;;
esac
"#,
    );
    (rig, repo.canonicalize().unwrap())
}

/// Each call starts with no reservations, so admission never decides a test's outcome.
fn place(rig: &Rig, repo: &Path, branch: &str, base: Option<&str>) -> (i32, Value) {
    rig.put("state/placements.json", &json!([]));
    let mut args = vec![
        "place",
        "--branch",
        branch,
        "--repo",
        repo.to_str().unwrap(),
    ];
    if let Some(base) = base {
        args.extend(["--base", base]);
    }
    rig.json(&args)
}

fn locks(repo: &Path) -> String {
    git(repo, &["worktree", "list", "--porcelain"])
}

#[test]
fn place_creates_a_locked_worktree_runs_setup_opens_it_and_records_the_anchor() {
    let (rig, repo) = place_rig("place-create");
    // The caller works in another linked worktree's subdirectory; placement uses the main checkout.
    git(
        &repo,
        &["worktree", "add", "-q", "-b", "caller", "../caller"],
    );
    let caller = rig.dir.join("home/caller/subdir");
    fs::create_dir_all(&caller).unwrap();
    let (code, placed) = place(&rig, &caller, "feat/a;literal", None);
    assert_eq!(code, 0, "{placed}");
    let tree = repo.join(".worktrees/feat-a;literal");
    assert_eq!(placed["path"], json!(tree));
    assert_eq!(placed["pane_id"], "w8:p1");
    assert_eq!(
        rig.log("setup.log"),
        format!("{} feat/a;literal\n", tree.display())
    );
    assert!(
        locks(&repo).contains(&format!(
            "worktree {}\nHEAD {}branch refs/heads/feat/a;literal\nlocked agent:worktree\n",
            tree.display(),
            git(&repo, &["rev-parse", "main"])
        )),
        "{}",
        locks(&repo)
    );
    assert!(rig.log("herdr.log").contains(&format!(
        "worktree open --cwd {} --path {} --no-focus",
        repo.display(),
        tree.display()
    )));
    assert_eq!(
        fs::read_to_string(repo.join(".git/agent-worktree-anchor")).unwrap(),
        "w7\n"
    );
    // The worktree is registered for doctor, and a reservation holds the capacity.
    assert_eq!(rig.read("state/worktrees.json")[0]["path"], json!(tree));
    assert_eq!(
        rig.read("state/placements.json").as_array().unwrap().len(),
        1
    );
    // A second child does not replace the anchor: the workspace was open before it.
    fs::write(repo.join(".git/agent-worktree-anchor"), "kept\n").unwrap();
    assert_eq!(place(&rig, &repo, "feat/b", None).0, 0);
    assert_eq!(
        fs::read_to_string(repo.join(".git/agent-worktree-anchor")).unwrap(),
        "kept\n"
    );
}

#[test]
fn place_bases_new_branches_reuses_existing_ones_and_refuses_foreign_work() {
    let (rig, repo) = place_rig("place-branches");
    let first = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "second"]);
    assert_eq!(place(&rig, &repo, "from-base", Some("HEAD~1")).0, 0);
    assert_eq!(git(&repo, &["rev-parse", "from-base"]), first);
    assert_eq!(place(&rig, &repo, "from-main", None).0, 0);
    assert_eq!(
        git(&repo, &["rev-parse", "from-main"]),
        git(&repo, &["rev-parse", "main"])
    );
    // An existing branch takes no base.
    let (code, error) = place(&rig, &repo, "from-base", Some("main"));
    assert_eq!(code, 2);
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("branch from-base exists; omit --base")
    );
    // An existing worktree is reused without a second setup.
    fs::write(rig.dir.join("setup.log"), "").unwrap();
    let (code, placed) = place(&rig, &repo, "from-base", None);
    assert_eq!(code, 0, "{placed}");
    assert_eq!(placed["path"], json!(repo.join(".worktrees/from-base")));
    assert_eq!(rig.log("setup.log"), "");
    // An existing branch without a worktree gets one, with setup.
    git(&repo, &["branch", "kept"]);
    assert_eq!(place(&rig, &repo, "kept", None).0, 0);
    assert!(rig.log("setup.log").contains("/.worktrees/kept kept"));
    // Someone else's lock, an open workspace, the main checkout, or a bad name is refused.
    let tree = repo.join(".worktrees/kept");
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
    let (_, error) = place(&rig, &repo, "kept", None);
    assert!(error["error"].as_str().unwrap().contains("locked (mine)"));
    fs::write(rig.dir.join("already-open"), "").unwrap();
    let (code, error) = place(&rig, &repo, "from-main", None);
    assert_eq!(code, 2);
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("Herdr workspace w8 already shows")
    );
    let (_, error) = place(&rig, &repo, "main", None);
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("checked out in the main checkout")
    );
    let (_, error) = place(&rig, &repo, "bad..name", None);
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("not a valid branch name")
    );
}

#[test]
fn setup_failure_removes_the_new_worktree_and_branch_and_keeps_the_reservation() {
    let (rig, repo) = place_rig("place-setup-fails");
    fs::write(rig.dir.join("setup-fails"), "").unwrap();
    let (code, error) = place(&rig, &repo, "feat/x", None);
    assert_eq!(code, 2, "{error}");
    let message = error["error"].as_str().unwrap();
    assert!(message.contains("setup command"), "{message}");
    assert!(message.contains("setup failed: no network"), "{message}");
    assert!(
        message.contains("the new worktree was removed"),
        "{message}"
    );
    assert!(!repo.join(".worktrees/feat-x").exists());
    assert!(!locks(&repo).contains("feat-x"));
    assert_eq!(git(&repo, &["branch", "--list", "feat/x"]), "");
    assert!(!rig.log("herdr.log").contains("worktree open"));
    assert_eq!(
        rig.read("state/placements.json").as_array().unwrap().len(),
        1
    );
    // A branch that existed before placement stays.
    git(&repo, &["branch", "kept"]);
    assert_eq!(place(&rig, &repo, "kept", None).0, 2);
    assert!(!repo.join(".worktrees/kept").exists());
    assert_ne!(git(&repo, &["branch", "--list", "kept"]), "");
}

#[test]
fn repository_and_host_config_errors_are_explicit() {
    let (rig, repo) = place_rig("place-config");
    fs::write(
        repo.join("dogsled.toml"),
        "[worktree]\nsetup = [\"echo {base}\"]\n",
    )
    .unwrap();
    let (code, error) = place(&rig, &repo, "feat/x", None);
    assert_eq!(code, 2);
    let message = error["error"].as_str().unwrap();
    assert!(message.contains("dogsled.toml"), "{message}");
    assert!(message.contains("unknown placeholder {base}"), "{message}");
    fs::write(repo.join("dogsled.toml"), "[worktree]\nhooks = []\n").unwrap();
    let (_, error) = place(&rig, &repo, "feat/x", None);
    assert!(error["error"].as_str().unwrap().contains("unknown field"));
    assert!(!repo.join(".worktrees/feat-x").exists());
    // The host hooks moved to the repository.
    rig.config("[hooks]\nprepare = ['true']\n");
    let (code, error) = place(&rig, &repo, "feat/x", None);
    assert_eq!(code, 2);
    assert!(error["error"].as_str().unwrap().contains("unknown field"));
}
