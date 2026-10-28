//! Real-Git contract tests for loss and the two cleanup scopes.
use crate::cleanup::{Scope, survey};
use crate::error::Result;
use crate::ledger::Ledger;
use crate::loss::{Evidence, Held};
use crate::shell::{Output, Shell};
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct System;
impl Shell for System {
    fn run(&self, program: &str, args: &[String]) -> Result<Output> {
        let stdout = match program {
            "herdr" => Some(if args.starts_with(&["pane".into(), "list".into()]) {
                "{\"result\":{\"panes\":[]}}"
            } else if args.starts_with(&["agent".into(), "list".into()]) {
                "{\"result\":{\"agents\":[]}}"
            } else if args.starts_with(&["workspace".into(), "list".into()]) {
                "{\"result\":{\"workspaces\":[]}}"
            } else {
                "{\"result\":{\"worktrees\":[]}}"
            }),
            "lsof" => Some("p1\nn/\n"),
            "ps" => Some(""),
            "df" => Some("FS 1000 500 400 60% /\n"),
            _ => None,
        };
        if let Some(stdout) = stdout {
            return Ok(Output {
                status: 0,
                stdout: stdout.into(),
                stderr: String::new(),
            });
        }
        let mut cmd = std::process::Command::new(program);
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                cmd.env_remove(key);
            }
        }
        let out = cmd.args(args).output()?;
        Ok(Output {
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

struct Repo {
    root: PathBuf,
    repo: PathBuf,
    remote: PathBuf,
    tree: PathBuf,
}
fn git(path: &Path, args: &[&str]) -> String {
    let mut words = vec![
        "-C".into(),
        path.display().to_string(),
        "-c".into(),
        "user.name=t".into(),
        "-c".into(),
        "user.email=t@t".into(),
    ];
    words.extend(args.iter().map(|s| (*s).into()));
    let out = System.run("git", &words).unwrap();
    assert!(out.ok(), "{}", out.stderr);
    out.stdout
}

impl Repo {
    fn new() -> Self {
        let root = std::env::temp_dir().join(crate::ledger::new_id());
        let repo = root.join("repo");
        let remote = root.join("remote.git");
        std::fs::create_dir_all(&repo).unwrap();
        let tree = root.join("tree");
        let this = Self {
            root,
            repo,
            remote,
            tree,
        };
        git(&this.repo, &["init", "-q", "-b", "main"]);
        git(&this.repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        git(
            &this.repo,
            &[
                "clone",
                "-q",
                "--bare",
                this.repo.to_str().unwrap(),
                this.remote.to_str().unwrap(),
            ],
        );
        git(
            &this.repo,
            &["remote", "add", "origin", this.remote.to_str().unwrap()],
        );
        git(
            &this.repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                this.tree.to_str().unwrap(),
            ],
        );
        this
    }
    fn evidence(&self) -> Evidence {
        Evidence::fetch(&System, self.repo.to_str().unwrap())
    }
    fn assess(&self) -> crate::loss::Assessment {
        self.evidence()
            .assess(
                &System,
                "refs/heads/feature",
                Some(self.tree.to_str().unwrap()),
            )
            .unwrap()
    }
    fn commit(&self) {
        std::fs::write(self.tree.join("work"), "work\n").unwrap();
        git(&self.tree, &["add", "work"]);
        git(&self.tree, &["commit", "-q", "-m", "work"]);
    }
}
impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn prunable_worktree_never_inherits_main_dirt_or_removes_leftover_content() {
    let mut repo = Repo::new();
    let nested = repo.repo.join(".worktrees/feature");
    std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
    git(
        &repo.repo,
        &[
            "worktree",
            "move",
            repo.tree.to_str().unwrap(),
            nested.to_str().unwrap(),
        ],
    );
    repo.tree = nested;
    std::fs::write(repo.repo.join("sandbox.sh"), "main content").unwrap();
    std::fs::write(repo.tree.join("unknown"), "leftover content").unwrap();
    // Even tagged output inside this unidentified directory must remain untouched.
    let target = repo.tree.join("target");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(
        target.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55",
    )
    .unwrap();
    std::fs::remove_file(repo.tree.join(".git")).unwrap();
    assert!(crate::worktree::same(
        git(&repo.tree, &["rev-parse", "--show-toplevel"]).trim(),
        repo.repo.to_str().unwrap()
    ));
    let listed = git(&repo.repo, &["worktree", "list", "--porcelain", "-z"]);
    assert!(
        listed
            .split('\0')
            .any(|field| field.starts_with("prunable"))
    );
    let ledger = Ledger::new(repo.root.join("state"));
    ledger
        .write("repositories.json", &vec![repo.repo.display().to_string()])
        .unwrap();
    let cleanup = crate::config::Cleanup::default();
    let node = crate::node::Node {
        shell: &System,
        ledger: &ledger,
        thresholds: crate::config::Thresholds::from(&crate::config::Policy::default()),
        home: repo.root.to_str().unwrap(),
        cleanup: &cleanup,
    };
    for scope in [Scope::Prune, Scope::Sweep] {
        let mut plan = survey(&node, scope, 100).unwrap();
        // Never apply findings from the developer's own checkout. Git reports resolved paths
        // (`/private/var` on macOS), so compare resolved paths.
        let root = crate::worktree::resolved(&repo.root);
        plan.entries
            .retain(|entry| crate::worktree::resolved(Path::new(&entry.id)).starts_with(&root));
        let finding = plan
            .entries
            .iter()
            .find(|entry| crate::worktree::same(&entry.id, repo.tree.to_str().unwrap()))
            .unwrap();
        assert_eq!(finding.kind.as_deref(), Some(crate::cleanup::UNLINKED));
        assert!(
            finding.detail.contains("git marks the worktree prunable"),
            "{}",
            finding.detail
        );
        assert!(!finding.removable);
        assert_eq!(finding.dirty_paths, Vec::<String>::new());
        assert_eq!(finding.dirty_count, None);
        assert!(
            !plan
                .entries
                .iter()
                .any(|entry| entry.kind.as_deref() == Some("landed_dirty"))
        );
        assert!(!plan.entries.iter().any(|entry| entry.class == "targets"));
        let applied = crate::cleanup::apply(&node, &plan, 100).unwrap();
        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        assert_eq!(
            std::fs::read_to_string(repo.tree.join("unknown")).unwrap(),
            "leftover content"
        );
        assert_eq!(
            std::fs::read_to_string(repo.repo.join("sandbox.sh")).unwrap(),
            "main content"
        );
        assert!(target.join("CACHEDIR.TAG").exists());
        assert_eq!(
            git(&repo.repo, &["worktree", "list", "--porcelain", "-z"]),
            listed
        );
    }
    // Once the owner removes the directory, nothing is left to remove or to report as dirt.
    std::fs::remove_dir_all(&repo.tree).unwrap();
    let plan = survey(&node, Scope::Prune, 100).unwrap();
    assert!(!plan.entries.iter().any(|entry| crate::worktree::same(
        &entry.id,
        repo.tree.to_str().unwrap()
    ) && (entry.removable
        || entry.kind.as_deref() == Some("landed_dirty"))));
}

#[test]
fn loss_refuses_non_root_paths_before_reading_parent_status() {
    let repo = Repo::new();
    let nested = repo.repo.join("nested");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(repo.repo.join("untracked"), "main checkout content").unwrap();
    for own_git in [false, true] {
        if own_git {
            // A .git directory alone does not establish Git's checkout identity.
            std::fs::create_dir(nested.join(".git")).unwrap();
        }
        let error = crate::loss::dirty_paths(&System, nested.to_str().unwrap(), false)
            .unwrap_err()
            .to_string();
        assert!(error.contains(nested.to_str().unwrap()), "{error}");
        assert!(error.contains("not a worktree root"), "{error}");
    }
}

#[test]
fn loss_refuses_missing_worktree_identity_for_assessment_and_recovery() {
    let repo = Repo::new();
    std::fs::remove_file(repo.tree.join(".git")).unwrap();
    let evidence = repo.evidence();
    let path = repo.tree.to_str().unwrap();
    for error in [
        evidence
            .assess(&System, "refs/heads/feature", Some(path))
            .unwrap_err(),
        evidence.restore_deletions(&System, path).unwrap_err(),
    ] {
        let error = error.to_string();
        assert!(error.contains(path), "{error}");
        assert!(error.contains("not a worktree root"), "{error}");
    }
}

#[test]
fn retained_branch_holds_local_commits_but_never_other_tips_or_dirty_content() {
    let repo = Repo::new();
    repo.commit();
    let evidence = Evidence::keeping_branch(repo.repo.to_str().unwrap(), "feature");
    let tip = git(&repo.tree, &["rev-parse", "HEAD"]);
    let assess = || {
        evidence
            .assess(&System, tip.trim(), Some(repo.tree.to_str().unwrap()))
            .unwrap()
    };
    assert_eq!(assess().held, Some(Held::Branch));
    assert!(assess().loses_nothing());
    std::fs::write(repo.tree.join("untracked"), "keep me").unwrap();
    assert!(!assess().loses_nothing());
    std::fs::remove_file(repo.tree.join("untracked")).unwrap();
    // A retained branch protects only its own ancestors, not a detached commit elsewhere.
    git(&repo.tree, &["checkout", "--detach"]);
    git(
        &repo.tree,
        &["commit", "-q", "--allow-empty", "-m", "detached"],
    );
    std::fs::write(repo.tree.join("work"), "unique detached content").unwrap();
    git(&repo.tree, &["add", "work"]);
    git(&repo.tree, &["commit", "-q", "-m", "unique"]);
    let other = git(&repo.tree, &["rev-parse", "HEAD"]);
    assert!(
        !evidence
            .assess(&System, other.trim(), Some(repo.tree.to_str().unwrap()))
            .unwrap()
            .loses_nothing()
    );
}

/// A killed worktree removal leaves tracked deletions, including unusual path names.
fn deleted_checkout(staged: bool) -> Repo {
    let repo = Repo::new();
    for name in ["work", ":(glob)*", "line\nbreak"] {
        std::fs::write(repo.tree.join(name), "held content\n").unwrap();
    }
    git(&repo.tree, &["add", "."]);
    git(&repo.tree, &["commit", "-q", "-m", "held content"]);
    git(&repo.repo, &["merge", "-q", "feature"]);
    git(&repo.repo, &["push", "-q", "origin", "main"]);
    for name in ["work", ":(glob)*", "line\nbreak"] {
        std::fs::remove_file(repo.tree.join(name)).unwrap();
    }
    if staged {
        git(&repo.tree, &["add", "-u"]);
    }
    repo
}

#[test]
fn held_tracked_deletions_are_lossless_and_cleanup_removes_them_without_force() {
    for staged in [false, true] {
        let repo = deleted_checkout(staged);
        let loss = repo.assess();
        assert!(loss.loses_nothing() && loss.landed());
        let ledger = Ledger::new(repo.root.join("state"));
        ledger
            .write("repositories.json", &vec![repo.repo.display().to_string()])
            .unwrap();
        let cleanup = crate::config::Cleanup::default();
        let node = crate::node::Node {
            shell: &System,
            ledger: &ledger,
            thresholds: crate::config::Thresholds::from(&crate::config::Policy::default()),
            home: repo.root.to_str().unwrap(),
            cleanup: &cleanup,
        };
        let mut plan = survey(&node, Scope::Sweep, 100).unwrap();
        plan.entries.retain(|e| {
            e.class == "worktrees"
                && e.tree.as_ref().is_some_and(|t| {
                    crate::worktree::resolved(Path::new(&t.path))
                        == crate::worktree::resolved(&repo.tree)
                })
        });
        assert_eq!(plan.entries.len(), 1);
        assert!(plan.entries[0].removable);
        let result = crate::cleanup::apply(&node, &plan, 100).unwrap();
        assert!(result.failures.is_empty(), "{:?}", result.failures);
        assert!(!repo.tree.exists());
        assert!(
            !git(&repo.repo, &["worktree", "list", "--porcelain"])
                .contains(repo.tree.to_str().unwrap())
        );
    }
}

#[test]
fn deletions_do_not_hide_modified_added_renamed_or_untracked_content() {
    for kind in ["modified", "added", "renamed", "untracked", "type_changed"] {
        let repo = deleted_checkout(false);
        git(
            &repo.tree,
            &["--literal-pathspecs", "checkout", "HEAD", "--", "work"],
        );
        match kind {
            "modified" => std::fs::write(repo.tree.join("work"), "unique\n").unwrap(),
            "added" => {
                std::fs::write(repo.tree.join("new"), "unique\n").unwrap();
                git(&repo.tree, &["add", "new"]);
            }
            "renamed" => {
                git(&repo.tree, &["mv", "work", "renamed"]);
            }
            "type_changed" => {
                std::fs::remove_file(repo.tree.join("work")).unwrap();
                std::os::unix::fs::symlink("elsewhere", repo.tree.join("work")).unwrap();
            }
            _ => std::fs::write(repo.tree.join("new"), "unique\n").unwrap(),
        }
        let loss = repo.assess();
        assert!(!loss.loses_nothing(), "{kind}: {loss:?}");
        assert_eq!(loss.dirty.len(), 1);
        assert!(
            repo.evidence()
                .restore_deletions(&System, repo.tree.to_str().unwrap())
                .is_err()
        );
        assert!(repo.tree.exists());
        assert!(!repo.tree.join(":(glob)*").exists());
    }
}

#[test]
fn deleted_files_require_a_held_tip_and_refreshed_remote_evidence() {
    let repo = Repo::new();
    repo.commit();
    std::fs::remove_file(repo.tree.join("work")).unwrap();
    let local = repo.assess();
    assert!(!local.loses_nothing());
    assert_eq!(local.dirty, ["work"]);
    assert!(
        repo.evidence()
            .restore_deletions(&System, repo.tree.to_str().unwrap())
            .is_err()
    );
    git(&repo.tree, &["push", "-q", "origin", "feature"]);
    let remote = repo.assess();
    assert!(remote.loses_nothing());
    assert_eq!(remote.held, Some(Held::Remote));
    let stale = Evidence::default_only(repo.repo.to_str().unwrap())
        .assess(
            &System,
            "refs/heads/feature",
            Some(repo.tree.to_str().unwrap()),
        )
        .unwrap();
    assert_eq!(stale.dirty, ["work"]);
    repo.evidence()
        .restore_deletions(&System, repo.tree.to_str().unwrap())
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.tree.join("work")).unwrap(),
        "work\n"
    );
}

#[test]
fn clean_integrated_and_ignored_only_are_lossless_but_dirty_is_not() {
    let repo = Repo::new();
    let clean = repo.assess();
    assert!(clean.loses_nothing() && clean.landed());
    std::fs::write(repo.repo.join(".git/info/exclude"), "ignored\n").unwrap();
    std::fs::write(repo.tree.join("ignored"), "cache").unwrap();
    assert!(repo.assess().loses_nothing());
    std::fs::write(repo.tree.join("unsaved path"), "work").unwrap();
    let dirty = repo.assess();
    assert!(!dirty.loses_nothing() && dirty.landed());
    assert_eq!(dirty.dirty, ["unsaved path"]);
}

#[test]
fn refreshed_remote_counts_but_gone_upstream_and_failed_fetch_do_not() {
    let repo = Repo::new();
    repo.commit();
    git(&repo.tree, &["push", "-q", "-u", "origin", "feature"]);
    assert!(matches!(repo.assess().held, Some(Held::Remote)));
    git(
        &repo.repo,
        &[
            "remote",
            "set-url",
            "origin",
            repo.root.join("missing").to_str().unwrap(),
        ],
    );
    let failed = repo.evidence();
    assert!(
        failed
            .warning
            .as_deref()
            .unwrap()
            .contains(repo.repo.to_str().unwrap())
    );
    assert!(
        !failed
            .assess(&System, "refs/heads/feature", None)
            .unwrap()
            .loses_nothing()
    );
    git(
        &repo.repo,
        &["remote", "set-url", "origin", repo.remote.to_str().unwrap()],
    );
    git(&repo.remote, &["update-ref", "-d", "refs/heads/feature"]);
    assert!(!repo.assess().loses_nothing());
    git(&repo.repo, &["merge", "-q", "--squash", "feature"]);
    git(&repo.repo, &["commit", "-q", "-m", "landed"]);
    // Landing means the remote default branch holds the work.
    git(&repo.repo, &["push", "-q", "origin", "main"]);
    assert!(repo.assess().loses_nothing());
    // Fetch failure still permits default-branch proof.
    git(
        &repo.repo,
        &[
            "remote",
            "set-url",
            "origin",
            repo.root.join("missing").to_str().unwrap(),
        ],
    );
    assert!(repo.assess().loses_nothing());
}

#[derive(Debug)]
struct ShellPanes {
    home: String,
}
impl Shell for ShellPanes {
    fn run(&self, program: &str, args: &[String]) -> Result<Output> {
        let value = if program == "herdr" && args == ["pane", "list"] {
            Some(serde_json::json!({"result":{"panes":[
                {"pane_id":"old-shell","cwd":self.home},
                {"pane_id":"young-shell","cwd":self.home},
                {"pane_id":"focused-shell","cwd":self.home,"focused":true}
            ]}}))
        } else if program == "herdr" && args.starts_with(&["pane".into(), "process-info".into()]) {
            Some(
                serde_json::json!({"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":10,"name":"zsh","argv":["zsh"]}]}}}),
            )
        } else {
            None
        };
        match value {
            Some(value) => Ok(Output {
                status: 0,
                stdout: value.to_string(),
                stderr: String::new(),
            }),
            None => System.run(program, args),
        }
    }
}

#[test]
fn prune_and_sweep_have_identical_idle_pane_candidates_at_the_fifteen_minute_boundary() {
    let repo = Repo::new();
    let ledger = Ledger::new(repo.root.join("state"));
    let now = 2_000_000;
    let snapshot = crate::node::Snapshot {
        sampled_ms: now,
        probe: crate::probe::Probe {
            host: "local".into(),
            label: "local".into(),
            load1: 0.0,
            cores: 1,
            cpu_ticks: None,
            busy: None,
            memory: crate::probe::Memory::default(),
            disk_free_percent: 90.0,
            agents: crate::probe::AgentCounts::default(),
            agent_panes: vec![],
            reservations: vec![],
            panes: [
                ("old-shell", 15),
                ("young-shell", 14),
                ("focused-shell", 30),
            ]
            .into_iter()
            .map(|(pane_id, minutes)| crate::probe::Pane {
                pane_id: pane_id.into(),
                idle_shell: true,
                idle_since_ms: Some(now - minutes * 60_000),
                ..Default::default()
            })
            .collect(),
        },
        jobs: vec![],
    };
    ledger.write("host.json", &snapshot).unwrap();
    let shell = ShellPanes {
        home: repo.root.display().to_string(),
    };
    let cleanup = crate::config::Cleanup::default();
    let node = crate::node::Node {
        shell: &shell,
        ledger: &ledger,
        thresholds: crate::config::Thresholds::from(&crate::config::Policy::default()),
        home: &shell.home,
        cleanup: &cleanup,
    };
    let decisions = |scope| {
        survey(&node, scope, now)
            .unwrap()
            .entries
            .into_iter()
            .filter(|e| matches!(e.class.as_str(), "panes" | "workspaces"))
            .map(|e| (e.id, e.detail))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        decisions(Scope::Prune),
        [("old-shell".into(), "shell idle for 15m".into())]
    );
    assert_eq!(decisions(Scope::Prune), decisions(Scope::Sweep));
}

#[test]
fn survey_continues_past_missing_repositories_and_scopes_agree() {
    let repo = Repo::new();
    let ledger = Ledger::new(repo.root.join("state"));
    let missing = repo.root.join("missing").display().to_string();
    ledger
        .write(
            "repositories.json",
            &vec![repo.repo.display().to_string(), missing.clone()],
        )
        .unwrap();
    git(&repo.repo, &["branch", "unused"]);
    let cleanup = crate::config::Cleanup::default();
    let node = crate::node::Node {
        shell: &System,
        ledger: &ledger,
        thresholds: crate::config::Thresholds::from(&crate::config::Policy::default()),
        home: repo.root.to_str().unwrap(),
        cleanup: &cleanup,
    };
    // The survey also finds the repository of the process's working directory. Under
    // `cargo test` that is the developer's own checkout, so every assertion reads only this
    // fixture's entries, named by its unique temporary root.
    let marker = repo.root.file_name().unwrap().to_str().unwrap().to_owned();
    let fixture = |e: &&crate::cleanup::Entry| {
        e.id.contains(&marker)
            || e.tree
                .as_ref()
                .is_some_and(|t| t.repo.contains(&marker) || t.path.contains(&marker))
    };
    let prune = survey(&node, Scope::Prune, 100).unwrap();
    let sweep = survey(&node, Scope::Sweep, 100).unwrap();
    let decisions = |plan: &crate::cleanup::Plan| {
        plan.entries
            .iter()
            .filter(fixture)
            .filter(|e| matches!(e.class.as_str(), "worktrees" | "branches"))
            .map(|e| (e.id.clone(), e.removable))
            .collect::<Vec<_>>()
    };
    assert_eq!(decisions(&prune), decisions(&sweep));
    assert!(decisions(&prune).iter().any(|(_, removable)| *removable));
    for plan in [&prune, &sweep] {
        assert!(plan.entries.iter().any(|e| e.class == "repositories"
            && e.id == missing
            && e.removable
            && e.detail.contains("repositories.json")));
    }
    std::fs::write(repo.tree.join("dirty"), "work").unwrap();
    let plan = survey(&node, Scope::Prune, 100).unwrap();
    // The survey also covers the caller's own repository, which may have findings of its own.
    let ours = |e: &&crate::cleanup::Entry| {
        e.tree.as_ref().is_some_and(|t| {
            crate::worktree::resolved(Path::new(&t.path)) == crate::worktree::resolved(&repo.tree)
        })
    };
    let finding = plan
        .entries
        .iter()
        .filter(ours)
        .find(|e| e.kind.as_deref() == Some("landed_dirty"))
        .unwrap();
    assert!(!finding.removable);
    assert_eq!(finding.dirty_count, Some(1));
    assert_eq!(finding.dirty_paths, ["dirty"]);
    assert_eq!(
        crate::worktree::resolved(Path::new(&finding.tree.as_ref().unwrap().repo)),
        crate::worktree::resolved(&repo.repo)
    );
    assert!(crate::cleanup::render(&plan).contains("dirty"));
    assert!(
        serde_json::to_string(&plan)
            .unwrap()
            .contains("landed_dirty")
    );
    // A preparation is a hold, not a different loss definition.
    ledger
        .write(
            "preparations.json",
            &vec![crate::cleanup::Tree {
                repo: repo.repo.display().to_string(),
                path: repo.tree.display().to_string(),
                branch: "feature".into(),
                head: String::new(),
                identity: None,
            }],
        )
        .unwrap();
    let held = survey(&node, Scope::Prune, 100).unwrap();
    let entry = held.entries.iter().find(ours).unwrap();
    assert_ne!(entry.kind.as_deref(), Some("landed_dirty"));
    assert!(
        entry.detail.contains("pending preparation"),
        "{}",
        entry.detail
    );
}

#[test]
fn gone_upstream_with_local_work_stays_but_integrated_branch_is_deleted() {
    let repo = Repo::new();
    repo.commit();
    git(&repo.tree, &["push", "-q", "-u", "origin", "feature"]);
    git(&repo.remote, &["update-ref", "-d", "refs/heads/feature"]);
    git(
        &repo.repo,
        &["worktree", "remove", repo.tree.to_str().unwrap()],
    );
    let ledger = Ledger::new(repo.root.join("state"));
    ledger
        .write("repositories.json", &vec![repo.repo.display().to_string()])
        .unwrap();
    let cleanup = crate::config::Cleanup::default();
    let node = crate::node::Node {
        shell: &System,
        ledger: &ledger,
        thresholds: crate::config::Thresholds::from(&crate::config::Policy::default()),
        home: repo.root.to_str().unwrap(),
        cleanup: &cleanup,
    };
    // Only this fixture's entries: the survey also reads the developer's own checkout, and
    // `apply` below must never act on it.
    let marker = repo.root.file_name().unwrap().to_str().unwrap().to_owned();
    let plan = survey(&node, Scope::Prune, 100).unwrap();
    assert!(
        plan.entries
            .iter()
            .any(|e| e.id.contains(&marker) && e.class == "branches" && !e.removable)
    );
    assert!(git(&repo.repo, &["branch", "-vv"]).contains("gone"));
    git(&repo.repo, &["merge", "-q", "--squash", "feature"]);
    git(&repo.repo, &["commit", "-q", "-m", "integrated"]);
    git(&repo.repo, &["push", "-q", "origin", "main"]);
    let mut plan = survey(&node, Scope::Prune, 100).unwrap();
    plan.entries
        .retain(|e| e.id.contains(&marker) && e.class == "branches");
    assert_eq!(plan.entries.len(), 1);
    assert!(plan.entries[0].removable);
    let applied = crate::cleanup::apply(&node, &plan, 100).unwrap();
    assert!(applied.failures.is_empty(), "{:?}", applied.failures);
    assert_eq!(git(&repo.repo, &["branch", "--list", "feature"]), "");
}

/// A pushed, clean worktree that the sweep survey finds removable, and its node.
struct Landed {
    repo: Repo,
    ledger: Ledger,
    cleanup: crate::config::Cleanup,
}

impl Landed {
    fn new() -> Self {
        let repo = Repo::new();
        repo.commit();
        git(&repo.tree, &["push", "-q", "-u", "origin", "feature"]);
        let ledger = Ledger::new(repo.root.join("state"));
        ledger
            .write("repositories.json", &vec![repo.repo.display().to_string()])
            .unwrap();
        Self {
            repo,
            ledger,
            cleanup: crate::config::Cleanup::default(),
        }
    }
    fn node<'a>(&'a self, shell: &'a dyn Shell) -> crate::node::Node<'a> {
        crate::node::Node {
            shell,
            ledger: &self.ledger,
            thresholds: crate::config::Thresholds::from(&crate::config::Policy::default()),
            home: self.repo.root.to_str().unwrap(),
            cleanup: &self.cleanup,
        }
    }
    /// The sweep survey, reduced to this fixture's worktree.
    fn plan(&self, shell: &dyn Shell) -> crate::cleanup::Plan {
        let mut plan = survey(&self.node(shell), Scope::Sweep, 100).unwrap();
        plan.entries.retain(|e| {
            e.class == "worktrees"
                && e.tree.as_ref().is_some_and(|t| {
                    crate::worktree::resolved(Path::new(&t.path))
                        == crate::worktree::resolved(&self.repo.tree)
                })
        });
        assert_eq!(plan.entries.len(), 1);
        plan
    }
}

/// `System`, with one more process whose cwd is `path` where the process table comes from lsof.
#[derive(Debug)]
struct Working {
    pid: u32,
    path: PathBuf,
}

impl Shell for Working {
    fn run(&self, program: &str, args: &[String]) -> Result<Output> {
        if program == "lsof" {
            return Ok(Output {
                status: 0,
                stdout: format!("p1\nn/\np{}\nn{}\n", self.pid, self.path.display()),
                stderr: String::new(),
            });
        }
        System.run(program, args)
    }
}

#[test]
fn landed_worktree_without_live_user_is_removed() {
    let fixture = Landed::new();
    let plan = fixture.plan(&System);
    assert!(plan.entries[0].removable, "{}", plan.entries[0].detail);
    let applied = crate::cleanup::apply_survey(&fixture.node(&System), &plan, 100);
    assert!(applied.failures.is_empty(), "{:?}", applied.failures);
    assert!(applied.skipped.is_empty(), "{:?}", applied.skipped);
    assert!(!fixture.repo.tree.exists());
}

#[test]
fn worktree_dirty_after_the_plan_is_skipped_not_removed_or_failed() {
    let fixture = Landed::new();
    let plan = fixture.plan(&System);
    assert!(plan.entries[0].removable, "{}", plan.entries[0].detail);
    std::fs::write(fixture.repo.tree.join("fresh"), "live work\n").unwrap();
    // The sweep applies its own survey; the removal recheck finds the new dirt.
    let applied = crate::cleanup::apply_survey(&fixture.node(&System), &plan, 100);
    assert!(applied.failures.is_empty(), "{:?}", applied.failures);
    assert_eq!(applied.skipped.len(), 1, "{:?}", applied.skipped);
    assert!(
        applied.skipped[0].starts_with("refuse ")
            && applied.skipped[0].contains("uncommitted changes"),
        "{}",
        applied.skipped[0]
    );
    assert!(applied.results.is_empty(), "{:?}", applied.results);
    assert!(fixture.repo.tree.join("fresh").exists());
    // Prune rebuilds the survey before it applies; that survey refuses the entry too.
    let applied = crate::cleanup::apply(&fixture.node(&System), &plan, 100).unwrap();
    assert!(applied.failures.is_empty(), "{:?}", applied.failures);
    assert_eq!(applied.skipped.len(), 1, "{:?}", applied.skipped);
    assert!(applied.render().contains("skipped: refuse "));
    assert!(fixture.repo.tree.join("fresh").exists());
}

#[test]
fn worktree_held_by_a_process_after_the_plan_is_skipped_not_removed_or_failed() {
    let fixture = Landed::new();
    let plan = fixture.plan(&System);
    assert!(plan.entries[0].removable, "{}", plan.entries[0].detail);
    // A job or a shell command started from another pane works in the checkout.
    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .current_dir(&fixture.repo.tree)
        .spawn()
        .unwrap();
    let shell = Working {
        pid: child.id(),
        path: fixture.repo.tree.clone(),
    };
    let applied = crate::cleanup::apply_survey(&fixture.node(&shell), &plan, 100);
    let _ = child.kill();
    let _ = child.wait();
    assert!(applied.failures.is_empty(), "{:?}", applied.failures);
    assert_eq!(applied.skipped.len(), 1, "{:?}", applied.skipped);
    assert!(
        applied.skipped[0].contains("held: a process works in"),
        "{}",
        applied.skipped[0]
    );
    assert!(fixture.repo.tree.join("work").exists());
}

/// `System`, with one idle shell pane whose cwd is `path`.
#[derive(Debug)]
struct ShellIn {
    path: PathBuf,
}

impl Shell for ShellIn {
    fn run(&self, program: &str, args: &[String]) -> Result<Output> {
        let value = if program == "herdr" && args == ["pane", "list"] {
            Some(serde_json::json!({"result":{"panes":[
                {"pane_id":"placed-shell","cwd":self.path.display().to_string()}
            ]}}))
        } else if program == "herdr" && args.starts_with(&["pane".into(), "process-info".into()]) {
            Some(
                serde_json::json!({"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":10,"name":"zsh","argv":["zsh"]}]}}}),
            )
        } else {
            None
        };
        match value {
            Some(value) => Ok(Output {
                status: 0,
                stdout: value.to_string(),
                stderr: String::new(),
            }),
            None => System.run(program, args),
        }
    }
}

fn shell_idle_for(fixture: &Landed, minutes: u64) {
    let now = crate::ledger::now_ms();
    let snapshot = crate::node::Snapshot {
        sampled_ms: now,
        probe: crate::probe::Probe {
            host: "local".into(),
            label: "local".into(),
            load1: 0.0,
            cores: 1,
            cpu_ticks: None,
            busy: None,
            memory: crate::probe::Memory::default(),
            disk_free_percent: 90.0,
            agents: crate::probe::AgentCounts::default(),
            agent_panes: vec![],
            reservations: vec![],
            panes: vec![crate::probe::Pane {
                pane_id: "placed-shell".into(),
                idle_shell: true,
                idle_since_ms: Some(now - minutes * 60_000),
                ..Default::default()
            }],
        },
        jobs: vec![],
    };
    fixture.ledger.write("host.json", &snapshot).unwrap();
}

#[test]
fn worktree_with_a_young_shell_is_held_and_an_old_shell_releases_it() {
    let fixture = Landed::new();
    let shell = ShellIn {
        path: fixture.repo.tree.clone(),
    };
    // `place` marks its preparation used at once, so only the new shell shows an owner.
    shell_idle_for(&fixture, 1);
    let plan = fixture.plan(&shell);
    assert!(!plan.entries[0].removable, "{}", plan.entries[0].detail);
    assert!(
        plan.entries[0]
            .detail
            .contains("placed-shell is a shell idle for less than"),
        "{}",
        plan.entries[0].detail
    );
    shell_idle_for(&fixture, 16);
    let plan = fixture.plan(&shell);
    assert!(plan.entries[0].removable, "{}", plan.entries[0].detail);
}

#[test]
fn commit_after_the_plan_is_skipped_not_an_identity_change() {
    let fixture = Landed::new();
    let plan = fixture.plan(&System);
    assert!(plan.entries[0].removable, "{}", plan.entries[0].detail);
    git(
        &fixture.repo.tree,
        &["commit", "-q", "--allow-empty", "-m", "moved"],
    );
    let applied = crate::cleanup::apply_survey(&fixture.node(&System), &plan, 100);
    assert!(applied.failures.is_empty(), "{:?}", applied.failures);
    assert_eq!(applied.skipped.len(), 1, "{:?}", applied.skipped);
    assert!(
        applied.skipped[0].starts_with("refuse ")
            && applied.skipped[0].contains("HEAD moved since the survey ("),
        "{}",
        applied.skipped[0]
    );
    assert!(applied.results.is_empty(), "{:?}", applied.results);
    assert!(fixture.repo.tree.exists());
}

#[test]
fn prune_commit_after_the_plan_is_skipped_not_an_identity_change() {
    let fixture = Landed::new();
    let plan = fixture.plan(&System);
    assert!(plan.entries[0].removable, "{}", plan.entries[0].detail);
    git(
        &fixture.repo.tree,
        &["commit", "-q", "--allow-empty", "-m", "moved"],
    );
    // Prune rebuilds the survey, so the fresh entry carries the new head: `apply_one` compares it.
    let applied = crate::cleanup::apply(&fixture.node(&System), &plan, 100).unwrap();
    assert!(applied.failures.is_empty(), "{:?}", applied.failures);
    assert_eq!(applied.skipped.len(), 1, "{:?}", applied.skipped);
    assert!(
        applied.skipped[0].starts_with("refuse ")
            && applied.skipped[0].contains("HEAD moved since the survey ("),
        "{}",
        applied.skipped[0]
    );
    assert!(applied.results.is_empty(), "{:?}", applied.results);
    assert!(fixture.repo.tree.exists());
}

/// The sweep plan for the fixture's branch once its worktree is gone: a removable branch entry.
fn branch_plan(fixture: &Landed) -> crate::cleanup::Plan {
    git(
        &fixture.repo.repo,
        &["worktree", "remove", fixture.repo.tree.to_str().unwrap()],
    );
    let mut plan = survey(&fixture.node(&System), Scope::Sweep, 100).unwrap();
    let marker = fixture.repo.root.file_name().unwrap().to_str().unwrap();
    plan.entries
        .retain(|e| e.class == "branches" && e.id.contains(marker));
    assert_eq!(plan.entries.len(), 1);
    assert!(plan.entries[0].removable, "{}", plan.entries[0].detail);
    plan
}

#[test]
fn sweep_branch_unchanged_after_the_plan_is_deleted() {
    let fixture = Landed::new();
    let plan = branch_plan(&fixture);
    let applied = crate::cleanup::apply_survey(&fixture.node(&System), &plan, 100);
    assert!(applied.failures.is_empty(), "{:?}", applied.failures);
    assert!(applied.skipped.is_empty(), "{:?}", applied.skipped);
    assert_eq!(
        git(&fixture.repo.repo, &["branch", "--list", "feature"]),
        ""
    );
}

#[test]
fn sweep_branch_moved_after_the_plan_is_skipped_and_kept() {
    let fixture = Landed::new();
    let plan = branch_plan(&fixture);
    let planned = plan.entries[0].tree.as_ref().unwrap().head.clone();
    // The owner commits on the branch between the plan and the apply.
    git(&fixture.repo.repo, &["switch", "-q", "feature"]);
    git(
        &fixture.repo.repo,
        &["commit", "-q", "--allow-empty", "-m", "moved"],
    );
    git(&fixture.repo.repo, &["switch", "-q", "main"]);
    let moved = git(&fixture.repo.repo, &["rev-parse", "feature"]);
    assert_ne!(moved.trim(), planned);
    let applied = crate::cleanup::apply_survey(&fixture.node(&System), &plan, 100);
    assert!(applied.failures.is_empty(), "{:?}", applied.failures);
    assert!(applied.results.is_empty(), "{:?}", applied.results);
    assert_eq!(applied.skipped.len(), 1, "{:?}", applied.skipped);
    assert!(
        applied.skipped[0].contains("HEAD moved since the survey ("),
        "{}",
        applied.skipped[0]
    );
    // Never deleted, and still at the owner's commit.
    assert_eq!(git(&fixture.repo.repo, &["rev-parse", "feature"]), moved);
}

#[test]
fn prune_branch_switch_after_the_plan_still_fails() {
    let fixture = Landed::new();
    let plan = fixture.plan(&System);
    assert!(plan.entries[0].removable, "{}", plan.entries[0].detail);
    git(&fixture.repo.tree, &["switch", "-q", "-c", "other"]);
    let applied = crate::cleanup::apply(&fixture.node(&System), &plan, 100).unwrap();
    assert!(applied.skipped.is_empty(), "{:?}", applied.skipped);
    assert_eq!(applied.failures.len(), 1, "{:?}", applied.failures);
    assert!(
        applied.failures[0].contains("planned worktree identity changed"),
        "{}",
        applied.failures[0]
    );
    assert!(applied.results.is_empty(), "{:?}", applied.results);
    assert!(fixture.repo.tree.exists());
}

#[test]
fn worktree_identity_change_after_the_plan_still_fails() {
    let fixture = Landed::new();
    let plan = fixture.plan(&System);
    assert!(plan.entries[0].removable, "{}", plan.entries[0].detail);
    // The same path now holds another branch.
    git(&fixture.repo.tree, &["switch", "-q", "-c", "other"]);
    let applied = crate::cleanup::apply_survey(&fixture.node(&System), &plan, 100);
    assert!(applied.skipped.is_empty(), "{:?}", applied.skipped);
    assert_eq!(applied.failures.len(), 1, "{:?}", applied.failures);
    assert!(
        applied.failures[0].contains("worktree identity changed"),
        "{}",
        applied.failures[0]
    );
    assert!(fixture.repo.tree.exists());
}

#[test]
fn worktree_whose_git_file_is_gone_is_held_and_removal_refuses_before_unlocking() {
    let repo = Repo::new();
    let path = repo.tree.display().to_string();
    git(
        &repo.repo,
        &["worktree", "lock", "--reason", "agent:worktree", &path],
    );
    std::fs::write(repo.tree.join("generated"), "output\n").unwrap();
    std::fs::remove_file(repo.tree.join(".git")).unwrap();
    let ledger = Ledger::new(repo.root.join("state"));
    ledger
        .write("repositories.json", &vec![repo.repo.display().to_string()])
        .unwrap();
    let cleanup = crate::config::Cleanup::default();
    let node = crate::node::Node {
        shell: &System,
        ledger: &ledger,
        thresholds: crate::config::Thresholds::from(&crate::config::Policy::default()),
        home: repo.root.to_str().unwrap(),
        cleanup: &cleanup,
    };
    let held = |scope| {
        survey(&node, scope, 100)
            .unwrap()
            .entries
            .into_iter()
            .find(|e| e.class == "worktrees" && crate::worktree::same(&e.id, &path))
            .unwrap()
    };
    // Locked, so Git still lists it and never calls it prunable; `git worktree remove` would
    // still fail on the missing `.git` file. Both scopes hold it.
    for scope in [Scope::Sweep, Scope::Prune] {
        let entry = held(scope);
        assert_eq!(entry.kind.as_deref(), Some(crate::cleanup::UNLINKED));
        assert!(!entry.removable);
        assert!(
            entry
                .detail
                .contains("worktree has no .git file; git cannot remove it; directory remains"),
            "{}",
            entry.detail
        );
    }
    let linked = crate::worktree::Linked {
        checkout: repo.repo.clone(),
        path: path.clone(),
        branch: "feature".into(),
        head: String::new(),
        gitdir: PathBuf::new(),
        identity: None,
    };
    let refused = crate::worktree::with_lifecycle(&System, &ledger, &repo.repo, |guard| {
        crate::worktree::remove(
            &System,
            &ledger,
            &linked,
            crate::worktree::Removal {
                guard,
                workspace: None,
                holders: &[],
                caller_workspace: None,
                evidence: &repo.evidence(),
            },
        )
    })
    .unwrap_err()
    .to_string();
    assert!(refused.contains("directory remains"), "{refused}");
    assert!(refused.contains("nothing was closed"), "{refused}");
    assert!(
        git(&repo.repo, &["worktree", "list", "--porcelain"]).contains("locked agent:worktree")
    );
    assert!(repo.tree.join("generated").is_file());

    // Unlocked, Git calls it prunable; the hold names that instead.
    git(&repo.repo, &["worktree", "unlock", &path]);
    let entry = held(Scope::Sweep);
    assert!(
        entry.detail.contains("git marks the worktree prunable"),
        "{}",
        entry.detail
    );
    assert!(!entry.removable);
    let mut plan = survey(&node, Scope::Prune, 100).unwrap();
    plan.entries
        .retain(|e| e.class == "worktrees" && crate::worktree::same(&e.id, &path));
    let applied = crate::cleanup::apply(&node, &plan, 100).unwrap();
    assert!(applied.failures.is_empty(), "{:?}", applied.failures);
    assert!(
        applied.results[0].ends_with(": kept"),
        "{:?}",
        applied.results
    );
    assert!(repo.tree.join("generated").is_file());
}

/// Runs `race` once, just before the first `git status` inside `tree`: another actor changes the
/// worktree after this pass listed it.
struct Racing {
    tree: PathBuf,
    race: Box<dyn Fn() + Send + Sync>,
    fired: std::sync::atomic::AtomicBool,
}
impl std::fmt::Debug for Racing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Racing")
            .field("tree", &self.tree)
            .field("fired", &self.fired)
            .finish_non_exhaustive()
    }
}
impl Racing {
    fn new(tree: &Path, race: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            tree: tree.to_path_buf(),
            race: Box::new(race),
            fired: std::sync::atomic::AtomicBool::new(false),
        }
    }
}
impl Shell for Racing {
    fn run(&self, program: &str, args: &[String]) -> Result<Output> {
        if program == "git"
            && args.get(2).is_some_and(|a| a == "status")
            && args
                .get(1)
                .is_some_and(|dir| crate::worktree::same(dir, self.tree.to_str().unwrap()))
            && !self.fired.swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            (self.race)();
        }
        System.run(program, args)
    }
}

/// The fixture's registered worktree, the survey of this fixture only, and its entry for it.
fn registered(repo: &Repo, ledger: &Ledger) -> crate::cleanup::Tree {
    let tree = crate::worktree::with_lifecycle(&System, ledger, &repo.repo, |guard| {
        Ok(guard
            .discover(&System, repo.tree.to_str().unwrap())?
            .unwrap()
            .tree())
    })
    .unwrap();
    ledger
        .write("repositories.json", &vec![tree.repo.clone()])
        .unwrap();
    crate::cleanup::remember(ledger, tree.clone()).unwrap();
    tree
}

fn fixture_plan(node: &crate::node::Node<'_>, scope: Scope, repo: &Repo) -> crate::cleanup::Plan {
    let mut plan = survey(node, scope, 100).unwrap();
    plan.entries.retain(|e| {
        e.class == "worktrees" && crate::worktree::same(&e.id, repo.tree.to_str().unwrap())
    });
    plan
}

fn recorded(ledger: &Ledger) -> Vec<crate::cleanup::Tree> {
    ledger
        .read::<Vec<crate::cleanup::Tree>>("worktrees.json")
        .unwrap()
        .unwrap_or_default()
}

#[test]
fn worktree_removed_after_listing_is_reconciled_not_failed() {
    let repo = Repo::new();
    let ledger = Ledger::new(repo.root.join("state"));
    let tree = registered(&repo, &ledger);
    let cleanup = crate::config::Cleanup::default();
    let (main, path) = (repo.repo.clone(), repo.tree.clone());
    let remove = move || {
        git(&main, &["worktree", "remove", path.to_str().unwrap()]);
    };

    // During the survey: listed, then removed before its dirty check.
    let racing = Racing::new(&repo.tree, remove.clone());
    let node = crate::node::Node {
        shell: &racing,
        ledger: &ledger,
        thresholds: crate::config::Thresholds::from(&crate::config::Policy::default()),
        home: repo.root.to_str().unwrap(),
        cleanup: &cleanup,
    };
    for scope in [Scope::Sweep, Scope::Prune] {
        if scope == Scope::Prune {
            git(
                &repo.repo,
                &[
                    "worktree",
                    "add",
                    "-q",
                    repo.tree.to_str().unwrap(),
                    "feature",
                ],
            );
            // This is a fresh checkout, not the incarnation saved by the previous survey.
            registered(&repo, &ledger);
            racing
                .fired
                .store(false, std::sync::atomic::Ordering::SeqCst);
        }
        let plan = fixture_plan(&node, scope, &repo);
        assert!(racing.fired.load(std::sync::atomic::Ordering::SeqCst));
        let [entry] = plan.entries.as_slice() else {
            panic!("{:?}", plan.entries);
        };
        assert!(entry.removable, "{}", entry.detail);
        assert!(entry.tree.is_none(), "{entry:?}");
        assert!(
            entry.detail.contains("removed during the survey"),
            "{}",
            entry.detail
        );
    }
    let plan = fixture_plan(&node, Scope::Sweep, &repo);
    let applied = crate::cleanup::apply_survey(&node, &plan, 100);
    assert!(applied.failures.is_empty(), "{:?}", applied.failures);
    assert!(
        applied.results[0].contains("dropped"),
        "{:?}",
        applied.results
    );
    assert_eq!(recorded(&ledger), [] as [crate::cleanup::Tree; 0]);

    // During the apply-time recheck: listed again, then removed before its dirty check.
    git(
        &repo.repo,
        &[
            "worktree",
            "add",
            "-q",
            repo.tree.to_str().unwrap(),
            "feature",
        ],
    );
    let _ = tree;
    registered(&repo, &ledger);
    let node = crate::node::Node {
        shell: &System,
        ..node
    };
    let plan = fixture_plan(&node, Scope::Sweep, &repo);
    assert!(
        plan.entries[0].removable && plan.entries[0].tree.is_some(),
        "{:?}",
        plan.entries
    );
    let racing = Racing::new(&repo.tree, remove);
    let node = crate::node::Node {
        shell: &racing,
        ..node
    };
    let applied = crate::cleanup::apply_survey(&node, &plan, 100);
    assert!(racing.fired.load(std::sync::atomic::Ordering::SeqCst));
    assert!(applied.failures.is_empty(), "{:?}", applied.failures);
    assert!(
        applied.results[0].contains("already removed"),
        "{:?}",
        applied.results
    );
    assert_eq!(recorded(&ledger), [] as [crate::cleanup::Tree; 0]);
    assert!(repo.repo.is_dir());
}

#[test]
fn worktree_git_lost_after_listing_with_its_directory_is_still_refused() {
    let repo = Repo::new();
    let ledger = Ledger::new(repo.root.join("state"));
    let tree = registered(&repo, &ledger);
    let cleanup = crate::config::Cleanup::default();
    std::fs::write(repo.tree.join("generated"), "content").unwrap();
    git(&repo.tree, &["add", "generated"]);
    git(&repo.tree, &["commit", "-q", "-m", "generated"]);
    git(&repo.repo, &["merge", "-q", "feature"]);
    let admin = repo.repo.join(".git/worktrees/tree");
    // Git forgets it (its admin directory goes) while the directory and its content stay.
    let racing = Racing::new(&repo.tree, move || {
        std::fs::remove_dir_all(&admin).unwrap();
    });
    let node = crate::node::Node {
        shell: &racing,
        ledger: &ledger,
        thresholds: crate::config::Thresholds::from(&crate::config::Policy::default()),
        home: repo.root.to_str().unwrap(),
        cleanup: &cleanup,
    };
    let plan = fixture_plan(&node, Scope::Sweep, &repo);
    assert!(racing.fired.load(std::sync::atomic::Ordering::SeqCst));
    let [entry] = plan.entries.as_slice() else {
        panic!("{:?}", plan.entries);
    };
    assert!(!entry.removable, "{}", entry.detail);
    assert!(
        entry.detail.contains("cannot check worktree"),
        "{}",
        entry.detail
    );
    let applied = crate::cleanup::apply_survey(&node, &plan, 100);
    assert!(applied.results.is_empty(), "{:?}", applied.results);
    assert!(repo.tree.join("generated").is_file());
    assert_eq!(recorded(&ledger), [tree]);
}

/// A real checkout with a stateful pane and controllable teardown.
struct LifecycleShell {
    path: PathBuf,
    cwd: std::sync::Arc<std::sync::Mutex<PathBuf>>,
    open: std::sync::atomic::AtomicBool,
    teardowns: std::sync::atomic::AtomicUsize,
    hook: Box<dyn Fn() + Send + Sync>,
    lock_attempt: Option<std::sync::mpsc::Sender<()>>,
    fail_listing: bool,
    terminal: String,
}

impl std::fmt::Debug for LifecycleShell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LifecycleShell")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl LifecycleShell {
    fn new(path: &Path, hook: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            path: path.into(),
            cwd: std::sync::Arc::new(std::sync::Mutex::new(path.into())),
            open: std::sync::atomic::AtomicBool::new(true),
            teardowns: std::sync::atomic::AtomicUsize::new(0),
            hook: Box::new(hook),
            lock_attempt: None,
            fail_listing: false,
            terminal: "c1-original".into(),
        }
    }
}

impl Shell for LifecycleShell {
    fn run(&self, program: &str, args: &[String]) -> Result<Output> {
        use std::sync::atomic::Ordering::SeqCst;
        if program == "git"
            && args.last().is_some_and(|a| a == "--git-common-dir")
            && std::thread::current().name() == Some("lifecycle-contender")
            && let Some(sender) = &self.lock_attempt
        {
            let _ = sender.send(());
        }
        if self.fail_listing && program == "git" && args.iter().any(|a| a == "--porcelain") {
            return Err(crate::error::Error::Invalid("listing unavailable".into()));
        }
        let value = if program == "herdr" && args == ["pane", "list"] {
            Some(if self.open.load(SeqCst) {
                serde_json::json!({"result":{"panes":[{"pane_id":"placed-shell","cwd":*self.cwd.lock().unwrap(),"terminal_id":self.terminal}]}})
            } else {
                serde_json::json!({"result":{"panes":[]}})
            })
        } else if program == "herdr" && args.starts_with(&["pane".into(), "close".into()]) {
            self.open.store(false, SeqCst);
            Some(serde_json::json!({}))
        } else if program == "herdr" && args.starts_with(&["pane".into(), "process-info".into()]) {
            Some(
                serde_json::json!({"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":10,"name":"zsh","argv":["zsh"]}]}}}),
            )
        } else if program == "sh" && args.last().is_some_and(|a| a.ends_with("c1-teardown")) {
            self.teardowns.fetch_add(1, SeqCst);
            (self.hook)();
            Some(serde_json::json!({}))
        } else {
            None
        };
        match value {
            Some(value) => Ok(Output {
                status: 0,
                stdout: value.to_string(),
                stderr: String::new(),
            }),
            None => System.run(program, args),
        }
    }
}

fn lifecycle_fixture(detached: bool) -> Landed {
    let mut fixture = Landed::new();
    let path = fixture.repo.repo.join(".worktrees/feature");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    git(
        &fixture.repo.repo,
        &[
            "worktree",
            "move",
            fixture.repo.tree.to_str().unwrap(),
            path.to_str().unwrap(),
        ],
    );
    fixture.repo.tree = path;
    if detached {
        git(&fixture.repo.tree, &["checkout", "-q", "--detach"]);
    }
    git(
        &fixture.repo.repo,
        &[
            "worktree",
            "lock",
            "--reason",
            "agent:worktree",
            fixture.repo.tree.to_str().unwrap(),
        ],
    );
    std::fs::write(
        fixture.repo.repo.join("dogsled.toml"),
        "[worktree]\nteardown = ['c1-teardown']\n",
    )
    .unwrap();
    registered(&fixture.repo, &fixture.ledger);
    shell_idle_for(&fixture, 16);
    fixture
}

fn external_remove(repo: &Repo) {
    git(
        &repo.repo,
        &[
            "worktree",
            "remove",
            "-f",
            "-f",
            repo.tree.to_str().unwrap(),
        ],
    );
}

fn close_preserves_head_moved_while_waiting(saved_identity: bool) {
    use std::sync::atomic::Ordering::SeqCst;
    let fixture = lifecycle_fixture(false);
    let captured = recorded(&fixture.ledger)[0].clone();
    if saved_identity {
        fixture.ledger.child("launches").write("c1-old.progress.json", &serde_json::json!({
            "request":{"id":"c1-old","action":{"type":"agent","kind":"pi","name":"worker","message":"go"},"host":"local","cwd":fixture.repo.tree,"pane":"placed-shell","terminal":"c1-original","workspace":null,"workspace_name":null,"owner":"parent"},
            "pane":"placed-shell","terminal":"c1-original","attempted":true,"confirmed":true,
            "result":{"checkout":captured},"error":null
        })).unwrap();
        fixture
            .ledger
            .child("launches")
            .write("c1-old.json", &serde_json::json!({"checkout": captured}))
            .unwrap();
        git(
            &fixture.repo.tree,
            &["commit", "-q", "--allow-empty", "-m", "work before close"],
        );
        git(&fixture.repo.tree, &["push", "-q", "origin", "feature"]);
    } else {
        std::fs::remove_file(
            captured
                .identity
                .as_ref()
                .unwrap()
                .gitdir
                .join("dogsled-checkout-id"),
        )
        .unwrap();
        fixture
            .ledger
            .write("worktrees.json", &Vec::<crate::cleanup::Tree>::new())
            .unwrap();
    }
    let request_head = git(&fixture.repo.tree, &["rev-parse", "HEAD"]);
    let (tx, rx) = std::sync::mpsc::channel();
    let mut shell = LifecycleShell::new(&fixture.repo.tree, || {});
    shell.lock_attempt = Some(tx);
    std::thread::scope(|scope| {
        let close =
            crate::worktree::with_lifecycle(&shell, &fixture.ledger, &fixture.repo.repo, |_| {
                let close = std::thread::Builder::new()
                    .name("lifecycle-contender".into())
                    .spawn_scoped(scope, || {
                        crate::close::local(
                            &shell,
                            &fixture.ledger,
                            "placed-shell",
                            &crate::close::Caller::default(),
                            crate::close::Guard::Clean,
                        )
                    })
                    .unwrap();
                // The contender has captured its plan and reached repository mutex lookup.
                rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
                git(
                    &fixture.repo.tree,
                    &["commit", "-q", "--allow-empty", "-m", "concurrent work"],
                );
                // Keep the new HEAD removable by the loss rule: only plan drift must protect it.
                git(&fixture.repo.tree, &["push", "-q", "origin", "feature"]);
                Ok(close)
            })
            .unwrap();
        let result = close.join().unwrap();
        assert!(result.is_err(), "{result:?}");
        let error = result.unwrap_err().to_string();
        assert!(error.contains("HEAD moved"), "{error}");
    });
    assert!(fixture.repo.tree.is_dir());
    let head = git(&fixture.repo.tree, &["rev-parse", "HEAD"]);
    assert_ne!(head.trim(), request_head.trim());
    assert_eq!(
        git(&fixture.repo.tree, &["log", "-1", "--format=%s"]).trim(),
        "concurrent work"
    );
    assert!(shell.open.load(SeqCst));
    assert_eq!(shell.teardowns.load(SeqCst), 0);
}

fn save_close_checkout(fixture: &Landed) {
    let captured = recorded(&fixture.ledger)[0].clone();
    fixture.ledger.child("launches").write("c1-old.progress.json", &serde_json::json!({
        "request":{"id":"c1-old","action":{"type":"agent","kind":"pi","name":"worker","message":"go"},"host":"local","cwd":fixture.repo.tree,"pane":"placed-shell","terminal":"c1-original","workspace":null,"workspace_name":null,"owner":"parent"},
        "pane":"placed-shell","terminal":"c1-original","attempted":true,"confirmed":true,
        "result":{"checkout":captured},"error":null
    })).unwrap();
    fixture
        .ledger
        .child("launches")
        .write("c1-old.json", &serde_json::json!({"checkout": captured}))
        .unwrap();
}

fn another_checkout(fixture: &Landed) -> PathBuf {
    let path = fixture.repo.repo.join(".worktrees/another");
    git(
        &fixture.repo.repo,
        &[
            "worktree",
            "add",
            "-q",
            "--lock",
            "--reason",
            "agent:worktree",
            "-b",
            "another",
            path.to_str().unwrap(),
            "main",
        ],
    );
    crate::worktree::with_lifecycle(&System, &fixture.ledger, &fixture.repo.repo, |guard| {
        guard.discover(&System, path.to_str().unwrap())
    })
    .unwrap();
    path
}

fn assert_moved_close_preserved(
    fixture: &Landed,
    shell: &LifecycleShell,
    other: &Path,
    captured: &crate::cleanup::Tree,
    result: Result<crate::close::Closed>,
) {
    use std::sync::atomic::Ordering::SeqCst;
    let error = result.unwrap_err().to_string();
    assert!(error.contains("pane moved to another checkout"), "{error}");
    assert!(fixture.repo.tree.is_dir(), "saved checkout was removed");
    assert!(other.is_dir(), "pane's current checkout was removed");
    assert!(shell.open.load(SeqCst));
    assert_eq!(recorded(&fixture.ledger), std::slice::from_ref(captured));
}

#[test]
fn saved_close_refuses_pane_in_another_checkout() {
    let fixture = lifecycle_fixture(false);
    let captured = recorded(&fixture.ledger)[0].clone();
    save_close_checkout(&fixture);
    let other = another_checkout(&fixture);
    let shell = LifecycleShell::new(&other, || {});
    let result = crate::close::local(
        &shell,
        &fixture.ledger,
        "placed-shell",
        &crate::close::Caller::default(),
        crate::close::Guard::Clean,
    );
    assert_moved_close_preserved(&fixture, &shell, &other, &captured, result);
    assert_eq!(shell.teardowns.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn close_refuses_pane_moved_while_waiting() {
    use std::sync::Barrier;
    for saved in [false, true] {
        let fixture = lifecycle_fixture(false);
        let captured = recorded(&fixture.ledger)[0].clone();
        if saved {
            save_close_checkout(&fixture);
        }
        let other = another_checkout(&fixture);
        let (tx, rx) = std::sync::mpsc::channel();
        let mut shell = LifecycleShell::new(&fixture.repo.tree, || {});
        shell.lock_attempt = Some(tx);
        let barrier = Barrier::new(2);
        std::thread::scope(|scope| {
            let close = crate::worktree::with_lifecycle(
                &shell,
                &fixture.ledger,
                &fixture.repo.repo,
                |_| {
                    let close = std::thread::Builder::new()
                        .name("lifecycle-contender".into())
                        .spawn_scoped(scope, || {
                            barrier.wait();
                            crate::close::local(
                                &shell,
                                &fixture.ledger,
                                "placed-shell",
                                &crate::close::Caller::default(),
                                crate::close::Guard::Clean,
                            )
                        })
                        .unwrap();
                    barrier.wait();
                    // Close has captured A and reached the held repository mutex.
                    rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
                    *shell.cwd.lock().unwrap() = other.clone();
                    Ok(close)
                },
            )
            .unwrap();
            let result = close.join().unwrap();
            assert_moved_close_preserved(&fixture, &shell, &other, &captured, result);
        });
        assert_eq!(shell.teardowns.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}

#[test]
fn close_refuses_pane_moved_during_locked_teardown() {
    use std::sync::{Arc, Barrier, atomic::Ordering::SeqCst};
    let fixture = lifecycle_fixture(false);
    let captured = recorded(&fixture.ledger)[0].clone();
    save_close_checkout(&fixture);
    let other = another_checkout(&fixture);
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let (ready, finish) = (entered.clone(), release.clone());
    let shell = LifecycleShell::new(&fixture.repo.tree, move || {
        ready.wait();
        finish.wait();
    });
    std::thread::scope(|scope| {
        let close = scope.spawn(|| {
            crate::close::local(
                &shell,
                &fixture.ledger,
                "placed-shell",
                &crate::close::Caller::default(),
                crate::close::Guard::Clean,
            )
        });
        entered.wait();
        assert!(
            Ledger::new(fixture.repo.repo.join(".git"))
                .locked("dogsled-lifecycle")
                .unwrap()
        );
        *shell.cwd.lock().unwrap() = other.clone();
        release.wait();
        assert_moved_close_preserved(&fixture, &shell, &other, &captured, close.join().unwrap());
    });
    assert_eq!(shell.teardowns.load(SeqCst), 1);
}

#[test]
fn saved_close_preserves_head_moved_while_waiting() {
    close_preserves_head_moved_while_waiting(true);
}

#[test]
fn discovered_close_preserves_head_moved_while_waiting() {
    close_preserves_head_moved_while_waiting(false);
}

fn unmark_checkout(fixture: &Landed) -> PathBuf {
    let marker = recorded(&fixture.ledger)[0]
        .identity
        .as_ref()
        .unwrap()
        .gitdir
        .join("dogsled-checkout-id");
    std::fs::remove_file(&marker).unwrap();
    fixture
        .ledger
        .write("worktrees.json", &Vec::<crate::cleanup::Tree>::new())
        .unwrap();
    marker
}

#[test]
fn unmarked_close_preserves_replacement_while_waiting() {
    use std::sync::{Barrier, atomic::Ordering::SeqCst};
    for marked_replacement in [false, true] {
        let fixture = lifecycle_fixture(false);
        let marker = unmark_checkout(&fixture);
        let original =
            crate::worktree::linked_or_detached(&System, fixture.repo.tree.to_str().unwrap())
                .unwrap()
                .unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut shell = LifecycleShell::new(&fixture.repo.tree, || {});
        shell.lock_attempt = Some(tx);
        let barrier = Barrier::new(2);
        std::thread::scope(|scope| {
            let close = crate::worktree::with_lifecycle(
                &shell,
                &fixture.ledger,
                &fixture.repo.repo,
                |_| {
                    let close = std::thread::Builder::new()
                        .name("lifecycle-contender".into())
                        .spawn_scoped(scope, || {
                            barrier.wait();
                            crate::close::local(
                                &shell,
                                &fixture.ledger,
                                "placed-shell",
                                &crate::close::Caller::default(),
                                crate::close::Guard::Clean,
                            )
                        })
                        .unwrap();
                    barrier.wait();
                    rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
                    // Real Git removes/recreates the checkout in another process while
                    // the contender waits; the administrative directory name is reused.
                    let replacement =
                        replace_checkout(&fixture.repo.repo, &fixture.repo.tree, false);
                    let identity = replacement.identity.as_ref().unwrap();
                    assert_eq!(identity.gitdir, original.gitdir);
                    assert_eq!(replacement.head, original.head);
                    assert_eq!(replacement.branch, original.branch);
                    assert_eq!(replacement.path, original.path);
                    if !marked_replacement {
                        std::fs::remove_file(&marker).unwrap();
                    }
                    Ok(close)
                },
            )
            .unwrap();
            let result = close.join().unwrap();
            assert!(result.is_err(), "{result:?}");
        });
        assert!(fixture.repo.tree.is_dir());
        assert!(shell.open.load(SeqCst));
        assert_eq!(shell.teardowns.load(SeqCst), 0);
        assert_eq!(marker.exists(), marked_replacement);
        assert_eq!(recorded(&fixture.ledger), []);
    }
}

#[test]
fn saved_close_accepts_work_committed_before_the_request() {
    use std::sync::atomic::Ordering::SeqCst;
    let fixture = lifecycle_fixture(false);
    save_close_checkout(&fixture);
    let captured = recorded(&fixture.ledger)[0].clone();
    git(
        &fixture.repo.tree,
        &["commit", "-q", "--allow-empty", "-m", "finished work"],
    );
    git(&fixture.repo.tree, &["push", "-q", "origin", "feature"]);
    let finished_head = git(&fixture.repo.tree, &["rev-parse", "HEAD"]);
    assert_ne!(captured.head, finished_head.trim());
    let shell = LifecycleShell::new(&fixture.repo.tree, || {});
    let closed = crate::close::local(
        &shell,
        &fixture.ledger,
        "placed-shell",
        &crate::close::Caller::default(),
        crate::close::Guard::Clean,
    )
    .unwrap();
    assert!(closed.worktree.is_some());
    assert!(!fixture.repo.tree.exists());
    assert!(!shell.open.load(SeqCst));
    assert_eq!(shell.teardowns.load(SeqCst), 1);
    assert_eq!(recorded(&fixture.ledger), []);
}

#[test]
fn unmarked_subdir_cwd_cannot_adopt_a_replaced_checkout() {
    use std::os::unix::fs::MetadataExt as _;
    use std::sync::{Barrier, atomic::Ordering::SeqCst};
    let fixture = lifecycle_fixture(false);
    let marker = unmark_checkout(&fixture);
    let subdir = fixture.repo.tree.join("subdir");
    std::fs::create_dir(&subdir).unwrap();
    let inode = std::fs::metadata(&subdir).unwrap().ino();
    let held = fixture.repo.root.join("held-subdir");
    let (tx, rx) = std::sync::mpsc::channel();
    let mut shell = LifecycleShell::new(&subdir, || {});
    shell.lock_attempt = Some(tx);
    let barrier = Barrier::new(2);
    std::thread::scope(|scope| {
        let close =
            crate::worktree::with_lifecycle(&shell, &fixture.ledger, &fixture.repo.repo, |_| {
                let close = std::thread::Builder::new()
                    .name("lifecycle-contender".into())
                    .spawn_scoped(scope, || {
                        barrier.wait();
                        crate::close::local(
                            &shell,
                            &fixture.ledger,
                            "placed-shell",
                            &crate::close::Caller::default(),
                            crate::close::Guard::Clean,
                        )
                    })
                    .unwrap();
                barrier.wait();
                rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
                // Keep the cwd inode but replace both the checkout root and its admin dir.
                std::fs::rename(&subdir, &held).unwrap();
                replace_checkout(&fixture.repo.repo, &fixture.repo.tree, false);
                std::fs::remove_file(&marker).unwrap();
                std::fs::rename(&held, &subdir).unwrap();
                Ok(close)
            })
            .unwrap();
        let error = close.join().unwrap().unwrap_err().to_string();
        assert!(error.contains("worktree identity changed"), "{error}");
    });
    assert_eq!(std::fs::metadata(&subdir).unwrap().ino(), inode);
    assert!(fixture.repo.tree.is_dir());
    assert!(!marker.exists(), "replacement acquired a generation marker");
    assert!(shell.open.load(SeqCst));
    assert_eq!(shell.teardowns.load(SeqCst), 0);
    assert_eq!(recorded(&fixture.ledger), []);
}

#[test]
fn unmarked_close_establishes_identity_under_the_same_lock() {
    use std::sync::atomic::Ordering::SeqCst;
    let fixture = lifecycle_fixture(false);
    let marker = unmark_checkout(&fixture);
    let lock = Ledger::new(fixture.repo.repo.join(".git"));
    let shell = LifecycleShell::new(&fixture.repo.tree, move || {
        assert!(lock.locked("dogsled-lifecycle").unwrap());
        let generation = std::fs::read_to_string(&marker).unwrap();
        assert_eq!(generation.trim().len(), 32);
    });
    crate::close::local(
        &shell,
        &fixture.ledger,
        "placed-shell",
        &crate::close::Caller::default(),
        crate::close::Guard::Clean,
    )
    .unwrap();
    assert!(!fixture.repo.tree.exists());
    assert!(!shell.open.load(SeqCst));
    assert_eq!(shell.teardowns.load(SeqCst), 1);
    assert_eq!(recorded(&fixture.ledger), []);
}

#[test]
fn concurrent_close_and_prune_run_one_teardown() {
    use std::sync::{Arc, Barrier, atomic::Ordering::SeqCst};
    for detached in [false, true] {
        for close_wins in [false, true] {
            let fixture = lifecycle_fixture(detached);
            let entered = Arc::new(Barrier::new(2));
            let release = Arc::new(Barrier::new(2));
            let (tx, rx) = std::sync::mpsc::channel();
            let (ready, finish) = (entered.clone(), release.clone());
            let mut shell = LifecycleShell::new(&fixture.repo.tree, move || {
                ready.wait();
                finish.wait();
            });
            shell.lock_attempt = Some(tx);
            let plan = fixture.plan(&shell);
            assert!(plan.entries[0].removable, "{:?}", plan.entries);
            std::thread::scope(|scope| {
                let run = |close| {
                    if close {
                        crate::close::local(
                            &shell,
                            &fixture.ledger,
                            "placed-shell",
                            &crate::close::Caller::default(),
                            crate::close::Guard::Clean,
                        )
                        .map(|_| ())
                    } else {
                        let applied = crate::cleanup::apply(&fixture.node(&shell), &plan, 100)?;
                        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
                        assert!(applied.skipped.is_empty(), "{:?}", applied.skipped);
                        Ok(())
                    }
                };
                let first = scope.spawn(move || run(close_wins));
                entered.wait();
                let second = std::thread::Builder::new()
                    .name("lifecycle-contender".into())
                    .spawn_scoped(scope, move || run(!close_wins))
                    .unwrap();
                rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
                release.wait();
                first.join().unwrap().unwrap();
                second.join().unwrap().unwrap();
            });
            assert_eq!(shell.teardowns.load(SeqCst), 1);
            assert!(!fixture.repo.tree.exists());
            assert_eq!(recorded(&fixture.ledger), []);
        }
    }
}

#[test]
fn concurrent_closes_reconcile_an_item_closed_by_the_winner() {
    use std::sync::{Arc, Barrier, atomic::Ordering::SeqCst};
    let fixture = lifecycle_fixture(false);
    save_close_checkout(&fixture);
    fixture
        .ledger
        .write(
            "items.json",
            &vec![crate::items::Item {
                id: "c1-old".into(),
                kind: "agent".into(),
                host: crate::hosts::LOCAL.into(),
                pane: "placed-shell".into(),
                owner_pane: "parent".into(),
                ..Default::default()
            }],
        )
        .unwrap();
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let (ready, finish) = (entered.clone(), release.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    let mut shell = LifecycleShell::new(&fixture.repo.tree, move || {
        ready.wait();
        finish.wait();
    });
    shell.lock_attempt = Some(tx);
    std::thread::scope(|scope| {
        let run = || {
            crate::close::with_local(
                &shell,
                &fixture.ledger,
                "placed-shell",
                &crate::close::Caller::default(),
                crate::close::Guard::Clean,
                |closed| {
                    crate::items::close(&fixture.ledger, crate::hosts::LOCAL, "placed-shell", 100)?;
                    Ok(closed)
                },
            )
        };
        let first = scope.spawn(run);
        entered.wait();
        let second = std::thread::Builder::new()
            .name("lifecycle-contender".into())
            .spawn_scoped(scope, run)
            .unwrap();
        rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        release.wait();
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();
    });
    let items = crate::items::list(&fixture.ledger).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].closed_ms, Some(100));
    assert_eq!(shell.teardowns.load(SeqCst), 1);
    assert!(!fixture.repo.tree.exists());
    assert!(!shell.open.load(SeqCst));
    assert_eq!(recorded(&fixture.ledger), []);
}

#[test]
fn absent_close_refuses_deleted_or_reassigned_captured_items() {
    use std::sync::atomic::Ordering::SeqCst;
    for change in ["deleted", "pane", "owner"] {
        let fixture = lifecycle_fixture(false);
        let captured = recorded(&fixture.ledger)[0].clone();
        let item = crate::items::Item {
            id: "c1-close".into(),
            kind: "agent".into(),
            host: crate::hosts::LOCAL.into(),
            pane: "placed-shell".into(),
            owner_pane: "parent".into(),
            ..Default::default()
        };
        fixture
            .ledger
            .write("items.json", &vec![item.clone()])
            .unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut shell = LifecycleShell::new(&fixture.repo.tree, || {});
        shell.lock_attempt = Some(tx);
        std::thread::scope(|scope| {
            let close = crate::worktree::with_lifecycle(
                &shell,
                &fixture.ledger,
                &fixture.repo.repo,
                |_| {
                    let close = std::thread::Builder::new()
                        .name("lifecycle-contender".into())
                        .spawn_scoped(scope, || {
                            crate::close::local(
                                &shell,
                                &fixture.ledger,
                                "placed-shell",
                                &crate::close::Caller::default(),
                                crate::close::Guard::Clean,
                            )
                        })
                        .unwrap();
                    rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
                    external_remove(&fixture.repo);
                    shell.open.store(false, SeqCst);
                    let mut current = vec![item];
                    match change {
                        "deleted" => current.clear(),
                        "pane" => current[0].pane = "another-pane".into(),
                        "owner" => current[0].owner_pane = "another-owner".into(),
                        _ => unreachable!(),
                    }
                    fixture.ledger.write("items.json", &current)?;
                    Ok(close)
                },
            )
            .unwrap();
            let error = close.join().unwrap().unwrap_err().to_string();
            assert!(
                error.contains("checkout item ownership changed"),
                "{change}: {error}"
            );
        });
        assert_eq!(recorded(&fixture.ledger), [captured]);
        assert_eq!(shell.teardowns.load(SeqCst), 0);
    }
}

#[test]
fn lifecycle_lock_inode_survives_close_and_cleanup() {
    use std::os::unix::fs::MetadataExt as _;
    for close in [false, true] {
        let fixture = lifecycle_fixture(false);
        let lock = fixture.repo.repo.join(".git/dogsled-lifecycle.lock");
        let pinned = std::fs::File::open(&lock).unwrap();
        let before = pinned.metadata().unwrap();
        let shell = LifecycleShell::new(&fixture.repo.tree, || {});
        if close {
            crate::close::local(
                &shell,
                &fixture.ledger,
                "placed-shell",
                &crate::close::Caller::default(),
                crate::close::Guard::Clean,
            )
            .unwrap();
        } else {
            let plan = fixture.plan(&shell);
            let applied = crate::cleanup::apply(&fixture.node(&shell), &plan, 100).unwrap();
            assert!(applied.failures.is_empty(), "{applied:?}");
            assert_eq!(applied.results.len(), 1, "{applied:?}");
        }
        assert!(!fixture.repo.tree.exists());
        let after = std::fs::metadata(&lock).unwrap();
        assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
        crate::worktree::with_lifecycle(&shell, &fixture.ledger, &fixture.repo.repo, |_| {
            let reacquired = std::fs::metadata(&lock)?;
            assert_eq!(
                (before.dev(), before.ino()),
                (reacquired.dev(), reacquired.ino())
            );
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn prune_after_prior_removal_reconciles_without_failure() {
    let fixture = lifecycle_fixture(false);
    let shell = LifecycleShell::new(&fixture.repo.tree, || {});
    let plan = fixture.plan(&shell);
    external_remove(&fixture.repo);
    crate::cleanup::forget_matching(&fixture.ledger, plan.entries[0].tree.as_ref().unwrap())
        .unwrap();
    let applied = crate::cleanup::apply(&fixture.node(&shell), &plan, 100).unwrap();
    assert!(applied.failures.is_empty(), "{:?}", applied.failures);
    assert_eq!(applied.results.len(), 1);
    assert_eq!(shell.teardowns.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn close_after_prior_removal_reconciles_without_teardown() {
    let fixture = lifecycle_fixture(false);
    let (tx, rx) = std::sync::mpsc::channel();
    let mut shell = LifecycleShell::new(&fixture.repo.tree, || {});
    shell.lock_attempt = Some(tx);
    std::thread::scope(|scope| {
        let close =
            crate::worktree::with_lifecycle(&shell, &fixture.ledger, &fixture.repo.repo, |_| {
                let close = std::thread::Builder::new()
                    .name("lifecycle-contender".into())
                    .spawn_scoped(scope, || {
                        crate::close::local(
                            &shell,
                            &fixture.ledger,
                            "placed-shell",
                            &crate::close::Caller::default(),
                            crate::close::Guard::Clean,
                        )
                    })
                    .unwrap();
                rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
                external_remove(&fixture.repo);
                shell.open.store(false, std::sync::atomic::Ordering::SeqCst);
                Ok(close)
            })
            .unwrap();
        close.join().unwrap().unwrap();
    });
    assert_eq!(shell.teardowns.load(std::sync::atomic::Ordering::SeqCst), 0);
}

fn replace_checkout(repo: &Path, path: &Path, alternate_admin: bool) -> crate::cleanup::Tree {
    let original_admin = git(path, &["rev-parse", "--absolute-git-dir"]);
    let admin_name = Path::new(original_admin.trim()).file_name().unwrap();
    git(
        repo,
        &["worktree", "remove", "-f", "-f", path.to_str().unwrap()],
    );
    let temporary = if alternate_admin {
        path.with_file_name("different-admin")
    } else {
        path.with_file_name(admin_name)
    };
    let create = &temporary;
    git(
        repo,
        &[
            "worktree",
            "add",
            "-q",
            "--lock",
            "--reason",
            "agent:worktree",
            create.to_str().unwrap(),
            "feature",
        ],
    );
    if create != path {
        git(repo, &["worktree", "unlock", create.to_str().unwrap()]);
        git(
            repo,
            &[
                "worktree",
                "move",
                create.to_str().unwrap(),
                path.to_str().unwrap(),
            ],
        );
        git(
            repo,
            &[
                "worktree",
                "lock",
                "--reason",
                "agent:worktree",
                path.to_str().unwrap(),
            ],
        );
    }
    let dir = git(path, &["rev-parse", "--absolute-git-dir"]);
    std::fs::write(
        Path::new(dir.trim()).join("dogsled-checkout-id"),
        "123456789abcdef0123456789abcdef0\n",
    )
    .unwrap();
    crate::worktree::linked_or_detached(&System, path.to_str().unwrap())
        .unwrap()
        .unwrap()
        .tree()
}

#[test]
fn same_path_same_head_replacement_is_preserved() {
    for alternate_admin in [false, true] {
        let fixture = lifecycle_fixture(false);
        let shell = LifecycleShell::new(&fixture.repo.tree, || {});
        let plan = fixture.plan(&shell);
        let replacement = replace_checkout(&fixture.repo.repo, &fixture.repo.tree, alternate_admin);
        crate::cleanup::remember(&fixture.ledger, replacement.clone()).unwrap();
        assert_eq!(
            replacement.head,
            plan.entries[0].tree.as_ref().unwrap().head
        );
        let applied = crate::cleanup::apply(&fixture.node(&shell), &plan, 100).unwrap();
        assert_eq!(applied.failures.len(), 1, "{applied:?}");
        assert!(fixture.repo.tree.is_dir());
        assert_eq!(recorded(&fixture.ledger), [replacement]);
        assert_eq!(shell.teardowns.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}

#[test]
fn gone_checkout_does_not_reconcile_a_reused_pane_or_item() {
    for replace_item in [false, true] {
        let fixture = lifecycle_fixture(false);
        let captured = recorded(&fixture.ledger)[0].clone();
        let progress = serde_json::json!({
            "request":{"id":"c1-old","action":{"type":"agent","kind":"pi","name":"worker","message":"go"},"host":"local","cwd":fixture.repo.tree,"pane":"placed-shell","terminal":"c1-original","workspace":null,"workspace_name":null,"owner":"parent"},
            "pane":"placed-shell","terminal":"c1-original","attempted":true,"confirmed":true,
            "result":{"checkout":captured},"error":null
        });
        fixture
            .ledger
            .child("launches")
            .write("c1-old.progress.json", &progress)
            .unwrap();
        fixture.ledger.write("items.json", &serde_json::json!([{
            "id":if replace_item {"c1-new"} else {"c1-old"},"kind":"agent","host":"local","pane":"placed-shell","owner_pane":"parent","name":"worker","created_ms":1,"closed_ms":null
        }])).unwrap();
        external_remove(&fixture.repo);
        let mut shell = LifecycleShell::new(&fixture.repo.tree, || {});
        if !replace_item {
            shell.terminal = "c1-replacement".into();
        }
        let reconciled = std::sync::atomic::AtomicBool::new(false);
        let result = crate::close::with_local(
            &shell,
            &fixture.ledger,
            "placed-shell",
            &crate::close::Caller::default(),
            crate::close::Guard::Clean,
            |_| {
                reconciled.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        );
        assert!(result.is_err(), "{result:?}");
        assert!(!reconciled.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(recorded(&fixture.ledger), [captured]);
        assert!(
            crate::items::list(&fixture.ledger).unwrap()[0]
                .closed_ms
                .is_none()
        );
        assert_eq!(shell.teardowns.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}

#[test]
fn handoff_skips_unproven_owner_and_closes_the_next_mark() {
    let fixture = lifecycle_fixture(false);
    let captured = recorded(&fixture.ledger)[0].clone();
    fixture.ledger.child("launches").write("c1-old.progress.json", &serde_json::json!({
        "request":{"id":"c1-old","action":{"type":"agent","kind":"pi","name":"worker","message":"go"},"host":"local","cwd":fixture.repo.tree,"pane":"placed-shell","terminal":"c1-original","workspace":null,"workspace_name":null,"owner":"parent"},
        "pane":"placed-shell","terminal":"c1-original","attempted":true,"confirmed":true,
        "result":{"checkout":captured},"error":null
    })).unwrap();
    fixture.ledger.write("items.json", &serde_json::json!([
        {"id":"a-missing","kind":"agent","host":"local","pane":"gone-child","owner_pane":"parent","created_ms":1,"closed_ms":null},
        {"id":"c1-old","kind":"agent","host":"local","pane":"placed-shell","owner_pane":"parent","created_ms":1,"closed_ms":null}
    ])).unwrap();
    fixture.ledger.write("done.json", &serde_json::json!({
        "[\"a-missing\",\"gone-child\",null]":{"pane":"gone-child","item":"a-missing","sealed":10,"marked_ms":1,"native_done":true,
            "reply":{"seq":10,"text":"unproven checkout","captured_ms":2,"done":true}},
        "[\"c1-old\",\"placed-shell\",null]":{"pane":"placed-shell","item":"c1-old","sealed":10,"marked_ms":1,"native_done":true,
            "reply":{"seq":10,"text":"sealed handoff","captured_ms":2,"done":true}}
    })).unwrap();
    external_remove(&fixture.repo);
    let shell = LifecycleShell::new(&fixture.repo.tree, || {});
    shell.open.store(false, std::sync::atomic::Ordering::SeqCst);
    crate::done::handoff(&fixture.ledger, &shell, 100).unwrap();
    let items = crate::items::list(&fixture.ledger).unwrap();
    assert!(items[0].closed_ms.is_none());
    assert_eq!(items[1].closed_ms, Some(100));
    let marks = crate::done::list(&fixture.ledger).unwrap();
    assert!(marks[0].closed_ms.is_none());
    assert_eq!(marks[1].closed_ms, Some(100));
}

#[test]
fn saved_close_never_recreates_checkout_identity() {
    for malformed in [false, true] {
        let fixture = lifecycle_fixture(false);
        let captured = recorded(&fixture.ledger)[0].clone();
        let marker = captured
            .identity
            .as_ref()
            .unwrap()
            .gitdir
            .join("dogsled-checkout-id");
        fixture.ledger.child("launches").write("c1-old.progress.json", &serde_json::json!({
            "request":{"id":"c1-old","action":{"type":"agent","kind":"pi","name":"worker","message":"go"},"host":"local","cwd":fixture.repo.tree,"pane":"placed-shell","terminal":"c1-original","workspace":null,"workspace_name":null,"owner":"parent"},
            "pane":"placed-shell","terminal":"c1-original","attempted":true,"confirmed":true,
            "result":{"checkout":captured},"error":null
        })).unwrap();
        if malformed {
            std::fs::write(&marker, "invalid generation").unwrap();
        } else {
            std::fs::remove_file(&marker).unwrap();
        }
        let shell = LifecycleShell::new(&fixture.repo.tree, || {});
        let result = crate::close::local(
            &shell,
            &fixture.ledger,
            "placed-shell",
            &crate::close::Caller::default(),
            crate::close::Guard::Clean,
        );
        assert!(result.is_err(), "{result:?}");
        if malformed {
            assert_eq!(
                std::fs::read_to_string(&marker).unwrap(),
                "invalid generation"
            );
        } else {
            assert!(
                !marker.exists(),
                "stale close recreated the checkout marker"
            );
        }
        assert!(shell.open.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(recorded(&fixture.ledger), [captured]);
        assert_eq!(shell.teardowns.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}

#[test]
fn stale_request_never_recreates_checkout_identity() {
    for malformed in [false, true] {
        let fixture = lifecycle_fixture(false);
        let shell = LifecycleShell::new(&fixture.repo.tree, || {});
        let plan = fixture.plan(&shell);
        let marker = plan.entries[0]
            .tree
            .as_ref()
            .unwrap()
            .identity
            .as_ref()
            .unwrap()
            .gitdir
            .join("dogsled-checkout-id");
        if malformed {
            std::fs::write(&marker, "invalid generation").unwrap();
        } else {
            std::fs::remove_file(&marker).unwrap();
        }
        let before = recorded(&fixture.ledger);
        let applied = crate::cleanup::apply(&fixture.node(&shell), &plan, 100).unwrap();
        assert_eq!(applied.failures.len(), 1, "{applied:?}");
        if malformed {
            assert_eq!(
                std::fs::read_to_string(marker).unwrap(),
                "invalid generation"
            );
        } else {
            assert!(!marker.exists());
        }
        assert_eq!(recorded(&fixture.ledger), before);
        assert_eq!(shell.teardowns.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}

#[test]
fn teardown_replacement_is_preserved() {
    let fixture = lifecycle_fixture(false);
    let (repo, path, ledger) = (
        fixture.repo.repo.clone(),
        fixture.repo.tree.clone(),
        fixture.ledger.clone(),
    );
    let shell = LifecycleShell::new(&fixture.repo.tree, move || {
        let replacement = replace_checkout(&repo, &path, false);
        crate::cleanup::remember(&ledger, replacement).unwrap();
    });
    let plan = fixture.plan(&shell);
    let applied = crate::cleanup::apply(&fixture.node(&shell), &plan, 100).unwrap();
    assert_eq!(applied.failures.len(), 1, "{applied:?}");
    assert!(shell.open.load(std::sync::atomic::Ordering::SeqCst));
    assert!(fixture.repo.tree.is_dir());
    assert_ne!(
        recorded(&fixture.ledger)[0].identity,
        plan.entries[0].tree.as_ref().unwrap().identity
    );
}

#[test]
fn absence_requires_git_disk_and_admin_agreement() {
    for case in [
        "retained-admin",
        "prunable-admin",
        "failed-listing",
        "dangling-symlink",
        "unregistered-content",
    ] {
        let fixture = lifecycle_fixture(false);
        let mut shell = LifecycleShell::new(&fixture.repo.tree, || {});
        let plan = fixture.plan(&shell);
        match case {
            "retained-admin" | "prunable-admin" => {
                if case == "prunable-admin" {
                    git(
                        &fixture.repo.repo,
                        &["worktree", "unlock", fixture.repo.tree.to_str().unwrap()],
                    );
                }
                std::fs::remove_dir_all(&fixture.repo.tree).unwrap();
            }
            _ => {
                external_remove(&fixture.repo);
                match case {
                    "failed-listing" => shell.fail_listing = true,
                    "dangling-symlink" => std::os::unix::fs::symlink(
                        fixture.repo.root.join("absent"),
                        &fixture.repo.tree,
                    )
                    .unwrap(),
                    "unregistered-content" => {
                        std::fs::create_dir(&fixture.repo.tree).unwrap();
                        std::fs::write(fixture.repo.tree.join("keep"), "unregistered content")
                            .unwrap();
                    }
                    _ => unreachable!(),
                }
            }
        }
        let before = recorded(&fixture.ledger);
        let applied = crate::cleanup::apply(&fixture.node(&shell), &plan, 100).unwrap();
        assert_eq!(applied.failures.len(), 1, "{case}: {applied:?}");
        assert_eq!(recorded(&fixture.ledger), before, "{case}");
        assert_eq!(
            shell.teardowns.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "{case}"
        );
        assert!(
            shell.open.load(std::sync::atomic::Ordering::SeqCst),
            "{case}"
        );
    }
}

#[test]
fn git_status_error_in_an_existing_listed_worktree_still_fails() {
    let repo = Repo::new();
    let ledger = Ledger::new(repo.root.join("state"));
    let tree = registered(&repo, &ledger);
    let cleanup = crate::config::Cleanup::default();
    let index = repo.repo.join(".git/worktrees/tree/index");
    git(&repo.tree, &["status"]);
    let corrupt = move || std::fs::write(&index, "not an index").unwrap();
    let node = crate::node::Node {
        shell: &System,
        ledger: &ledger,
        thresholds: crate::config::Thresholds::from(&crate::config::Policy::default()),
        home: repo.root.to_str().unwrap(),
        cleanup: &cleanup,
    };
    let plan = fixture_plan(&node, Scope::Sweep, &repo);
    assert!(plan.entries[0].removable, "{:?}", plan.entries);

    // The index breaks after the survey: the apply-time recheck fails.
    let racing = Racing::new(&repo.tree, corrupt);
    let node = crate::node::Node {
        shell: &racing,
        ..node
    };
    let applied = crate::cleanup::apply_survey(&node, &plan, 100);
    assert!(racing.fired.load(std::sync::atomic::Ordering::SeqCst));
    let [failure] = applied.failures.as_slice() else {
        panic!("{applied:?}");
    };
    assert!(failure.contains("cannot check worktree"), "{failure}");
    assert!(repo.tree.is_dir());
    assert_eq!(recorded(&ledger), std::slice::from_ref(&tree));

    // The survey reports it as unreadable, never as removable.
    let plan = fixture_plan(&node, Scope::Sweep, &repo);
    let [entry] = plan.entries.as_slice() else {
        panic!("{:?}", plan.entries);
    };
    assert!(!entry.removable);
    assert!(
        entry.detail.contains("unreadable: cannot check worktree"),
        "{}",
        entry.detail
    );
    assert_eq!(recorded(&ledger), [tree]);
}
