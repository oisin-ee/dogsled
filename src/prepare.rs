//! Prepare work in the repository checkout, never in the caller's worktree.

use crate::config;
use crate::error::{Error, Result};
use crate::hosts::{Fleet, Host};
use crate::shell::{Shell, quote};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prepared {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

pub fn main_checkout(shell: &dyn Shell, repo: &str) -> Result<PathBuf> {
    let out = shell.run_ok(
        "git",
        &[
            "-C".into(),
            repo.into(),
            "rev-parse".into(),
            "--path-format=absolute".into(),
            "--git-common-dir".into(),
        ],
    )?;
    let path = Path::new(out.stdout.trim());
    if path.as_os_str().is_empty() {
        return Err(Error::Invalid("git returned no checkout".into()));
    }
    Ok(if path.file_name().is_some_and(|name| name == ".git") {
        path.parent()
            .ok_or_else(|| Error::Invalid("git returned no checkout parent".into()))?
            .to_path_buf()
    } else {
        path.to_path_buf()
    })
}

pub fn run(
    fleet: &Fleet,
    host: &Host,
    repo: &str,
    branch: &str,
    base: Option<&str>,
) -> Result<Prepared> {
    let checkout = main_checkout(fleet.shell, repo)?;
    if host.is_local() {
        return local(
            fleet.shell,
            fleet.policy,
            &checkout.display().to_string(),
            branch,
            base,
        );
    }
    let home = Path::new(&fleet.home);
    let canonical = home.canonicalize().ok();
    let remote = checkout
        .strip_prefix(home)
        .ok()
        .or_else(|| {
            canonical
                .as_deref()
                .and_then(|home| checkout.strip_prefix(home).ok())
        })
        .unwrap_or(&checkout)
        .display()
        .to_string();
    let mut args = vec![
        "prepare".into(),
        "--repo".into(),
        remote,
        "--branch".into(),
        branch.into(),
    ];
    if let Some(base) = base {
        args.extend(["--base".into(), base.into()]);
    }
    fleet.remote_setup(host, &args)
}

pub fn local(
    shell: &dyn Shell,
    policy: &config::Policy,
    repo: &str,
    branch: &str,
    base: Option<&str>,
) -> Result<Prepared> {
    let checkout = main_checkout(shell, repo)?;
    let argv = render(&policy.hooks.prepare, branch, base)?;
    let command = format!(
        "cd {} && exec {}",
        quote(&checkout.display().to_string()),
        argv.iter().map(|w| quote(w)).collect::<Vec<_>>().join(" ")
    );
    let out = shell.run_setup_ok("sh", &["-c".into(), command])?;
    parse_output(&out.stdout)
}

/// Parse the template once. Substituted values are data, not templates.
fn render(template: &[String], branch: &str, base: Option<&str>) -> Result<Vec<String>> {
    if template.is_empty() {
        return Err(Error::Invalid("hooks.prepare is empty".into()));
    }
    template
        .iter()
        .filter(|word| !(word.as_str() == "{base}" && base.is_none()))
        .map(|word| {
            let mut output = String::new();
            let mut rest = word.as_str();
            while let Some(start) = rest.find('{') {
                output.push_str(&rest[..start]);
                let end = rest[start..]
                    .find('}')
                    .map(|end| start + end)
                    .ok_or_else(|| {
                        Error::Invalid(format!("unclosed hook placeholder {}", &rest[start..]))
                    })?;
                let placeholder = &rest[start..=end];
                output.push_str(match placeholder {
                    "{branch}" => branch,
                    "{base}" => base.unwrap_or(""),
                    _ => {
                        return Err(Error::Invalid(format!(
                            "unknown hook placeholder {placeholder}"
                        )));
                    }
                });
                rest = &rest[end + 1..];
            }
            output.push_str(rest);
            Ok(output)
        })
        .collect()
}

pub fn parse_output(stdout: &str) -> Result<Prepared> {
    for text in std::iter::once(stdout.trim()).chain(stdout.lines().rev().map(str::trim)) {
        if let Ok(prepared) = serde_json::from_str::<Prepared>(text)
            && (prepared.pane_id.is_some() || prepared.path.is_some())
        {
            return Ok(prepared);
        }
    }
    Err(Error::Hook {
        detail: stdout.trim().lines().last().unwrap_or("no output").into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Debug)]
    struct TimedHook {
        elapsed: std::time::Duration,
        budget: crate::budget::Budget,
    }

    impl Shell for TimedHook {
        fn budget(&self) -> crate::budget::Budget {
            self.budget
        }

        fn run(&self, program: &str, args: &[String]) -> Result<crate::shell::Output> {
            if program == "git" {
                return Ok(crate::shell::Output {
                    status: 0,
                    stdout: "/repo/.git".into(),
                    stderr: String::new(),
                });
            }
            self.run_bounded(program, args, self.budget.remaining(program)?)
        }

        fn run_bounded(
            &self,
            program: &str,
            args: &[String],
            timeout: std::time::Duration,
        ) -> Result<crate::shell::Output> {
            // Inject elapsed child time rather than sleep for production limits.
            if self.elapsed > timeout {
                return Err(Error::Deadline(format!("{program} {}", args.join(" "))));
            }
            Ok(crate::shell::Output {
                status: 0,
                stdout: "{\"path\":\"/repo/tree\"}".into(),
                stderr: String::new(),
            })
        }
    }

    #[test]
    fn prepare_uses_setup_limit_and_honors_shorter_command_deadline() {
        use crate::budget::{Budget, CALL_LIMIT, SETUP_LIMIT};
        use std::time::{Duration, Instant};
        let policy = config::Policy::default();
        let shell = TimedHook {
            elapsed: CALL_LIMIT + Duration::from_secs(1),
            budget: Budget::default(),
        };
        assert_eq!(
            local(&shell, &policy, "/repo", "feature", None)
                .unwrap()
                .path
                .as_deref(),
            Some("/repo/tree")
        );
        let ledger = crate::ledger::Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()));
        let host = Host {
            id: "peer".into(),
            label: "peer".into(),
            target: Some("peer-ssh".into()),
        };
        let fleet = Fleet {
            shell: &shell,
            ledger: &ledger,
            policy: &policy,
            home: "/home".into(),
            hosts: vec![host.clone()],
        };
        assert_eq!(
            run(&fleet, &host, "/repo", "feature", None)
                .unwrap()
                .path
                .as_deref(),
            Some("/repo/tree")
        );
        let shell = TimedHook {
            elapsed: SETUP_LIMIT + Duration::from_secs(1),
            budget: Budget::default(),
        };
        let error = local(&shell, &policy, "/repo", "feature", None).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("deadline reached: sh -c cd /repo && exec mise")
        );
        let shell = TimedHook {
            elapsed: CALL_LIMIT + Duration::from_secs(1),
            budget: Budget::until(Instant::now() + CALL_LIMIT),
        };
        assert!(matches!(
            local(&shell, &policy, "/repo", "feature", None),
            Err(Error::Deadline(_))
        ));
    }

    #[test]
    fn default_template_and_embedded_values_are_literal() {
        let template = config::Policy::default().hooks.prepare;
        assert_eq!(
            render(&template, "feat/{base}", Some("main/{branch}")).unwrap(),
            [
                "mise",
                "run",
                "-q",
                "agent:worktree",
                "--",
                "feat/{base}",
                "main/{branch}"
            ]
        );
        assert_eq!(
            render(&template, "feat/{base}", None).unwrap(),
            ["mise", "run", "-q", "agent:worktree", "--", "feat/{base}"]
        );
        assert_eq!(
            render(&["prefix:{base}".into(), String::new()], "b", None).unwrap(),
            ["prefix:", ""]
        );
        assert!(
            render(&["{repo}".into()], "b", None)
                .unwrap_err()
                .to_string()
                .contains("{repo}")
        );
        assert!(render(&["{unclosed".into()], "b", None).is_err());
    }

    #[test]
    fn prepare_accepts_path_or_pane_and_rejects_empty_results() {
        assert_eq!(
            parse_output("progress\n{\"path\":\"/repo/work\"}\n")
                .unwrap()
                .path
                .as_deref(),
            Some("/repo/work")
        );
        assert_eq!(
            parse_output("{\"pane_id\":\"w1:p1\"}")
                .unwrap()
                .pane_id
                .as_deref(),
            Some("w1:p1")
        );
        assert!(parse_output("{}").is_err());
    }
}
