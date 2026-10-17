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
        &json!({"result":{"agents":[{"pane_id":"w6:p1","agent":"pi","agent_status":"idle"}]}}),
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
    let journal = serve(&rig);
    let log = rig.log("herdr.log");
    for (kind, id) in [("pane", "w4:p1"), ("pane", "w4:p4"), ("workspace", "w5")] {
        assert!(log.contains(&format!("{kind} close {id}\n")), "{log}");
        assert!(
            journal.contains(&format!("wrangle sweep: closed {kind} {id}\n")),
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

#[test]
fn serve_sweeps_a_clean_integrated_worktree_only_when_nothing_holds_it() {
    let rig = watchdog_rig("sweep-worktree");
    let tree = rig.dir.join("home/repo/tree");
    let kept = |rig: &Rig, why: &str| {
        let journal = serve(rig);
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
    // Held by an idle shell, which `prune` would close, or by a working agent.
    let holder = |agent: Value, status: &str| {
        json!({"result":{"panes":[{"pane_id":"w3:p1","workspace_id":"w3","agent":agent,
            "agent_status":status,"cwd":tree}]}})
    };
    rig.put("panes.json", &holder(Value::Null, "unknown"));
    kept(&rig, "idle shell");
    rig.put("panes.json", &holder(json!("pi"), "working"));
    kept(&rig, "working agent");
    // A process with no pane works in it.
    rig.put("panes.json", &json!({"result":{"panes":[]}}));
    {
        let _worker = Worker::start(&tree);
        kept(&rig, "process");
    }
    // Not integrated.
    fs::write(rig.dir.join("unmerged"), "").unwrap();
    kept(&rig, "unmerged");
    fs::remove_file(rig.dir.join("unmerged")).unwrap();
    // Clean, integrated, and unheld: the sweep removes it as `prune` would, and journals it.
    let journal = serve(&rig);
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
            "wrangle sweep: removed {}; branch deleted",
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
    // `done` is squash-merged but on no remote: integrated, which only `prune` takes.
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
    git(
        &local,
        &["commit", "-q", "--allow-empty", "-m", "local only"],
    );
    let dirty = detached("dirty");
    fs::write(dirty.join("scratch"), "x").unwrap();
    let journal = serve(&rig);
    assert!(!branch_exists(&rig, "pushed"), "{journal}");
    assert!(
        journal.contains("wrangle sweep: deleted branch pushed in"),
        "{journal}"
    );
    for kept in ["done", "local", "feat/x"] {
        assert!(branch_exists(&rig, kept), "{kept}: {journal}");
    }
    assert!(!pushed.exists(), "{journal}");
    assert!(
        journal.contains(&format!("wrangle sweep: removed {}", pushed.display())),
        "{journal}"
    );
    // Unpushed, dirty, and a fresh worktree with no commits yet are not the sweep's to take.
    for kept in [&local, &dirty, &tree] {
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
            {"pane_id":"w2:p1","agent":"pi","agent_status":status}
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
fn serve_closes_an_orphan_only_when_it_is_idle_clean_and_pushed() {
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
                "wrangle sweep: keep orphan w2:p1 on local: {reason}"
            )),
            "{journal}"
        );
        untouched(&rig, status);
    }
    // Idle, but dirty.
    child(&rig, "idle", &tree);
    fs::write(tree.join("note"), "x").unwrap();
    serve(&rig);
    assert!(
        kept_reason(&rig).ends_with("has uncommitted changes"),
        "{}",
        kept_reason(&rig)
    );
    untouched(&rig, "dirty");
    fs::remove_file(tree.join("note")).unwrap();
    // Idle and clean, but its commit is on no remote.
    git(&repo, &["update-ref", "refs/remotes/origin/main", "main"]);
    serve(&rig);
    assert!(
        kept_reason(&rig).ends_with("has 1 commit on no remote"),
        "{}",
        kept_reason(&rig)
    );
    // Status and the prompt brief report the kept orphan.
    let text = String::from_utf8_lossy(&rig.run(&["status"]).stdout).into_owned();
    assert!(text.contains("orphaned: owner w9:p1 is gone;"), "{text}");
    let brief = String::from_utf8_lossy(&rig.run(&["status", "--brief"]).stdout).into_owned();
    assert!(brief.contains("1 orphaned"), "{brief}");
    untouched(&rig, "unpushed");
    // Idle, clean, and pushed: closed exactly as `wrangle close` would.
    git(&repo, &["update-ref", "refs/remotes/origin/main", "feat/x"]);
    let journal = serve(&rig);
    assert!(
        journal.contains("wrangle sweep: closed orphan w2:p1 on local (owner w9:p1 is gone)"),
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

/// A child with an owner that is alive, in workspace w2: its agent state, the completion Herdr
/// numbered, and a Pi transcript whose last assistant message is `reply`.
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
                "terminal_id":"term_child","completion_seq":seq,
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
    // Finished but dirty: the reply is captured, and the child stays.
    owned_child(&rig, "idle", &tree, Some(1), "reader");
    fs::write(tree.join("scratch"), "x").unwrap();
    let journal = serve(&rig);
    assert!(item_open(&rig));
    assert!(
        journal.contains("wrangle sweep: keep w2:p1 on local: uncommitted changes"),
        "{journal}"
    );
    let reply = &rig.read("state/items.json")[0]["reply"];
    assert_eq!(reply["seq"], 1);
    assert_eq!(reply["text"], "Found 3 bugs; notes in review.md.");
    // The reason is journaled once, not on every pass.
    assert!(!serve(&rig).contains("keep w2:p1"));
    fs::remove_file(tree.join("scratch")).unwrap();
    // Clean, with no commits: closed as `wrangle close` would, and the reply is kept.
    let journal = serve(&rig);
    assert!(
        journal.contains("wrangle sweep: closed finished w2:p1 on local"),
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
    // No commits yet: a writer has nothing landed.
    let journal = serve(&rig);
    assert!(item_open(&rig), "{journal}");
    assert!(journal.contains("has no commits to land yet"), "{journal}");
    // A commit on no remote and not in main: it stays, and says how much is left to land.
    fs::write(tree.join("fix"), "fix\n").unwrap();
    git(&tree, &["add", "fix"]);
    git(&tree, &["commit", "-q", "-m", "fix"]);
    let journal = serve(&rig);
    assert!(item_open(&rig));
    assert!(
        journal.contains("wrangle sweep: keep w2:p1 on local: +1 commit to land"),
        "{journal}"
    );
    let (_, board) = rig.json(&["board"]);
    assert_eq!(board["rows"][0]["group"], "to_land", "{board}");
    // The branch merges: the next pass closes the child and deletes the landed branch.
    git(&repo, &["merge", "-q", "--no-ff", "-m", "merge", "feat/x"]);
    let journal = serve(&rig);
    assert!(
        journal.contains("wrangle sweep: closed finished w2:p1 on local"),
        "{journal}"
    );
    assert!(!item_open(&rig));
    assert!(!tree.exists());
    assert!(!branch_exists(&rig, "feat/x"));
}
