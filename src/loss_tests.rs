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
                serde_json::json!({"result":{"process_info":{"shell_pid":10,"foreground_processes":[{"pid":10,"name":"zsh"}]}}}),
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
