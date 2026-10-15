//! Whether a branch's work is already in the repository's default branch, from Git alone.
//!
//! A merge makes the branch an ancestor. A squash, rebase-to-one, or cherry-pick does not, so
//! the second rule looks for commits on the default branch that apply the branch's work. The
//! branch's commits split into consecutive runs, and each run has one commit that applies it:
//! merging the run into that commit's parent reproduces the commit on every path the run
//! changed. One run is a squash of the whole branch. Several runs are a stack whose lower
//! branches merged first. Pull request hosts and merge styles do not matter.

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
    /// Each of `commits` on `base` applies one consecutive run of the branch, in branch order.
    Stacked { base: String, commits: Vec<String> },
}

fn abbreviated(commit: &str) -> &str {
    &commit[..commit.len().min(10)]
}

impl std::fmt::Display for Integrated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Deleted => write!(f, "deleted"),
            Self::Ancestor { base } => write!(f, "merged into {base}"),
            Self::Applied { base, commit } => {
                write!(f, "applied to {base} by {}", abbreviated(commit))
            }
            Self::Stacked { base, commits } => {
                let commits: Vec<&str> = commits.iter().map(|c| abbreviated(c)).collect();
                write!(f, "applied to {base} by {}", commits.join(", "))
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
    let branch = commits(
        shell,
        repo,
        &format!("{fork}..{reference}"),
        &["--first-parent"],
        &[],
    )?;
    let touched: BTreeSet<String> = branch.iter().flat_map(|c| c.paths.clone()).collect();
    if touched.is_empty() {
        return Ok(Some(Integrated::Ancestor { base: name }));
    }
    let pathspec: Vec<String> = touched.iter().map(|p| format!(":(literal){p}")).collect();
    let candidates = commits(
        shell,
        repo,
        &format!("{fork}..{base}"),
        &["--no-merges"],
        &pathspec,
    )?;
    // Branch positions: 0 is the fork, k is the branch's k-th commit.
    let mut points = vec![fork];
    points.extend(branch.iter().map(|c| c.id.clone()));
    let Some(applied) = runs(shell, repo, &points, &branch, &candidates)? else {
        return Ok(None);
    };
    Ok(Some(match <[String; 1]>::try_from(applied) {
        Ok([commit]) => Integrated::Applied { base: name, commit },
        Err(commits) => Integrated::Stacked {
            base: name,
            commits,
        },
    }))
}

/// One commit from `git log`, with the paths it changed (limited to the pathspec, if any).
struct Commit {
    id: String,
    parent: String,
    paths: BTreeSet<String>,
}

/// Commits in `range`, oldest first, with their changed paths.
fn commits(
    shell: &dyn Shell,
    repo: &str,
    range: &str,
    options: &[&str],
    pathspec: &[String],
) -> Result<Vec<Commit>> {
    let mut args = vec![
        "log",
        "--reverse",
        "--no-renames",
        "--name-only",
        "-z",
        "--format=%x01%H %P",
    ];
    args.extend(options);
    args.push(range);
    args.push("--");
    args.extend(pathspec.iter().map(String::as_str));
    let out = git(shell, repo, &args)?;
    if !out.ok() {
        return Err(failed(repo, &format!("list commits {range}"), &out));
    }
    Ok(out
        .stdout
        .split('\u{1}')
        .filter_map(|record| {
            let mut fields = record.split('\0');
            let mut header = fields.next()?.split_whitespace();
            Some(Commit {
                id: header.next()?.to_string(),
                parent: header.next().unwrap_or_default().to_string(),
                paths: fields
                    .map(|p| p.trim_start_matches('\n'))
                    .filter(|p| !p.is_empty())
                    .map(ToString::to_string)
                    .collect(),
            })
        })
        .collect())
}

/// Splits the branch into consecutive runs that `candidates` apply, longest runs first.
/// Returns the applying commits in branch order, or `None` when some part of the branch is
/// not applied.
fn runs(
    shell: &dyn Shell,
    repo: &str,
    points: &[String],
    branch: &[Commit],
    candidates: &[Commit],
) -> Result<Option<Vec<String>>> {
    let last = points.len() - 1;
    // reached[j] is the run start and the commit that reach position j.
    let mut reached: Vec<Option<(usize, String)>> = vec![None; points.len()];
    for start in 0..last {
        if start > 0 && reached[start].is_none() {
            continue;
        }
        for end in (start + 1..=last).rev() {
            if reached[end].is_some() {
                continue;
            }
            let paths: BTreeSet<&String> =
                branch[start..end].iter().flat_map(|c| &c.paths).collect();
            // Newest candidate first, as `git log` lists them without --reverse.
            for candidate in candidates.iter().rev() {
                if candidate.parent.is_empty()
                    || !paths.iter().all(|p| candidate.paths.contains(*p))
                {
                    continue;
                }
                if applies(shell, repo, &points[start], &points[end], candidate)? {
                    reached[end] = Some((start, candidate.id.clone()));
                    break;
                }
            }
        }
        if reached[last].is_some() {
            break;
        }
    }
    let mut applied = Vec::new();
    let mut position = last;
    while position > 0 {
        let Some((start, commit)) = reached[position].clone() else {
            return Ok(None);
        };
        applied.push(commit);
        position = start;
    }
    applied.reverse();
    Ok(Some(applied))
}

/// `commit` applies the run `start..end` when it touches every path the run changed, and
/// merging the run into its parent yields `commit`'s content on those paths.
fn applies(shell: &dyn Shell, repo: &str, start: &str, end: &str, commit: &Commit) -> Result<bool> {
    let changed = git(
        shell,
        repo,
        &["diff", "--name-only", "-z", "--no-renames", start, end],
    )?;
    if !changed.ok() {
        return Err(failed(
            repo,
            &format!("list changes {start}..{end}"),
            &changed,
        ));
    }
    let paths: Vec<String> = changed
        .stdout
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(|p| format!(":(literal){p}"))
        .collect();
    // A run that reverts itself changes nothing, so it cannot name an applying commit.
    if paths.is_empty() {
        return Ok(false);
    }
    let merge_base = format!("--merge-base={start}");
    let merged = git(
        shell,
        repo,
        &[
            "merge-tree",
            "--write-tree",
            &merge_base,
            &commit.parent,
            end,
        ],
    )?;
    match merged.status {
        0 => (),
        // A conflict means the run does not apply cleanly at this point.
        1 => return Ok(false),
        _ => return Err(failed(repo, "run git merge-tree --write-tree", &merged)),
    }
    let tree = merged.stdout.lines().next().unwrap_or_default().trim();
    let mut args = vec!["diff", "--quiet", "--no-renames", &commit.id, tree, "--"];
    args.extend(paths.iter().map(String::as_str));
    let same = git(shell, repo, &args)?;
    match same.status {
        0 => Ok(true),
        1 => Ok(false),
        _ => Err(failed(repo, &format!("compare {}", commit.id), &same)),
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
    fn a_stack_is_found_when_each_branch_landed_as_its_own_squash() {
        let repo = Repo::new("stack");
        repo.git(&["switch", "-q", "-c", "lower"]);
        repo.commit("b", "lower\n", "lower");
        repo.git(&["switch", "-q", "-c", "upper"]);
        repo.commit("a", "1\n2\nupper\n", "upper one");
        repo.commit("c", "upper\n", "upper two");
        repo.git(&["switch", "-q", "main"]);
        repo.git(&["merge", "-q", "--squash", "lower"]);
        repo.git(&["commit", "-q", "-m", "squash lower"]);
        let lower = repo.git(&["rev-parse", "HEAD"]);
        // Base edits a file of the upper branch between the two squashes.
        repo.commit("a", "one\n2\n3\n", "base between");
        // Before the upper squash lands, the lower run alone is applied.
        assert_eq!(repo.check("upper"), None);
        repo.git(&["merge", "-q", "--squash", "upper"]);
        repo.git(&["commit", "-q", "-m", "squash upper"]);
        let upper = repo.git(&["rev-parse", "HEAD"]);
        repo.commit("a", "one\ntwo\nupper\n", "base after");
        assert_eq!(
            repo.check("upper"),
            Some(Integrated::Stacked {
                base: "main".into(),
                commits: vec![lower.clone(), upper],
            })
        );
        assert_eq!(
            repo.check("lower"),
            Some(Integrated::Applied {
                base: "main".into(),
                commit: lower,
            })
        );
        // More work on top of the landed stack is not integrated.
        repo.git(&["switch", "-q", "upper"]);
        repo.commit("d", "later\n", "after both");
        repo.git(&["switch", "-q", "main"]);
        assert_eq!(repo.check("upper"), None);
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
