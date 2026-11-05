//! Host-owned main checkout discovery and provisioning for GitHub slugs.

use super::CreateContext;
use crate::checkout::Repository;
use crate::error::{Error, Result};
use std::path::Path;

pub(super) fn validate(slug: &str, prompt: &str) -> Result<()> {
    let valid_part = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    if !matches!(slug.split_once('/'), Some((owner, name)) if valid_part(owner) && valid_part(name))
    {
        return Err(Error::Invalid(
            "repository must be a GitHub slug owner/name".into(),
        ));
    }
    if prompt.trim().is_empty() {
        return Err(Error::Invalid("the first prompt is empty".into()));
    }
    Ok(())
}

fn matching(context: &CreateContext<'_>, path: &Path, origin: &str) -> Result<Option<Repository>> {
    // A linked worktree is not a candidate main checkout. An unrelated Git repository
    // without an origin does not stop the search through the rest of ~/dev.
    if !path.join(".git").is_dir() {
        return Ok(None);
    }
    let url = context.shell.run(
        "git",
        &[
            "-C".into(),
            path.display().to_string(),
            "remote".into(),
            "get-url".into(),
            "origin".into(),
        ],
    )?;
    if !url.ok() || !crate::repos::same_origin(url.stdout.trim(), origin) {
        return Ok(None);
    }
    let found = crate::checkout::discover(
        context.shell,
        &path.display().to_string(),
        context.home,
        true,
    )?;
    Ok(found.filter(|repo| std::fs::canonicalize(&repo.path).ok() == path.canonicalize().ok()))
}

pub(super) fn ensure(context: &CreateContext<'_>, slug: &str) -> Result<Repository> {
    let origin = format!("git@github.com:{slug}.git");
    let dev = Path::new(context.home).join("dev");
    // Serialize destination choice with clone, including different owners of the same name.
    context.ledger.with_lock("lead-checkout", || {
        std::fs::create_dir_all(&dev)?;
        let mut paths = std::fs::read_dir(&dev)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        paths.sort();
        for path in paths {
            if let Some(repo) = matching(context, &path, &origin)? {
                return Ok(repo);
            }
        }
        let (owner, name) = slug
            .split_once('/')
            .ok_or_else(|| Error::Invalid("invalid repository slug".into()))?;
        let preferred = dev.join(name);
        let path = if preferred.try_exists()? || preferred.symlink_metadata().is_ok() {
            dev.join(format!("{owner}-{name}"))
        } else {
            preferred
        };
        if path.try_exists()? || path.symlink_metadata().is_ok() {
            return Err(Error::Invalid(
                "lead clone destination is already occupied".into(),
            ));
        }
        let cloned = context.shell.run_setup(
            "git",
            &["clone".into(), origin.clone(), path.display().to_string()],
        )?;
        if !cloned.ok() {
            // Git stderr can contain credentials. It must not reach a remote caller.
            return Err(Error::Invalid("lead repository clone failed".into()));
        }
        matching(context, &path, &origin)?.ok_or_else(|| {
            Error::Invalid("cloned lead checkout does not match its repository".into())
        })
    })
}
