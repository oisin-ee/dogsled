//! Repository identity comparison and the published GitHub catalogue's CLI display.

use portal_types::agents::RepoList;

pub(crate) fn same_origin(left: &str, right: &str) -> bool {
    match (
        crate::checkout::identity(left),
        crate::checkout::identity(right),
    ) {
        (Some(left), Some(right)) => left == right,
        _ => left == right,
    }
}

pub fn render(list: &RepoList) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    for repo in &list.repos {
        let _ = writeln!(
            text,
            "{}\t{}\t{}\t{}",
            repo.slug,
            if repo.private { "private" } else { "public" },
            repo.pushed_at.as_deref().unwrap_or(""),
            repo.description
                .as_deref()
                .unwrap_or("")
                .replace(['\n', '\r', '\t'], " ")
        );
    }
    if let Some(error) = &list.error {
        let _ = writeln!(text, "GitHub catalogue error: {error}");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_origin_matches_github_ssh_and_https() {
        assert!(same_origin(
            "git@github.com:owner/repo.git",
            "https://github.com/owner/repo"
        ));
        assert!(same_origin(
            "ssh://git@github.com/owner/repo.git",
            "https://github.com/owner/repo.git"
        ));
        assert!(!same_origin(
            "git@github.com:owner/repo.git",
            "https://github.com/another/repo"
        ));
    }
}
