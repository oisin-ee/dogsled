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
    let prune = survey(&node, Scope::Prune, 100).unwrap();
    let sweep = survey(&node, Scope::Sweep, 100).unwrap();
    let decisions = |plan: &crate::cleanup::Plan| {
        plan.entries
            .iter()
            .filter(|e| matches!(e.class.as_str(), "worktrees" | "branches"))
            .map(|e| (e.id.clone(), e.removable))
            .collect::<Vec<_>>()
    };
    assert_eq!(decisions(&prune), decisions(&sweep));
    assert!(decisions(&prune).iter().any(|(_, removable)| *removable));
    for plan in [&prune, &sweep] {
        assert!(plan.entries.iter().any(|e| e.class == "repositories"
            && e.id == missing
            && e.detail.contains("repositories.json")));
    }
    std::fs::write(repo.tree.join("dirty"), "work").unwrap();
    let plan = survey(&node, Scope::Prune, 100).unwrap();
    let finding = plan
        .entries
        .iter()
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
    assert!(
        !held
            .entries
            .iter()
            .any(|e| e.kind.as_deref() == Some("landed_dirty"))
    );
    assert!(
        held.entries
            .iter()
            .any(|e| e.detail.contains("pending preparation"))
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
    let plan = survey(&node, Scope::Prune, 100).unwrap();
    assert!(
        plan.entries
            .iter()
            .any(|e| e.class == "branches" && !e.removable)
    );
    assert!(git(&repo.repo, &["branch", "-vv"]).contains("gone"));
    git(&repo.repo, &["merge", "-q", "--squash", "feature"]);
    git(&repo.repo, &["commit", "-q", "-m", "integrated"]);
    git(&repo.repo, &["push", "-q", "origin", "main"]);
    let mut plan = survey(&node, Scope::Prune, 100).unwrap();
    plan.entries.retain(|e| e.class == "branches");
    assert_eq!(plan.entries.len(), 1);
    assert!(plan.entries[0].removable);
    let applied = crate::cleanup::apply(&node, &plan, 100).unwrap();
    assert!(applied.failures.is_empty(), "{:?}", applied.failures);
    assert_eq!(git(&repo.repo, &["branch", "--list", "feature"]), "");
}
