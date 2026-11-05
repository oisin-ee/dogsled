//! Serve's retained GitHub catalogue. Readers never invoke GitHub themselves.

use crate::error::{Error, Result};
use crate::shell::Shell;
use portal_types::agents::{RepoChoice, RepoList};
use serde::Deserialize;

const HOUR_MS: u64 = 60 * 60 * 1000;
const ENDPOINT: &str = "user/repos?per_page=100&affiliation=owner,collaborator,organization_member";

#[derive(Debug, Default)]
pub(super) struct Catalogue {
    attempted_ms: Option<u64>,
    list: RepoList,
}

#[derive(Deserialize)]
struct GithubRepo {
    full_name: String,
    description: Option<String>,
    pushed_at: Option<String>,
    private: bool,
    archived: bool,
}

impl Catalogue {
    pub fn refresh(&mut self, shell: &dyn Shell, now: u64) -> Option<RepoList> {
        if self
            .attempted_ms
            .is_some_and(|previous| now.saturating_sub(previous) < HOUR_MS)
        {
            return None;
        }
        self.attempted_ms = Some(now);
        match read(shell) {
            Ok(repos) => self.list = RepoList { repos, error: None },
            Err(error) => self.list.error = Some(error.to_string()),
        }
        Some(self.list.clone())
    }
}

fn read(shell: &dyn Shell) -> Result<Vec<RepoChoice>> {
    let output = shell.run("gh", &["api".into(), ENDPOINT.into(), "--paginate".into()])?;
    if !output.ok() {
        return Err(Error::Invalid(format!(
            "GitHub catalogue: gh exited {}: {}",
            output.status,
            output.stderr.trim()
        )));
    }
    // gh --paginate emits a separate JSON array for each page, not an outer array.
    let mut repos = Vec::new();
    let mut pages = 0;
    for page in serde_json::Deserializer::from_str(&output.stdout).into_iter::<Vec<GithubRepo>>() {
        pages += 1;
        for repo in page? {
            if !repo.archived {
                repos.push(RepoChoice {
                    slug: repo.full_name,
                    description: repo.description,
                    pushed_at: repo.pushed_at,
                    private: repo.private,
                });
            }
        }
    }
    if pages == 0 {
        return Err(Error::Invalid(
            "GitHub catalogue: gh returned no JSON pages".into(),
        ));
    }
    repos.sort_by(|left, right| {
        right
            .pushed_at
            .cmp(&left.pushed_at)
            .then_with(|| left.slug.cmp(&right.slug))
    });
    Ok(repos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::{Fake, Output};

    fn command() -> String {
        format!("gh api {ENDPOINT} --paginate")
    }

    #[test]
    fn catalogue_retains_previous_list_and_error_until_hourly_recovery() {
        let shell = Fake::new();
        shell.on(&command(), r#"[{"full_name":"owner/fork","description":null,"pushed_at":null,"private":true,"archived":false,"fork":true}]"#);
        let mut catalogue = Catalogue::default();
        let good = catalogue.refresh(&shell, 0).unwrap();
        assert_eq!(good.repos[0].slug, "owner/fork");
        shell.on_output(
            &command(),
            Output {
                status: 1,
                stdout: String::new(),
                stderr: "authentication failed".into(),
            },
        );
        assert!(catalogue.refresh(&shell, HOUR_MS - 1).is_none());
        let failed = catalogue.refresh(&shell, HOUR_MS).unwrap();
        assert_eq!(failed.repos, good.repos);
        assert!(
            failed
                .error
                .as_deref()
                .unwrap()
                .contains("authentication failed")
        );
        shell.on(&command(), "[]");
        assert!(catalogue.refresh(&shell, HOUR_MS + 1).is_none());
        assert_eq!(
            catalogue.refresh(&shell, HOUR_MS * 2).unwrap(),
            RepoList::default()
        );
    }

    #[test]
    fn catalogue_malformed_later_page_retains_the_entire_previous_list() {
        let shell = Fake::new();
        shell.on(&command(), "[]");
        let mut catalogue = Catalogue::default();
        assert!(catalogue.refresh(&shell, 0).is_some());
        shell.on(&command(), "[]\n{\"message\":\"bad page\"}");
        let list = catalogue.refresh(&shell, HOUR_MS).unwrap();
        assert!(list.error.is_some());
        assert_eq!(list.repos, []);
    }
}
