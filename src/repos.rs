//! The configured repository list and comment-preserving edits to host policy.

use std::path::{Path, PathBuf};

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, Value, value};

use crate::config::{self, Policy, Repo};
use crate::error::{Error, Result};
use crate::shell::Shell;

fn read(config_path: &Path) -> Result<(Policy, DocumentMut)> {
    let text = match std::fs::read_to_string(config_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let policy = config::parse(&text).map_err(|source| Error::Config {
        path: config_path.display().to_string(),
        source,
    })?;
    let document = text.parse::<DocumentMut>().map_err(|error| {
        Error::Invalid(format!("cannot edit {}: {error}", config_path.display()))
    })?;
    Ok((policy, document))
}

fn write(config_path: &Path, document: &DocumentMut) -> Result<()> {
    if let Some(parent) = config_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    // Write through the selected path, rather than replacing a possible symlink.
    std::fs::write(config_path, document.to_string())?;
    Ok(())
}

fn same_origin(left: &str, right: &str) -> bool {
    match (
        crate::checkout::identity(left),
        crate::checkout::identity(right),
    ) {
        (Some(left), Some(right)) => left == right,
        _ => left == right,
    }
}

pub fn add(shell: &dyn Shell, home: &str, config_path: &Path, dir: &str) -> Result<Repo> {
    let repository = crate::checkout::discover(shell, dir, home, true)?
        .ok_or_else(|| Error::Invalid(format!("not a repository: {dir}")))?;
    if Path::new(&repository.relative).is_absolute() {
        return Err(Error::Invalid(format!(
            "repository is outside the home directory: {}",
            repository.path
        )));
    }
    let (policy, mut document) = read(config_path)?;
    if policy
        .repos
        .iter()
        .any(|repo| same_origin(&repo.origin, &repository.origin))
    {
        return Err(Error::Invalid(format!(
            "repository is already configured: {}",
            repository.origin
        )));
    }
    let repo = Repo {
        path: format!("~/{}", repository.relative),
        origin: repository.origin,
    };
    let mut table = Table::new();
    table.insert("path", value(&repo.path));
    table.insert("origin", value(&repo.origin));
    let entries = document
        .entry("repos")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()));
    match entries {
        Item::ArrayOfTables(tables) => tables.push(table),
        Item::Value(Value::Array(array)) => array.push(table.into_inline_table()),
        _ => {
            return Err(Error::Invalid(
                "repos must be an array of repository tables".into(),
            ));
        }
    }
    write(config_path, &document)?;
    Ok(repo)
}

fn selector_path(path: &str) -> Result<PathBuf> {
    let path = path.strip_prefix("~/").map_or_else(
        || PathBuf::from(path),
        |relative| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(relative),
    );
    let path = std::env::current_dir()?.join(path);
    Ok(path.canonicalize().unwrap_or(path))
}

pub fn remove(config_path: &Path, selector: &str) -> Result<Repo> {
    let (policy, mut document) = read(config_path)?;
    let selected_path = selector_path(selector)?;
    let mut selected = None;
    for (index, repo) in policy.repos.iter().enumerate() {
        if same_origin(&repo.origin, selector)
            || repo.path == selector
            || selector_path(&repo.path)? == selected_path
        {
            selected = Some(index);
            break;
        }
    }
    let index = selected
        .ok_or_else(|| Error::Invalid(format!("repository is not configured: {selector}")))?;
    let entries = document
        .get_mut("repos")
        .ok_or_else(|| Error::Invalid("repos must be an array of repository tables".into()))?;
    match entries {
        Item::ArrayOfTables(tables) => {
            tables.remove(index);
        }
        Item::Value(Value::Array(array)) => {
            array.remove(index);
        }
        _ => {
            return Err(Error::Invalid(
                "repos must be an array of repository tables".into(),
            ));
        }
    }
    write(config_path, &document)?;
    policy
        .repos
        .into_iter()
        .nth(index)
        .ok_or_else(|| Error::Invalid("selected repository disappeared".into()))
}

pub fn list(policy: &Policy) -> &[Repo] {
    &policy.repos
}

pub fn render(repos: &[Repo]) -> String {
    use std::fmt::Write as _;
    repos.iter().fold(String::new(), |mut text, repo| {
        let _ = writeln!(text, "{}\t{}", repo.path, repo.origin);
        text
    })
}
