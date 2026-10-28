//! The last-resort sweep in `serve`: one case per class it applies, and the work it keeps.
use super::close::{branch_exists, close_rig, git, hook_log};
use super::watchdog::{NOW, shells_rig, watchdog_rig};
use super::*;

/// Cargo's `CACHEDIR.TAG`.
const TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by cargo.\n";

/// One `serve --once` pass. It must succeed; its stderr is the journal.
fn serve(rig: &Rig) -> String {
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    let journal = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{journal}");
    journal
}

/// Doctor cleanup is requested by Stop, not by the publication interval.
fn requested_sweep(rig: &Rig) -> String {
    let out = rig
        .cmd(&["hook", "gate"])
        .env("HERDR_ENV", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serve(rig)
}

/// A process that works in `dir` until it is dropped.
struct Worker(std::process::Child);
impl Worker {
    fn start(dir: &Path) -> Self {
        Self(
            Command::new("sleep")
                .arg("60")
                .current_dir(dir)
                .spawn()
                .unwrap(),
        )
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn serve_closes_ended_job_deleted_directory_and_old_shells_but_keeps_unfinished_agents() {
    let rig = shells_rig("sweep-panes");
    // The worktree that every pane's directory resolves to is unmerged: not this test's class.
    fs::write(rig.dir.join("unmerged"), "").unwrap();
    let home = rig.dir.join("home");
    let gone = rig.dir.join("home/deleted");
    let pane = |id: &str, workspace: &str, cwd: &Path| json!({"pane_id":id,"workspace_id":workspace,"cwd":cwd});
    // w4:p1 ended its job; w4:p2's job runs; w4:p3 has idled for two days; w4:p4's directory is
    // gone. w5 holds two ended jobs. w1:p1 is the caller, in a deleted directory too. w6:p1 is an
    // agent idle for two days.
    let mut agent = pane("w6:p1", "w6", &home);
    agent["agent"] = json!("pi");
    agent["agent_status"] = json!("idle");
    let live = [
        pane("w4:p1", "w4", &home),
        pane("w4:p2", "w4", &home),
        pane("w4:p3", "w4", &home),
        pane("w4:p4", "w4", &gone),
        pane("w5:p1", "w5", &home),
        pane("w5:p2", "w5", &home),
        pane("w1:p1", "w1", &gone),
        agent,
    ];
    rig.put("panes.json", &json!({"result":{"panes":live}}));
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{"pane_id":"w6:p1","agent":"pi","agent_status":"idle",
            "terminal_id":"term_w6","completion_seq":1,"state_change_seq":1}]}}),
    );
    fs::write(rig.dir.join("busy"), "w4:p2\n").unwrap();
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
    job("r-5", "w5:p1", Some(NOW - 1_000));
    job("r-6", "w5:p2", Some(NOW - 1_000));
    // The last sample saw w4:p3 idle and w6:p1's agent idle for two days.
    rig.snapshot("state/host.json", NOW, 0.0, 1);
    let mut snapshot = rig.read("state/host.json");
    snapshot["probe"]["agent_panes"][0]["pane"] = json!("w6:p1");
    snapshot["probe"]["agent_panes"][0]["state_since_ms"] = json!(1);
    snapshot["probe"]["panes"] = json!([{"pane_id":"w4:p3","workspace_id":"w4","cwd":home,
        "idle_shell":true,"idle_since_ms":1}]);
    rig.put("state/host.json", &snapshot);
    let journal = requested_sweep(&rig);
    let log = rig.log("herdr.log");
    for (kind, id) in [
        ("pane", "w4:p1"),
        ("pane", "w4:p3"),
        ("pane", "w4:p4"),
        ("workspace", "w5"),
    ] {
        assert!(log.contains(&format!("{kind} close {id}\n")), "{log}");
        assert!(
            journal.contains(&format!("dogsled sweep: closed {kind} {id}\n")),
            "{journal}"
        );
    }
    // Busy shells, the caller's own pane, and agents without done stay in both scopes.
    for kept in ["w4:p2", "w1:p1", "w6:p1", "w5:p"] {
        assert!(!log.contains(&format!("pane close {kept}")), "{log}");
    }
    for kept in ["w1", "w4", "w6"] {
        assert!(!log.contains(&format!("workspace close {kept}\n")), "{log}");
    }
    // The job records stay until they expire. The fake close retains the idle shell fixture.
    assert!(rig.dir.join("state/jobs/r-1.json").exists());
    let (_, plan) = rig.json(&["doctor"]);
    assert!(
        plan["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == "w4:p3" && e["removable"] == true),
        "{plan}"
    );
}

#[test]
fn sweep_keeps_retryable_launches_and_recovers_final_shells_without_rewriting_evidence() {
    for state in ["unattempted", "unknown", "refused"] {
        let (rig, tree) = close_rig(&format!("sweep-final-{state}"));
        // Dirty work stays; only the stranded launch pane can close.
        fs::write(tree.join("keep"), "unfinished work").unwrap();
        rig.put(
            "panes.json",
            &json!({"result":{"panes":[{
                "pane_id":"w2:p2","terminal_id":"term_launch","workspace_id":"w2","cwd":tree
            }]}}),
        );
        fs::create_dir_all(rig.dir.join("state/launches")).unwrap();
        let progress = json!({"request":{"id":"final","action":{"type":"agent","kind":"pi","name":"worker","message":"go"},
            "host":"local","cwd":tree,"owner":"w1:p1"},"pane":"w2:p2","terminal":"term_launch",
            "attempted":state != "unattempted","confirmed":false,"result":{},"error":"response lost"});
        rig.put("state/launches/final.progress.json", &progress);
        if state == "refused" {
            rig.put(
                "state/launches/final.json",
                &json!({"failed":"agent_pane_busy"}),
            );
        }
        let receipt = fs::read(rig.dir.join("state/launches/final.json")).ok();
        let journal = requested_sweep(&rig);
        let log = rig.log("herdr.log");
        assert_eq!(
            log.contains("pane close w2:p2"),
            state != "unattempted",
            "{state}: {journal}\n{log}"
        );
        assert!(!log.contains("workspace close w2"), "{log}");
        assert!(tree.join("keep").exists());
        assert_eq!(rig.read("state/launches/final.progress.json"), progress);
        assert_eq!(
            fs::read(rig.dir.join("state/launches/final.json")).ok(),
            receipt
        );
    }
}

#[test]
fn sweep_uses_fifteen_minutes_and_keeps_focused_caller_busy_and_unfinished_agent_panes() {
    fifteen_minute_shells(None);
}

#[test]
fn obsolete_launch_terminals_use_guarded_fifteen_minute_shell_cleanup() {
    for state in ["refused", "unconfirmed"] {
        fifteen_minute_shells(Some(state));
    }
}

fn fifteen_minute_shells(launch: Option<&str>) {
    let rig = shells_rig(&format!(
        "sweep-fifteen-minutes-{}",
        launch.unwrap_or("none")
    ));
    fs::write(rig.dir.join("unmerged"), "").unwrap();
    let home = rig.dir.join("home");
    let mut live = vec![];
    for (id, minutes, focused) in [
        ("old", 15, false),
        ("young", 14, false),
        ("focused", 30, true),
        ("busy", 30, false),
        ("w1:p1", 30, false),
        ("agent", 30, false),
        ("parent", 30, false),
    ] {
        live.push(json!({"pane_id":id,"cwd":home,"focused":focused,"idle_shell":true,"idle_since_ms":NOW - minutes * 60_000}));
    }
    let mut evidence = Vec::new();
    if let Some(state) = launch {
        fs::create_dir_all(rig.dir.join("state/launches")).unwrap();
        for (index, pane) in live.iter_mut().enumerate() {
            pane["terminal_id"] = json!(format!("term_restored_{index}"));
            let id = format!("obsolete-{index}");
            let progress_path = format!("state/launches/{id}.progress.json");
            rig.put(&progress_path, &json!({
                "request":{"id":id,"action":{"type":"agent","kind":"pi","name":"worker","message":"go"},
                    "host":"local","cwd":home,"owner":"w1:p1"},
                "pane":pane["pane_id"],"terminal":format!("term_old_{index}"),
                "attempted":true,"confirmed":false,"result":{},"error":"response lost"
            }));
            let receipt_path = format!("state/launches/{id}.json");
            if state == "refused" {
                rig.put(&receipt_path, &json!({"failed":"agent_pane_busy"}));
            }
            for path in [progress_path, receipt_path] {
                evidence.push((path.clone(), fs::read(rig.dir.join(&path)).ok()));
            }
        }
    }
    for pane in &mut live[5..] {
        pane["agent"] = json!("pi");
        pane["agent_status"] = json!("idle");
    }
    rig.put("panes.json", &json!({"result":{"panes":live}}));
    rig.put("agents.json", &json!({"result":{"agents":[
        {"pane_id":"agent","agent":"pi","agent_status":"idle","terminal_id":"term_agent","completion_seq":1},
        {"pane_id":"parent","agent":"pi","agent_status":"idle","terminal_id":"term_parent","completion_seq":1}
    ]}}));
    rig.done("term_parent", "parent", 1);
    rig.put("state/items.json", &json!([{"id":"child","kind":"agent","host":"remote","pane":"remote-child","owner_pane":"parent","created_ms":1}]));
    fs::write(rig.dir.join("busy"), "busy\n").unwrap();
    rig.snapshot("state/host.json", NOW, 0.0, 1);
    let mut snapshot = rig.read("state/host.json");
    snapshot["probe"]["panes"] = json!(live);
    snapshot["probe"]["agent_panes"] = json!([
        {"pane":"agent","agent":"pi","state":"idle","state_since_ms":1},
        {"pane":"parent","agent":"pi","state":"idle","state_since_ms":1}
    ]);
    rig.put("state/host.json", &snapshot);
    let (code, plan) = rig.json(&["doctor"]);
    assert_eq!(code, 0, "{plan}");
    let candidates: Vec<_> = plan["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["class"] == "panes")
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(candidates, ["old"], "{plan}");
    let old = plan["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "old")
        .unwrap();
    assert_eq!(old["detail"], "shell idle for 15m");
    let journal = requested_sweep(&rig);
    let log = rig.log("herdr.log");
    assert!(log.contains("pane close old\n"), "{journal}\n{log}");
    for kept in ["young", "focused", "busy", "w1:p1", "agent", "parent"] {
        assert!(!log.contains(&format!("pane close {kept}\n")), "{log}");
    }
    for (path, bytes) in evidence {
        assert_eq!(fs::read(rig.dir.join(&path)).ok(), bytes, "{path}");
    }
}

/// Agent pane w3:p1, idle at completion 5.
fn idle_holder(rig: &Rig) {
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{"pane_id":"w3:p1","agent":"pi","agent_status":"idle",
            "terminal_id":"term_w3","completion_seq":5,"state_change_seq":5}]}}),
    );
}

/// Agent pane w3:p1, idle after the turn that called `done`.
fn done_holder(rig: &Rig) {
    idle_holder(rig);
    rig.done("term_w3", "w3:p1", 5);
}

#[test]
fn an_idle_agent_that_has_not_called_done_holds_its_worktree() {
    let rig = watchdog_rig("sweep-waiting-holder");
    let tree = rig.dir.join("home/repo/tree");
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[{"pane_id":"w3:p1","workspace_id":"w3","agent":"pi",
            "agent_status":"idle","cwd":tree}]}}),
    );
    idle_holder(&rig);
    let entry = |rig: &Rig| {
        let (_, plan) = rig.json(&["doctor"]);
        plan["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["class"] == "worktrees")
            .unwrap()
            .clone()
    };
    let held = entry(&rig);
    assert_eq!(held["removable"], false, "{held}");
    assert!(
        held["detail"]
            .as_str()
            .unwrap()
            .contains("w3:p1 waits for input; it has not called done"),
        "{held}"
    );
    let journal = requested_sweep(&rig);
    assert!(!rig.log("git.log").contains("worktree remove"), "{journal}");
    assert!(!rig.log("herdr.log").contains("close"), "{journal}");
    done_holder(&rig);
    assert_eq!(entry(&rig)["removable"], true);
}

#[test]
fn serve_sweeps_a_clean_integrated_worktree_only_when_nothing_holds_it() {
    let rig = watchdog_rig("sweep-worktree");
    let tree = rig.dir.join("home/repo/tree");
    let kept = |rig: &Rig, why: &str| {
        let journal = requested_sweep(rig);
        assert!(
            !rig.log("git.log").contains("worktree remove"),
            "{why}: {journal}"
        );
        assert!(!rig.log("herdr.log").contains("close"), "{why}");
    };
    // Dirty.
    fs::write(rig.dir.join("dirty"), "").unwrap();
    kept(&rig, "dirty");
    fs::remove_file(rig.dir.join("dirty")).unwrap();
    // A working agent holds it; an idle agent that called `done` does not.
    let holder = |agent: Value, status: &str| {
        json!({"result":{"panes":[{"pane_id":"w3:p1","workspace_id":"w3","agent":agent,
            "agent_status":status,"cwd":tree}]}})
    };
    rig.put("panes.json", &holder(json!("pi"), "working"));
    kept(&rig, "working agent");
    // A process with no pane works in it.
    rig.put("panes.json", &json!({"result":{"panes":[]}}));
    {
        let _worker = Worker::start(&tree);
        kept(&rig, "process");
    }
    // A pane's own processes are decided by its status, not by the process table.
    {
        // The shell starts its child before it reads stdin, so the table shows both.
        let mut shell = Command::new("sh")
            .args(["-c", "sleep 60 & echo ready; wait"])
            .current_dir(&tree)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut ready = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(shell.stdout.take().unwrap()),
            &mut ready,
        )
        .unwrap();
        let shell = Worker(shell);
        let pid = shell.0.id().to_string();
        fs::write(rig.dir.join("shell_pid"), &pid).unwrap();
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
        // A done agent owns its descendants: closing the pane loses nothing.
        rig.put("panes.json", &holder(json!("pi"), "idle"));
        done_holder(&rig);
        let entry = worktree(&rig);
        assert_eq!(entry["removable"], true, "{entry}");
        // A shell owns only itself: a job it left in the background still holds the checkout.
        rig.put("panes.json", &holder(Value::Null, "unknown"));
        let entry = worktree(&rig);
        assert_eq!(entry["removable"], false, "{entry}");
        assert!(
            entry["detail"]
                .as_str()
                .unwrap()
                .contains("a process works in")
        );
        fs::remove_file(rig.dir.join("shell_pid")).unwrap();
        let _ = Command::new("pkill").args(["-P", &pid]).status();
    }
    rig.put("panes.json", &json!({"result":{"panes":[]}}));
    // Not integrated.
    fs::write(rig.dir.join("unmerged"), "").unwrap();
    kept(&rig, "unmerged");
    fs::remove_file(rig.dir.join("unmerged")).unwrap();
    // Clean, integrated, and unheld: the sweep removes it as `prune` would, and journals it.
    let journal = requested_sweep(&rig);
    let git = rig.log("git.log");
    assert!(
        git.contains(&format!("worktree unlock {}", tree.display())),
        "{git}"
    );
    assert!(
        git.contains(&format!("worktree remove {}", tree.display())),
        "{git}"
    );
    assert!(git.contains("branch -D feature"), "{git}");
    assert!(!git.contains("remove -f"), "{git}");
    assert!(
        journal.contains(&format!(
            "dogsled sweep: removed {}; branch deleted",
            tree.display()
        )),
        "{journal}"
    );
    assert_eq!(rig.read("state/worktrees.json"), json!([]));
}

/// A Herdr whose `process-info` reports a root process for each pane. A pane named in
/// `$RIG/busy` runs a command, a pane named in `$RIG/script` runs a script, and every
/// other pane waits at its prompt.
fn root_process_rig(name: &str) -> Rig {
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
 if grep -qx "$4" "$RIG/busy" 2>/dev/null; then
  printf '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":10,"name":"sh","argv":["sh","-c","curl -H token:s3cr3t https://example.invalid"]}]}}}\n'
 elif grep -qx "$4" "$RIG/script" 2>/dev/null; then
  printf '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":10,"name":"zsh","argv":["zsh","build.sh"]}]}}}\n'
 else
  printf '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":10,"name":"zsh","argv":["zsh"]}]}}}\n'
 fi ;;
 'pane close'*|'workspace close'*) printf '{}\n' ;;
 *) echo unexpected >&2; exit 2 ;;
esac
"#);
    rig
}

/// A pane whose snapshot says it idled two days ago, and whose workspace holds it.
fn idle_snapshot(rig: &Rig, pane: &str) {
    let home = rig.dir.join("home");
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[{"pane_id":pane,"workspace_id":"w3","cwd":home}]}}),
    );
    rig.snapshot("state/host.json", NOW, 0.0, 1);
    let mut snapshot = rig.read("state/host.json");
    snapshot["probe"]["agent_panes"] = json!([]);
    snapshot["probe"]["panes"] = json!([{
        "pane_id":pane,"workspace_id":"w3","cwd":home,
        "idle_shell":true,"idle_since_ms":NOW - 48 * 60 * 60 * 1000
    }]);
    rig.put("state/host.json", &snapshot);
}

#[test]
fn the_sweep_keeps_a_pane_whose_root_process_runs_a_command() {
    let rig = root_process_rig("sweep-command-pane");
    // The snapshot says the pane idled two days ago. The live evidence says its root
    // process runs a command, so the pane keeps its work.
    idle_snapshot(&rig, "w3:p1");
    fs::write(rig.dir.join("busy"), "w3:p1\n").unwrap();
    let journal = requested_sweep(&rig);
    let log = rig.log("herdr.log");
    assert!(!log.contains("pane close w3:p1"), "{log}");
    assert!(!log.contains("workspace close w3\n"), "{log}");
    assert!(!journal.contains("closed pane w3:p1"), "{journal}");
    // The pane is not a removal candidate at all.
    let (_, plan) = rig.json(&["doctor"]);
    let ids: Vec<_> = plan["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["class"] == "panes" || e["class"] == "workspaces")
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect();
    assert!(!ids.iter().any(|id| id == "w3:p1" || id == "w3"), "{ids:?}");
}

#[test]
fn the_sweep_keeps_a_pane_whose_root_process_runs_a_script() {
    let rig = root_process_rig("sweep-script-pane");
    // A shell that runs a script is not waiting at its prompt, whatever its name says.
    idle_snapshot(&rig, "w3:p1");
    fs::write(rig.dir.join("script"), "w3:p1\n").unwrap();
    let journal = requested_sweep(&rig);
    let log = rig.log("herdr.log");
    assert!(!log.contains("pane close w3:p1"), "{log}");
    assert!(!log.contains("workspace close w3\n"), "{log}");
    assert!(!journal.contains("closed pane w3:p1"), "{journal}");
}

#[test]
fn a_hold_never_shows_the_arguments_of_a_command() {
    let rig = root_process_rig("sweep-secret-pane");
    let tree = rig.dir.join("home/repo/tree");
    // The pane works in the checkout and runs a command. The hold that keeps the
    // checkout names the program, never its arguments.
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[{"pane_id":"w3:p1","workspace_id":"w3","cwd":tree}]}}),
    );
    fs::write(rig.dir.join("busy"), "w3:p1\n").unwrap();
    let (_, plan) = rig.json(&["doctor"]);
    let rendered = serde_json::to_string(&plan).unwrap();
    assert!(rendered.contains("is busy (running sh)"), "{rendered}");
    assert!(!rendered.contains("s3cr3t"), "{rendered}");
}

#[test]
fn the_sweep_closes_a_pane_whose_root_process_waits_at_a_prompt() {
    let rig = root_process_rig("sweep-idle-pane");
    idle_snapshot(&rig, "w3:p1");
    let journal = requested_sweep(&rig);
    let log = rig.log("herdr.log");
    // The pane is the workspace's last pane, so the workspace closes with it.
    assert!(log.contains("workspace close w3\n"), "{log}");
    assert!(
        journal.contains("dogsled sweep: closed workspace w3"),
        "{journal}"
    );
}

#[test]
fn the_sweep_keeps_a_pane_whose_command_started_after_the_plan() {
    let rig = root_process_rig("sweep-late-command");
    idle_snapshot(&rig, "w3:p1");
    // The pane waits at its prompt while the plan is made, so the plan lists it.
    let (_, plan) = rig.json(&["doctor"]);
    let listed = plan["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["id"] == "w3" && e["removable"] == true);
    assert!(listed, "{plan}");
    // Its command starts after the plan. The apply-time recheck must keep it.
    fs::write(rig.dir.join("busy"), "w3:p1\n").unwrap();
    let journal = requested_sweep(&rig);
    let log = rig.log("herdr.log");
    assert!(!log.contains("workspace close w3\n"), "{log}");
    assert!(!journal.contains("closed workspace w3"), "{journal}");
}

/// A rig whose pane starts a command between the sweep's plan and its close.
///
/// The transition is marked by the sweep's own phases, not by a count of reads.
/// The survey reads the workspaces before it checks idle panes; the apply-time
/// recheck reads them again after the plan is complete. The first read marks the
/// survey, and every read after it marks the recheck. Only the recheck sees the
/// command, so the plan and the refusal are the same on every run, whatever else
/// reads processes at the same time.
fn post_plan_command_rig(name: &str, argv: &str) -> Rig {
    let rig = watchdog_rig(name);
    let script = format!(
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/herdr.log"
case "$*" in
 'machine list --json') cat "$RIG/machines.json" ;;
 'agent list') cat "$RIG/agents.json" ;;
 'pane list') cat "$RIG/panes.json" ;;
 'workspace list')
 if ! mkdir "$RIG/survey-started" 2>/dev/null; then
  touch "$RIG/recheck-started"
 fi
 printf '%s\n' '{{"result":{{"workspaces":[{{"workspace_id":"w3","label":"done-work"}}]}}}}' ;;
 'worktree list --cwd'*) printf '%s\n' '{{"result":{{"worktrees":[]}}}}' ;;
 'pane process-info --pane '*)
 if test -f "$RIG/recheck-started"; then
  touch "$RIG/recheck-busy"
  printf '{{"result":{{"process_info":{{"shell_pid":10,"foreground_processes":[{{"pid":10,"name":"sh","argv":{argv}}}]}}}}}}\n'
 else
  test ! -d "$RIG/survey-started" || touch "$RIG/survey-idle"
  printf '{{"result":{{"process_info":{{"shell_pid":10,"foreground_processes":[{{"pid":10,"name":"zsh","argv":["zsh"]}}]}}}}}}\n'
 fi ;;
 'pane close'*|'workspace close'*) printf '{{}}\n' ;;
 *) echo unexpected >&2; exit 2 ;;
esac
"#
    );
    rig.script("herdr", &script);
    rig
}

/// The markers prove the sweep planned an idle pane and rechecked a busy one.
fn assert_post_plan_command(rig: &Rig) {
    for marker in ["survey-idle", "recheck-started", "recheck-busy"] {
        assert!(rig.dir.join(marker).exists(), "missing {marker}");
    }
}

#[test]
fn the_sweep_keeps_a_pane_whose_command_starts_between_the_plan_and_the_close() {
    let rig = post_plan_command_rig("sweep-transition-pane", r#"["sh","-c","sleep 60"]"#);
    idle_snapshot(&rig, "w3:p1");
    let journal = requested_sweep(&rig);
    assert_post_plan_command(&rig);
    let log = rig.log("herdr.log");
    assert!(!log.contains("workspace close w3\n"), "{log}");
    assert!(!journal.contains("closed workspace w3"), "{journal}");
    assert!(
        journal.contains("skipped: refuse w3: workspace is no longer idle"),
        "{journal}"
    );
}

#[test]
fn a_post_plan_busy_checkout_hold_omits_command_secrets_from_sweep_reports() {
    let rig = post_plan_command_rig(
        "sweep-transition-checkout-secret",
        r#"["sh","-c","curl -H token:sweep-argv-secret https://example.invalid"]"#,
    );
    idle_snapshot(&rig, "w3:p1");
    let tree = rig.dir.join("home/repo/tree");
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[{"pane_id":"w3:p1","workspace_id":"w3","cwd":tree}]}}),
    );
    let journal = requested_sweep(&rig);
    assert_post_plan_command(&rig);
    let hold = format!("refuse {}: ", tree.display());
    let busy = "w3:p1 is busy (running sh)";
    assert!(journal.contains(&format!("skipped: {hold}")), "{journal}");
    assert!(journal.contains(busy), "{journal}");
    assert!(tree.join(".git").exists());
    assert_eq!(rig.read("state/worktrees.json")[0]["path"], json!(tree));
    let herdr = rig.log("herdr.log");
    let git = rig.log("git.log");
    assert!(!herdr.contains("pane close w3:p1"), "{herdr}");
    assert!(!herdr.contains("workspace close w3\n"), "{herdr}");
    assert!(!git.contains("worktree remove"), "{git}");
    for output in [&journal, &herdr, &git] {
        assert!(!output.contains("sweep-argv-secret"), "{output}");
    }
    let latest = rig.read("state/sweep.json");
    let history = fs::read_to_string(rig.dir.join("state/sweeps.jsonl")).unwrap();
    assert!(!history.contains("sweep-argv-secret"), "{history}");
    let reports: Vec<Value> = history
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(!reports.is_empty(), "{history}");
    assert_eq!(reports.last().unwrap(), &latest);
    for report in std::iter::once(&latest).chain(&reports) {
        let skipped = report["skipped"].as_array().unwrap();
        assert!(
            skipped.iter().any(|entry| {
                let reason = entry.as_str().unwrap();
                reason.starts_with(&hold) && reason.contains(busy)
            }),
            "{report}"
        );
        assert_eq!(report["failures"], json!([]), "{report}");
        assert!(
            !report.to_string().contains("sweep-argv-secret"),
            "{report}"
        );
    }
}

#[test]
fn prune_keeps_a_pane_whose_root_process_runs_a_command() {
    let rig = root_process_rig("prune-command-pane");
    idle_snapshot(&rig, "w3:p1");
    fs::write(rig.dir.join("busy"), "w3:p1\n").unwrap();
    let out = rig.run(&["prune", "--yes", "panes", "workspaces"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let log = rig.log("herdr.log");
    assert!(!log.contains("pane close w3:p1"), "{log}");
    assert!(!log.contains("workspace close w3\n"), "{log}");
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(!printed.contains("s3cr3t"), "{printed}");
}

#[test]
fn serve_sweeps_pushed_branches_and_detached_worktrees_and_keeps_unpushed_ones() {
    let (rig, tree) = close_rig("sweep-branches");
    let repo = rig.dir.join("home/repo");
    rig.put("state/repositories.json", &json!([repo]));
    rig.put("panes.json", &json!({"result":{"panes":[]}}));
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
    // `done` is squash-merged but on no remote: both scopes remove it.
    git(&repo, &["merge", "-q", "--squash", "done"]);
    git(&repo, &["commit", "-q", "-m", "done (#1)"]);
    git(&repo, &["update-ref", "refs/remotes/origin/main", "main"]);
    git(
        &repo,
        &["update-ref", "refs/remotes/origin/pushed", "pushed"],
    );
    let detached = |name: &str| {
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &format!(".worktrees/{name}"),
            ],
        );
        repo.join(format!(".worktrees/{name}"))
            .canonicalize()
            .unwrap()
    };
    let pushed = detached("pushed");
    let local = detached("local");
    fs::write(local.join("local-only"), "work").unwrap();
    git(&local, &["add", "local-only"]);
    git(&local, &["commit", "-q", "-m", "local only"]);
    let dirty = detached("dirty");
    fs::write(dirty.join("scratch"), "x").unwrap();
    let journal = requested_sweep(&rig);
    assert!(!branch_exists(&rig, "pushed"), "{journal}");
    assert!(
        journal.contains("dogsled sweep: deleted branch pushed in"),
        "{journal}"
    );
    assert!(branch_exists(&rig, "local"), "{journal}");
    for removed in ["done", "feat/x"] {
        assert!(!branch_exists(&rig, removed), "{removed}: {journal}");
    }
    assert!(!pushed.exists(), "{journal}");
    assert!(
        journal.contains(&format!("dogsled sweep: removed {}", pushed.display())),
        "{journal}"
    );
    assert!(
        !tree.exists(),
        "a fresh worktree without a preparation hold loses nothing"
    );
    // Local-only commits and dirty files stay.
    for kept in [&local, &dirty] {
        assert!(kept.exists(), "{}: {journal}", kept.display());
    }
}

/// The child's pane, in workspace w2 with a shell tab, and its agent state.
fn child(rig: &Rig, status: &str, tree: &Path) {
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[
            {"pane_id":"w1:p1","workspace_id":"w1","agent":"pi","agent_status":"working"},
            {"pane_id":"w2:p1","workspace_id":"w2","agent":"pi","agent_status":status,"cwd":tree},
            {"pane_id":"w2:p2","workspace_id":"w2","agent":null,"cwd":tree}
        ]}}),
    );
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[
            {"pane_id":"w1:p1","agent":"pi","agent_status":"working"},
            {"pane_id":"w2:p1","agent":"pi","agent_status":status,"terminal_id":"term_child",
                "completion_seq":1,"state_change_seq":1}
        ]}}),
    );
}

/// An open child with the id `id`, whose owner pane is `owner`. Every child has its own id,
/// because the custody record of a child is written once and never changes its custodian.
fn orphan(rig: &Rig, id: &str, owner: &str, host: &str) {
    rig.put(
        "state/items.json",
        &json!([{"id":id,"kind":"agent","host":host,"pane":"w2:p1","owner_pane":owner,
            "name":"worker","created_ms":1,"closed_ms":null}]),
    );
}

/// A child whose owner pane is `owner`, and whose owner session is `session`.
fn child_with_owner_session(rig: &Rig, owner: &str, session: Option<&str>) {
    let mut item = json!({"id":"i1","kind":"agent","host":"local","pane":"w2:p1","owner_pane":owner,
        "name":"worker","created_ms":1,"closed_ms":null});
    if let Some(session) = session {
        item["owner_session"] = json!(session);
    }
    rig.put("state/items.json", &json!([item]));
}

fn kept_reason(rig: &Rig) -> String {
    let (_, status) = rig.json(&["status"]);
    status["orphans"][0]["reason"]
        .as_str()
        .unwrap_or_else(|| panic!("{status}"))
        .to_owned()
}

#[test]
fn the_sweep_classifies_a_child_by_its_current_custodian_not_its_origin_owner() {
    let (rig, tree) = close_rig("sweep-custody");
    // The origin owner is gone, but a lead claimed the child. The custodian lives and runs
    // its session, so the child is not an orphan.
    child_with_owner_session(&rig, "w9:p1", Some("owner-session"));
    rig.put(
        "state/custody.json",
        &json!([{"item":"i1","host":"local","owner_pane":"w9:p1","owner_session":"owner-session",
            "custodian_pane":"w1:p1","custodian_session":"claimant-session","revision":1,
            "claims":[{"id":"claim-1","claimant_pane":"w1:p1","claimant_session":"claimant-session",
                "claimed_ms":1,"previous_custodian":"w9:p1","revision":1}]}]),
    );
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[
            {"pane_id":"w1:p1","workspace_id":"w1","agent":"pi","agent_status":"working"},
            {"pane_id":"w2:p1","workspace_id":"w2","agent":"pi","agent_status":"working","cwd":tree}
        ]}}),
    );
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[
            {"pane_id":"w1:p1","agent":"pi","agent_status":"working",
                "agent_session":{"kind":"id","value":"claimant-session"}},
            {"pane_id":"w2:p1","agent":"pi","agent_status":"working","terminal_id":"term_child",
                "completion_seq":1,"state_change_seq":1}
        ]}}),
    );
    serve(&rig);
    let (_, status) = rig.json(&["status"]);
    assert_eq!(status["orphans"], json!([]), "{status}");
    assert!(
        rig.read("state/items.json")[0]["closed_ms"].is_null(),
        "a claimed child is not closed as an orphan"
    );
    // The custodian pane is gone: the child is an orphan, and the claim shows in the status.
    rig.put(
        "panes.json",
        &json!({"result":{"panes":[
            {"pane_id":"w2:p1","workspace_id":"w2","agent":"pi","agent_status":"working","cwd":tree}
        ]}}),
    );
    serve(&rig);
    let (_, status) = rig.json(&["status"]);
    assert_eq!(status["orphans"].as_array().unwrap().len(), 1, "{status}");
    assert_eq!(
        status["orphans"][0]["status"],
        json!("orphaned"),
        "{status}"
    );
    assert_eq!(status["orphans"][0]["claimed"], json!(true), "{status}");
    assert!(
        status["orphans"][0]["custodian_pane"] == json!("w1:p1"),
        "{status}"
    );
}

#[test]
fn a_child_with_a_legacy_owner_session_is_never_called_session_ended() {
    let (rig, tree) = close_rig("sweep-legacy-session");
    // The owner pane lives, but the item records no owner session. The session-ended rule
    // needs a recorded identity, so the child is unknown, not orphaned.
    child_with_owner_session(&rig, "w1:p1", None);
    child(&rig, "working", &tree);
    serve(&rig);
    let (_, status) = rig.json(&["status"]);
    let orphans = status["orphans"].as_array().unwrap();
    assert_eq!(orphans.len(), 1, "{status}");
    assert_eq!(orphans[0]["status"], json!("unknown"), "{status}");
    assert!(
        orphans[0]["reason"]
            .as_str()
            .unwrap()
            .contains("custodian session"),
        "{status}"
    );
    // The child is not closed as an orphan: the evidence proves nothing.
    assert!(
        rig.read("state/items.json")[0]["closed_ms"].is_null(),
        "{status}"
    );
}

/// A custody record that cannot be read proves nothing. The child stays listed as unknown
/// instead of dropping out of the sweep, and the other children are still classified.
#[test]
fn a_child_with_an_unreadable_custody_record_stays_listed_as_unknown() {
    let (rig, tree) = close_rig("sweep-custody-unreadable");
    rig.put(
        "state/items.json",
        &json!([
            {"id":"i1","kind":"agent","host":"local","pane":"w2:p1","owner_pane":"w9:p1",
                "name":"first","created_ms":1,"closed_ms":null},
            {"id":"i2","kind":"agent","host":"local","pane":"w2:p2","owner_pane":"w9:p2",
                "name":"second","created_ms":1,"closed_ms":null}
        ]),
    );
    // The custody file is corrupt, so no child's record can be read.
    fs::write(rig.dir.join("state/custody.json"), "{ this is not json").unwrap();
    // The sweep already kept the second child, so its age does not start again.
    rig.put(
        "state/orphans.json",
        &json!([{"item":"i2","host":"local","pane":"w2:p2","owner_pane":"w9:p2",
            "name":"second","reason":"owner w9:p2 is gone; the child is working",
            "since_ms":42}]),
    );
    child(&rig, "working", &tree);
    serve(&rig);
    let (_, status) = rig.json(&["status"]);
    let orphans = status["orphans"].as_array().unwrap();
    assert_eq!(orphans.len(), 2, "{status}");
    for orphan in orphans {
        assert_eq!(orphan["status"], json!("unknown"), "{status}");
        assert!(
            orphan["reason"]
                .as_str()
                .unwrap()
                .contains("cannot read its custody"),
            "{status}"
        );
    }
    // The age of a child the sweep kept is not restarted.
    let kept = orphans.iter().find(|o| o["item"] == json!("i2")).unwrap();
    assert_eq!(kept["since_ms"], json!(42), "{status}");
    // The evidence proves nothing, so no child is closed.
    for item in rig.read("state/items.json").as_array().unwrap() {
        assert!(item["closed_ms"].is_null(), "{item}");
    }
}

/// A custody record that cannot be written proves nothing either. The child stays listed as
/// unknown, and the other child of the same sweep is still classified as an orphan.
#[test]
fn a_child_whose_custody_cannot_be_written_stays_unknown_and_the_other_is_still_orphaned() {
    let (rig, tree) = close_rig("sweep-custody-unwritable");
    rig.put(
        "state/items.json",
        &json!([
            {"id":"i1","kind":"agent","host":"local","pane":"w2:p1","owner_pane":"w9:p1",
                "name":"first","created_ms":1,"closed_ms":null},
            {"id":"i2","kind":"agent","host":"local","pane":"w2:p2","owner_pane":"w9:p2",
                "name":"second","created_ms":1,"closed_ms":null}
        ]),
    );
    // The first child has a custody record, so it is classified: its owner is gone.
    rig.put(
        "state/custody.json",
        &json!([{"item":"i1","host":"local","owner_pane":"w9:p1","custodian_pane":"w9:p1",
            "claims":[]}]),
    );
    // The custody lock cannot be opened, so the record of the second child cannot be
    // written. The record that is there still reads.
    fs::create_dir_all(rig.dir.join("state/custody.lock")).unwrap();
    rig.put(
        "state/orphans.json",
        &json!([{"item":"i2","host":"local","pane":"w2:p2","owner_pane":"w9:p2",
            "name":"second","reason":"owner w9:p2 is gone; the child is working",
            "since_ms":42}]),
    );
    child(&rig, "working", &tree);
    serve(&rig);
    let (_, status) = rig.json(&["status"]);
    let orphans = status["orphans"].as_array().unwrap();
    assert_eq!(orphans.len(), 2, "{status}");
    // The other child of the same sweep is still classified.
    let first = orphans.iter().find(|o| o["item"] == json!("i1")).unwrap();
    assert_eq!(first["status"], json!("orphaned"), "{status}");
    // The child whose custody cannot be written stays listed as unknown.
    let second = orphans.iter().find(|o| o["item"] == json!("i2")).unwrap();
    assert_eq!(second["status"], json!("unknown"), "{status}");
    assert!(
        second["reason"]
            .as_str()
            .unwrap()
            .contains("cannot read its custody"),
        "{status}"
    );
    assert_eq!(second["since_ms"], json!(42), "{status}");
    // The evidence proves nothing, so the child is not closed.
    assert!(rig.read("state/items.json")[1]["closed_ms"].is_null());
}

#[test]
fn the_status_lists_an_unclaimed_orphan_once() {
    let (rig, tree) = close_rig("sweep-status-once");
    // The owner pane is gone, so the child is an orphan the sweep keeps.
    orphan(&rig, "i1", "w9:p1", "local");
    child(&rig, "working", &tree);
    serve(&rig);
    let (_, status) = rig.json(&["status"]);
    let orphans = status["orphans"].as_array().unwrap();
    assert_eq!(orphans.len(), 1, "{status}");
    assert_eq!(orphans[0]["status"], json!("orphaned"), "{status}");
    assert_eq!(orphans[0]["claimed"], json!(false), "{status}");
    // The human status names it once, on its own item line.
    let human = String::from_utf8_lossy(&rig.run(&["status"]).stdout).into_owned();
    assert!(human.contains("orphaned (unclaimed)"), "{human}");
    assert_eq!(human.matches("orphaned (unclaimed)").count(), 1, "{human}");
    // The brief counts the orphan once.
    let brief = String::from_utf8_lossy(&rig.run(&["status", "--brief"]).stdout).into_owned();
    assert!(brief.contains("1 orphaned"), "{brief}");
    assert_eq!(brief.matches("1 orphaned").count(), 1, "{brief}");
}

#[test]
fn serve_closes_an_orphan_only_when_done_and_clean_keeping_local_commits() {
    let (rig, tree) = close_rig("sweep-orphan");
    let repo = rig.dir.join("home/repo");
    fs::write(tree.join("work"), "work\n").unwrap();
    git(&tree, &["add", "work"]);
    git(&tree, &["commit", "-q", "-m", "work"]);
    git(&repo, &["update-ref", "refs/remotes/origin/main", "feat/x"]);
    let untouched = |rig: &Rig, why: &str| {
        assert!(!rig.log("herdr.log").contains("close"), "{why}");
        assert!(tree.exists(), "{why}");
        assert!(
            rig.read("state/items.json")[0]["closed_ms"].is_null(),
            "{why}"
        );
    };
    // An owner that still exists closes its own children.
    orphan(&rig, "i1", "w1:p1", "local");
    child(&rig, "idle", &tree);
    serve(&rig);
    untouched(&rig, "owner alive");
    // The owner is gone. A working or blocked child is reported, not closed.
    orphan(&rig, "i2", "w9:p1", "local");
    for status in ["working", "blocked"] {
        child(&rig, status, &tree);
        let journal = serve(&rig);
        let reason = format!("owner w9:p1 is gone; the child is {status}");
        assert_eq!(kept_reason(&rig), reason);
        assert!(
            journal.contains(&format!(
                "dogsled sweep: keep orphan w2:p1 on local: {reason}"
            )),
            "{journal}"
        );
        untouched(&rig, status);
    }
    // Idle without a correlated stopped episode never authorizes closure.
    child(&rig, "idle", &tree);
    serve(&rig);
    assert_eq!(kept_reason(&rig), "stopped orphan has no recorded episode");
    untouched(&rig, "not done");
    // Done, but dirty.
    rig.done("term_child", "w2:p1", 1);
    let mut marks = rig.read("state/done.json");
    marks["term_child"]["reply"] = json!({"seq":1,"text":"Finished","captured_ms":1,"done":true});
    rig.put("state/done.json", &marks);
    fs::write(tree.join("note"), "x").unwrap();
    serve(&rig);
    assert!(
        kept_reason(&rig).contains("uncommitted changes"),
        "{}",
        kept_reason(&rig)
    );
    untouched(&rig, "dirty");
    fs::remove_file(tree.join("note")).unwrap();
    // Done, clean, and local-only: keep the branch, release the child.
    git(&repo, &["update-ref", "refs/remotes/origin/main", "main"]);
    let journal = serve(&rig);
    assert!(
        journal.contains("dogsled handoff: closed w2:p1")
            || journal
                .contains("dogsled sweep: closed orphan w2:p1 on local (owner w9:p1 is gone)"),
        "{journal}"
    );
    assert!(rig.log("herdr.log").contains("workspace close w2"));
    assert_eq!(rig.log("hook.log"), hook_log(&rig, &tree));
    assert!(!tree.exists());
    assert!(branch_exists(&rig, "feat/x"));
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_u64());
    let (_, status) = rig.json(&["status"]);
    assert_eq!(status["orphans"], json!([]));
}

/// A stopped child with session/turn evidence recorded by the stopped watchdog episode.
fn stopped_orphan(rig: &Rig, tree: &Path) {
    orphan(rig, "i1", "w9:p1", "local");
    child(rig, "idle", tree);
    let transcript = rig.dir.join("stopped.jsonl");
    fs::write(
        &transcript,
        format!(
            "{}\n",
            json!({"type":"message","message":{
                "role":"assistant","content":[{"type":"text","text":"Stopped last reply"}]}
            })
        ),
    )
    .unwrap();
    let mut agents = rig.read("agents.json");
    agents["result"]["agents"][1]["agent_session"] = json!({"kind":"path","value":transcript});
    rig.put("agents.json", &agents);
    rig.put(
        "state/watchdog.json",
        &json!({
            "last_scan_ms":null,"sent":{},"events":{},"marks":{},"notified":{},
            "stopped":{"i1":{"pane":"w2:p1","agent_session":transcript,
                "turn":{"state_change_seq":1,"state_since_ms":null},
                "message_id":null,"attempts":0,"delivered":false,"prompted_after":null}}
        }),
    );
}

#[test]
fn a_clean_stopped_orphan_closes_and_records_output_not_a_result() {
    let (rig, tree) = close_rig("sweep-stopped-clean");
    stopped_orphan(&rig, &tree);
    // Local-only committed work stays on the retained branch, just like done orphans.
    fs::write(tree.join("committed"), "retained\n").unwrap();
    git(&tree, &["add", "committed"]);
    git(&tree, &["commit", "-q", "-m", "local-only work"]);
    let journal = serve(&rig);
    assert!(journal.contains("dogsled sweep: closed stopped orphan w2:p1 on local; reply recorded; task not completed"), "{journal}");
    assert!(!tree.exists());
    assert!(branch_exists(&rig, "feat/x"));
    let item = &rig.read("state/items.json")[0];
    assert!(item["closed_ms"].is_u64(), "{item}");
    assert!(
        item["orphan_reply"]["text"]
            .as_str()
            .unwrap()
            .contains("Stopped last reply"),
        "{item}"
    );
    assert!(item["reply"].is_null(), "{item}");
    assert_ne!(item["native_done"], true);
    assert!(item["orphan_reply"]["result_id"].is_null());
    assert_ne!(item["orphan_reply"]["done"], true);
}

#[test]
fn a_dirty_stopped_orphan_stays_until_p3_preservation() {
    let (rig, tree) = close_rig("sweep-stopped-dirty");
    stopped_orphan(&rig, &tree);
    fs::write(tree.join("dirty"), "unsaved\n").unwrap();
    serve(&rig);
    assert_eq!(
        kept_reason(&rig),
        "stopped orphan has unsaved work; kept until preservation (P3)"
    );
    assert!(tree.exists());
    assert!(!rig.log("herdr.log").contains("close"));
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_null());
}

#[test]
fn a_stopped_child_with_a_live_owner_stays() {
    let (rig, tree) = close_rig("sweep-stopped-owned");
    stopped_orphan(&rig, &tree);
    orphan(&rig, "i1", "w1:p1", "local");
    serve(&rig);
    assert!(tree.exists());
    assert!(!rig.log("herdr.log").contains("close"));
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_null());
}

#[test]
fn protected_orphan_lifecycles_never_close() {
    for state in [
        "working",
        "waiting",
        "asking",
        "blocked",
        "finishing",
        "unknown",
        "lost",
    ] {
        let (rig, tree) = close_rig(&format!("sweep-protected-{state}"));
        stopped_orphan(&rig, &tree);
        match state {
            "working" | "blocked" | "unknown" => {
                child(&rig, state, &tree);
            }
            "waiting" => {
                let mut items = rig.read("state/items.json");
                items
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"id":"nested","kind":"job","host":"local",
                    "pane":"w3:p1","owner_pane":"w2:p1","created_ms":1,"closed_ms":null}));
                rig.put("state/items.json", &items);
            }
            "asking" => {
                let agents = rig.read("agents.json");
                let session = agents["result"]["agents"][1]["agent_session"]["value"].clone();
                rig.put("state/done.json", &json!({"term_child":{
                    "pane":"w2:p1","session":session,"marked_ms":1,
                    "signal":{"kind":"ask","text":"Need an answer","event_id":"ask-1",
                        "turn":{"state_change_seq":1,"state_since_ms":null},"session":session,"recorded_ms":1,"cleared_ms":null}
                }}));
            }
            "finishing" => {
                child(&rig, "working", &tree);
                rig.done("term_child", "w2:p1", 1);
            }
            "lost" => {
                child(&rig, "unknown", &tree);
                rig.put("agents.json", &json!({"result":{"agents":[]}}));
            }
            _ => unreachable!(),
        }
        serve(&rig);
        assert!(tree.exists(), "{state}");
        assert!(!rig.log("herdr.log").contains("close"), "{state}");
        assert!(
            rig.read("state/items.json")[0]["closed_ms"].is_null(),
            "{state}"
        );
    }
}

#[test]
fn stopped_orphan_resource_close_leaves_the_unit_unresolved_across_sweeps() {
    use super::unit_requests::{commit_input, prepare};
    let (rig, tree) = close_rig("sweep-stopped-unit");
    stopped_orphan(&rig, &tree);
    rig.put(
        "workspaces.json",
        &json!({"result":{"workspaces":[{"workspace_id":"w1","label":"test"}]}}),
    );
    let execution = prepare(&rig);
    let id = execution["requestId"].as_str().unwrap();
    let path = format!("state/admissions/{id}.json");
    let mut attempt = rig.read(&path);
    attempt["unit"]["committed"] = commit_input(&execution, 0);
    rig.put(&path, &attempt);
    let mut items = rig.read("state/items.json");
    items[0]["id"] = json!(id);
    rig.put("state/items.json", &items);
    let mut watchdog = rig.read("state/watchdog.json");
    let episode = watchdog["stopped"]["i1"].clone();
    watchdog["stopped"] = json!({id:episode});
    rig.put("state/watchdog.json", &watchdog);
    serve(&rig);
    assert!(!tree.exists());
    let item = &rig.read("state/items.json")[0];
    assert!(item["resource_closed_ms"].is_u64(), "{item}");
    assert_eq!(
        item["resource_close_reason"],
        "resource-only close; task unresolved"
    );
    assert!(item["reply"].is_null());
    for _ in 0..2 {
        serve(&rig);
        let saved = rig.read(&path);
        assert_eq!(saved, attempt);
        assert!(saved["unit"]["returned"].is_null());
        assert!(saved["unit"]["stopped"].is_null());
        assert!(saved["unit"]["not_started"].is_null());
        assert!(saved["unit"]["settlement_ack"].is_null());
    }
}

#[test]
fn serve_closes_a_remote_orphan_over_ssh_with_the_pushed_check() {
    let (rig, tree) = close_rig("sweep-remote-orphan");
    child(&rig, "idle", &tree);
    rig.peer();
    let mut remote = rig.read("remote.json");
    // The peer lists the child's pane, so item reconciliation keeps the item open.
    remote["probe"]["agent_panes"][0]["pane"] = json!("w2:p1");
    // The peer's own pass judged that the child called `done`.
    remote["probe"]["agent_panes"][0]["done"] = json!(true);
    remote["probe"]["panes"] = json!([{"pane_id":"w2:p1","workspace_id":"w2"}]);
    rig.put("remote.json", &remote);
    orphan(&rig, "i1", "w9:p1", "8103");
    rig.script(
        "ssh",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/ssh.log"
case "$*" in
  *'host snapshot'*) cat "$RIG/remote.json" ;;
  *'host close w2:p1 --pushed'*) printf '%s\n' '{"pane":"w2:p1","worktree":null,"pane_closed":true}' ;;
  *) echo unexpected >&2; exit 2 ;;
esac
"#,
    );
    let journal = serve(&rig);
    assert!(
        rig.log("ssh.log").contains("host close w2:p1 --pushed"),
        "{journal}"
    );
    assert!(
        journal.contains("closed orphan w2:p1 on netcup"),
        "{journal}"
    );
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_u64());
}

#[test]
fn build_output_is_report_only_for_serve_and_removed_by_prune_when_unused() {
    let (rig, tree) = close_rig("sweep-targets");
    let repo = rig.dir.join("home/repo").canonicalize().unwrap();
    rig.put("state/repositories.json", &json!([repo]));
    // The main checkout shows in no workspace; the worktree's panes hold it.
    rig.put(
        "worktrees.json",
        &json!({"result":{"worktrees":[
            {"path":repo,"open_workspace_id":null,"is_linked_worktree":false},
            {"path":tree,"open_workspace_id":"w2","is_linked_worktree":true}
        ]}}),
    );
    child(&rig, "working", &tree);
    let tagged = |dir: &Path| {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("CACHEDIR.TAG"), TAG).unwrap();
    };
    tagged(&repo.join("target"));
    tagged(&tree.join("target"));
    // A directory without Cargo's signature is not build output.
    fs::create_dir_all(repo.join("cache")).unwrap();
    fs::write(repo.join("cache/CACHEDIR.TAG"), "not a tag\n").unwrap();
    serve(&rig);
    assert!(repo.join("target").exists() && tree.join("target").exists());
    let targets = |rig: &Rig| -> Vec<(String, bool)> {
        let (_, plan) = rig.json(&["doctor"]);
        plan["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["class"] == "targets")
            .map(|e| {
                (
                    e["id"].as_str().unwrap().to_owned(),
                    e["removable"].as_bool().unwrap(),
                )
            })
            .collect()
    };
    let main = repo.join("target").display().to_string();
    let child_target = tree.join("target").display().to_string();
    assert_eq!(
        targets(&rig),
        [(main.clone(), true), (child_target.clone(), false)]
    );
    // A process working in the main checkout uses it.
    {
        let _worker = Worker::start(&repo);
        assert_eq!(targets(&rig)[0], (main.clone(), false));
    }
    let out = rig.run(&["prune", "--yes", "targets"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(
        stdout.contains(&format!("removed build output {main}")),
        "{stdout}"
    );
    assert!(!repo.join("target").exists());
    assert!(tree.join("target").exists() && repo.join("cache").exists());
}

/// A child with an owner that is alive, in workspace w2: its agent state, Herdr's state counter
/// `seq` (also the completion's number while it waits), and a Pi transcript whose last assistant
/// message is `reply`.
fn owned_child(rig: &Rig, status: &str, tree: &Path, seq: Option<u64>, role: &str) {
    child(rig, status, tree);
    let transcript = rig.dir.join("child.jsonl");
    fs::write(
        &transcript,
        format!(
            "{}\n{}\n",
            json!({"type":"message","message":{"role":"user","content":"review it"}}),
            json!({"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"Found 3 bugs; notes in review.md."}]}})
        ),
    )
    .unwrap();
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[
            {"pane_id":"w1:p1","agent":"pi","agent_status":"working"},
            {"pane_id":"w2:p1","agent":"pi","agent_status":status,"cwd":tree,
                "terminal_id":"term_child","state_change_seq":seq,
                "completion_seq":seq.filter(|_| matches!(status, "idle" | "done")),
                "agent_session":{"agent":"pi","kind":"path","source":"herdr:pi","value":transcript}}
        ]}}),
    );
    rig.put(
        "state/items.json",
        &json!([{"id":"i1","kind":"agent","host":"local","pane":"w2:p1","owner_pane":"w1:p1",
            "name":"worker","created_ms":1,"closed_ms":null,"role":role}]),
    );
}

/// An open child with an untold reply and an origin owner session, but no custody record.
/// The sweep must create origin custody before deciding whether the live owner receives it.
fn open_child_origin_session(name: &str, live_session: Option<&str>) -> (Rig, PathBuf) {
    let (rig, tree) = close_rig(name);
    owned_child(&rig, "idle", &tree, Some(4), "writer");
    let mut items = rig.read("state/items.json");
    items[0]["owner_session"] = json!("origin-session");
    // The host already captured the reply, as in the inbox custody fixtures. This tests
    // delivery even when the owner no longer holds custody and landing cannot capture it.
    items[0]["reply"] = json!({"seq":4,"result_id":null,
        "text":"Found 3 bugs; notes in review.md.","captured_ms":2,
        "reported_ms":null,"done":false});
    rig.put("state/items.json", &items);
    let mut agents = rig.read("agents.json");
    agents["result"]["agents"][0]["terminal_id"] = json!("term_lead");
    if let Some(session) = live_session {
        agents["result"]["agents"][0]["agent_session"] = json!({"kind":"path","value":session});
    }
    rig.put("agents.json", &agents);
    let (code, claimed) = rig.json(&["inbox", "claim", "--harness", "pi"]);
    assert_eq!((code, claimed), (0, json!([])));
    assert!(!rig.dir.join("state/custody.json").exists());
    (rig, tree)
}

#[test]
fn an_open_child_reply_reaches_its_matching_origin_owner_session() {
    let (rig, tree) =
        open_child_origin_session("sweep-open-origin-session-match", Some("origin-session"));
    serve(&rig);
    serve(&rig);
    assert!(item_open(&rig));
    assert!(tree.exists());
    assert!(!rig.log("herdr.log").contains("close"));
    let custody = rig.read("state/custody.json");
    assert_eq!(custody[0]["custodian_pane"], "w1:p1", "{custody}");
    assert_eq!(
        custody[0]["custodian_session"], "origin-session",
        "{custody}"
    );
    let inbox = rig.read("state/inbox/term_lead.json");
    assert_eq!(inbox.as_array().unwrap().len(), 1, "{inbox}");
    assert_eq!(inbox[0]["kind"], "reply");
    assert_eq!(inbox[0]["pane"], "w1:p1");
    assert!(
        inbox[0]["text"]
            .as_str()
            .unwrap()
            .ends_with("Found 3 bugs; notes in review.md."),
        "{inbox}"
    );
    assert!(rig.read("state/items.json")[0]["reply"]["reported_ms"].is_u64());
    let (code, claimed) = rig.json(&["inbox", "claim", "--harness", "pi"]);
    assert_eq!(code, 0, "{claimed}");
    assert_eq!(claimed[0]["kind"], "reply", "{claimed}");
    let (_, status) = rig.json(&["status"]);
    assert_eq!(status["orphans"], json!([]), "{status}");
    assert!(!rig.log("herdr.log").contains("agent prompt"));
}

/// A refused reply stays untold, and status keeps the open child visible for custody.
fn assert_origin_reply_held(rig: &Rig, tree: &Path, expected_status: &str) {
    serve(rig);
    serve(rig);
    assert!(item_open(rig));
    assert!(tree.exists());
    assert!(!rig.log("herdr.log").contains("close"));
    let (code, inbox) = rig.json(&["inbox", "list"]);
    assert_eq!(code, 0, "{inbox}");
    assert!(
        inbox["received"]
            .as_array()
            .unwrap()
            .iter()
            .all(|record| record["kind"] != "reply"),
        "{inbox}"
    );
    let (code, claimed) = rig.json(&["inbox", "claim", "--harness", "pi"]);
    assert_eq!(code, 0, "{claimed}");
    assert!(
        claimed
            .as_array()
            .unwrap()
            .iter()
            .all(|record| record["kind"] != "reply"),
        "{claimed}"
    );
    let items = rig.read("state/items.json");
    assert_eq!(items[0]["owner_session"], "origin-session", "{items}");
    let reply = &items[0]["reply"];
    assert_eq!(
        reply["text"], "Found 3 bugs; notes in review.md.",
        "{reply}"
    );
    assert!(reply["reported_ms"].is_null(), "{reply}");
    let custody = rig.read("state/custody.json");
    assert_eq!(custody[0]["custodian_pane"], "w1:p1", "{custody}");
    assert_eq!(
        custody[0]["custodian_session"], "origin-session",
        "{custody}"
    );
    let (_, status) = rig.json(&["status"]);
    let orphans = status["orphans"].as_array().unwrap();
    assert_eq!(orphans.len(), 1, "{status}");
    assert_eq!(orphans[0]["item"], "i1", "{status}");
    assert_eq!(orphans[0]["status"], expected_status, "{status}");
    assert_eq!(orphans[0]["custodian_pane"], "w1:p1", "{status}");
    assert!(
        orphans[0]["reason"].as_str().unwrap().contains("session"),
        "{status}"
    );
    assert!(!rig.log("herdr.log").contains("agent prompt"));
}

#[test]
fn an_open_child_reply_is_held_when_its_origin_owner_pane_runs_another_session() {
    let (rig, tree) =
        open_child_origin_session("sweep-open-origin-session-reused", Some("another-session"));
    // The reused owner cannot receive the old reply. The clean stopped orphan releases
    // resources, but preserves the old reply without delivering it to the new session.
    serve(&rig);
    serve(&rig);
    assert!(!item_open(&rig));
    assert!(!tree.exists());
    let item = &rig.read("state/items.json")[0];
    assert!(item["reply"]["reported_ms"].is_null(), "{item}");
    assert!(item["orphan_reply"].is_object(), "{item}");
    let (_, inbox) = rig.json(&["inbox", "list"]);
    assert!(
        inbox["received"]
            .as_array()
            .unwrap()
            .iter()
            .all(|record| record["kind"] != "reply"),
        "{inbox}"
    );
}

#[test]
fn an_open_child_reply_is_held_when_its_origin_owner_session_is_unreadable() {
    let (rig, tree) = open_child_origin_session("sweep-open-origin-session-unreadable", None);
    assert_origin_reply_held(&rig, &tree, "unknown");
}

fn item_open(rig: &Rig) -> bool {
    rig.read("state/items.json")[0]["closed_ms"].is_null()
}

#[test]
fn serve_closes_a_finished_reader_with_nothing_to_land_after_capturing_its_reply() {
    let (rig, tree) = close_rig("sweep-reader");
    // Working, blocked, and idle before its first completed turn: open, nothing captured.
    for (status, seq) in [("working", Some(1)), ("blocked", Some(1)), ("idle", None)] {
        owned_child(&rig, status, &tree, seq, "reader");
        serve(&rig);
        assert!(item_open(&rig), "{status}");
        assert!(
            rig.read("state/items.json")[0]["reply"].is_null(),
            "{status}"
        );
        assert!(!rig.log("herdr.log").contains("close"), "{status}");
    }
    // A completed turn without `done` waits for input: the reply is captured, and it stays.
    owned_child(&rig, "idle", &tree, Some(1), "reader");
    let journal = serve(&rig);
    assert!(item_open(&rig), "{journal}");
    assert!(!rig.log("herdr.log").contains("close"));
    let reply = &rig.read("state/items.json")[0]["reply"];
    assert_eq!((&reply["seq"], &reply["done"]), (&json!(1), &Value::Null));
    // The next turn calls `dogsled done`, and ends dirty: captured, and it stays.
    owned_child(&rig, "working", &tree, Some(2), "reader");
    let out = rig
        .cmd(&["done"])
        .env("HERDR_PANE_ID", "w2:p1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    owned_child(&rig, "idle", &tree, Some(3), "reader");
    fs::write(tree.join("scratch"), "x").unwrap();
    let journal = serve(&rig);
    assert!(item_open(&rig));
    assert!(journal.contains("uncommitted changes"), "{journal}");
    let reply = &rig.read("state/items.json")[0]["reply"];
    assert_eq!((&reply["seq"], &reply["done"]), (&json!(3), &json!(true)));
    assert_eq!(reply["text"], "Found 3 bugs; notes in review.md.");
    // The reason is journaled once, not on every pass.
    assert!(!serve(&rig).contains("keep w2:p1"));
    fs::remove_file(tree.join("scratch")).unwrap();
    // Clean, with no commits: closed as `dogsled close` would, and the reply is kept.
    let journal = serve(&rig);
    assert!(
        journal.contains("dogsled handoff: closed w2:p1"),
        "{journal}"
    );
    assert!(!item_open(&rig));
    assert!(rig.log("herdr.log").contains("workspace close w2"));
    assert_eq!(rig.log("hook.log"), hook_log(&rig, &tree));
    assert!(!tree.exists());
    assert_eq!(
        rig.read("state/items.json")[0]["reply"]["text"],
        "Found 3 bugs; notes in review.md."
    );
}

#[test]
fn serve_hands_off_unlanded_writer_and_keeps_branch_on_board_without_a_pane() {
    let (rig, tree) = close_rig("sweep-writer");
    let repo = rig.dir.join("home/repo");
    fs::write(tree.join("fix"), "fix\n").unwrap();
    git(&tree, &["add", "fix"]);
    git(&tree, &["commit", "-q", "-m", "fix"]);
    owned_child(&rig, "working", &tree, Some(2), "writer");
    let out = rig
        .cmd(&["done"])
        .env("HERDR_PANE_ID", "w2:p1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Restore keeps the pane and session, but resets the terminal and every counter.
    owned_child(&rig, "idle", &tree, None, "writer");
    let mut agents = rig.read("agents.json");
    agents["result"]["agents"][1]["terminal_id"] = json!("restored_terminal");
    rig.put("agents.json", &agents);
    serve(&rig);
    assert!(!item_open(&rig));
    assert!(!tree.exists());
    assert!(branch_exists(&rig, "feat/x"));
    assert_eq!(
        rig.read("state/items.json")[0]["reply"]["text"],
        "Found 3 bugs; notes in review.md."
    );
    rig.put("agents.json", &json!({"result":{"agents":[]}}));
    rig.put("panes.json", &json!({"result":{"panes":[]}}));
    rig.service();
    let (_, board) = rig.json(&["board"]);
    assert_eq!(board["rows"][0]["group"], "to_land", "{board}");
    assert_eq!(board["rows"][0]["activity"], "branch feat/x +1 to land");
    assert!(board["rows"][0]["pane"].is_null());
    let status = String::from_utf8_lossy(&rig.run(&["status"]).stdout).into_owned();
    assert!(
        status.contains("worker: branch feat/x +1 to land"),
        "{status}"
    );
    let (code, read) = rig.json(&["read", "worker"]);
    assert_eq!(code, 0, "{read}");
    assert_eq!(read["reply"], "Found 3 bugs; notes in review.md.");
    rig.service.lock().unwrap().take();
    git(&repo, &["merge", "-q", "--no-ff", "-m", "merge", "feat/x"]);
    serve(&rig);
    let (_, board) = rig.json(&["board"]);
    assert_eq!(board["rows"], json!([]));
}

#[test]
fn a_finished_childs_reply_reaches_its_owners_inbox_once() {
    let (rig, tree) = close_rig("sweep-reply-inbox");
    owned_child(&rig, "idle", &tree, Some(3), "reader");
    rig.done("term_child", "w2:p1", 3);
    // The owner reads its inbox; the host's snapshot names both terminals.
    let mut agents = rig.read("agents.json");
    agents["result"]["agents"][0]["terminal_id"] = json!("term_lead");
    rig.put("agents.json", &agents);
    let out = rig
        .cmd(&["inbox", "claim", "--harness", "pi", "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Dirty: it stays open, but the reply is told at once.
    fs::write(tree.join("scratch"), "x").unwrap();
    serve(&rig);
    serve(&rig);
    assert!(item_open(&rig));
    let inbox = rig.read("state/inbox/term_lead.json");
    assert_eq!(inbox.as_array().unwrap().len(), 1, "{inbox}");
    assert_eq!(inbox[0]["kind"], "reply");
    assert_eq!(inbox[0]["from"]["name"], "dogsled");
    let text = inbox[0]["text"].as_str().unwrap();
    assert!(
        text.starts_with("worker (w2:p1 on local) finished."),
        "{text}"
    );
    assert!(
        text.ends_with("Found 3 bugs; notes in review.md."),
        "{text}"
    );
    assert!(rig.read("state/items.json")[0]["reply"]["reported_ms"].is_u64());
    // The claim delivers it; nothing was typed into the owner's pane.
    let (_, claimed) = rig.json(&["inbox", "claim", "--harness", "pi"]);
    assert_eq!(claimed[0]["kind"], "reply");
    assert!(!rig.log("herdr.log").contains("agent prompt"));
}

#[test]
fn a_child_that_waits_for_input_stays_open_and_its_owner_hears_its_question() {
    let (rig, tree) = close_rig("sweep-question");
    owned_child(&rig, "idle", &tree, Some(4), "writer");
    let mut agents = rig.read("agents.json");
    agents["result"]["agents"][0]["terminal_id"] = json!("term_lead");
    rig.put("agents.json", &agents);
    assert!(
        rig.cmd(&["inbox", "claim", "--harness", "pi"])
            .output()
            .unwrap()
            .status
            .success()
    );
    // Clean, with nothing to land, but it never called `done`: it stays, however many passes.
    serve(&rig);
    serve(&rig);
    assert!(item_open(&rig));
    assert!(tree.exists());
    assert!(!rig.log("herdr.log").contains("close"));
    let inbox = rig.read("state/inbox/term_lead.json");
    assert_eq!(inbox.as_array().unwrap().len(), 1, "{inbox}");
    let text = inbox[0]["text"].as_str().unwrap();
    assert!(
        text.starts_with("worker (w2:p1 on local) ended a turn without calling done"),
        "{text}"
    );
    assert!(text.contains("dogsled send w2:p1 --host local"), "{text}");
    // `done` is for the turn that completes the task: an idle agent cannot call it.
    let out = rig
        .cmd(&["done"])
        .env("HERDR_PANE_ID", "w2:p1")
        .output()
        .unwrap();
    assert!(!out.status.success());
    // An orchestrator with an open child cannot call it either.
    owned_child(&rig, "working", &tree, Some(5), "writer");
    let mut items = rig.read("state/items.json");
    items
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"i2","kind":"agent","host":"local",
        "pane":"w3:p1","owner_pane":"w2:p1","name":"grandchild","created_ms":2,"closed_ms":null}));
    rig.put("state/items.json", &items);
    let out = rig
        .cmd(&["done"])
        .env("HERDR_PANE_ID", "w2:p1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        stderr.contains("still owns open work: grandchild"),
        "{stderr}"
    );
    assert!(!rig.dir.join("state/done.json").exists());
}
