//! Repository identity and home-relative checkout provisioning.

use crate::diagnostic::{Cause, Context, Diagnostic, Operation};
use crate::error::{Error, Result};
use crate::hosts::{Fleet, Host};
use crate::prepare;
use crate::shell::Shell;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    pub path: String,
    pub origin: String,
    pub relative: String,
    pub working_root: String,
}

pub fn discover(
    shell: &dyn Shell,
    cwd: &str,
    home: &str,
    required: bool,
) -> Result<Option<Repository>> {
    let check = shell.run(
        "git",
        &[
            "-C".into(),
            cwd.into(),
            "rev-parse".into(),
            "--is-inside-work-tree".into(),
        ],
    )?;
    if !check.ok() {
        if required {
            return Err(Error::Invalid(format!(
                "not a repository: {cwd}: {}",
                check.stderr.trim()
            )));
        }
        return Ok(None);
    }
    let path = prepare::main_checkout(shell, cwd)?;
    let origin = shell
        .run_ok(
            "git",
            &[
                "-C".into(),
                path.display().to_string(),
                "remote".into(),
                "get-url".into(),
                "origin".into(),
            ],
        )?
        .stdout
        .trim()
        .to_owned();
    if origin.is_empty() {
        return Err(Error::Invalid("repository origin is empty".into()));
    }
    let home = Path::new(home)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(home));
    let relative = path
        .strip_prefix(home)
        .unwrap_or(&path)
        .display()
        .to_string();
    let working_root = shell
        .run_ok(
            "git",
            &[
                "-C".into(),
                cwd.into(),
                "rev-parse".into(),
                "--show-toplevel".into(),
            ],
        )?
        .stdout
        .trim()
        .to_owned();
    Ok(Some(Repository {
        path: path.display().to_string(),
        origin,
        relative,
        working_root,
    }))
}

pub fn ensure(fleet: &Fleet, host: &Host, repo: &Repository) -> Result<String> {
    if host.is_local() {
        return local(
            fleet.shell,
            &fleet.home,
            &host.label,
            &repo.path,
            &repo.origin,
        );
    }
    fleet.remote_setup(
        host,
        &[
            "checkout".into(),
            "--path".into(),
            repo.relative.clone(),
            "--origin".into(),
            repo.origin.clone(),
            "--host-label".into(),
            host.label.clone(),
        ],
    )
}

/// Host and repository path that a git remote URL names. The ssh, scp-like, and
/// https spellings of one repository give the same identity. A local path has none.
fn parts(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let (authority, path) = if let Some((scheme, rest)) = url.split_once("://") {
        if !matches!(
            scheme,
            "https" | "http" | "ssh" | "git" | "git+ssh" | "ssh+git"
        ) {
            return None;
        }
        rest.split_once('/')?
    } else {
        // scp-like `[user@]host:path`. Git treats a slash before the colon as a local path.
        let (authority, path) = url.split_once(':')?;
        if authority.contains('/') {
            return None;
        }
        (authority, path)
    };
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = host.split_once(':').map_or(host, |(h, _)| h);
    let path = path.trim_matches('/');
    let path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_end_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some((host.to_ascii_lowercase(), path.to_owned()))
}

/// Canonical `host/path` identity of a remote URL: host lowercased, path case kept, no `.git`.
/// Matches `repositoryIdentity` in `pi/unit-protocol.ts`; both read
/// `tests/fixtures/repository-identity.json`.
pub fn identity(url: &str) -> Option<String> {
    parts(url).map(|(host, path)| format!("{host}/{path}"))
}

/// Whether two origin URLs name the same repository. URLs without a host and
/// repository path must match exactly.
fn same_repository(a: &str, b: &str) -> bool {
    match (parts(a), parts(b)) {
        (Some(a), Some(b)) => a == b,
        _ => a.trim() == b.trim(),
    }
}

pub fn local(
    shell: &dyn Shell,
    home: &str,
    host: &str,
    path: &str,
    origin: &str,
) -> Result<String> {
    let path = Path::new(home).join(path);
    let display = path.display().to_string();
    if path.exists() {
        let actual = shell.run(
            "git",
            &[
                "-C".into(),
                display.clone(),
                "remote".into(),
                "get-url".into(),
                "origin".into(),
            ],
        )?;
        let root = shell.run(
            "git",
            &[
                "-C".into(),
                display.clone(),
                "rev-parse".into(),
                "--show-toplevel".into(),
            ],
        )?;
        let same_root = root.ok()
            && Path::new(root.stdout.trim()).canonicalize().ok() == path.canonicalize().ok();
        if !actual.ok() || !same_repository(actual.stdout.trim(), origin) || !same_root {
            return Err(
                Error::Invalid(format!("{host}: checkout identity mismatch")).with_diagnostic(
                    Diagnostic::new(
                        Operation::Checkout,
                        Cause::CheckoutIdentityMismatch,
                        Context::new(None, None, Some(&display)),
                    ),
                ),
            );
        }
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let cloned = shell
            .run_setup(
                "git",
                &["clone".into(), "--".into(), origin.into(), display.clone()],
            )
            .map_err(|error| {
                error.with_diagnostic(Diagnostic::new(
                    Operation::Checkout,
                    Cause::CloneFailed,
                    Context::new(None, None, Some(&display)),
                ))
            })?;
        if !cloned.ok() {
            return Err(Error::Refused {
                code: crate::error::Refusal::CloneFailed,
                detail: format!("{host}: the peer could not clone the repository"),
            }
            .with_diagnostic(Diagnostic::new(
                Operation::Checkout,
                Cause::CloneFailed,
                Context::new(None, None, Some(&display)),
            )));
        }
        trust_new_checkout(shell, &path).map_err(|error| {
            error.with_diagnostic(Diagnostic::new(
                Operation::Checkout,
                Cause::SetupFailed,
                Context::new(None, None, Some(&display)),
            ))
        })?;
    }
    Ok(display)
}

/// Only the new checkout may acquire trust. Never trust ancestors or all configs.
fn trust_new_checkout(shell: &dyn Shell, path: &Path) -> Result<()> {
    let args = [
        "-C".into(),
        path.display().to_string(),
        "config".into(),
        "ls".into(),
        "--json".into(),
    ];
    let checked = shell.run_setup("mise", &args)?;
    if checked.ok() {
        return Ok(());
    }
    if !checked.stderr.to_lowercase().contains("trust") {
        return Err(Error::Invalid("new checkout setup failed".into()));
    }
    let config = [
        "mise.toml",
        ".mise.toml",
        ".config/mise.toml",
        ".config/mise/config.toml",
    ]
    .into_iter()
    .map(|p| path.join(p))
    .find(|p| p.is_file())
    .ok_or_else(|| Error::Invalid("untrusted config outside new checkout".into()))?;
    shell.run_setup_ok("mise", &["trust".into(), config.display().to_string()])?;
    shell.run_setup_ok("mise", &args)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{identity, same_repository};

    #[test]
    fn identity_matches_the_shared_fixture() {
        let cases: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../tests/fixtures/repository-identity.json"))
                .unwrap();
        assert_eq!(cases.len(), 15);
        for case in cases {
            let origin = case["origin"].as_str().unwrap();
            assert_eq!(
                identity(origin).as_deref(),
                case["identity"].as_str(),
                "{origin:?}"
            );
        }
    }

    #[test]
    fn ssh_and_https_spellings_name_one_repository() {
        let https = "https://github.com/oisin-ee/rondo.git";
        for other in [
            "git@github.com:oisin-ee/rondo.git",
            "git@github.com:oisin-ee/rondo",
            "ssh://git@github.com/oisin-ee/rondo.git",
            "ssh://git@github.com:22/oisin-ee/rondo.git/",
            "https://user@GitHub.com/oisin-ee/rondo",
            "github.com:oisin-ee/rondo.git",
        ] {
            assert!(same_repository(https, other), "{other}");
        }
    }

    #[test]
    fn different_repositories_and_local_paths_stay_distinct() {
        let https = "https://github.com/oisin-ee/rondo.git";
        for other in [
            "git@github.com:oisin-ee/dogsled.git",
            "git@github.com:someone/rondo.git",
            "git@gitlab.com:oisin-ee/rondo.git",
            "/srv/git/oisin-ee/rondo.git",
            "file:///github.com/oisin-ee/rondo.git",
        ] {
            assert!(!same_repository(https, other), "{other}");
        }
        assert!(same_repository("/srv/git/rondo.git", "/srv/git/rondo.git"));
        assert!(!same_repository("/srv/git/rondo.git", "/srv/git/rondo"));
    }
}
