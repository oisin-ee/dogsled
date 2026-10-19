//! Whether a branch's work is already in the repository's default branch, from Git alone.
//!
//! A merge makes the branch an ancestor. A squash, rebase-to-one, or cherry-pick does not, so
//! the second rule looks for commits on the default branch that apply the branch's work. The
//! branch's commits split into consecutive runs, and each run has one commit that applies it:
//! merging the run into that commit's parent reproduces the commit on every path the run
//! changed. One run is a squash of the whole branch. Several runs are a stack whose lower
//! branches merged first. Pull request hosts and merge styles do not matter.
//!
//! Each path merges on its own, from blob ids in `git log --raw`, with `git merge-file` only
//! where both sides changed a file. A doubtful path is a conflict, so the error is to keep a
//! branch, never to remove one. Finally, a clean `git merge-tree` result identical to the
//! default branch's tree proves that merging the branch would change nothing.

use crate::error::{Error, Result};
use crate::shell::Shell;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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
    /// A clean merge into `base` produces its existing tree.
    MergeTree { base: String },
}

fn abbreviated(commit: &str) -> &str {
    &commit[..commit.len().min(10)]
}

impl std::fmt::Display for Integrated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Deleted => write!(f, "deleted"),
            Self::Ancestor { base } => write!(f, "merged into {base}"),
            Self::MergeTree { base } => {
                write!(f, "integrated into {base} by merge-tree (no changes)")
            }
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
pub fn base(shell: &dyn Shell, repo: &str) -> Result<String> {
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

pub fn short(reference: &str) -> String {
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
    check_ref(shell, repo, &reference)
}

/// The same integration proof for a branch, detached HEAD, or immutable commit ID.
/// Equality is ancestry too: pending preparation is a hold, not unheld work.
pub fn check_ref(shell: &dyn Shell, repo: &str, reference: &str) -> Result<Option<Integrated>> {
    let branch = reference;
    let base = base(shell, repo)?;
    let name = short(&base);
    let ancestor = git(
        shell,
        repo,
        &["merge-base", "--is-ancestor", reference, &base],
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
    let fork = git(shell, repo, &["merge-base", &base, reference])?;
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
        return Ok(merge_tree_unchanged(shell, repo, &base, reference)?
            .then_some(Integrated::MergeTree { base: name }));
    };
    Ok(Some(match <[String; 1]>::try_from(applied) {
        Ok([commit]) => Integrated::Applied { base: name, commit },
        Err(commits) => Integrated::Stacked {
            base: name,
            commits,
        },
    }))
}

/// A disposable object store: no repository config, info/attributes, or writes to the source.
struct MergeRepo(std::path::PathBuf);

impl MergeRepo {
    fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("dogsled-merge-{}", crate::ledger::new_id()));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }

    fn git(&self, shell: &dyn Shell, args: &[&str]) -> Result<crate::shell::Output> {
        // Git hooks can export repository paths and command-scope configuration. Neither
        // those nor system/global config and attributes belong in this merge calculation.
        let mut words = Vec::new();
        for (key, _) in std::env::vars() {
            if key.starts_with("GIT_") {
                words.extend(["-u".into(), key]);
            }
        }
        words.extend([
            "GIT_CONFIG_NOSYSTEM=1".into(),
            "GIT_CONFIG_GLOBAL=/dev/null".into(),
            "GIT_ATTR_NOSYSTEM=1".into(),
            "git".into(),
            "-C".into(),
            self.0.display().to_string(),
            "-c".into(),
            "core.attributesFile=/dev/null".into(),
        ]);
        words.extend(args.iter().map(ToString::to_string));
        shell.run("env", &words)
    }
}

impl Drop for MergeRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Compare whole trees with Git's built-in merge, independent of all user merge drivers
/// and attributes. An empty attribute source also excludes committed .gitattributes.
fn merge_tree_unchanged(shell: &dyn Shell, repo: &str, base: &str, branch: &str) -> Result<bool> {
    let source = git(
        shell,
        repo,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--show-object-format",
            "--git-path",
            "objects",
        ],
    )?;
    if !source.ok() {
        return Err(failed(repo, "locate merge objects", &source));
    }
    let fields: Vec<&str> = source.stdout.lines().collect();
    let [format, objects] = fields.as_slice() else {
        return Err(Error::Invalid(format!(
            "{repo}: invalid merge object store"
        )));
    };
    let tips = git(
        shell,
        repo,
        &["rev-parse", base, branch, &format!("{base}^{{tree}}")],
    )?;
    if !tips.ok() {
        return Err(failed(repo, "resolve merge trees", &tips));
    }
    let fields: Vec<&str> = tips.stdout.lines().collect();
    let [base, branch, tree] = fields.as_slice() else {
        return Err(Error::Invalid(format!("{repo}: invalid merge tips")));
    };
    let isolated = MergeRepo::new()?;
    let init = isolated.git(
        shell,
        &[
            "init",
            "--bare",
            "--quiet",
            "--template=",
            &format!("--object-format={format}"),
        ],
    )?;
    if !init.ok() {
        return Err(failed(repo, "initialize merge object store", &init));
    }
    // Git reads original objects through an alternate, but writes merge objects only here.
    std::fs::write(
        isolated.0.join("objects/info/alternates"),
        format!("{}\n", serde_json::to_string(objects)?),
    )?;
    let empty = isolated.git(
        shell,
        &["hash-object", "-t", "tree", "-w", "--", "/dev/null"],
    )?;
    if !empty.ok() {
        return Err(failed(repo, "create empty attribute source", &empty));
    }
    let merged = isolated.git(
        shell,
        &[
            &format!("--attr-source={}", empty.stdout.trim()),
            "merge-tree",
            "--write-tree",
            "--no-messages",
            base,
            branch,
        ],
    )?;
    match merged.status {
        0 => Ok(!tree.is_empty() && merged.stdout.trim() == *tree),
        1 => Ok(false),
        _ => Err(failed(repo, "merge trees", &merged)),
    }
}

/// One commit from `git log`, with what it changed against its first parent (limited to the
/// pathspec, if any).
struct Commit {
    id: String,
    parent: String,
    paths: BTreeSet<String>,
    changes: Changes,
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
        "--no-ext-diff",
        "--no-textconv",
        "--reverse",
        "--no-renames",
        "--raw",
        "--no-abbrev",
        "--diff-merges=first-parent",
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
            let changes = raw(fields);
            Some(Commit {
                id: header.next()?.to_string(),
                parent: header.next().unwrap_or_default().to_string(),
                paths: changes.keys().cloned().collect(),
                changes,
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
            let touched: BTreeSet<&String> =
                branch[start..end].iter().flat_map(|c| &c.paths).collect();
            let mut net = None;
            // Newest candidate first, as `git log` lists them without --reverse.
            for candidate in candidates.iter().rev() {
                if candidate.parent.is_empty()
                    || !touched.iter().all(|p| candidate.paths.contains(*p))
                {
                    continue;
                }
                // A run that reverts itself changes nothing, so no commit applies it.
                let net = match &net {
                    Some(net) => net,
                    None => net.insert(changes(
                        shell,
                        repo,
                        &points[start],
                        &points[end],
                        &touched,
                    )?),
                };
                if net.is_empty() {
                    break;
                }
                if applies(shell, repo, net, candidate)? {
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

/// A path's mode and object before and after, from `git diff --raw`. Git's zero id is absent.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Change {
    modes: (String, String),
    objects: (String, String),
}

type Changes = BTreeMap<String, Change>;

/// What `from..to` changed on `paths`.
fn changes(
    shell: &dyn Shell,
    repo: &str,
    from: &str,
    to: &str,
    paths: &BTreeSet<&String>,
) -> Result<Changes> {
    let pathspec: Vec<String> = paths.iter().map(|p| format!(":(literal){p}")).collect();
    let mut args = vec![
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--raw",
        "-z",
        "--no-renames",
        "--no-abbrev",
        from,
        to,
        "--",
    ];
    args.extend(pathspec.iter().map(String::as_str));
    let out = git(shell, repo, &args)?;
    if !out.ok() {
        return Err(failed(repo, &format!("list changes {from}..{to}"), &out));
    }
    Ok(raw(out.stdout.split('\0')))
}

/// Parse `--raw -z` records: a `:modes objects status` field, then the path.
fn raw<'a>(mut fields: impl Iterator<Item = &'a str>) -> Changes {
    let mut result = Changes::new();
    while let (Some(header), Some(path)) = (fields.next(), fields.next()) {
        let words: Vec<&str> = header
            .trim_start_matches('\n')
            .trim_start_matches(':')
            .split(' ')
            .collect();
        if let [old_mode, new_mode, old, new, _] = words[..] {
            result.insert(
                path.into(),
                Change {
                    modes: (old_mode.into(), new_mode.into()),
                    objects: (old.into(), new.into()),
                },
            );
        }
    }
    result
}

/// `commit` applies a run when it touches every path the run changed, and merging the run into
/// its parent yields `commit`'s content on those paths. Each path merges on its own: when the
/// parent holds the run's start or end the result is known, and otherwise both sides must be
/// files with one mode, merged with `git merge-file`. An addition, deletion, or mode change
/// that both sides make differently is a conflict, so the run does not apply there.
fn applies(shell: &dyn Shell, repo: &str, net: &Changes, commit: &Commit) -> Result<bool> {
    for (path, change) in net {
        // The commit does not change this path, so the merge result would have to be the
        // parent's content, which the run changes.
        let Some(theirs) = commit.changes.get(path) else {
            return Ok(false);
        };
        let start = (&change.modes.0, &change.objects.0);
        let end = (&change.modes.1, &change.objects.1);
        let parent = (&theirs.modes.0, &theirs.objects.0);
        let result = (&theirs.modes.1, &theirs.objects.1);
        let merged = if parent == start {
            end
        } else if parent == end {
            parent
        } else if [start, end, parent].iter().any(|(_, o)| absent(o))
            || start.0 != end.0
            || parent.0 != start.0
        {
            return Ok(false);
        } else {
            let out = git(
                shell,
                repo,
                &["merge-file", "--object-id", parent.1, start.1, end.1],
            )?;
            match out.status {
                0 => {
                    if (end.0, &out.stdout.trim().to_string()) != result {
                        return Ok(false);
                    }
                    continue;
                }
                // The number of conflicts, at most 127. An error is negative, so exit 255.
                1..=127 => return Ok(false),
                _ => return Err(failed(repo, &format!("merge {path}"), &out)),
            }
        };
        if merged != result {
            return Ok(false);
        }
    }
    Ok(true)
}

fn absent(object: &str) -> bool {
    object.bytes().all(|b| b == b'0')
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
                .join(format!("dogsled-integration-{}-{name}", std::process::id()));
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
        assert!(matches!(
            repo.check("fresh"),
            Some(Integrated::Ancestor { .. })
        ));
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
    fn a_squashed_deletion_and_addition_apply_and_a_different_addition_does_not() {
        let repo = Repo::new("added");
        repo.git(&["switch", "-q", "-c", "feat"]);
        repo.git(&["rm", "-q", "a"]);
        repo.commit("b", "moved\n", "feat moves a to b");
        repo.git(&["switch", "-q", "-c", "other", "main"]);
        repo.commit("b", "different\n", "other adds b");
        repo.git(&["switch", "-q", "main"]);
        repo.commit("x", "unrelated\n", "base moves");
        repo.git(&["merge", "-q", "--squash", "feat"]);
        repo.git(&["commit", "-q", "-m", "squash feat"]);
        assert!(applied(repo.check("feat").as_ref()));
        // `other` adds the same path with other content: an add/add conflict, not applied.
        assert_eq!(repo.check("other"), None);
    }

    #[test]
    fn merge_tree_finds_work_applied_across_commits_without_user_tools() {
        let repo = Repo::new("merge-tree");
        repo.commit(
            ".gitattributes",
            "a merge=custom diff=custom\n",
            "attributes",
        );
        repo.git(&["switch", "-q", "-c", "feat"]);
        std::fs::write(repo.0.join("b"), "new\n").unwrap();
        repo.commit("a", "1\n2\nthree\n", "two paths together");
        repo.git(&["switch", "-q", "main"]);
        repo.commit("a", "one\n2\nthree\n", "apply a with another change");
        repo.commit("b", "new\n", "apply b separately");
        // No single commit applies the branch, but merging it changes nothing. The
        // text merge must work even if a configured driver is missing or dishonest.
        repo.git(&["config", "diff.external", "dogsled-missing-diff"]);
        repo.git(&["config", "diff.custom.command", "dogsled-missing-diff"]);
        repo.git(&["config", "diff.custom.textconv", "dogsled-missing-diff"]);
        repo.git(&["config", "merge.default", "custom"]);
        for driver in ["dogsled-missing-merge", "false", "true"] {
            repo.git(&["config", "merge.custom.driver", driver]);
            let found = repo.check("feat").unwrap();
            assert_eq!(
                found,
                Integrated::MergeTree {
                    base: "main".into()
                }
            );
            assert_eq!(
                found.to_string(),
                "integrated into main by merge-tree (no changes)"
            );
        }
    }

    #[test]
    fn merge_tree_conflict_is_not_integrated_even_with_a_successful_user_driver() {
        let repo = Repo::new("merge-tree-conflict");
        repo.commit(".gitattributes", "a merge=custom\n", "attributes");
        repo.git(&["switch", "-q", "-c", "feat"]);
        repo.commit("a", "branch\n", "branch change");
        repo.git(&["switch", "-q", "main"]);
        repo.commit("a", "main\n", "different change");
        // A failed driver can print main's tree while reporting a conflict. The tree
        // alone is not proof. A successful driver that discards work is unsafe too.
        repo.git(&["config", "merge.custom.driver", "false"]);
        let merged = git(
            &Isolated,
            repo.path(),
            &["merge-tree", "--write-tree", "main", "feat"],
        )
        .unwrap();
        assert_eq!(merged.status, 1);
        assert_eq!(
            merged.stdout.lines().next().unwrap(),
            repo.git(&["rev-parse", "main^{tree}"])
        );
        assert_eq!(repo.check("feat"), None);
        repo.git(&["config", "merge.custom.driver", "true"]);
        assert_eq!(repo.check("feat"), None);
        // Attributes must not replace a real conflict with the built-in union driver.
        std::fs::write(repo.0.join(".git/info/attributes"), "a merge=union\n").unwrap();
        assert_eq!(repo.check("feat"), None);
    }

    #[test]
    fn merge_tree_errors_are_reported() {
        let repo = Repo::new("merge-tree-error");
        let result = merge_tree_unchanged(&Isolated, repo.path(), "main", "missing");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("resolve merge trees")
        );
    }

    #[test]
    fn merge_tree_that_readds_a_deleted_file_is_not_integrated() {
        let repo = Repo::new("merge-tree-readd");
        repo.git(&["switch", "-q", "-c", "feat"]);
        std::fs::write(repo.0.join("b"), "new\n").unwrap();
        repo.commit("c", "new\n", "add b and c");
        repo.git(&["switch", "-q", "main"]);
        repo.commit("b", "new\n", "apply b");
        repo.commit("c", "new\n", "apply c");
        repo.git(&["rm", "-q", "b"]);
        repo.git(&["commit", "-q", "-m", "delete b"]);
        let merged = repo.git(&["merge-tree", "--write-tree", "main", "feat"]);
        assert_ne!(merged, repo.git(&["rev-parse", "main^{tree}"]));
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
