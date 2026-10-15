use super::*;
const NOW: u64 = 172_800_001;

pub(super) fn watchdog_rig(name: &str) -> Rig {
    let rig = Rig::new(name);
    fs::write(rig.dir.join("clock"), NOW.to_string()).unwrap();
    rig.script("herdr", r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/herdr.log"
case "$*" in
 'machine list --json') cat "$RIG/machines.json" ;;
 'agent list') cat "$RIG/agents.json" ;;
 'pane list') cat "$RIG/panes.json" ;;
 'pane read'*) printf '1\n2\n3\n4\n5\n' ;;
 'pane report-metadata'*|'notification show'*|'agent prompt'*|'pane close'*) printf '{}\n' ;;
 'pane process-info'*) printf '%s\n' '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[]}}}' ;;
 'worktree list --cwd'*) printf '%s\n' '{"result":{"worktrees":[]}}' ;;
 *) echo unexpected >&2; exit 2 ;;
esac
"#);
    rig.script(
        "ps",
        "#!/bin/sh\nprintf '10 1 0 shell\n11 10 150 pane-child\n20 1 110 orphan\n'\n",
    );
    rig.script("df", "#!/bin/sh\nprintf 'FS 1000 950 50 95%% /\n'\n");
    rig.script("du", "#!/bin/sh\nprintf '42 path\n'\n");
    rig.script(
        "mise",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/mise.log"
case "$*" in
 *'agent:worktree-remove -- feature') touch "$RIG/removed"; printf '{}\n' ;;
 *) exit 2 ;;
esac
"#,
    );
    rig.script("git", r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/git.log"
case "$*" in
 *'rev-parse --path-format=absolute --git-common-dir') printf '%s\n' "$RIG/home/repo/.git" ;;
 *'worktree list --porcelain -z')
 head=abc; tree=tree
 if test -f "$RIG/changed"; then head=def; fi
 if test -f "$RIG/second-tree"; then tree=second; fi
 printf 'worktree %s\0HEAD abc\0branch refs/heads/main\0\0worktree %s\0HEAD %s\0branch refs/heads/feature\0\0' "$RIG/home/repo" "$RIG/home/repo/$tree" "$head" ;;
 *'worktree list --porcelain')
 printf 'worktree %s\n\n' "$RIG/home/repo"
 if ! test -f "$RIG/removed"; then printf 'worktree %s\nlocked agent:worktree\n\n' "$RIG/home/repo/tree"; fi ;;
 *'show-ref --verify --quiet refs/heads/feature') if test -f "$RIG/deleted"; then exit 1; fi ;;
 *'symbolic-ref --quiet refs/remotes/origin/HEAD') printf 'refs/remotes/origin/main\n' ;;
 *'merge-base --is-ancestor'*) if test -f "$RIG/unmerged"; then exit 1; fi ;;
 *'rev-parse refs/heads/feature refs/remotes/origin/main') printf 'abc\nmain\n' ;;
 *'merge-base refs/remotes/origin/main refs/heads/feature') printf 'fork\n' ;;
 *'log --reverse --no-renames --name-only -z --format=%x01%H %P --first-parent fork..refs/heads/feature --') printf '\001abc fork\0\nfile\0' ;;
 *'log --reverse --no-renames --name-only -z --format=%x01%H %P --no-merges fork..refs/remotes/origin/main -- :(literal)file') ;;
 *'status --porcelain --untracked-files=all')
 if test -f "$RIG/change-during-plan"; then touch "$RIG/changed"; fi
 if test -f "$RIG/dirty"; then printf ' M file\n'; fi ;;
 *) echo unexpected >&2; exit 2 ;;
esac
"#);
    fs::create_dir_all(rig.dir.join("home/repo/tree")).unwrap();
    rig.put("state/worktrees.json", &json!([{"repo":rig.dir.join("home/repo"),"path":rig.dir.join("home/repo/tree"),"branch":"feature"}]));
    rig
}

#[test]
fn stalled_snapshot_sidebar_notification_and_brief_survive_serve_restart() {
    let rig = watchdog_rig("watchdog-stall");
    // No auto-removal in this stall test.
    rig.put("state/worktrees.json", &json!([]));
    let path = rig.dir.join("transcript.jsonl");
    fs::write(&path, "test\n").unwrap();
    let old = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(old))
        .unwrap();
    rig.put("agents.json", &json!({"result":{"agents":[{"pane_id":"w1:p1","agent":"pi","agent_status":"working","agent_session":{"kind":"path","value":path}}]}}));
    for _ in 0..2 {
        assert!(
            rig.run(&["serve", "--once", "--interval", "1"])
                .status
                .success()
        );
    }
    assert!(rig.read("state/host.json")["probe"]["agent_panes"][0]["stalled_since_ms"].is_number());
    let log = rig.log("herdr.log");
    assert_eq!(log.matches("notification show").count(), 1);
    assert!(log.contains("--token wrangle=stalled 48h"));
    let out = rig.run(&["status", "--brief"]);
    let brief = String::from_utf8(out.stdout).unwrap();
    assert!(brief.contains("1 stalls"));
    assert!(brief.lines().count() <= 6);
    let hook = rig
        .cmd(&["hook", "context", "--harness", "pi"])
        .env("HERDR_ENV", "1")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&hook.stdout).contains("1 stalls"));
    assert!(String::from_utf8_lossy(&rig.run(&["status"]).stdout).contains("stalled"));
}

#[test]
fn doctor_lists_all_classes_without_mutations_and_remote_uses_ssh() {
    let rig = watchdog_rig("doctor");
    rig.snapshot("state/host.json", NOW, 0.0, 1);
    let mut snapshot = rig.read("state/host.json");
    snapshot["probe"]["agent_panes"][0]["state_since_ms"] = json!(1);
    rig.put("state/host.json", &snapshot);
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{"pane_id":"w1:p1","agent":"pi","agent_status":"idle"}]}}),
    );
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    rig.put("state/jobs/old.json", &json!({"id":"old","cmd":["test"],"cwd":"/","host":"local","owner":"w1:p1","pane":"job-pane","state":"idle","exit":0,"signal":null,"reason":null,"started_ms":1,"ended_ms":2,"timeout_ms":null,"idle_timeout_ms":null}));
    fs::write(rig.dir.join("state/jobs/old.log"), "hello").unwrap();
    let (code, plan) = rig.json(&["doctor"]);
    assert_eq!(code, 0, "{plan}");
    for class in ["worktrees", "panes", "jobs", "disk", "processes"] {
        assert!(
            plan["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["class"] == class),
            "{plan}"
        );
    }
    assert!(plan.to_string().contains("orphan"));
    assert!(!plan.to_string().contains("pane-child"));
    assert!(!rig.log("herdr.log").contains("pane close"));
    assert!(!rig.log("git.log").contains("worktree remove"));
    assert!(rig.dir.join("state/jobs/old.log").exists());
    rig.peer();
    rig.put("plan.json", &plan);
    rig.script(
        "ssh",
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$RIG/ssh.log\"\ncat \"$RIG/plan.json\"\n",
    );
    assert_eq!(rig.json(&["doctor", "--host", "netcup"]).1, plan);
    assert!(rig.log("ssh.log").contains("host doctor"));
}

#[test]
fn prune_refuses_dirty_live_and_working_candidates_and_never_kills_processes() {
    let rig = watchdog_rig("prune-refusal");
    let (_, plan) = rig.json(&["doctor"]);
    let plan = json!({"entries": plan["entries"].as_array().unwrap().iter().filter(|e| e["class"] == "worktrees").collect::<Vec<_>>()}).to_string();
    fs::write(rig.dir.join("dirty"), "").unwrap();
    let refused = |plan: &str| {
        let (code, applied) = rig.json(&["host", "prune", "--plan", plan]);
        assert_eq!(code, 0, "{applied}");
        assert_eq!(applied["results"], json!([]), "{applied}");
        applied["failures"][0].as_str().unwrap().to_owned()
    };
    assert!(refused(&plan).contains("no longer safe"));
    fs::remove_file(rig.dir.join("dirty")).unwrap();
    let alias = rig.dir.join("alias");
    std::os::unix::fs::symlink(rig.dir.join("home/repo/tree"), &alias).unwrap();
    // Missing subdirectories still belong to the live pane, through a symlink.
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[{"pane_id":"live","agent":"pi","agent_status":"working","cwd":alias.join("subdir")}]}}),
    );
    assert!(refused(&plan).contains("no longer safe"));
    rig.snapshot("state/host.json", NOW, 0.0, 1);
    let mut snapshot = rig.read("state/host.json");
    snapshot["probe"]["agent_panes"][0]["state_since_ms"] = json!(1);
    rig.put("state/host.json", &snapshot);
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{"pane_id":"w1:p1","agent_status":"working"}]}}),
    );
    let pane_plan=json!({"entries":[{"class":"panes","id":"w1:p1","detail":"old","removable":true,"tree":null}]}).to_string();
    assert_eq!(
        refused(&pane_plan),
        "refuse panes w1:p1: no longer safe to remove"
    );
    assert!(!rig.log("herdr.log").contains("pane close"));
    assert!(!rig.log("git.log").contains("worktree remove"));
    assert!(rig.run(&["prune", "--yes", "processes"]).status.success());
}

#[test]
fn prune_confirmation_and_configured_remove_hook() {
    let rig = watchdog_rig("prune-method");
    assert!(rig.run(&["prune", "worktrees"]).status.success());
    assert_eq!(rig.log("mise.log"), "");
    // An idle shell in the worktree does not hold it; prune closes it, then runs the hook.
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[{"pane_id":"shell","cwd":rig.dir.join("home/repo/tree")}]}}),
    );
    let out = rig.run(&["prune", "--yes", "worktrees"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        rig.log("mise.log"),
        "run -q agent:worktree-remove -- feature\n"
    );
    assert!(rig.log("herdr.log").contains("pane close shell"));
    assert!(!rig.log("git.log").contains("worktree remove"));
    assert!(String::from_utf8_lossy(&out.stdout).contains("and closed shell"));
    assert_eq!(rig.read("state/worktrees.json"), json!([]));
}

#[test]
fn doctor_reports_held_integrated_worktrees_and_prune_leaves_them() {
    let rig = watchdog_rig("prune-held");
    let tree = rig.dir.join("home/repo/tree");
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[{"pane_id":"child","agent":"pi","agent_status":"done","cwd":tree}]}}),
    );
    let (_, plan) = rig.json(&["doctor"]);
    let entry = plan["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["class"] == "worktrees")
        .unwrap()
        .clone();
    assert_eq!(entry["removable"], false, "{entry}");
    assert!(entry["detail"].as_str().unwrap().contains("wrangle close"));
    assert!(rig.run(&["prune", "--yes", "worktrees"]).status.success());
    assert_eq!(rig.log("mise.log"), "");
    // A stale agent no longer holds it.
    rig.snapshot("state/host.json", NOW, 0.0, 1);
    let mut snapshot = rig.read("state/host.json");
    snapshot["probe"]["agent_panes"][0]["pane"] = json!("child");
    snapshot["probe"]["agent_panes"][0]["state_since_ms"] = json!(1);
    rig.put("state/host.json", &snapshot);
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{"pane_id":"child","agent":"pi","agent_status":"idle"}]}}),
    );
    let out = rig.run(&["prune", "--yes", "worktrees"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(rig.log("herdr.log").contains("pane close child"));
    assert!(rig.log("mise.log").contains("agent:worktree-remove"));
}

#[test]
fn doctor_skips_a_branch_that_no_base_commit_applies() {
    let rig = watchdog_rig("doctor-unmerged");
    fs::write(rig.dir.join("unmerged"), "").unwrap();
    let (code, plan) = rig.json(&["doctor"]);
    assert_eq!(code, 0, "{plan}");
    assert!(
        !plan["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["class"] == "worktrees"),
        "{plan}"
    );
    assert!(
        rig.log("git.log")
            .contains("--first-parent fork..refs/heads/feature")
    );
}

#[test]
fn serve_never_removes_worktrees() {
    // Removal is the owner's `wrangle close` or a confirmed `prune`, never a timer.
    let rig = watchdog_rig("serve-no-removal");
    fs::write(rig.dir.join("deleted"), "").unwrap();
    for clock in ["172800001", "176400001"] {
        fs::write(rig.dir.join("clock"), clock).unwrap();
        let out = rig.run(&["serve", "--once", "--interval", "1"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert!(!rig.log("git.log").contains("worktree remove"));
    assert!(rig.dir.join("home/repo/tree").exists());
}
