//! The single definition of loss: a clean checkout and a tip held by the default branch
//! or by remote refs refreshed in this pass. Holds on otherwise safe work are separate.

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
            warning,
        }
    }

    /// Board observation does not refresh remote refs or authorize removal.
    pub fn default_only(repo: &str) -> Self {
        Self {
            repo: repo.into(),
            refreshed: false,
            warning: None,
        }
    }

    pub fn assess(
        &self,
        shell: &dyn Shell,
        tip: &str,
        checkout: Option<&str>,
    ) -> Result<Assessment> {
        let dirty = checkout
            .map(|path| dirty_paths(shell, path))
            .transpose()?
            .unwrap_or_default();
        let integrated = match tip.strip_prefix("refs/heads/") {
            Some(branch) => crate::integration::check(shell, &self.repo, branch)?,
            None => crate::integration::check_ref(shell, &self.repo, tip)?,
        };
        if matches!(integrated, Some(Integrated::Deleted)) {
            return Err(Error::Invalid(format!(
                "{}: tip {tip} disappeared",
                self.repo
            )));
        }
        let held = if let Some(integrated) = integrated {
            Some(Held::Default { integrated })
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
        Ok(Assessment { dirty, held })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Held {
    Default { integrated: Integrated },
    Remote,
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
pub fn dirty_paths(shell: &dyn Shell, checkout: &str) -> Result<Vec<String>> {
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
        paths.push(path.to_string());
        if field[..2].contains(['R', 'C']) {
            fields.next();
        }
    }
    Ok(paths)
}
