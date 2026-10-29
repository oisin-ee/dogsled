use super::*;

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
