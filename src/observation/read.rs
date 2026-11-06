//! Reader-only selection from one immutable publication. No observation capabilities.

use super::{Gap, Published, project::View};
use crate::error::{Error, Result};
use serde::de::DeserializeOwned;
use serde_json::Value;

fn missing(domain: &str) -> Error {
    Error::Invalid(format!(
        "observation unavailable: {domain} was not published"
    ))
}

/// One gap as a reader shows it.
pub(crate) fn describe(gap: &Gap) -> String {
    format!("{}: {:?}: {}", gap.domain, gap.availability, gap.detail)
}

/// A one-shot command reports the gaps of the publication it printed on stderr, once each.
/// Selection stays silent: a full-screen reader such as the sidebar owns its terminal and
/// shows gaps in its own layout, and a stray stderr line would draw over it.
pub(crate) fn report(published: &Published, domain: Option<&str>) {
    let lines: std::collections::BTreeSet<_> = published
        .gaps
        .iter()
        .filter(|gap| domain.is_none_or(|domain| gap.domain == domain))
        .map(describe)
        .collect();
    for line in lines {
        eprintln!("observation gap: {line}");
    }
}

pub(crate) fn view<T: DeserializeOwned>(published: &Published, domain: &str) -> Result<T> {
    let value: View<T> = serde_json::from_value(
        published
            .views
            .get(domain)
            .ok_or_else(|| missing(domain))?
            .clone(),
    )?;
    value.data.ok_or_else(|| {
        if value.gaps.is_empty() {
            missing(domain)
        } else {
            Error::Invalid(format!(
                "observation unavailable: {domain}; gaps: {}",
                value
                    .gaps
                    .iter()
                    .map(describe)
                    .collect::<Vec<_>>()
                    .join("; ")
            ))
        }
    })
}

pub(crate) fn local<T: DeserializeOwned>(published: &Published, domain: &str) -> Result<T> {
    Ok(serde_json::from_value(
        published
            .local
            .get(domain)
            .ok_or_else(|| missing(domain))?
            .clone(),
    )?)
}

fn owned<T: DeserializeOwned>(
    published: &Published,
    domain: &str,
    owner: Option<&str>,
    all: &str,
    empty: &str,
) -> Result<T> {
    let Some(owner) = owner else {
        return view(published, all);
    };
    let owners: std::collections::BTreeMap<String, Option<Value>> = view(published, domain)?;
    match owners.get(owner) {
        Some(Some(value)) => Ok(serde_json::from_value(value.clone())?),
        Some(None) => Err(missing(domain)),
        None => view(published, empty),
    }
}

pub(crate) fn board(published: &Published, owner: Option<&str>) -> Result<crate::board::Board> {
    owned(published, "boards", owner, "board", "empty_board")
}

pub(crate) fn sidebar(published: &Published, owner: &str) -> Result<crate::sidebar::model::Data> {
    owned(
        published,
        "sidebars",
        Some(owner),
        "empty_sidebar",
        "empty_sidebar",
    )
}

pub(crate) fn findings(published: &Published, owner: &str) -> Result<crate::gate::Findings> {
    let mut findings: std::collections::BTreeMap<String, crate::gate::Findings> =
        view(published, "findings")?;
    Ok(findings.remove(owner).unwrap_or_default())
}

/// The owner's jobs, finished ones included, with the host identities that label them.
pub(crate) fn job_history(published: &Published, owner: &str) -> Result<Value> {
    owned(
        published,
        "job_history",
        Some(owner),
        "empty_job_history",
        "empty_job_history",
    )
}

pub(crate) fn inventory(published: &Published, owner: Option<&str>) -> Result<Value> {
    owned(
        published,
        "inventories",
        owner,
        "inventory",
        "empty_inventory",
    )
}
