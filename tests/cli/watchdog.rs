use super::*;
pub(super) const NOW: u64 = 172_800_001;

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
 'pane process-info --pane gone') printf '%s\n' '{"error":{"code":"pane_not_found","message":"pane not found"}}' >&2; exit 1 ;;
 'pane process-info'*) printf '{"result":{"process_info":{"shell_pid":%s,"foreground_processes":[{"pid":%s,"name":"zsh","argv":["zsh"]}]}}}\n' "$(cat "$RIG/shell_pid" 2>/dev/null || echo 10)" "$(cat "$RIG/shell_pid" 2>/dev/null || echo 10)" ;;
 'worktree list --cwd'*) printf '%s\n' '{"result":{"worktrees":[]}}' ;;
 'workspace list') cat "$RIG/workspaces.json" 2>/dev/null || printf '%s\n' '{"result":{"workspaces":[]}}' ;;
 'workspace close'*) printf '{}\n' ;;
 *) echo unexpected >&2; exit 2 ;;
esac
"#);
    rig.script(
        "ps",
        // The process-table parents are real, so pane ownership sees real processes.
        "#!/bin/sh\ntest \"$2\" = pid=,ppid= && exec /bin/ps \"$@\"\nprintf '10 1 0 shell\n11 10 150 pane-child\n20 1 110 orphan\n'\n",
    );
    rig.script("df", "#!/bin/sh\nprintf 'FS 1000 950 50 95%% /\n'\n");
    rig.script("du", "#!/bin/sh\nprintf '42 path\n'\n");
    rig.script("git", r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/git.log"
case "$*" in
 *'rev-parse --path-format=absolute --git-common-dir') printf '%s\n' "$RIG/home/repo/.git" ;;
 *'rev-parse --path-format=absolute --git-common-dir --git-dir --show-toplevel HEAD --abbrev-ref HEAD')
 common="$RIG/home/repo/.git"; admin="$common/worktrees/$(basename "$2")"; branch=feature; head=abc
 if test "$2" = "$RIG/home/repo"; then admin="$common"; branch=main; fi
 if test -f "$RIG/changed"; then head=def; fi
 printf '%s\n' "$common" "$admin" "$2" "$head" "$branch" ;;
 *'rev-parse HEAD') printf 'abc\n' ;;
 *'worktree list --porcelain -z')
 head=abc; tree=tree
 if test -f "$RIG/changed"; then head=def; fi
 if test -f "$RIG/second-tree"; then tree=second; fi
 printf 'worktree %s\0HEAD abc\0branch refs/heads/main\0\0' "$RIG/home/repo"
 if ! test -f "$RIG/removed"; then printf 'worktree %s\0HEAD %s\0branch refs/heads/feature\0\0' "$RIG/home/repo/$tree" "$head"; fi ;;
 *'worktree list --porcelain')
 printf 'worktree %s\n\n' "$RIG/home/repo"
 if ! test -f "$RIG/removed"; then printf 'worktree %s\nlocked agent:worktree\n\n' "$RIG/home/repo/tree"; fi ;;
 *'worktree unlock '*) ;;
 *'worktree remove '*) touch "$RIG/removed"; rm -rf "$RIG/home/repo/tree" "$RIG/home/repo/.git/worktrees/tree" ;;
 *'branch -D feature') touch "$RIG/deleted" ;;
 *'show-ref --verify --quiet refs/heads/feature') if test -f "$RIG/deleted"; then exit 1; fi ;;
 *'symbolic-ref --quiet refs/remotes/origin/HEAD') printf 'refs/remotes/origin/main\n' ;;
 *'merge-base --is-ancestor'*) if test -f "$RIG/unmerged"; then exit 1; fi ;;
 *'rev-parse refs/heads/feature refs/remotes/origin/main') printf 'abc\nmain\n' ;;
 *'rev-parse abc refs/remotes/origin/main') printf 'abc\nmain\n' ;;
 *'merge-base main abc') printf 'fork\n' ;;
 *'diff --no-ext-diff --no-textconv --raw -z --no-renames --no-abbrev fork abc --')
 printf ':100644 100644 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 M\0file\0' ;;
 *'for-each-ref --contains abc --format=%(objectname) %(refname) refs/heads/ refs/remotes/') printf 'abc refs/heads/feature\n' ;;
 *'log --no-ext-diff --no-textconv --reverse --no-renames --raw --no-abbrev --diff-merges=first-parent -z --format=%x01%H %P --first-parent fork..abc --')
 printf '\001abc fork\0\n:100644 100644 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 M\0file\0' ;;
 *'log --no-ext-diff --no-textconv --reverse --no-renames --raw --no-abbrev --diff-merges=first-parent -z --format=%x01%H %P --no-merges fork..main -- :(literal)file') ;;
 *'rev-parse --path-format=absolute --show-object-format --git-path objects') printf 'sha1\n%s\n' "$RIG/home/repo/.git/objects" ;;
 *'rev-parse main abc main^{tree}') printf 'main\nabc\nmain-tree\n' ;;
 *'init --bare --quiet --template= --object-format=sha1') mkdir -p "$2/objects/info" ;;
 *'hash-object -t tree -w -- /dev/null') printf 'empty-tree\n' ;;
 *'merge-tree --write-tree --no-messages main abc') printf 'branch-tree\n' ;;
 *'for-each-ref --format=%(objectname) %(refname) refs/heads/') printf 'abc refs/heads/main\nabc refs/heads/feature\n' ;;
 *'rev-list --count refs/remotes/origin/main..refs/heads/feature') printf '2\n' ;;
 *'rev-list --count refs/heads/feature --not --remotes') printf '1\n' ;;
 *'log -1 --format=%ct refs/heads/feature') printf '86400\n' ;;
 *'fetch --all --prune') ;;
 *'rev-parse --show-toplevel') printf '%s\n' "$2" ;;
 *'status --porcelain --untracked-files=all -z')
 if test -f "$RIG/change-during-plan"; then touch "$RIG/changed"; fi
 if test -f "$RIG/dirty"; then printf ' M file\0'; fi ;;
 *) echo unexpected >&2; exit 2 ;;
esac
"#);
    fs::create_dir_all(rig.dir.join("home/repo/tree")).unwrap();
    for name in ["tree", "second"] {
        let admin = rig.dir.join("home/repo/.git/worktrees").join(name);
        fs::create_dir_all(&admin).unwrap();
        fs::write(
            admin.join("dogsled-checkout-id"),
            "0123456789abcdef0123456789abcdef\n",
        )
        .unwrap();
    }
    // A linked worktree has a `.git` file; without one Git cannot remove it.
    fs::write(
        rig.dir.join("home/repo/tree/.git"),
        "gitdir: ../.git/worktrees/tree\n",
    )
    .unwrap();
    rig.put("state/worktrees.json", &json!([{"repo":rig.dir.join("home/repo"),"path":rig.dir.join("home/repo/tree"),"branch":"feature","head":"abc","identity":{
        "common":rig.dir.join("home/repo/.git"),"gitdir":rig.dir.join("home/repo/.git/worktrees/tree"),"generation":"0123456789abcdef0123456789abcdef"
    }}]));
    rig
}

fn stopped_rig(name: &str, reader: bool) -> Rig {
    let rig = watchdog_rig(name);
    rig.put("state/worktrees.json", &json!([]));
    rig.put(
        "state/items.json",
        &json!([{"id":"child","kind":"agent","host":"local",
        "pane":"w2:p1","owner_pane":"w1:p1","created_ms":1,"closed_ms":null}]),
    );
    fs::create_dir_all(rig.dir.join("state/inbox")).unwrap();
    if reader {
        rig.put(
            "state/inbox/term_child.reader.json",
            &json!({"pane":"w2:p1","harness":"pi","registered_ms":1}),
        );
    }
    rig
}

fn stopped_tick(rig: &Rig, now: u64, raw: &str, seq: u64) {
    fs::write(rig.dir.join("clock"), now.to_string()).unwrap();
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[
            {"pane_id":"w1:p1","agent":"pi","agent_status":"working","terminal_id":"term_owner"},
            {"pane_id":"w2:p1","agent":"pi","agent_status":raw,"terminal_id":"term_child",
                "agent_session":{"kind":"id","value":"child-session"},"state_change_seq":seq}
        ]}}),
    );
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn corrective_messages(rig: &Rig) -> Vec<Value> {
    rig.read("state/inbox/term_child.json")
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| {
            record["text"] == "End your turn with done, ask, or blocked, or wait for your children."
        })
        .cloned()
        .collect()
}

#[test]
fn stopped_child_gets_exactly_one_prompt_across_five_restarts() {
    let rig = stopped_rig("stopped-once", true);
    for offset in 0..5 {
        stopped_tick(&rig, NOW + offset * 1_000, "idle", 1);
    }
    let messages = corrective_messages(&rig);
    assert_eq!(messages.len(), 1);
    let saved = rig.read("state/watchdog.json");
    assert_eq!(saved["stopped"]["child"]["pane"], "w2:p1");
    assert_eq!(saved["stopped"]["child"]["agent_session"], "child-session");
    assert_eq!(saved["stopped"]["child"]["turn"]["state_change_seq"], 1);
    assert_eq!(saved["stopped"]["child"]["message_id"], messages[0]["id"]);
    assert_eq!(saved["stopped"]["child"]["delivered"], true);
    rig.service();
    let out = rig.run(&["board"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("stopped; prompted 4s ago"));
}

#[test]
fn stopped_failed_put_retries_same_id_three_times_only() {
    let rig = stopped_rig("stopped-failed", false);
    let mut id = Value::Null;
    for offset in 0..5 {
        stopped_tick(&rig, NOW + offset * 1_000, "idle", 1);
        let saved = rig.read("state/watchdog.json");
        let episode = &saved["stopped"]["child"];
        if offset == 0 {
            id = episode["message_id"].clone();
        }
        assert_eq!(episode["message_id"], id);
        assert_eq!(episode["attempts"], (offset + 1).min(3));
        assert_eq!(episode["delivered"], false);
    }
    rig.put(
        "state/inbox/term_child.reader.json",
        &json!({"pane":"w2:p1","harness":"pi","registered_ms":1}),
    );
    stopped_tick(&rig, NOW + 6_000, "idle", 1);
    assert!(!rig.dir.join("state/inbox/term_child.json").exists());
    rig.service();
    let out = rig.run(&["board"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("stopped; prompt undelivered"));
}

#[test]
fn stopped_prompt_does_not_loop_but_a_later_working_turn_rearms() {
    let rig = stopped_rig("stopped-no-loop", true);
    for (offset, raw, seq) in [
        (0, "idle", 1),
        (1_000, "working", 2),
        (2_000, "idle", 3),
        (125_000, "idle", 3),
    ] {
        stopped_tick(&rig, NOW + offset, raw, seq);
    }
    assert_eq!(corrective_messages(&rig).len(), 1);
    stopped_tick(&rig, NOW + 130_000, "working", 4);
    stopped_tick(&rig, NOW + 140_000, "idle", 5);
    assert_eq!(corrective_messages(&rig).len(), 2);
}

#[test]
fn stopped_consecutive_working_turns_rearm_after_the_prompt_window() {
    let rig = stopped_rig("stopped-working-turns", true);
    for (offset, raw, seq) in [
        (0, "idle", 1),
        (1_000, "working", 2),
        (130_000, "working", 4),
        (140_000, "idle", 5),
    ] {
        stopped_tick(&rig, NOW + offset, raw, seq);
    }
    assert_eq!(corrective_messages(&rig).len(), 2);
}

#[test]
fn working_blocked_and_unknown_children_get_no_corrective_prompt() {
    for raw in ["working", "blocked", "unreadable"] {
        let rig = stopped_rig(&format!("stopped-excluded-{raw}"), true);
        stopped_tick(&rig, NOW, raw, 1);
        assert!(
            !rig.dir.join("state/inbox/term_child.json").exists(),
            "{raw}"
        );
        assert!(
            rig.read("state/watchdog.json")["stopped"]
                .as_object()
                .unwrap()
                .is_empty()
        );
    }
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
    assert!(log.contains("--token dogsled=stalled 48h"));
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
fn a_check_in_does_not_repeat_the_orphans_the_sweep_keeps() {
    let rig = watchdog_rig("watchdog-orphan-notice");
    // The owner is idle with an open child, so it gets a check-in.
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{"pane_id":"w1:p1","agent":"pi","agent_status":"idle",
            "terminal_id":"term_owner","completion_seq":1,"state_change_seq":1}]}}),
    );
    rig.snapshot("state/host.json", NOW, 0.0, 1);
    let mut snapshot = rig.read("state/host.json");
    snapshot["probe"]["agent_panes"] = json!([{"pane":"w1:p1","agent":"pi","state":"idle",
        "terminal_id":"term_owner"}]);
    rig.put("state/host.json", &snapshot);
    // The owner's own item is open, and the sweep keeps an orphan on this host.
    // The orphan has an open item whose owner pane is gone, so the sweep keeps it, and
    // the check-in used to name it on every turn.
    rig.put(
        "state/items.json",
        &json!([{"id":"child","kind":"agent","host":"local","pane":"w2:p1","owner_pane":"w1:p1",
            "name":"worker","created_ms":1,"closed_ms":null},
            {"id":"orphan","kind":"agent","host":"local","pane":"w3:p1","owner_pane":"gone",
            "name":"lost-child","created_ms":1,"closed_ms":null}]),
    );
    // The orphan's pane works, so the sweep keeps the child instead of closing it.
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[
            {"pane_id":"w1:p1","terminal_id":"term_owner"},
            {"pane_id":"w2:p1","terminal_id":"term_child"},
            {"pane_id":"w3:p1","terminal_id":"term_lost"}
        ]}}),
    );
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[
            {"pane_id":"w1:p1","agent":"pi","agent_status":"idle","terminal_id":"term_owner",
                "completion_seq":1,"state_change_seq":1},
            {"pane_id":"w3:p1","agent":"pi","agent_status":"working","terminal_id":"term_lost",
                "completion_seq":1,"state_change_seq":1}
        ]}}),
    );
    rig.put(
        "state/orphans.json",
        &json!([{"item":"orphan","host":"local","pane":"w3:p1","owner_pane":"gone",
            "name":"lost-child","reason":"owner gone is gone; the child is working","since_ms":1}]),
    );
    // The owner reads its inbox.
    fs::create_dir_all(rig.dir.join("state/inbox")).unwrap();
    rig.put(
        "state/inbox/term_owner.reader.json",
        &json!({"pane":"w1:p1","harness":"pi","registered_ms":1}),
    );
    assert!(
        rig.run(&["serve", "--once", "--interval", "1"])
            .status
            .success()
    );
    let inbox = rig.read("state/inbox/term_owner.json");
    let text = inbox
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["kind"] == "check-in")
        .map(|record| record["text"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!text.contains("Orphaned children"), "{text}");
    assert!(!text.contains("lost-child"), "{text}");
    // The orphan is still there: the sweep kept it, so the check-in had one to repeat.
    let orphans = rig.read("state/orphans.json");
    assert_eq!(orphans[0]["item"], json!("orphan"), "{orphans}");
    assert_eq!(orphans[0]["name"], json!("lost-child"), "{orphans}");
    let items = rig.read("state/items.json");
    let orphan = items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == json!("orphan"))
        .unwrap();
    assert!(orphan["closed_ms"].is_null(), "{items}");
}

#[test]
fn doctor_lists_all_classes_without_mutations_and_remote_uses_ssh() {
    let rig = watchdog_rig("doctor");
    rig.snapshot("state/host.json", NOW, 0.0, 1);
    let mut snapshot = rig.read("state/host.json");
    snapshot["probe"]["agent_panes"][0]["state_since_ms"] = json!(1);
    snapshot["probe"]["agent_panes"][0]["pane"] = json!("w2:p1");
    rig.put("state/host.json", &snapshot);
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{"pane_id":"w2:p1","agent":"pi","agent_status":"idle",
            "terminal_id":"term_child","completion_seq":1,"state_change_seq":1}]}}),
    );
    // A non-caller idle for a day after it called `done`: either scope may close it.
    rig.done("term_child", "w2:p1", 1);
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
fn prune_rechecks_job_terminal_evidence_after_selection() {
    for reversed in [false, true] {
        let rig = watchdog_rig(&format!("prune-job-recheck-{reversed}"));
        let job = json!({"id":"old","cmd":["test"],"cwd":rig.dir.join("home/repo/tree"),
            "host":"local","owner":"w1:p1","pane":"job-pane","state":"idle","exit":0,
            "signal":null,"reason":null,"started_ms":1,"ended_ms":2,
            "timeout_ms":null,"idle_timeout_ms":null});
        fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
        rig.put("state/jobs/old.json", &job);
        fs::write(rig.dir.join("state/jobs/old.log"), "old output").unwrap();
        let mut changed = job;
        if reversed {
            changed["started_ms"] = json!(3);
        } else {
            changed["state"] = json!("future-state");
        }
        rig.put("changed-job.json", &changed);
        // Disk inspection follows job selection in the fresh apply-time survey.
        rig.script("df", "#!/bin/sh\ncp \"$RIG/changed-job.json\" \"$RIG/state/jobs/old.json\"\nprintf 'FS 1000 950 50 95%% /\\n'\n");
        let plan =
            json!({"entries":[{"class":"jobs","id":"old","detail":"ended","removable":true}]});
        let (_, applied) = rig.json(&["host", "prune", "--plan", &plan.to_string()]);
        assert!(
            applied["results"].as_array().unwrap().is_empty(),
            "{applied}"
        );
        assert!(
            applied.to_string().contains("job is not expired"),
            "{applied}"
        );
        assert_eq!(rig.read("state/jobs/old.json"), changed);
        assert_eq!(rig.log("state/jobs/old.log"), "old output");
        assert!(rig.dir.join("home/repo/tree").exists());
    }
}

#[test]
fn doctor_finds_processes_when_a_pane_closes_after_the_list() {
    let rig = watchdog_rig("doctor-closed-pane");
    // Herdr listed "gone", then it closed before doctor asked for its processes.
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[{"pane_id":"gone","cwd":"/"},{"pane_id":"w1:p1","cwd":"/"}]}}),
    );
    let (code, plan) = rig.json(&["doctor"]);
    assert_eq!(code, 0, "{plan}");
    assert!(plan.to_string().contains("orphan"), "{plan}");
    assert!(!plan.to_string().contains("pane-child"), "{plan}");
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
        // The recheck refusal is a skip: the entry stays, and nothing failed.
        assert_eq!(applied["failures"], json!([]), "{applied}");
        applied["skipped"][0].as_str().unwrap().to_owned()
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
    let reason = refused(&pane_plan);
    assert!(
        reason.starts_with("refuse panes w1:p1: no longer safe to remove"),
        "{reason}"
    );
    assert!(!rig.log("herdr.log").contains("pane close"));
    assert!(!rig.log("git.log").contains("worktree remove"));
    assert!(rig.run(&["prune", "--yes", "processes"]).status.success());
}

#[test]
fn prune_confirmation_and_native_removal() {
    let rig = watchdog_rig("prune-method");
    assert!(rig.run(&["prune", "worktrees"]).status.success());
    assert!(!rig.log("git.log").contains("worktree remove"));
    // An idle shell in the worktree does not hold it; prune removes the worktree, then closes it.
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
    let tree = rig.dir.join("home/repo/tree");
    let git = rig.log("git.log");
    let unlock = git
        .find(&format!("worktree unlock {}", tree.display()))
        .unwrap();
    let remove = git
        .find(&format!("worktree remove {}", tree.display()))
        .unwrap();
    assert!(unlock < remove, "{git}");
    // Integrated by ancestry, so the branch goes too; never a forced worktree removal.
    assert!(git.contains("branch -D feature"));
    assert!(!git.contains("remove -f"));
    assert!(rig.log("herdr.log").contains("pane close shell"));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("and closed shell; branch deleted"),
        "{stdout}"
    );
    assert_eq!(rig.read("state/worktrees.json"), json!([]));
}

#[test]
fn doctor_reports_held_integrated_worktrees_and_prune_leaves_them() {
    let rig = watchdog_rig("prune-held");
    let tree = rig.dir.join("home/repo/tree");
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[
            {"pane_id":"owner","agent":"pi","agent_status":"working","cwd":rig.dir},
            {"pane_id":"child","agent":"pi","agent_status":"done","cwd":tree}
        ]}}),
    );
    // An open item holds its checkout: its owner closes it, keeping the reply.
    rig.put(
        "state/items.json",
        &json!([{"id":"i1","kind":"agent","host":"local","pane":"child","owner_pane":"owner",
            "name":"worker","created_ms":1,"closed_ms":null}]),
    );
    let worktree = |rig: &Rig| {
        let (_, plan) = rig.json(&["doctor"]);
        plan["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["class"] == "worktrees")
            .unwrap()
            .clone()
    };
    let entry = worktree(&rig);
    assert_eq!(entry["removable"], false, "{entry}");
    assert!(entry["detail"].as_str().unwrap().contains("dogsled close"));
    assert!(rig.run(&["prune", "--yes", "worktrees"]).status.success());
    assert!(!rig.log("git.log").contains("worktree remove"));
    // Without an open item, an idle agent that has not called `done` still holds it.
    rig.put("state/items.json", &json!([]));
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{"pane_id":"child","agent":"pi","agent_status":"done",
            "terminal_id":"term_child","completion_seq":7,"state_change_seq":7}]}}),
    );
    let entry = worktree(&rig);
    assert_eq!(entry["removable"], false, "{entry}");
    assert!(
        entry["detail"]
            .as_str()
            .unwrap()
            .contains("child waits for input")
    );
    // A done agent does not hold it; removal closes the pane.
    rig.done("term_child", "child", 7);
    assert_eq!(worktree(&rig)["removable"], true);
    let out = rig.run(&["prune", "--yes", "worktrees"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(rig.log("herdr.log").contains("pane close child"));
    assert!(rig.log("git.log").contains("worktree remove"));
}

#[test]
fn doctor_reports_an_unused_unmerged_worktree_and_prune_keeps_it() {
    let rig = watchdog_rig("doctor-unmerged");
    fs::write(rig.dir.join("unmerged"), "").unwrap();
    let (code, plan) = rig.json(&["doctor"]);
    assert_eq!(code, 0, "{plan}");
    let entry = plan["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["class"] == "worktrees")
        .unwrap_or_else(|| panic!("{plan}"))
        .clone();
    assert_eq!(entry["removable"], false, "{entry}");
    assert_eq!(
        entry["detail"],
        "branch feature commits are not held by the default branch or a refreshed remote ref; \
         no holds"
    );
    assert!(rig.log("git.log").contains("--first-parent fork..abc"));
    assert!(rig.run(&["prune", "--yes", "worktrees"]).status.success());
    assert!(!rig.log("git.log").contains("worktree remove"));
}

/// Herdr whose `process-info` shows a foreground command for each pane named in `$RIG/busy`.
pub(super) fn shells_rig(name: &str) -> Rig {
    let rig = watchdog_rig(name);
    rig.script("herdr", r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/herdr.log"
case "$*" in
 'machine list --json') cat "$RIG/machines.json" ;;
 'agent list') cat "$RIG/agents.json" ;;
 'pane list') cat "$RIG/panes.json" ;;
 'workspace list') printf '%s\n' '{"result":{"workspaces":[{"workspace_id":"w3","label":"done-work"}]}}' ;;
 'worktree list --cwd'*) printf '%s\n' '{"result":{"worktrees":[]}}' ;;
 'pane process-info --pane '*)
 if grep -qx "$4" "$RIG/busy" 2>/dev/null; then fg='{"pid":11,"name":"vim","argv":["vim"]}'; else fg='{"pid":10,"name":"zsh","argv":["zsh"]}'; fi
 printf '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[%s]}}}\n' "$fg" ;;
 'pane close'*|'workspace close'*) printf '{}\n' ;;
 *) echo unexpected >&2; exit 2 ;;
esac
"#);
    rig
}

#[test]
fn serve_keeps_an_idle_shell_start_until_the_shell_runs_a_command() {
    let rig = shells_rig("idle-shell");
    let tick = |clock: u64| {
        fs::write(rig.dir.join("clock"), clock.to_string()).unwrap();
        let out = rig.run(&["serve", "--once", "--interval", "1"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        rig.read("state/host.json")["probe"]["panes"][0].clone()
    };
    assert_eq!(tick(1_000)["idle_since_ms"], 1_000);
    assert_eq!(tick(2_000)["idle_since_ms"], 1_000);
    fs::write(rig.dir.join("busy"), "w1:p1\n").unwrap();
    assert_eq!(tick(3_000)["idle_since_ms"], Value::Null);
    fs::remove_file(rig.dir.join("busy")).unwrap();
    assert_eq!(tick(4_000)["idle_since_ms"], 4_000);
}

#[test]
fn doctor_closes_an_all_idle_workspace_whole_and_other_idle_panes_alone() {
    let rig = shells_rig("idle-workspaces");
    let tree = rig.dir.join("home/repo/tree");
    let pane = |id: &str, workspace: &str| json!({"pane_id":id,"workspace_id":workspace,"cwd":rig.dir.join("home")});
    let mut held = pane("w5:p1", "w5");
    held["cwd"] = json!(tree);
    // w3 is all idle; w4 has a busy pane; w1 is the caller's; w5's pane goes with the worktree.
    let live = [
        pane("w3:p1", "w3"),
        pane("w3:p2", "w3"),
        pane("w4:p1", "w4"),
        pane("w4:p2", "w4"),
        pane("w1:p1", "w1"),
        held,
    ];
    rig.put("panes.json", &json!({"result":{"panes":live}}));
    fs::write(rig.dir.join("busy"), "w4:p2\n").unwrap();
    rig.snapshot("state/host.json", NOW, 0.0, 1);
    let mut snapshot = rig.read("state/host.json");
    snapshot["probe"]["agent_panes"] = json!([]);
    snapshot["probe"]["panes"] = json!(
        live.iter()
            .map(|p| {
                let mut p = p.clone();
                p["idle_shell"] = json!(true);
                p["idle_since_ms"] = json!(1);
                p
            })
            .collect::<Vec<_>>()
    );
    rig.put("state/host.json", &snapshot);
    let (code, plan) = rig.json(&["doctor"]);
    assert_eq!(code, 0, "{plan}");
    let idle: Vec<_> = plan["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["class"] == "workspaces" || e["class"] == "panes")
        .map(|e| {
            (
                e["class"].as_str().unwrap(),
                e["id"].as_str().unwrap(),
                e["detail"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        idle,
        [
            (
                "workspaces",
                "w3",
                "done-work: w3:p1 shell idle for 48h; w3:p2 shell idle for 48h"
            ),
            ("panes", "w4:p1", "shell idle for 48h"),
        ],
        "{plan}"
    );
    let out = rig.run(&["prune", "--yes", "workspaces", "panes"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let log = rig.log("herdr.log");
    assert!(!log.contains("pane close w1:p1"), "{log}");
    for closed in ["workspace close w3", "pane close w4:p1"] {
        assert!(log.contains(closed), "{log}");
    }
    assert!(
        !log.contains("pane close w4:p2") && !log.contains("pane close w3:"),
        "{log}"
    );
    // A shell that ran a command since doctor is kept.
    fs::write(rig.dir.join("busy"), "w4:p1\nw4:p2\n").unwrap();
    let plan = json!({"entries":[{"class":"panes","id":"w4:p1","detail":"","removable":true,"tree":null}]});
    let (_, applied) = rig.json(&["host", "prune", "--plan", &plan.to_string()]);
    assert_eq!(applied["failures"], json!([]), "{applied}");
    let reason = applied["skipped"][0].as_str().unwrap();
    assert!(
        reason.starts_with("refuse panes w4:p1: no longer safe to remove"),
        "{reason}"
    );
}

#[test]
fn doctor_closes_ended_job_tabs_and_shells_in_deleted_directories_at_once() {
    let rig = shells_rig("spent-panes");
    let home = rig.dir.join("home");
    let gone = rig.dir.join("home/repo/.git/wt/trash/old (deleted)");
    let pane = |id: &str, cwd: &Path| json!({"pane_id":id,"workspace_id":"w4","cwd":cwd});
    // w4:p1 ended its job; w4:p2's job runs; w4:p3 ended but runs a command again; w4:p4's
    // directory is gone; w4:p5 is an ordinary shell seen idle for minutes.
    let live = [
        pane("w4:p1", &home),
        pane("w4:p2", &home),
        pane("w4:p3", &home),
        pane("w4:p4", &gone),
        pane("w4:p5", &home),
    ];
    rig.put("panes.json", &json!({"result":{"panes":live}}));
    fs::write(rig.dir.join("busy"), "w4:p2\nw4:p3\n").unwrap();
    fs::create_dir_all(rig.dir.join("state/jobs")).unwrap();
    let job = |id: &str, pane: &str, ended: Option<u64>| {
        rig.put(
            &format!("state/jobs/{id}.json"),
            &json!({"id":id,"cmd":["true"],"cwd":home,"host":"h","owner":"w1:p1","pane":pane,
                "state":if ended.is_some() {"idle"} else {"working"},"exit":ended.map(|_| 0),
                "signal":null,"reason":null,"started_ms":NOW - 60_000,"ended_ms":ended,
                "timeout_ms":null,"idle_timeout_ms":null}),
        );
    };
    job("r-1", "w4:p1", Some(NOW - 1_000));
    job("r-2", "w4:p2", None);
    job("r-3", "w4:p3", Some(NOW - 1_000));
    rig.snapshot("state/host.json", NOW, 0.0, 1);
    let mut snapshot = rig.read("state/host.json");
    snapshot["probe"]["agent_panes"] = json!([]);
    snapshot["probe"]["panes"] = json!(
        live.iter()
            .map(|p| {
                let mut p = p.clone();
                p["idle_shell"] = json!(true);
                p["idle_since_ms"] = json!(NOW - 60_000);
                p
            })
            .collect::<Vec<_>>()
    );
    rig.put("state/host.json", &snapshot);
    let (code, plan) = rig.json(&["doctor"]);
    assert_eq!(code, 0, "{plan}");
    let panes: Vec<_> = plan["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["class"] == "panes" || e["class"] == "workspaces")
        .map(|e| (e["id"].as_str().unwrap(), e["detail"].as_str().unwrap()))
        .collect();
    assert_eq!(
        panes,
        [
            (
                "w4:p1",
                "job r-1 ended with exit 0; its log keeps the output"
            ),
            ("w4:p4", "shell in a deleted directory"),
        ],
        "{plan}"
    );
    let out = rig.run(&["prune", "--yes", "panes"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let log = rig.log("herdr.log");
    for closed in ["pane close w4:p1", "pane close w4:p4"] {
        assert!(log.contains(closed), "{log}");
    }
    for kept in ["w4:p2", "w4:p3", "w4:p5"] {
        assert!(!log.contains(&format!("pane close {kept}")), "{log}");
    }
    // The job record and log stay until they expire.
    assert!(rig.dir.join("state/jobs/r-1.json").exists());
}

#[test]
fn the_sweep_keeps_a_sidebar_pane_whose_root_process_is_a_running_command() {
    let rig = shells_rig("sidebar-pane");
    let home = rig.dir.join("home");
    // Herdr runs a plugin pane's command as the pane's own root process, so the sidebar
    // reports its own pid as the shell pid and has no other foreground process.
    rig.script("herdr", r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/herdr.log"
case "$*" in
 'machine list --json') cat "$RIG/machines.json" ;;
 'agent list') cat "$RIG/agents.json" ;;
 'pane list') cat "$RIG/panes.json" ;;
 'workspace list') printf '%s\n' '{"result":{"workspaces":[{"workspace_id":"w3","label":"done-work"}]}}' ;;
 'worktree list --cwd'*) printf '%s\n' '{"result":{"worktrees":[]}}' ;;
 'pane process-info --pane '*)
 case "$4" in
  'w3:p1') printf '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":10,"name":"dogsled","argv":["bin/dogsled","sidebar"]}]}}}\n' ;;
  *) printf '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":10,"name":"zsh","argv":["zsh"]}]}}}\n' ;;
 esac ;;
 'pane close'*|'workspace close'*) printf '{}\n' ;;
 *) echo unexpected >&2; exit 2 ;;
esac
"#);
    let pane =
        |id: &str, workspace: &str| json!({"pane_id":id,"workspace_id":workspace,"cwd":home});
    let live = [pane("w3:p1", "w3"), pane("w4:p1", "w4")];
    rig.put("panes.json", &json!({"result":{"panes":live}}));
    rig.snapshot("state/host.json", NOW, 0.0, 1);
    let mut snapshot = rig.read("state/host.json");
    snapshot["probe"]["agent_panes"] = json!([]);
    // The last sample saw both panes as idle shells.
    snapshot["probe"]["panes"] = json!(
        live.iter()
            .map(|p| {
                let mut p = p.clone();
                p["idle_shell"] = json!(true);
                p["idle_since_ms"] = json!(NOW - 48 * 60 * 60 * 1000);
                p
            })
            .collect::<Vec<_>>()
    );
    rig.put("state/host.json", &snapshot);
    let (code, plan) = rig.json(&["doctor"]);
    assert_eq!(code, 0, "{plan}");
    // The sidebar pane is not a candidate. The ordinary idle shell still is.
    let panes: Vec<_> = plan["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["class"] == "panes" || e["class"] == "workspaces")
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        !panes.iter().any(|id| id == "w3:p1" || id == "w3"),
        "{panes:?} in {plan}"
    );
    let out = rig.run(&["prune", "--yes", "panes", "workspaces"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let log = rig.log("herdr.log");
    assert!(!log.contains("pane close w3:p1"), "{log}");
    assert!(!log.contains("workspace close w3"), "{log}");
}
