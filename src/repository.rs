//! A repository's own `dogsled.toml`: where its linked worktrees go, and the commands that set
//! one up after creation and tear it down before removal.

use crate::error::{Error, Result};
use crate::shell::{Shell, quote};
use serde::Deserialize;
use std::path::{Component, Path};

/// The file at the root of the main checkout.
pub const FILE: &str = "dogsled.toml";

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub worktree: Worktree,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Worktree {
    /// Where linked worktrees go, relative to the main checkout.
    pub dir: String,
    /// Run in a new worktree after creation, in order. A failure removes the worktree.
    pub setup: Vec<String>,
    /// Run in a worktree before removal, in order. A failure keeps the worktree.
    pub teardown: Vec<String>,
}

impl Default for Worktree {
    fn default() -> Self {
        Self {
            dir: ".worktrees".into(),
            setup: Vec::new(),
            teardown: Vec::new(),
        }
    }
}

/// The values a command may name. Each is substituted as one shell word.
#[derive(Debug, Clone, Copy)]
pub struct Values<'a> {
    pub main: &'a Path,
    pub path: &'a str,
    pub branch: &'a str,
}

/// The settings of the repository whose main checkout is `main`. A missing file means
/// defaults; an unreadable or invalid file is an error.
pub fn load(main: &Path) -> Result<Settings> {
    let file = main.join(FILE);
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(e) => return Err(e.into()),
    };
    parse(&text).map_err(|detail| Error::Invalid(format!("{}: {detail}", file.display())))
}

fn parse(text: &str) -> std::result::Result<Settings, String> {
    let settings: Settings = toml::from_str(text).map_err(|e| e.to_string())?;
    let dir = Path::new(&settings.worktree.dir);
    if settings.worktree.dir.is_empty()
        || !dir
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(format!(
            "worktree.dir {:?} must be a relative path inside the main checkout",
            settings.worktree.dir
        ));
    }
    let probe = Values {
        main: Path::new(""),
        path: "",
        branch: "",
    };
    for (key, commands) in [
        ("setup", &settings.worktree.setup),
        ("teardown", &settings.worktree.teardown),
    ] {
        for command in commands {
            render(command, probe).map_err(|e| format!("worktree.{key}: {e}"))?;
        }
    }
    Ok(settings)
}

/// Substitute `{main}`, `{path}`, and `{branch}`, each quoted for the shell. Any other `{…}` is
/// an error, so a typo never runs as a literal.
pub fn render(command: &str, values: Values<'_>) -> std::result::Result<String, String> {
    let mut output = String::new();
    let mut rest = command;
    while let Some(start) = rest.find('{') {
        output.push_str(&rest[..start]);
        let end = rest[start..]
            .find('}')
            .map(|end| start + end)
            .ok_or_else(|| format!("unclosed placeholder {}", &rest[start..]))?;
        let value = match &rest[start..=end] {
            "{main}" => values.main.display().to_string(),
            "{path}" => values.path.to_string(),
            "{branch}" => values.branch.to_string(),
            other => return Err(format!("unknown placeholder {other} in {command:?}")),
        };
        output.push_str(&quote(&value));
        rest = &rest[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

/// Run `commands` in order with `sh -c` in the worktree, stopping at the first failure.
pub fn run(shell: &dyn Shell, stage: &str, commands: &[String], values: Values<'_>) -> Result<()> {
    for command in commands {
        let rendered = render(command, values).map_err(Error::Invalid)?;
        let script = format!("cd {} && {rendered}", quote(values.path));
        let out = shell.run_setup("sh", &["-c".into(), script])?;
        if !out.ok() {
            let lines: Vec<&str> = out
                .stderr
                .lines()
                .chain(out.stdout.lines())
                .filter(|l| !l.trim().is_empty())
                .collect();
            let tail = lines[lines.len().saturating_sub(4)..].join(" | ");
            return Err(Error::Invalid(format!(
                "{stage} command {command:?} exited {} in {}: {}",
                out.status,
                values.path,
                if tail.is_empty() { "no output" } else { &tail }
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values() -> Values<'static> {
        Values {
            main: Path::new("/r/main"),
            path: "/r/main/.worktrees/a b",
            branch: "feat/x",
        }
    }

    #[test]
    fn missing_or_empty_file_is_defaults() {
        assert_eq!(parse("").unwrap(), Settings::default());
        assert_eq!(Settings::default().worktree.dir, ".worktrees");
        assert_eq!(
            load(Path::new("/nonexistent/repository")).unwrap(),
            Settings::default()
        );
    }

    #[test]
    fn placeholders_are_quoted_words_and_others_are_errors() {
        assert_eq!(
            render("node {main}/x.ts --at {path} {branch}", values()).unwrap(),
            "node /r/main/x.ts --at '/r/main/.worktrees/a b' feat/x"
        );
        assert!(
            render("echo {base}", values())
                .unwrap_err()
                .contains("unknown placeholder {base}")
        );
        assert!(render("echo {main", values()).is_err());
        let error = parse("[worktree]\nteardown = [\"echo {repo}\"]\n").unwrap_err();
        assert!(
            error.contains("worktree.teardown") && error.contains("{repo}"),
            "{error}"
        );
    }

    #[test]
    fn unknown_keys_and_escaping_dirs_are_errors() {
        assert!(parse("[worktree]\nhooks = []\n").is_err());
        assert!(parse("[other]\n").is_err());
        for dir in ["/abs", "../out", "a/../../b", ""] {
            assert!(
                parse(&format!("[worktree]\ndir = {dir:?}\n")).is_err(),
                "{dir}"
            );
        }
        let settings = parse("[worktree]\ndir = \"trees/agents\"\nsetup = [\"true\"]\n").unwrap();
        assert_eq!(settings.worktree.dir, "trees/agents");
        assert_eq!(settings.worktree.setup, ["true"]);
        assert_eq!(settings.worktree.teardown, Vec::<String>::new());
    }
}
