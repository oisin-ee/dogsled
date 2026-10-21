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
fn serve_closes_ended_job_tabs_and_shells_in_deleted_directories_and_nothing_else() {
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
    rig.done("term_w6", "w6:p1", 1);
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
    for (kind, id) in [("pane", "w4:p1"), ("pane", "w4:p4"), ("workspace", "w5")] {
        assert!(log.contains(&format!("{kind} close {id}\n")), "{log}");
        assert!(
            journal.contains(&format!("dogsled sweep: closed {kind} {id}\n")),
            "{journal}"
        );
    }
    // Busy, idle for a day, the caller's own, and a stale agent stay for `prune`.
    for kept in ["w4:p2", "w4:p3", "w1:p1", "w6:p1", "w5:p"] {
        assert!(!log.contains(&format!("pane close {kept}")), "{log}");
    }
    for kept in ["w1", "w4", "w6"] {
        assert!(!log.contains(&format!("workspace close {kept}\n")), "{log}");
    }
    // The job records stay until they expire, and `prune` still lists the idle shell and agent.
    assert!(rig.dir.join("state/jobs/r-1.json").exists());
    let (_, plan) = rig.json(&["doctor"]);
    for id in ["w4:p3", "w6"] {
        assert!(
            plan["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["id"] == id && e["removable"] == true),
            "{plan}"
        );
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

fn orphan(rig: &Rig, owner: &str, host: &str) {
    rig.put(
        "state/items.json",
        &json!([{"id":"i1","kind":"agent","host":host,"pane":"w2:p1","owner_pane":owner,
            "name":"worker","created_ms":1,"closed_ms":null}]),
    );
}

fn kept_reason(rig: &Rig) -> String {
    let (_, status) = rig.json(&["status"]);
    status["orphans"][0]["reason"]
        .as_str()
        .unwrap_or_else(|| panic!("{status}"))
        .to_owned()
}

#[test]
fn serve_closes_an_orphan_only_when_it_is_done_clean_and_pushed() {
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
    orphan(&rig, "w1:p1", "local");
    child(&rig, "idle", &tree);
    serve(&rig);
    untouched(&rig, "owner alive");
    // The owner is gone. A working or blocked child is reported, not closed.
    orphan(&rig, "w9:p1", "local");
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
    // Idle, but it never called `done`: it may wait on a question.
    child(&rig, "idle", &tree);
    serve(&rig);
    assert_eq!(
        kept_reason(&rig),
        "owner w9:p1 is gone; the child has not called done"
    );
    untouched(&rig, "not done");
    // Done, but dirty.
    rig.done("term_child", "w2:p1", 1);
    fs::write(tree.join("note"), "x").unwrap();
    serve(&rig);
    assert!(
        kept_reason(&rig).contains("uncommitted changes"),
        "{}",
        kept_reason(&rig)
    );
    untouched(&rig, "dirty");
    fs::remove_file(tree.join("note")).unwrap();
    // Idle and clean, but its commit is on no remote.
    git(&repo, &["update-ref", "refs/remotes/origin/main", "main"]);
    serve(&rig);
    assert!(
        kept_reason(&rig).contains("commits are not held"),
        "{}",
        kept_reason(&rig)
    );
    // Status and the prompt brief report the kept orphan.
    let text = String::from_utf8_lossy(&rig.run(&["status"]).stdout).into_owned();
    assert!(text.contains("orphaned: owner w9:p1 is gone;"), "{text}");
    let brief = String::from_utf8_lossy(&rig.run(&["status", "--brief"]).stdout).into_owned();
    assert!(brief.contains("1 orphaned"), "{brief}");
    untouched(&rig, "unpushed");
    // Done, clean, and pushed: closed exactly as `dogsled close` would.
    git(&repo, &["update-ref", "refs/remotes/origin/main", "feat/x"]);
    let journal = serve(&rig);
    assert!(
        journal.contains("dogsled sweep: closed orphan w2:p1 on local (owner w9:p1 is gone)"),
        "{journal}"
    );
    assert!(rig.log("herdr.log").contains("workspace close w2"));
    assert_eq!(rig.log("hook.log"), hook_log(&rig, &tree));
    assert!(!tree.exists());
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_u64());
    let (_, status) = rig.json(&["status"]);
    assert_eq!(status["orphans"], json!([]));
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
    orphan(&rig, "w9:p1", "8103");
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
    assert!(
        journal.contains("dogsled sweep: keep w2:p1 on local: uncommitted changes"),
        "{journal}"
    );
    let reply = &rig.read("state/items.json")[0]["reply"];
    assert_eq!((&reply["seq"], &reply["done"]), (&json!(3), &json!(true)));
    assert_eq!(reply["text"], "Found 3 bugs; notes in review.md.");
    // The reason is journaled once, not on every pass.
    assert!(!serve(&rig).contains("keep w2:p1"));
    fs::remove_file(tree.join("scratch")).unwrap();
    // Clean, with no commits: closed as `dogsled close` would, and the reply is kept.
    let journal = serve(&rig);
    assert!(
        journal.contains("dogsled sweep: closed finished w2:p1 on local"),
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
fn serve_closes_a_finished_writer_once_its_branch_lands() {
    let (rig, tree) = close_rig("sweep-writer");
    let repo = rig.dir.join("home/repo");
    owned_child(&rig, "done", &tree, Some(2), "writer");
    rig.done("term_child", "w2:p1", 2);
    // At the default tip, only a pending preparation holds a fresh checkout.
    rig.put(
        "state/preparations.json",
        &json!([{ "repo":repo,"path":tree,"branch":"feat/x","head":"" }]),
    );
    let journal = serve(&rig);
    assert!(item_open(&rig), "{journal}");
    assert!(journal.contains("pending preparation"), "{journal}");
    rig.put("state/preparations.json", &json!([]));
    // A commit on no remote and not in main: it stays, and says how much is left to land.
    fs::write(tree.join("fix"), "fix\n").unwrap();
    git(&tree, &["add", "fix"]);
    git(&tree, &["commit", "-q", "-m", "fix"]);
    let journal = serve(&rig);
    assert!(item_open(&rig));
    assert!(
        journal.contains("dogsled sweep: keep w2:p1 on local: +1 commit to land"),
        "{journal}"
    );
    // Model a service that is running again after the one-shot service exited.
    rig.service();
    let (_, board) = rig.json(&["board"]);
    assert_eq!(board["rows"][0]["group"], "to_land", "{board}");
    // The branch merges: the next pass closes the child and deletes the landed branch.
    git(&repo, &["merge", "-q", "--no-ff", "-m", "merge", "feat/x"]);
    let journal = serve(&rig);
    assert!(
        journal.contains("dogsled sweep: closed finished w2:p1 on local"),
        "{journal}"
    );
    assert!(!item_open(&rig));
    assert!(!tree.exists());
    assert!(!branch_exists(&rig, "feat/x"));
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
