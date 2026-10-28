use super::*;
use crate::shell::Fake;
use serde_json::json;

fn ledger() -> Ledger {
    Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()))
}

fn agent(state: &str) -> Agent {
    Agent {
        pane: "w2:p1".into(),
        state: state.into(),
        agent: Some("pi".into()),
        agent_session: Some("session".into()),
        terminal_id: Some("old".into()),
        state_change_seq: Some(10),
        ..Agent::default()
    }
}

fn mark_at() -> Mark {
    Mark {
        pane: "w2:p1".into(),
        item: Some("launch".into()),
        session: Some("session".into()),
        name: Some("worker".into()),
        after: Some(10),
        sealed: None,
        marked_ms: 1,
        branch: None,
        detached: false,
        reply: None,
        native_done: false,
        closed_ms: None,
        dropped_ms: None,
    }
}

fn put(ledger: &Ledger, mark: Mark) {
    ledger
        .write(FILE, &Marks::from([(key(&mark), mark)]))
        .unwrap();
}

#[derive(Debug)]
struct BranchRepo {
    path: std::path::PathBuf,
    calls: std::sync::Mutex<Vec<String>>,
}

impl crate::shell::Shell for BranchRepo {
    fn run(&self, program: &str, args: &[String]) -> Result<crate::shell::Output> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("{program} {}", args.join(" ")));
        if program == "herdr" {
            assert_eq!(args, ["agent", "list"]);
            return Ok(crate::shell::Output {
                status: 0,
                stdout: r#"{"result":{"agents":[]}}"#.into(),
                stderr: String::new(),
            });
        }
        let mut command = std::process::Command::new(program);
        // A commit hook's repository environment must not leak into this fixture.
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                command.env_remove(key);
            }
        }
        let out = command.args(args).output()?;
        Ok(crate::shell::Output {
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

impl BranchRepo {
    fn new() -> Self {
        let repo = Self {
            path: std::env::temp_dir().join(crate::ledger::new_id()),
            calls: std::sync::Mutex::new(Vec::new()),
        };
        std::fs::create_dir(&repo.path).unwrap();
        repo.git(&["init", "--quiet", "-b", "main"]);
        repo.git(&[
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.test",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "base",
        ]);
        repo
    }

    fn git(&self, args: &[&str]) {
        use crate::shell::Shell as _;
        let mut words = vec!["-C".into(), self.path.display().to_string()];
        words.extend(args.iter().map(ToString::to_string));
        self.run_ok("git", &words).unwrap();
    }

    /// Run git in another directory of the same repository.
    fn git_in(&self, dir: &std::path::Path, args: &[&str]) -> String {
        use crate::shell::Shell as _;
        let mut words = vec!["-C".into(), dir.display().to_string()];
        words.extend(args.iter().map(ToString::to_string));
        self.run_ok("git", &words).unwrap().stdout
    }

    /// A locked detached worktree with one commit, as an agent checkout whose branch is gone.
    fn detached_worktree(&self) -> std::path::PathBuf {
        self.git(&[
            "worktree",
            "add",
            "-q",
            "--lock",
            "--reason",
            "agent:worktree",
            "--detach",
            ".worktrees/detached",
        ]);
        let detached = self.path.join(".worktrees/detached");
        std::fs::write(detached.join("work"), "detached work\n").unwrap();
        self.git_in(&detached, &["add", "work"]);
        self.git_in(
            &detached,
            &[
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.test",
                "commit",
                "-q",
                "-m",
                "detached work",
            ],
        );
        detached
    }

    fn mark(&self, ledger: &Ledger) {
        let mut mark = mark_at();
        mark.closed_ms = Some(2);
        mark.branch = Some(Branch {
            repo: self.path.display().to_string(),
            name: "feature".into(),
            ahead: 7,
            landed: false,
        });
        put(ledger, mark);
    }
}

impl Drop for BranchRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn a_detached_checkout_is_preflighted_without_a_branch() {
    let repo = BranchRepo::new();
    let detached = repo.detached_worktree();
    // The default branch holds the detached commit, so nothing is lost with the checkout.
    let head = repo.git_in(&detached, &["rev-parse", "HEAD"]);
    repo.git(&["merge", "-q", "--ff-only", head.trim()]);
    let mut child = agent("working");
    child.cwd = Some(detached.display().to_string());

    match preflight(&repo, &child).unwrap() {
        Preflight::Detached => {}
        other => panic!("a detached checkout must have no branch: {other:?}"),
    }
}

#[test]
fn a_detached_checkout_with_unheld_commits_is_refused() {
    let repo = BranchRepo::new();
    let detached = repo.detached_worktree();
    let mut child = agent("working");
    child.cwd = Some(detached.display().to_string());

    let error = preflight(&repo, &child).unwrap_err();
    let text = error.to_string();
    assert!(text.contains("not held"), "{text}");
    assert!(text.contains("Preserve the work"), "{text}");
}

#[test]
fn closed_handoff_deleted_local_landed_branch_settles_once() {
    let ledger = ledger();
    let repo = BranchRepo::new();
    repo.git(&["branch", "feature"]);
    repo.git(&["update-ref", "refs/remotes/origin/feature", "main"]);
    repo.git(&["branch", "-d", "feature"]);
    repo.mark(&ledger);

    handoff(&ledger, &repo, 3).unwrap();
    let branch = list(&ledger).unwrap().remove(0).branch.unwrap();
    assert!(branch.landed);
    assert_eq!(branch.ahead, 0);
    repo.calls.lock().unwrap().clear();
    handoff(&ledger, &repo, 4).unwrap();
    assert_eq!(*repo.calls.lock().unwrap(), ["herdr agent list"]);
}

#[test]
fn closed_handoff_pushed_only_branch_still_has_one_commit_to_land() {
    let ledger = ledger();
    let repo = BranchRepo::new();
    repo.git(&["checkout", "-q", "-b", "feature"]);
    std::fs::write(repo.path.join("feature.txt"), "work\n").unwrap();
    repo.git(&["add", "feature.txt"]);
    repo.git(&[
        "-c",
        "user.name=test",
        "-c",
        "user.email=test@example.test",
        "commit",
        "--quiet",
        "-m",
        "feature",
    ]);
    repo.git(&["update-ref", "refs/remotes/origin/feature", "HEAD"]);
    repo.git(&["checkout", "-q", "main"]);
    repo.git(&["branch", "-D", "feature"]);
    repo.mark(&ledger);

    handoff(&ledger, &repo, 3).unwrap();
    let branch = list(&ledger).unwrap().remove(0).branch.unwrap();
    assert!(!branch.landed);
    assert_eq!(branch.ahead, 1);
}

#[test]
fn closed_handoff_missing_both_refs_settles_and_leaves_the_board() {
    let ledger = ledger();
    let repo = BranchRepo::new();
    repo.mark(&ledger);

    handoff(&ledger, &repo, 3).unwrap();
    let mark = list(&ledger).unwrap().remove(0);
    let branch = mark.branch.as_ref().unwrap();
    assert!(branch.landed);
    assert_eq!(branch.ahead, 0);

    let status = crate::commands::Status {
        hosts: vec![],
        items: vec![crate::items::Item {
            id: "launch".into(),
            kind: "agent".into(),
            closed_ms: mark.closed_ms,
            branch: mark.branch,
            ..Default::default()
        }],
        queue: vec![],
        orphans: vec![],
        sweep: None,
    };
    assert_eq!(crate::board::build(&status, None, 3).rows, []);
    repo.calls.lock().unwrap().clear();
    handoff(&ledger, &repo, 4).unwrap();
    assert_eq!(*repo.calls.lock().unwrap(), ["herdr agent list"]);
}

#[test]
fn handoff_local_ref_takes_precedence_over_a_landed_origin_ref() {
    let ledger = ledger();
    let repo = BranchRepo::new();
    repo.git(&["update-ref", "refs/remotes/origin/feature", "main"]);
    repo.git(&["checkout", "-q", "-b", "feature"]);
    std::fs::write(repo.path.join("feature.txt"), "local work\n").unwrap();
    repo.git(&["add", "feature.txt"]);
    repo.git(&[
        "-c",
        "user.name=test",
        "-c",
        "user.email=test@example.test",
        "commit",
        "--quiet",
        "-m",
        "feature",
    ]);
    repo.mark(&ledger);

    handoff(&ledger, &repo, 3).unwrap();
    let branch = list(&ledger).unwrap().remove(0).branch.unwrap();
    assert!(!branch.landed);
    assert_eq!(branch.ahead, 1);
}

#[test]
fn unclosed_settled_handoff_still_refreshes_its_branch() {
    let ledger = ledger();
    let repo = BranchRepo::new();
    repo.mark(&ledger);
    let mut mark = list(&ledger).unwrap().remove(0);
    mark.closed_ms = None;
    mark.branch.as_mut().unwrap().landed = true;
    put(&ledger, mark);
    repo.calls.lock().unwrap().clear();

    handoff(&ledger, &repo, 3).unwrap();
    assert!(
        repo.calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.contains("show-ref"))
    );
    assert!(list(&ledger).unwrap()[0].closed_ms.is_none());
}

#[test]
fn branch_refresh_keeps_real_git_failures_as_errors_without_settling() {
    for status in [2, 128] {
        let shell = Fake::new();
        shell.on_output(
            "git -C /repo show-ref --verify --quiet refs/heads/feature",
            crate::shell::Output {
                status,
                stdout: String::new(),
                stderr: "repository is unavailable".into(),
            },
        );
        let mut branch: Branch = serde_json::from_value(json!({
            "repo": "/repo", "name": "feature", "ahead": 7, "landed": false
        }))
        .unwrap();
        let before = branch.clone();
        let error = refresh_branch(&shell, &mut branch).unwrap_err();
        assert!(error.to_string().contains("repository is unavailable"));
        assert_eq!(branch, before);
    }
}

/// A recorded handoff needs the saved final reply and the terminal launch evidence. Without
/// them the native `done` was only accepted, so the lifecycle derives `finishing`.
#[test]
fn a_handoff_is_recorded_only_with_its_reply_and_terminal_evidence() {
    let ledger = ledger();
    let mut mark = mark_at();
    mark.sealed = Some(0);
    put(&ledger, mark);
    // The turn ended and the mark is sealed, but no final reply was saved yet.
    let mut agents = vec![agent("idle")];
    observe(&ledger, &mut agents).unwrap();
    assert!(agents[0].done, "the native done was accepted");
    assert_eq!(agents[0].handoff.as_deref(), Some("accepted"));
    assert!(agents[0].finishing);
    assert_eq!(
        crate::lifecycle::of_agent(&agents[0], crate::lifecycle::Host::Up).state,
        crate::lifecycle::State::Finishing
    );
    // The final reply is saved, and no unit launch holds the handoff back.
    let mut mark = list(&ledger).unwrap().remove(0);
    mark.reply = Some(crate::items::Reply {
        seq: 0,
        result_id: None,
        text: "the final reply".into(),
        captured_ms: 2,
        reported_ms: None,
        done: true,
    });
    put(&ledger, mark);
    let mut agents = vec![agent("idle")];
    observe(&ledger, &mut agents).unwrap();
    assert_eq!(agents[0].handoff.as_deref(), Some("recorded"));
    assert!(!agents[0].finishing);
    assert_eq!(
        crate::lifecycle::of_agent(&agents[0], crate::lifecycle::Host::Up).state,
        crate::lifecycle::State::Done
    );
}

/// The mark the child host keeps carries the native `done` provenance once its handoff
/// saves the final reply. Only that provenance proves a handoff later, when the pane is
/// gone.
#[test]
fn the_mark_records_the_native_done_provenance_with_its_reply() {
    let ledger = ledger();
    let mut mark = mark_at();
    mark.sealed = Some(0);
    put(&ledger, mark);
    // The native done path saves the reply on the mark. The mark is the durable record.
    let mut mark = list(&ledger).unwrap().remove(0);
    mark.reply = Some(crate::items::Reply {
        seq: 0,
        result_id: None,
        text: "the final reply".into(),
        captured_ms: 2,
        reported_ms: None,
        done: true,
    });
    mark.native_done = true;
    put(&ledger, mark);
    assert!(list(&ledger).unwrap()[0].native_done);
    // A mark from before the provenance existed has none, and it never proves a handoff.
    let mut legacy = list(&ledger).unwrap().remove(0);
    legacy.native_done = false;
    put(&ledger, legacy);
    assert!(!list(&ledger).unwrap()[0].native_done);
}

#[test]
fn restored_terminal_without_counters_seals_the_same_session() {
    let ledger = ledger();
    put(&ledger, mark_at());
    let mut restored = agent("idle");
    restored.terminal_id = Some("new".into());
    restored.state_change_seq = None;
    assert!(
        finished(&ledger, &[restored.clone()])
            .unwrap()
            .contains("w2:p1")
    );
    assert_eq!(list(&ledger).unwrap()[0].sealed, Some(0));
    restored.state = "working".into();
    assert!(finished(&ledger, &[restored.clone()]).unwrap().is_empty());
    assert_eq!(list(&ledger).unwrap()[0].sealed, Some(0));
    restored.state = "idle".into();
    restored.agent_session = Some("other".into());
    assert!(finished(&ledger, &[restored]).unwrap().is_empty());
}

#[test]
fn idle_without_done_and_orchestrators_stay_open() {
    let ledger = ledger();
    assert!(finished(&ledger, &[agent("idle")]).unwrap().is_empty());
    put(&ledger, mark_at());
    crate::items::record(
        &ledger,
        crate::items::Item {
            id: "child".into(),
            kind: "agent".into(),
            host: "local".into(),
            pane: "w3:p1".into(),
            owner_pane: "w2:p1".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(finished(&ledger, &[agent("idle")]).unwrap().is_empty());
}

#[test]
fn inbox_cancels_only_during_the_done_turn_and_never_after_it() {
    let ledger = ledger();
    put(&ledger, mark_at());
    input(
        &ledger,
        &agent("working"),
        Input::Put { from: None },
        || Ok(((), true)),
    )
    .unwrap();
    assert_eq!(list(&ledger).unwrap(), []);
    put(&ledger, mark_at());
    let refused = input(
        &ledger,
        &agent("idle"),
        Input::Put { from: None },
        || -> Result<((), bool)> { panic!("must not deliver") },
    );
    assert!(
        refused
            .unwrap_err()
            .to_string()
            .contains("worker finished; start a new child for more work")
    );
    assert!(
        input(
            &ledger,
            &agent("working"),
            Input::Put { from: None },
            || Ok(((), true))
        )
        .is_err()
    );
    assert_eq!(list(&ledger).unwrap().len(), 1);
}

#[test]
fn an_overlapping_branch_refresh_cannot_erase_a_terminal_seal() {
    let ledger = ledger();
    let older = mark_at();
    put(&ledger, older.clone());
    finished(&ledger, &[agent("idle")]).unwrap();
    save(&ledger, &older).unwrap();
    assert_eq!(list(&ledger).unwrap()[0].sealed, Some(0));
    assert!(
        input(
            &ledger,
            &agent("working"),
            Input::Put { from: None },
            || Ok(((), true))
        )
        .is_err()
    );
}

#[test]
fn legacy_migration_retains_missing_marks_and_rekeys_restored_panes_once() {
    let ledger = ledger();
    ledger
        .write(
            FILE,
            &json!({
                "old_terminal":{"pane":"w2:p1","after":10,"sealed":12,"marked_ms":1},
                "missing_terminal":{"pane":"w9:p1","after":20,"marked_ms":2}
            }),
        )
        .unwrap();
    let mut restored = agent("idle");
    restored.terminal_id = Some("new".into());
    assert!(
        finished(&ledger, &[restored.clone()])
            .unwrap()
            .contains("w2:p1")
    );
    let first: Marks = ledger.read(FILE).unwrap().unwrap();
    assert_eq!(first.len(), 2);
    assert!(!first.contains_key("old_terminal"));
    assert!(first.contains_key("missing_terminal"));
    // A mark from before the native `done` provenance existed has none.
    assert!(first.values().all(|mark| !mark.native_done));
    finished(&ledger, &[restored]).unwrap();
    assert_eq!(first, ledger.read(FILE).unwrap().unwrap());
}

#[test]
fn done_refuses_every_nonworking_state_without_recording() {
    for state in ["idle", "done", "blocked", "unknown"] {
        let ledger = ledger();
        let shell = Fake::new();
        shell.on(
            "herdr agent list",
            &json!({"result":{"agents":[{
                "pane_id":"w2:p1","agent":"pi","agent_status":state,
                "agent_session":{"kind":"path","value":"session"}
            }]}})
            .to_string(),
        );
        assert!(
            mark(&ledger, &shell, "w2:p1", 1)
                .unwrap_err()
                .to_string()
                .contains("call done during the turn")
        );
        assert!(!ledger.dir.join(FILE).exists());
    }
}

#[test]
fn done_refuses_open_children_and_pending_or_admitting_tickets() {
    for queued in [None, Some("pending"), Some("admitting")] {
        let ledger = ledger();
        let shell = Fake::new();
        shell.on("herdr agent list", r#"{"result":{"agents":[{"pane_id":"w2:p1","agent":"pi","agent_status":"working","agent_session":{"kind":"path","value":"session"}}]}}"#);
        if let Some(state) = queued {
            ledger.write("queue.json", &json!([{
                "id":"q","state":state,"created_ms":1,"waiting":false,"waiting_until_ms":null,"notified":false,"result":null,"error":null,
                "request":{"id":"q","action":{"type":"place"},"host":null,"branch":null,"base":null,"cwd":"/tmp","repository":null,"workspace":null,"workspace_name":null,"owner":"w2:p1"}
            }])).unwrap();
        } else {
            crate::items::record(
                &ledger,
                crate::items::Item {
                    id: "child".into(),
                    owner_pane: "w2:p1".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        assert!(
            mark(&ledger, &shell, "w2:p1", 1)
                .unwrap_err()
                .to_string()
                .contains("still owns open work")
        );
        assert!(!ledger.dir.join(FILE).exists());
    }
}
