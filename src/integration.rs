//! Whether a branch's work is already in the repository's default branch, from Git alone.
//!
//! A merge makes the branch an ancestor. A squash, rebase-to-one, or cherry-pick does not, so
//! the second rule looks for one commit on the default branch that applies the branch's work:
//! merging the branch into that commit's parent reproduces the commit on every path the branch
//! changed. Pull request hosts and merge styles do not matter.

use crate::error::{Error, Result};
use crate::shell::Shell;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Integrated {
    /// The branch no longer exists.
    Deleted,
    /// Every branch commit is on `base`, or the branch changes nothing since it forked.
    Ancestor { base: String },
    /// `commit` on `base` applies the branch's changes.
    Applied { base: String, commit: String },
}

impl std::fmt::Display for Integrated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Deleted => write!(f, "deleted"),
            Self::Ancestor { base } => write!(f, "merged into {base}"),
            Self::Applied { base, commit } => {
                write!(
                    f,
                    "applied to {base} by {}",
                    &commit[..commit.len().min(10)]
                )
            }
        }
    }
}

fn git(shell: &dyn Shell, repo: &str, args: &[&str]) -> Result<crate::shell::Output> {
    let mut words = vec!["-C".to_string(), repo.to_string()];
    words.extend(args.iter().map(ToString::to_string));
    shell.run("git", &words)
}

fn failed(repo: &str, what: &str, out: &crate::shell::Output) -> Error {
    Error::Invalid(format!("{repo}: cannot {what}: {}", out.stderr.trim()))
}

/// The remote default branch is authoritative; local `main` is the fallback.
fn base(shell: &dyn Shell, repo: &str) -> Result<String> {
    let remote = git(
        shell,
        repo,
        &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
    )?;
    Ok(if remote.ok() {
        remote.stdout.trim().to_string()
    } else {
        "refs/heads/main".into()
    })
}

fn short(reference: &str) -> String {
    reference
        .strip_prefix("refs/remotes/")
        .or_else(|| reference.strip_prefix("refs/heads/"))
        .unwrap_or(reference)
        .to_string()
}

/// `None` means the branch has work that `base` does not hold.
pub fn check(shell: &dyn Shell, repo: &str, branch: &str) -> Result<Option<Integrated>> {
    let reference = format!("refs/heads/{branch}");
    let exists = git(
        shell,
        repo,
        &["show-ref", "--verify", "--quiet", &reference],
    )?;
    match exists.status {
        0 => (),
        1 => return Ok(Some(Integrated::Deleted)),
        _ => return Err(failed(repo, &format!("inspect branch {branch}"), &exists)),
    }
    let base = base(shell, repo)?;
    let name = short(&base);
    // A branch on the base tip has no work yet: a new worktree looks the same as a merged one.
    let tips = git(shell, repo, &["rev-parse", &reference, &base])?;
    if !tips.ok() {
        return Err(failed(repo, &format!("resolve {branch} and {name}"), &tips));
    }
    let mut tips = tips.stdout.lines();
    if tips.next() == tips.next() {
        return Ok(None);
    }
    let ancestor = git(
        shell,
        repo,
        &["merge-base", "--is-ancestor", &reference, &base],
    )?;
    match ancestor.status {
        0 => return Ok(Some(Integrated::Ancestor { base: name })),
        1 => (),
        _ => {
            return Err(failed(
                repo,
                &format!("compare {branch} with {name}"),
                &ancestor,
            ));
        }
    }
    let fork = git(shell, repo, &["merge-base", &base, &reference])?;
    // Unrelated histories share no fork point, so nothing on base can apply the branch.
    if fork.status == 1 {
        return Ok(None);
    }
    if !fork.ok() {
        return Err(failed(repo, &format!("find the fork of {branch}"), &fork));
    }
    let fork = fork.stdout.trim().to_string();
    let changed = git(
        shell,
        repo,
        &[
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            &fork,
            &reference,
        ],
    )?;
    if !changed.ok() {
        return Err(failed(repo, &format!("list changes of {branch}"), &changed));
    }
    let paths: BTreeSet<String> = changed
        .stdout
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(ToString::to_string)
        .collect();
    if paths.is_empty() {
        return Ok(Some(Integrated::Ancestor { base: name }));
    }
    let pathspec: Vec<String> = paths.iter().map(|p| format!(":(literal){p}")).collect();
    let range = format!("{fork}..{base}");
    let mut args = vec!["log", "--no-merges", "--format=%H %P", &range, "--"];
    args.extend(pathspec.iter().map(String::as_str));
    let log = git(shell, repo, &args)?;
    if !log.ok() {
        return Err(failed(repo, &format!("list {name} commits"), &log));
    }
    for line in log.stdout.lines() {
        let mut words = line.split_whitespace();
        let (Some(commit), Some(parent), None) = (words.next(), words.next(), words.next()) else {
            continue;
        };
        if applies(shell, repo, commit, parent, &reference, &paths, &pathspec)? {
            return Ok(Some(Integrated::Applied {
                base: name,
                commit: commit.into(),
            }));
        }
    }
    Ok(None)
}

/// `commit` applies the branch when it touches every branch path and merging the branch into
/// its parent yields `commit`'s content on those paths.
fn applies(
    shell: &dyn Shell,
    repo: &str,
    commit: &str,
    parent: &str,
    reference: &str,
    paths: &BTreeSet<String>,
    pathspec: &[String],
) -> Result<bool> {
    let mut args = vec![
        "diff-tree",
        "-r",
        "-z",
        "--name-only",
        "--no-renames",
        "--no-commit-id",
        commit,
        "--",
    ];
    args.extend(pathspec.iter().map(String::as_str));
    let touched = git(shell, repo, &args)?;
    if !touched.ok() {
        return Err(failed(repo, &format!("list paths of {commit}"), &touched));
    }
    if touched.stdout.split('\0').filter(|p| !p.is_empty()).count() != paths.len() {
        return Ok(false);
    }
    let merged = git(
        shell,
        repo,
        &["merge-tree", "--write-tree", parent, reference],
    )?;
    match merged.status {
        0 => (),
        // A conflict means the branch does not apply cleanly at this point.
        1 => return Ok(false),
        _ => return Err(failed(repo, "run git merge-tree --write-tree", &merged)),
    }
    let tree = merged.stdout.lines().next().unwrap_or_default().trim();
    let mut args = vec!["diff", "--quiet", "--no-renames", commit, tree, "--"];
    args.extend(pathspec.iter().map(String::as_str));
    let same = git(shell, repo, &args)?;
    match same.status {
        0 => Ok(true),
        1 => Ok(false),
        _ => Err(failed(repo, &format!("compare {commit}"), &same)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::Output;
    use std::path::{Path, PathBuf};

    /// Real Git without the caller's `GIT_*` variables, which a Git hook sets.
    #[derive(Debug)]
    struct Isolated;

    impl Shell for Isolated {
        fn run(&self, program: &str, args: &[String]) -> Result<Output> {
            let out = isolated(program).args(args).output()?;
            Ok(Output {
                status: out.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            })
        }
    }

    fn isolated(program: &str) -> std::process::Command {
        let mut command = std::process::Command::new(program);
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                command.env_remove(key);
            }
        }
        command
    }

    struct Repo(PathBuf);

    impl Repo {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("wrangle-integration-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let repo = Self(dir);
            repo.git(&["init", "-q", "-b", "main"]);
            repo.commit("a", "1\n2\n3\n", "init");
            repo
        }
        fn git(&self, args: &[&str]) -> String {
            let out = isolated("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(&self.0)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        }
        fn commit(&self, file: &str, text: &str, message: &str) {
            std::fs::write(self.0.join(file), text).unwrap();
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "-m", message]);
        }
        fn path(&self) -> &str {
            self.0.to_str().unwrap()
        }
        fn check(&self, branch: &str) -> Option<Integrated> {
            check(&Isolated, self.path(), branch).unwrap()
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(Path::new(&self.0));
        }
    }

    fn applied(found: Option<&Integrated>) -> bool {
        matches!(found, Some(Integrated::Applied { .. }))
    }

    #[test]
    fn merge_and_missing_branch_are_integrated_and_new_work_is_not() {
        let repo = Repo::new("merge");
        repo.git(&["branch", "fresh"]);
        assert_eq!(repo.check("fresh"), None);
        repo.git(&["switch", "-q", "-c", "feat"]);
        repo.commit("b", "new\n", "feat");
        repo.git(&["switch", "-q", "main"]);
        assert_eq!(repo.check("feat"), None);
        repo.git(&["merge", "-q", "--no-ff", "-m", "merge", "feat"]);
        assert_eq!(
            repo.check("feat"),
            Some(Integrated::Ancestor {
                base: "main".into()
            })
        );
        assert_eq!(repo.check("gone"), Some(Integrated::Deleted));
    }

    #[test]
    fn squash_is_found_even_after_base_changes_the_same_files() {
        let repo = Repo::new("squash");
        repo.git(&["switch", "-q", "-c", "feat"]);
        repo.commit("a", "1\n2\nthree\n", "feat one");
        repo.commit("b", "new\n", "feat two");
        repo.git(&["switch", "-q", "main"]);
        // Base edits the same file before and after the squash lands.
        repo.commit("a", "one\n2\n3\n", "base before");
        repo.git(&["merge", "-q", "--squash", "feat"]);
        repo.git(&["commit", "-q", "-m", "squash"]);
        repo.commit("a", "one\ntwo\nthree\n", "base after");
        assert!(applied(repo.check("feat").as_ref()));
        // More work on the branch after the squash is not integrated.
        repo.git(&["switch", "-q", "feat"]);
        repo.commit("c", "later\n", "after squash");
        repo.git(&["switch", "-q", "main"]);
        assert_eq!(repo.check("feat"), None);
    }

    #[test]
    fn a_commit_touching_only_some_branch_paths_is_not_the_branch() {
        let repo = Repo::new("partial");
        repo.git(&["switch", "-q", "-c", "feat"]);
        repo.commit("b", "new\n", "feat b");
        repo.commit("c", "new\n", "feat c");
        repo.git(&["switch", "-q", "main"]);
        repo.git(&["checkout", "feat", "--", "b"]);
        repo.git(&["commit", "-q", "-m", "only b"]);
        assert_eq!(repo.check("feat"), None);
    }
}
