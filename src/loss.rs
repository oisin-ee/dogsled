//! The single definition of loss: no unique checkout content and a tip held by the default
//! branch or by remote refs refreshed in this pass. Holds on otherwise safe work are separate.

use crate::error::{Error, Result};
use crate::integration::Integrated;
use crate::shell::Shell;
use serde::{Deserialize, Serialize};

fn git(shell: &dyn Shell, repo: &str, args: &[&str]) -> Result<crate::shell::Output> {
    let mut words = vec!["-C".into(), repo.into()];
    words.extend(args.iter().map(|s| (*s).into()));
    shell.run("git", &words)
}

/// Remote evidence belongs to one repository and one pass; callers cannot manufacture it.
#[derive(Debug)]
pub struct Evidence {
    repo: String,
    refreshed: bool,
    proofs: crate::integration::Proofs,
    /// A local branch retained by this operation. Never authorizes deleting that branch.
    kept_branch: Option<String>,
    pub warning: Option<String>,
}

impl Evidence {
    pub fn fetch(shell: &dyn Shell, repo: &str) -> Self {
        let result = git(shell, repo, &["fetch", "--all", "--prune"]);
        let warning = match result {
            Ok(out) if out.ok() => None,
            Ok(out) => Some(format!(
                "{repo}: fetch --prune failed: {}; only the default branch counts",
                out.stderr.trim()
            )),
            Err(error) => Some(format!(
                "{repo}: fetch --prune failed: {error}; only the default branch counts"
            )),
        };
        Self {
            repo: repo.into(),
            refreshed: warning.is_none(),
            kept_branch: None,
            proofs: crate::integration::Proofs::default(),
            warning,
        }
    }

    /// Board observation does not refresh remote refs or authorize removal.
    pub fn default_only(repo: &str) -> Self {
        Self {
            repo: repo.into(),
            refreshed: false,
            kept_branch: None,
            proofs: crate::integration::Proofs::default(),
            warning: None,
        }
    }

    /// Removing a checkout can retain its local branch, unlike deleting the branch itself.
    pub fn keeping_branch(repo: &str, branch: &str) -> Self {
        Self {
            kept_branch: Some(format!("refs/heads/{branch}")),
            ..Self::default_only(repo)
        }
    }

    pub fn keeps_branch(&self) -> bool {
        self.kept_branch.is_some()
    }

    pub fn assess(
        &self,
        shell: &dyn Shell,
        tip: &str,
        checkout: Option<&str>,
    ) -> Result<Assessment> {
        let changes = checkout
            .map(|path| changes(shell, path))
            .transpose()?
            .unwrap_or_default();
        let integrated = crate::integration::check_cached(shell, &self.repo, tip, &self.proofs)?;
        let retained = self
            .kept_branch
            .as_ref()
            .map(|branch| {
                git(
                    shell,
                    &self.repo,
                    &["merge-base", "--is-ancestor", tip, branch],
                )
                .map(|out| out.ok())
            })
            .transpose()?
            .unwrap_or(false);
        let held = if let Some(integrated) = integrated {
            Some(Held::Default { integrated })
        } else if retained {
            Some(Held::Branch)
        } else if self.refreshed {
            let out = git(
                shell,
                &self.repo,
                &["rev-list", "--count", tip, "--not", "--remotes"],
            )?;
            if !out.ok() {
                return Err(Error::Invalid(format!(
                    "{}: cannot count remote-held commits: {}",
                    self.repo,
                    out.stderr.trim()
                )));
            }
            let count: u64 = out
                .stdout
                .trim()
                .parse()
                .map_err(|_| Error::Invalid(format!("{}: invalid commit count", self.repo)))?;
            (count == 0).then_some(Held::Remote)
        } else {
            None
        };
        let dirty = lossy_paths(changes, held.is_some());
        Ok(Assessment { dirty, held })
    }

    /// Git refuses deletion-only checkouts too. Recover their held content before an ordinary,
    /// unforced removal. Other dirt refuses recovery without changing the index or checkout.
    pub fn restore_deletions(&self, shell: &dyn Shell, checkout: &str) -> Result<()> {
        let changes = changes(shell, checkout)?;
        if !changes.iter().any(|change| change.deleted) {
            return Ok(());
        }
        let head = git(shell, checkout, &["rev-parse", "HEAD"])?;
        if !head.ok() || head.stdout.trim().is_empty() {
            return Err(Error::Invalid(format!("cannot read HEAD in {checkout}")));
        }
        let loss = self.assess(shell, head.stdout.trim(), Some(checkout))?;
        if changes.iter().any(|change| !change.deleted) || !loss.loses_nothing() {
            return Err(Error::Invalid(format!(
                "refuse {checkout}: cannot recover deletions with other dirt or unheld commits"
            )));
        }
        let mut args = vec!["--literal-pathspecs", "checkout", head.stdout.trim(), "--"];
        args.extend(changes.iter().map(|change| change.path.as_str()));
        let out = git(shell, checkout, &args)?;
        crate::shell::checked(
            "git",
            &args.iter().map(ToString::to_string).collect::<Vec<_>>(),
            out,
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Held {
    Default {
        integrated: Integrated,
    },
    Remote,
    /// This operation keeps the local branch containing the tip.
    Branch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assessment {
    pub dirty: Vec<String>,
    pub held: Option<Held>,
}

impl Assessment {
    pub fn loses_nothing(&self) -> bool {
        self.dirty.is_empty() && self.held.is_some()
    }

    pub fn landed(&self) -> bool {
        matches!(self.held, Some(Held::Default { .. }))
    }

    pub fn detail(&self) -> String {
        let held = match &self.held {
            Some(Held::Default { integrated }) => integrated.to_string(),
            Some(Held::Remote) => "every commit is on a refreshed remote ref".into(),
            Some(Held::Branch) => "every commit is on the retained local branch".into(),
            None => "commits are not held by the default branch or a refreshed remote ref".into(),
        };
        if self.dirty.is_empty() {
            held
        } else {
            format!(
                "{held}; uncommitted changes ({} paths): {}",
                self.dirty.len(),
                self.dirty
                    .iter()
                    .take(10)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

/// Porcelain's NUL form preserves spaces, newlines, and rename paths without unquoting.
pub fn dirty_paths(shell: &dyn Shell, checkout: &str, held: bool) -> Result<Vec<String>> {
    Ok(lossy_paths(changes(shell, checkout)?, held))
}

fn lossy_paths(changes: Vec<Change>, held: bool) -> Vec<String> {
    changes
        .into_iter()
        .filter(|change| !held || !change.deleted)
        .map(|change| change.path)
        .collect()
}

struct Change {
    path: String,
    deleted: bool,
}

fn changes(shell: &dyn Shell, checkout: &str) -> Result<Vec<Change>> {
    let path = std::path::Path::new(checkout);
    if !path.join(".git").exists() {
        return Err(Error::Invalid(format!("{checkout}: not a worktree root")));
    }
    let root = git(shell, checkout, &["rev-parse", "--show-toplevel"])?;
    let top = root.stdout.strip_suffix('\n').unwrap_or(&root.stdout);
    if !root.ok() || top.is_empty() || !crate::worktree::same(checkout, top) {
        return Err(Error::Invalid(format!("{checkout}: not a worktree root")));
    }
    let out = git(
        shell,
        checkout,
        &["status", "--porcelain", "--untracked-files=all", "-z"],
    )?;
    if !out.ok() {
        return Err(Error::Invalid(format!(
            "cannot check worktree {checkout}: {}",
            out.stderr.trim()
        )));
    }
    let mut paths = Vec::new();
    let mut fields = out.stdout.split('\0').filter(|s| !s.is_empty());
    while let Some(field) = fields.next() {
        let Some(path) = field.get(3..) else {
            return Err(Error::Invalid(format!(
                "{checkout}: invalid porcelain status"
            )));
        };
        paths.push(Change {
            path: path.to_string(),
            deleted: matches!(&field[..2], " D" | "D "),
        });
        if field[..2].contains(['R', 'C']) {
            fields.next();
        }
    }
    Ok(paths)
}
