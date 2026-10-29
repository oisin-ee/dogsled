use super::*;
use std::io::Write as _;

fn git(repo: &Path, args: &[&str]) -> String {
    let out = isolated("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}

/// Persist the P1 format without exposing a new production capture command.
pub(super) fn saved(repo: &Path, unborn: bool) -> Value {
    repository(repo);
    git(repo, &["config", "user.name", "Test"]);
    git(repo, &["config", "user.email", "test@example.invalid"]);
    fs::write(repo.join("file"), b"base\n").unwrap();
    fs::write(repo.join("old"), b"rename\n").unwrap();
    fs::write(repo.join("deleted"), b"delete\n").unwrap();
    if !unborn {
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", "base"]);
    }
    fs::rename(repo.join("old"), repo.join("renamed")).unwrap();
    fs::remove_file(repo.join("deleted")).unwrap();
    fs::write(repo.join("exec"), b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(repo.join("exec"), fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("file", repo.join("link")).unwrap();
    fs::write(repo.join("file"), b"staged\n").unwrap();
    git(repo, &["add", "."]);
    let index = git(repo, &["write-tree"]);
    fs::write(repo.join("file"), b"raw\0\xff\n").unwrap();
    fs::write(repo.join("untracked"), b"untracked\n").unwrap();
    git(repo, &["add", "."]);
    let working = git(repo, &["write-tree"]);
    git(repo, &["read-tree", &index]);
    let head = if unborn {
        json!({"kind":"unborn","branch":"refs/heads/main"})
    } else {
        json!({"kind":"branch","name":"refs/heads/main","commit":git(repo, &["rev-parse", "HEAD"])})
    };
    let id = "0123456789abcdef0123456789abcdef";
    let reference = format!("refs/dogsled/recovery/{id}");
    let record = json!({"id":id,"ref_name":reference,"head":head,"index_tree":index,
        "worktree_tree":working,"raw_worktree_bytes":true,"created_ms":1,"ignored_files_captured":false});
    let mut args = vec![
        "commit-tree",
        index.as_str(),
        "-m",
        "Dogsled recovery staged index",
    ];
    if let Some(commit) = head["commit"].as_str() {
        args.extend(["-p", commit]);
    }
    let staged = git(repo, &args);
    let message = record.to_string();
    let mut args = vec!["commit-tree", working.as_str(), "-m", message.as_str()];
    if let Some(commit) = head["commit"].as_str() {
        args.extend(["-p", commit]);
    }
    args.extend(["-p", &staged]);
    let commit = git(repo, &args);
    git(repo, &["update-ref", &reference, &commit]);
    let mut snapshot = record;
    snapshot["commit"] = commit.into();
    snapshot
}

pub(super) fn p3_rig(name: &str) -> (Rig, PathBuf) {
    let (rig, tree) = super::close::close_rig(name);
    rig.put("panes.json", &json!({"result":{"panes":[
        {"pane_id":"w1:p1","workspace_id":"w1","agent":"pi","agent_status":"working"},
        {"pane_id":"w2:p1","terminal_id":"term_child","workspace_id":"w2","agent":"pi","agent_status":"idle","cwd":tree}
    ]}}));
    rig.put(
        "state/items.json",
        &json!([{"id":"p3-child","kind":"agent","host":"local",
        "pane":"w2:p1","owner_pane":"w1:p1","created_ms":1,"closed_ms":null}]),
    );
    (rig, tree)
}

struct PausedClose {
    child: Option<std::process::Child>,
    go: fs::File,
    _fifo: Fifo,
}

impl PausedClose {
    fn finish(&mut self) -> Output {
        (&self.go).write_all(b"continue\n").unwrap();
        self.child.take().unwrap().wait_with_output().unwrap()
    }

    fn interrupt(&mut self) {
        let mut child = self.child.take().unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
    }
}

impl Drop for PausedClose {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn pause_p3(rig: &Rig, phase: &str) -> PausedClose {
    let base = rig.dir.join("barrier");
    let fifo = Fifo::new(&rig.dir, "barrier.go");
    let go = fifo.hold();
    let child = rig
        .cmd(&["close", "w2:p1", "--json"])
        .env(format!("DOGSLED_TEST_LAUNCH_PRESERVATION_{phase}"), &base)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut paused = PausedClose {
        child: Some(child),
        go,
        _fifo: fifo,
    };
    let deadline = Instant::now() + PROCESS_DEADLINE;
    while !base.with_extension("ready").exists() {
        if let Some(status) = paused.child.as_mut().unwrap().try_wait().unwrap() {
            panic!("close exited before {phase}: {status}");
        }
        assert!(Instant::now() < deadline, "no {phase} barrier");
        std::thread::sleep(Duration::from_millis(5));
    }
    paused
}

#[test]
fn p3_owner_close_saves_before_removal() {
    let (rig, tree) = p3_rig("p3-owner");
    let repo = rig.dir.join("home/repo");
    fs::write(tree.join("staged"), b"index\n").unwrap();
    super::close::git(&tree, &["add", "staged"]);
    fs::write(tree.join("staged"), b"working\0\xff\n").unwrap();
    fs::write(tree.join("draft"), b"untracked\n").unwrap();
    fs::set_permissions(tree.join("draft"), fs::Permissions::from_mode(0o755)).unwrap();
    let index = git(&tree, &["write-tree"]);
    let head = git(&tree, &["rev-parse", "HEAD"]);
    let mut paused = pause_p3(&rig, "BEFORE_NORMALIZE");
    let operation = operations(&rig).remove(0);
    assert_eq!(operation["stage"], "final_captured");
    let before_id = operation["initial_id"].as_str().unwrap();
    assert_eq!(
        git(
            &repo,
            &["rev-parse", &format!("refs/dogsled/recovery/{before_id}")]
        ),
        operation["initial"]["commit"]
    );
    assert_eq!(git(&tree, &["write-tree"]), index);
    assert_eq!(fs::read(tree.join("staged")).unwrap(), b"working\0\xff\n");
    assert_eq!(fs::read(tree.join("draft")).unwrap(), b"untracked\n");
    assert!(!rig.log("herdr.log").contains("workspace close"));
    let output = paused.finish();
    let code = output.status.code().unwrap();
    let closed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(code, 0, "{closed}");
    let recovery = &closed["recovery"];
    assert!(recovery["id"].is_string(), "{closed}");
    assert!(!tree.exists());
    assert_eq!(rig.read("state/items.json")[0]["recovery"], *recovery);
    assert_eq!(
        git(
            &repo,
            &[
                "rev-parse",
                &format!("refs/dogsled/recovery/{}", recovery["id"].as_str().unwrap())
            ]
        ),
        recovery["pinned_oid"]
    );
    let (code, saved) = rig.json(&[
        "recovery",
        "show",
        recovery["id"].as_str().unwrap(),
        "--repo",
        repo.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{saved}");
    assert_eq!(saved["context"]["item"], "p3-child");
    let to = rig.dir.join("restored");
    let (code, restored) = restore(&rig, &repo, &saved, &to, &[]);
    assert_eq!(code, 0, "{restored}");
    assert_eq!(git(&to, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&to, &["write-tree"]), index);
    assert_eq!(fs::read(to.join("staged")).unwrap(), b"working\0\xff\n");
    assert_eq!(fs::read(to.join("draft")).unwrap(), b"untracked\n");
    assert_ne!(
        fs::metadata(to.join("draft")).unwrap().permissions().mode() & 0o111,
        0
    );
    let (code, read) = rig.json(&["read", "p3-child"]);
    assert_eq!(code, 0, "{read}");
    assert_eq!(read["recovery"], *recovery);
    let human = rig.run(&["read", "p3-child"]);
    assert!(String::from_utf8_lossy(&human.stdout).contains(&format!(
        "saved (recovery {})",
        recovery["id"].as_str().unwrap()
    )));
    assert!(
        !rig.read("state/items.json")[0]["native_done"]
            .as_bool()
            .unwrap_or(false)
    );
}

#[test]
fn p3_deletion_only_close_preserves_deletions() {
    let (rig, tree) = p3_rig("p3-delete");
    let repo = rig.dir.join("home/repo");
    for name in ["staged", "working"] {
        fs::write(tree.join(name), b"delete me\n").unwrap();
    }
    super::close::git(&tree, &["add", "."]);
    super::close::git(&tree, &["commit", "-qm", "files"]);
    super::close::git(&repo, &["merge", "--ff-only", "feat/x"]);
    super::close::git(&tree, &["rm", "staged"]);
    fs::remove_file(tree.join("working")).unwrap();
    let (code, closed) = rig.json(&["close", "w2:p1"]);
    assert_eq!(code, 0, "{closed}");
    let (code, saved) = rig.json(&[
        "recovery",
        "show",
        closed["recovery"]["id"].as_str().unwrap(),
        "--repo",
        repo.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{saved}");
    let to = rig.dir.join("deleted-restored");
    let (code, restored) = restore(&rig, &repo, &saved, &to, &[]);
    assert_eq!(code, 0, "{restored}");
    assert!(!to.join("staged").exists());
    assert!(!to.join("working").exists());
    assert_eq!(git(&to, &["ls-files", "staged"]), "");
    assert_eq!(git(&to, &["ls-files", "working"]), "working");
}

fn operations(rig: &Rig) -> Vec<Value> {
    let dir = rig.dir.join("state/preservation-operations");
    if !dir.exists() {
        return Vec::new();
    }
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| serde_json::from_slice(&fs::read(entry.unwrap().path()).unwrap()).unwrap())
        .collect()
}

fn assert_p3_kept(rig: &Rig, tree: &Path) {
    assert!(tree.exists());
    assert_eq!(fs::read(tree.join("draft")).unwrap(), b"live work\n");
    assert!(!rig.log("herdr.log").contains("workspace close"));
    assert!(!rig.log("herdr.log").contains("pane close"));
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_null());
}

#[test]
fn p3_snapshot_failure_keeps_live_work() {
    for phase in ["intent", "hash-object", "update-ref", "verification"] {
        let (rig, tree) = p3_rig(&format!("p3-fail-{phase}"));
        fs::write(tree.join("draft"), b"live work\n").unwrap();
        let index = git(&tree, &["write-tree"]);
        if phase == "intent" {
            fs::write(rig.dir.join("state/preservation-operations"), b"blocked").unwrap();
        } else {
            let real = std::env::split_paths(&std::env::var_os("PATH").unwrap())
                .map(|dir| dir.join("git"))
                .find(|path| path.is_file())
                .unwrap();
            let condition = if phase == "verification" {
                "case \"$*\" in *'show-ref --verify --hash refs/dogsled/recovery/'*) exit 1;; esac"
                    .to_owned()
            } else {
                format!("case \"$*\" in *'{phase}'*) exit 1;; esac")
            };
            rig.script(
                "git",
                &format!("#!/bin/sh\n{condition}\nexec '{}' \"$@\"\n", real.display()),
            );
        }
        let out = rig.run(&["close", "w2:p1"]);
        assert!(!out.status.success(), "{phase}");
        assert_p3_kept(&rig, &tree);
        assert_eq!(git(&tree, &["write-tree"]), index);
        assert_eq!(rig.log("hook.log"), "");
    }
}

#[test]
fn p3_teardown_changes_require_a_second_snapshot() {
    let (rig, tree) = p3_rig("p3-teardown");
    let repo = rig.dir.join("home/repo");
    fs::write(tree.join("draft"), b"before hook\n").unwrap();
    fs::write(rig.dir.join("hook-dirt"), b"").unwrap();
    fs::write(rig.dir.join("hook-commit"), b"").unwrap();
    let (code, closed) = rig.json(&["close", "w2:p1"]);
    assert_eq!(code, 0, "{closed}");
    assert!(!tree.exists());
    assert_eq!(rig.log("hook.log").lines().count(), 1);
    let recovery = &closed["recovery"];
    assert_eq!(recovery["predecessors"].as_array().unwrap().len(), 1);
    let predecessor = recovery["predecessors"][0].as_str().unwrap();
    assert_ne!(predecessor, recovery["id"].as_str().unwrap());
    assert_ne!(
        git(
            &repo,
            &["rev-parse", &format!("refs/dogsled/recovery/{predecessor}")]
        ),
        ""
    );
    let (code, saved) = rig.json(&[
        "recovery",
        "show",
        recovery["id"].as_str().unwrap(),
        "--repo",
        repo.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{saved}");
    let to = rig.dir.join("post-hook");
    let (code, restored) = restore(&rig, &repo, &saved, &to, &[]);
    assert_eq!(code, 0, "{restored}");
    assert_eq!(
        fs::read(to.join("teardown-dirt.txt")).unwrap(),
        b"teardown dirt\n"
    );
    assert_eq!(
        fs::read(to.join("teardown-work.txt")).unwrap(),
        b"teardown work\n"
    );
    assert_eq!(fs::read(to.join("draft")).unwrap(), b"before hook\n");
    assert_eq!(operations(&rig)[0]["stage"], "finished");
}

fn interrupt_p3(rig: &Rig, phase: &str) {
    pause_p3(rig, phase).interrupt();
}

#[test]
fn p3_failed_or_unknown_teardown_keeps_checkout() {
    for failed in [true, false] {
        let (rig, tree) = p3_rig(if failed {
            "p3-failed-hook"
        } else {
            "p3-unknown-hook"
        });
        fs::write(tree.join("draft"), b"live work\n").unwrap();
        if failed {
            fs::write(rig.dir.join("hook-fails"), b"").unwrap();
            assert!(!rig.run(&["close", "w2:p1"]).status.success());
        } else {
            interrupt_p3(&rig, "TEARDOWN_ENTERED");
        }
        assert_p3_kept(&rig, &tree);
        let before = rig.log("hook.log");
        let out = rig.run(&["close", "w2:p1"]);
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success());
        assert!(
            error.contains(if failed {
                "failed teardown"
            } else {
                "unknown teardown outcome"
            }),
            "{error}"
        );
        assert!(error.contains("recovery"), "{error}");
        assert!(error.contains("will not be rerun"), "{error}");
        assert_eq!(before, rig.log("hook.log"));
        let _out = rig.run(&["prune", "--yes", "worktrees"]);
        assert_eq!(
            before,
            rig.log("hook.log"),
            "ordinary prune must not retry an uncertain hook"
        );
        assert_p3_kept(&rig, &tree);
        let operation = &operations(&rig)[0];
        let repo = rig.dir.join("home/repo");
        assert_ne!(
            git(
                &repo,
                &[
                    "rev-parse",
                    &format!(
                        "refs/dogsled/recovery/{}",
                        operation["initial_id"].as_str().unwrap()
                    )
                ]
            ),
            ""
        );
    }
}

#[test]
fn p3_crash_boundaries_keep_a_recoverable_copy() {
    for phase in ["INTENT", "AFTER_PIN", "BEFORE_NORMALIZE", "AFTER_REMOVE"] {
        let (rig, tree) = p3_rig(&format!("p3-crash-{phase}"));
        fs::write(tree.join("draft"), b"live work\n").unwrap();
        interrupt_p3(&rig, phase);
        let operation = operations(&rig).remove(0);
        if phase == "INTENT" {
            assert_p3_kept(&rig, &tree);
        } else {
            let repo = rig.dir.join("home/repo");
            let (code, saved) = rig.json(&[
                "recovery",
                "show",
                operation["initial_id"].as_str().unwrap(),
                "--repo",
                repo.to_str().unwrap(),
            ]);
            assert_eq!(code, 0, "{saved}");
            let to = rig.dir.join("crash-restored");
            let (code, restored) = restore(&rig, &repo, &saved, &to, &[]);
            assert_eq!(code, 0, "{restored}");
            assert_eq!(fs::read(to.join("draft")).unwrap(), b"live work\n");
        }
        if phase == "AFTER_REMOVE" {
            assert!(!tree.exists());
            fs::create_dir(&tree).unwrap();
            fs::write(tree.join("replacement"), b"do not touch\n").unwrap();
        }
        let (code, replay) = rig.json(&["close", "w2:p1"]);
        if phase == "BEFORE_NORMALIZE" {
            assert_ne!(code, 0, "{replay}");
            assert_p3_kept(&rig, &tree);
        } else {
            assert_eq!(code, 0, "{replay}");
            assert_eq!(replay["recovery"]["operation"], operation["id"]);
            if phase == "AFTER_REMOVE" {
                assert_eq!(
                    fs::read(tree.join("replacement")).unwrap(),
                    b"do not touch\n"
                );
            }
        }
    }
}

#[test]
fn p3_unsupported_content_reports_a_hold() {
    for content in ["conflict", "nested", "unreadable", "socket", "submodule"] {
        let (rig, tree) = p3_rig(&format!("p3-unsupported-{content}"));
        fs::write(tree.join("draft"), b"live work\n").unwrap();
        let mut socket = None;
        let expected = match content {
            "conflict" => {
                let oid = git(&tree, &["hash-object", "-w", "dogsled.toml"]);
                // Stage 1 requires index-info input; feed it through the normal Git executable.
                let mut cmd = isolated("git")
                    .arg("-C")
                    .arg(&tree)
                    .args(["update-index", "--index-info"])
                    .stdin(std::process::Stdio::piped())
                    .spawn()
                    .unwrap();
                writeln!(
                    cmd.stdin.take().unwrap(),
                    "0 {}\tconflicted\n100644 {oid} 1\tconflicted",
                    "0".repeat(oid.len())
                )
                .unwrap();
                assert!(cmd.wait().unwrap().success());
                "unmerged entries"
            }
            "nested" => {
                let nested = tree.join("nested");
                fs::create_dir(&nested).unwrap();
                super::close::git(&nested, &["init", "-b", "main"]);
                "nested repository"
            }
            "unreadable" => {
                fs::set_permissions(tree.join("draft"), fs::Permissions::from_mode(0o0)).unwrap();
                "unreadable path"
            }
            "submodule" => {
                let source = rig.dir.join("submodule");
                repository(&source);
                fs::write(source.join("file"), b"base\n").unwrap();
                super::close::git(&source, &["add", "."]);
                super::close::git(&source, &["commit", "-qm", "base"]);
                super::close::git(
                    &tree,
                    &[
                        "-c",
                        "protocol.file.allow=always",
                        "submodule",
                        "add",
                        source.to_str().unwrap(),
                        "sub",
                    ],
                );
                fs::write(tree.join("sub/file"), b"changed\n").unwrap();
                "dirty submodule"
            }
            _ => {
                socket = Some(std::os::unix::net::UnixListener::bind(tree.join("socket")).unwrap());
                "file type"
            }
        };
        let out = rig.run(&["close", "w2:p1"]);
        assert!(!out.status.success(), "{content}");
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(error.contains(expected), "{content}: {error}");
        if content == "unreadable" {
            fs::set_permissions(tree.join("draft"), fs::Permissions::from_mode(0o644)).unwrap();
        }
        assert_p3_kept(&rig, &tree);
        assert_eq!(rig.log("hook.log"), "");
        drop(socket);
    }
}

#[test]
fn p3_remote_lost_close_reply_replays_saved_recovery() {
    let (rig, tree) = p3_rig("p3-lost-reply");
    rig.peer();
    let mut item = rig.read("state/items.json");
    item[0]["host"] = json!("8103");
    rig.put("state/items.json", &item);
    fs::write(tree.join("draft"), b"live work\n").unwrap();
    fs::write(rig.dir.join("lose-reply"), b"").unwrap();
    rig.script(
        "remote-close",
        &format!(
            "#!/bin/sh\nwhile test \"$1\" != host; do shift; done\nexec '{}' \"$@\"\n",
            env!("CARGO_BIN_EXE_dogsled")
        ),
    );
    rig.script("ssh", r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/ssh.log"
case "$*" in
  *'host snapshot'*) cat "$RIG/remote.json" ;;
  *'host close '*)
    for remote do :; done
    DOGSLED_STATE="$RIG/peer-state" sh -c "$RIG/bin/remote-close $remote" > "$RIG/peer-close.json" || exit $?
    if test -f "$RIG/lose-reply"; then rm "$RIG/lose-reply"; exit 255; fi
    cat "$RIG/peer-close.json" ;;
  *) echo unexpected >&2; exit 2 ;;
esac
"#);
    let out = rig.run(&["close", "w2:p1", "--host", "8103"]);
    assert!(!out.status.success());
    assert!(!tree.exists(), "{}", String::from_utf8_lossy(&out.stderr));
    let original: Value = rig.read("peer-close.json");
    assert!(original["recovery"]["id"].is_string(), "{original}");
    assert!(rig.read("state/items.json")[0]["closed_ms"].is_null());
    fs::create_dir(&tree).unwrap();
    super::close::git(&tree, &["init", "-q", "-b", "main"]);
    fs::write(tree.join("replacement"), b"new owner\n").unwrap();
    let replacement = fs::read(tree.join(".git/HEAD")).unwrap();
    let (code, replay) = rig.json(&["close", "w2:p1", "--host", "8103"]);
    assert_eq!(code, 0, "{replay}");
    assert_eq!(replay, original);
    assert_eq!(
        rig.read("state/items.json")[0]["recovery"],
        original["recovery"]
    );
    assert_eq!(fs::read(tree.join("replacement")).unwrap(), b"new owner\n");
    assert_eq!(fs::read(tree.join(".git/HEAD")).unwrap(), replacement);
    assert_eq!(rig.log("hook.log").lines().count(), 1);
}

#[test]
fn p3_replacement_or_head_drift_never_adopts_new_work() {
    for drift in ["head", "checkout", "pane"] {
        let (rig, tree) = p3_rig(&format!("p3-drift-{drift}"));
        fs::write(tree.join("draft"), b"live work\n").unwrap();
        let mut paused = pause_p3(&rig, "BEFORE_NORMALIZE");
        match drift {
            "head" => {
                super::close::git(
                    &tree,
                    &["commit", "--allow-empty", "-qm", "concurrent head"],
                );
            }
            "checkout" => {
                fs::rename(&tree, rig.dir.join("original")).unwrap();
                fs::create_dir(&tree).unwrap();
                super::close::git(&tree, &["init", "-q", "-b", "main"]);
                fs::write(tree.join("replacement"), b"new content\n").unwrap();
            }
            _ => {
                let mut panes = rig.read("panes.json");
                panes["result"]["panes"][1]["terminal_id"] = json!("replacement");
                rig.put("panes.json", &panes);
            }
        }
        let out = paused.finish();
        assert!(!out.status.success(), "{drift}");
        assert!(!rig.log("herdr.log").contains("workspace close"));
        assert!(!rig.log("herdr.log").contains("pane close"));
        assert!(rig.read("state/items.json")[0]["closed_ms"].is_null());
        if drift == "checkout" {
            assert_eq!(
                fs::read(tree.join("replacement")).unwrap(),
                b"new content\n"
            );
        } else {
            assert_eq!(fs::read(tree.join("draft")).unwrap(), b"live work\n");
        }
    }
}

fn protected_state(rig: &Rig) -> Vec<(PathBuf, Vec<u8>)> {
    [
        "preparations.json",
        "preparation-receipts/original.json",
        "preparation-receipts/original.disposition.json",
        "admissions/original.json",
        "placement-grants/original.json",
        "launches/original.json",
        "tasks/original.json",
    ]
    .into_iter()
    .map(|name| {
        let path = rig.dir.join("state").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = format!("{{\"original\":\"{name}\"}}\n").into_bytes();
        fs::write(&path, &bytes).unwrap();
        (path, bytes)
    })
    .collect()
}
fn assert_protected(state: &[(PathBuf, Vec<u8>)]) {
    for (path, bytes) in state {
        assert_eq!(&fs::read(path).unwrap(), bytes, "{}", path.display());
    }
}

#[test]
fn p2_list_and_show_survive_original_checkout_removal() {
    let rig = Rig::short("p2-archive");
    let repo = rig.dir.join("repo");
    let snapshot = saved(&repo, false);
    let archive = rig.dir.join("archive.git");
    assert!(
        isolated("git")
            .args(["clone", "--bare", "--no-hardlinks"])
            .arg(&repo)
            .arg(&archive)
            .output()
            .unwrap()
            .status
            .success()
    );
    git(
        &archive,
        &[
            "update-ref",
            snapshot["ref_name"].as_str().unwrap(),
            snapshot["commit"].as_str().unwrap(),
        ],
    );
    fs::remove_dir_all(&repo).unwrap();
    let repo = archive;
    let state = protected_state(&rig);
    let before = rig.log("herdr.log");
    let (code, entries) = rig.json(&["recovery", "list", "--repo", repo.to_str().unwrap()]);
    assert_eq!(code, 0, "{entries}");
    assert_eq!(entries[0]["id"], snapshot["id"]);
    let (code, shown) = rig.json(&[
        "recovery",
        "show",
        snapshot["id"].as_str().unwrap(),
        "--repo",
        repo.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{shown}");
    assert_eq!(shown, snapshot);
    assert_eq!(before, rig.log("herdr.log"));
    let restored = rig.dir.join("restored-from-archive");
    let (code, result) = restore(&rig, &repo, &snapshot, &restored, &[]);
    assert_eq!(code, 0, "{result}");
    assert_eq!(fs::read(restored.join("file")).unwrap(), b"raw\0\xff\n");
    assert_protected(&state);
}

fn restore(rig: &Rig, repo: &Path, snapshot: &Value, to: &Path, extra: &[&str]) -> (i32, Value) {
    let mut args = vec![
        "recovery",
        "restore",
        snapshot["id"].as_str().unwrap(),
        "--repo",
        repo.to_str().unwrap(),
        "--to",
        to.to_str().unwrap(),
    ];
    args.extend_from_slice(extra);
    rig.json(&args)
}

#[test]
fn p2_restore_preserves_index_worktree_and_raw_bytes() {
    let rig = Rig::short("p2-bytes");
    let repo = rig.dir.join("repo");
    let snapshot = saved(&repo, false);
    let before = git(&repo, &["status", "--porcelain=v1"]);
    let state = protected_state(&rig);
    let to = rig.dir.join("restored");
    let (code, result) = restore(&rig, &repo, &snapshot, &to, &[]);
    assert_eq!(code, 0, "{result}");
    assert_eq!(git(&to, &["write-tree"]), snapshot["index_tree"]);
    assert_eq!(fs::read(to.join("file")).unwrap(), b"raw\0\xff\n");
    assert_eq!(fs::read(to.join("untracked")).unwrap(), b"untracked\n");
    assert_eq!(git(&to, &["ls-files", "untracked"]), "");
    assert_eq!(git(&to, &["show", ":file"]), "staged");
    assert_eq!(
        fs::read_link(to.join("link")).unwrap(),
        PathBuf::from("file")
    );
    assert_ne!(
        fs::metadata(to.join("exec")).unwrap().permissions().mode() & 0o111,
        0
    );
    assert!(!to.join("old").exists());
    assert!(!to.join("deleted").exists());
    assert_eq!(fs::read(to.join("renamed")).unwrap(), b"rename\n");
    assert_eq!(before, git(&repo, &["status", "--porcelain=v1"]));
    assert_eq!(fs::read(repo.join("file")).unwrap(), b"raw\0\xff\n");
    assert_eq!(git(&to, &["rev-parse", "HEAD"]), snapshot["head"]["commit"]);
    assert_protected(&state);
}

#[test]
fn p2_restore_does_not_run_filters_or_setup() {
    let rig = Rig::short("p2-filters");
    let repo = rig.dir.join("repo");
    let snapshot = saved(&repo, false);
    let counter = rig.dir.join("filter-counter");
    let setup_counter = rig.dir.join("setup-counter");
    let checkout_counter = rig.dir.join("checkout-counter");
    git(
        &repo,
        &[
            "config",
            "filter.observable.smudge",
            &format!("touch {}; cat", counter.display()),
        ],
    );
    fs::write(repo.join(".git/info/attributes"), b"* filter=observable\n").unwrap();
    git(&repo, &["cat-file", "--filters", "HEAD:file"]);
    assert!(
        counter.exists(),
        "negative control must execute the configured filter"
    );
    fs::remove_file(&counter).unwrap();
    git(
        &repo,
        &[
            "config",
            "filter.observable.clean",
            &format!("touch {}; cat", counter.display()),
        ],
    );
    let hooks = repo.join(".git/hooks");
    fs::write(
        hooks.join("post-checkout"),
        format!("#!/bin/sh\ntouch {}\n", checkout_counter.display()),
    )
    .unwrap();
    fs::set_permissions(
        hooks.join("post-checkout"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    fs::write(
        hooks.join("reference-transaction"),
        format!(
            "#!/bin/sh\ncat >/dev/null\ntouch {}\n",
            checkout_counter.display()
        ),
    )
    .unwrap();
    fs::set_permissions(
        hooks.join("reference-transaction"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    git(&repo, &["update-ref", "refs/heads/hook-control", "HEAD"]);
    assert!(
        checkout_counter.exists(),
        "reference-transaction hook negative control"
    );
    git(&repo, &["update-ref", "-d", "refs/heads/hook-control"]);
    fs::remove_file(&checkout_counter).unwrap();
    fs::write(
        repo.join("dogsled.toml"),
        format!(
            "[worktree]\nsetup = ['touch {}']\n",
            setup_counter.display()
        ),
    )
    .unwrap();
    let to = rig.dir.join("restored");
    let (code, result) = restore(&rig, &repo, &snapshot, &to, &[]);
    assert_eq!(code, 0, "{result}");
    assert_eq!(fs::read(to.join("file")).unwrap(), b"raw\0\xff\n");
    assert!(!counter.exists());
    assert!(!setup_counter.exists());
    assert!(!checkout_counter.exists());
}

#[test]
fn p2_restore_never_reuses_an_existing_destination() {
    let rig = Rig::short("p2-occupied");
    let repo = rig.dir.join("repo");
    let snapshot = saved(&repo, false);
    let to = rig.dir.join("occupied");
    let registrations = git(&repo, &["worktree", "list", "--porcelain"]);
    for kind in ["file", "directory", "symlink", "equivalent"] {
        match kind {
            "file" => fs::write(&to, b"keep").unwrap(),
            "directory" => fs::create_dir(&to).unwrap(),
            "symlink" => std::os::unix::fs::symlink("missing", &to).unwrap(),
            _ => {}
        }
        let target = if kind == "equivalent" {
            repo.join("../repo")
        } else {
            to.clone()
        };
        let (code, result) = restore(&rig, &repo, &snapshot, &target, &[]);
        assert_ne!(code, 0, "{result}");
        assert_eq!(
            registrations,
            git(&repo, &["worktree", "list", "--porcelain"])
        );
        match kind {
            "file" => {
                assert_eq!(fs::read(&to).unwrap(), b"keep");
                fs::remove_file(&to).unwrap();
            }
            "symlink" => {
                assert_eq!(fs::read_link(&to).unwrap(), PathBuf::from("missing"));
                fs::remove_file(&to).unwrap();
            }
            "directory" => {
                assert!(fs::read_dir(&to).unwrap().next().is_none());
                fs::remove_dir(&to).unwrap();
            }
            _ => {}
        }
    }
    let other = rig.dir.join("other");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "--detach",
            other.to_str().unwrap(),
            "HEAD",
        ],
    );
    let registrations = git(&repo, &["worktree", "list", "--porcelain"]);
    assert_ne!(restore(&rig, &repo, &snapshot, &other, &[]).0, 0);
    assert_eq!(
        registrations,
        git(&repo, &["worktree", "list", "--porcelain"])
    );
}

#[test]
fn p2_restore_handles_detached_and_unborn_heads_explicitly() {
    for unborn in [false, true] {
        let rig = Rig::short(if unborn { "p2-unborn" } else { "p2-detached" });
        let repo = rig.dir.join("repo");
        let snapshot = saved(&repo, unborn);
        let to = rig.dir.join("restored");
        if !unborn {
            assert_ne!(
                restore(&rig, &repo, &snapshot, &to, &["--branch", "main"]).0,
                0
            );
            assert!(!to.exists());
        }
        if unborn {
            assert_ne!(restore(&rig, &repo, &snapshot, &to, &[]).0, 0);
            assert!(!to.exists());
            // A branch absent as a ref but already used by an unborn worktree is occupied.
            assert_ne!(
                restore(&rig, &repo, &snapshot, &to, &["--branch", "main"]).0,
                0
            );
            assert!(!to.exists());
        }
        let extra = if unborn {
            vec!["--branch", "recovered"]
        } else {
            vec![]
        };
        let branches = git(&repo, &["for-each-ref", "refs/heads/"]);
        let (code, result) = restore(&rig, &repo, &snapshot, &to, &extra);
        assert_eq!(code, 0, "{result}");
        assert_eq!(git(&to, &["write-tree"]), snapshot["index_tree"]);
        assert_eq!(branches, git(&repo, &["for-each-ref", "refs/heads/"]));
        let out = isolated("git")
            .arg("-C")
            .arg(&to)
            .args(["symbolic-ref", "-q", "HEAD"])
            .output()
            .unwrap();
        if unborn {
            assert!(out.status.success());
            assert_eq!(
                String::from_utf8(out.stdout).unwrap().trim(),
                "refs/heads/recovered"
            );
            assert!(
                !isolated("git")
                    .arg("-C")
                    .arg(&to)
                    .args(["rev-parse", "--verify", "HEAD"])
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        } else {
            assert!(!out.status.success());
        }
    }
}

#[test]
fn p2_corrupt_or_changed_recovery_ref_refuses_before_materialization() {
    let rig = Rig::short("p2-corrupt");
    let repo = rig.dir.join("repo");
    let snapshot = saved(&repo, false);
    let id = snapshot["id"].as_str().unwrap();
    let reference = snapshot["ref_name"].as_str().unwrap();
    let mut record = snapshot.clone();
    record.as_object_mut().unwrap().remove("commit");
    let head = snapshot["head"]["commit"].as_str().unwrap();
    let working = snapshot["worktree_tree"].as_str().unwrap();
    let staged = git(
        &repo,
        &[
            "rev-parse",
            &format!("{}^2", snapshot["commit"].as_str().unwrap()),
        ],
    );
    let to = rig.dir.join("refused");
    for corruption in ["metadata", "parents", "tree", "raw_format", "missing"] {
        let mut metadata = record.clone();
        if corruption == "metadata" {
            metadata["id"] = "wrong".into();
        }
        if corruption == "raw_format" {
            metadata["raw_worktree_bytes"] = false.into();
        }
        let message = metadata.to_string();
        let tree = if corruption == "tree" {
            snapshot["index_tree"].as_str().unwrap()
        } else {
            working
        };
        let mut args = vec!["commit-tree", tree, "-p", head, "-m", message.as_str()];
        if corruption != "parents" {
            args.extend(["-p", &staged]);
        }
        let corrupt = git(&repo, &args);
        git(&repo, &["update-ref", reference, &corrupt]);
        if corruption == "missing" {
            fs::write(
                repo.join(".git").join(reference),
                format!("{}\n", "1".repeat(40)),
            )
            .unwrap();
        }
        let (code, error) = restore(&rig, &repo, &snapshot, &to, &[]);
        assert_ne!(code, 0, "{corruption}: {error}");
        assert!(!to.exists(), "{corruption}");
        let (code, entries) = rig.json(&["recovery", "list", "--repo", repo.to_str().unwrap()]);
        assert_eq!(code, 0, "{corruption}: {entries}");
        assert!(
            entries[0]["error"].as_str().is_some(),
            "{corruption}: {entries}"
        );
        // Restore the exact original ref before the next independent corruption.
        fs::write(
            repo.join(".git").join(reference),
            format!("{}\n", snapshot["commit"].as_str().unwrap()),
        )
        .unwrap();
    }
    git(&repo, &["update-ref", reference, "HEAD"]);
    assert_ne!(restore(&rig, &repo, &snapshot, &to, &[]).0, 0);
    assert!(!to.exists());
    let (code, entries) = rig.json(&["recovery", "list", "--repo", repo.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert_eq!(entries[0]["id"], id);
    assert!(entries[0]["error"].as_str().is_some());
    for invalid in ["HEAD", "../main", "0123456789abcdef0123456789abcdef^1"] {
        let (code, _) = rig.json(&[
            "recovery",
            "restore",
            invalid,
            "--repo",
            repo.to_str().unwrap(),
            "--to",
            to.to_str().unwrap(),
        ]);
        assert_ne!(code, 0);
        assert!(!to.exists());
    }
}

#[test]
fn p2_remote_restore_uses_selected_host_only() {
    let rig = Rig::short("p2-remote");
    rig.peer();
    let repo = rig.dir.join("remote-repo");
    let snapshot = saved(&repo, false);
    rig.script(
        "ssh",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/ssh.log"
for remote do :; done
if test -f "$RIG/unsupported"; then echo 'recovery unsupported' >&2; exit 2; fi
DOGSLED_STATE="$RIG/peer-state" sh -c "$remote"
"#,
    );
    rig.config(&format!(
        "remote_command = ['{}']\n",
        env!("CARGO_BIN_EXE_dogsled")
    ));
    let to = rig.dir.join("remote-destination");
    let (code, result) = restore(&rig, &repo, &snapshot, &to, &["--host", "netcup"]);
    assert_eq!(code, 0, "{result}");
    assert!(to.exists());
    assert!(rig.log("ssh.log").contains("host recovery restore"));
    assert_eq!(
        rig.log("ssh.log").lines().count(),
        1,
        "no fleet search or other peer"
    );
    assert!(rig.log("ssh.log").contains("netcup"));
    assert_eq!(
        git(
            &repo,
            &["rev-parse", snapshot["ref_name"].as_str().unwrap()]
        ),
        snapshot["commit"]
    );
    fs::write(rig.dir.join("unsupported"), b"yes").unwrap();
    let other = rig.dir.join("unsupported-destination");
    let (code, error) = restore(&rig, &repo, &snapshot, &other, &["--host", "netcup"]);
    assert_ne!(code, 0);
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("ensure the peer supports host recovery"),
        "{error}"
    );
    assert!(!other.exists());
}

#[test]
fn p2_recovery_refs_remain_local_and_pinned() {
    let rig = Rig::short("p2-pinned");
    let repo = rig.dir.join("repo");
    let snapshot = saved(&repo, false);
    let remote = rig.dir.join("remote.git");
    assert!(
        isolated("git")
            .args(["init", "--bare", "-b", "main"])
            .arg(&remote)
            .output()
            .unwrap()
            .status
            .success()
    );
    git(
        &repo,
        &["remote", "set-url", "origin", remote.to_str().unwrap()],
    );
    git(&repo, &["push", "origin", "main"]);
    assert_eq!(
        git(&remote, &["for-each-ref", "refs/dogsled/recovery/"]),
        ""
    );
    rig.config(
        "free_core_fraction_min = 0.0\nmemory_reserve_bytes = 0\ndisk_free_min_percent = 0.0\n",
    );
    rig.snapshot("state/host.json", now(), 0.0, 8);
    let out = rig.run(&["serve", "--once", "--interval", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        git(
            &repo,
            &["rev-parse", snapshot["ref_name"].as_str().unwrap()]
        ),
        snapshot["commit"]
    );
    assert_eq!(
        rig.json(&[
            "recovery",
            "show",
            snapshot["id"].as_str().unwrap(),
            "--repo",
            repo.to_str().unwrap()
        ])
        .0,
        0
    );
}
