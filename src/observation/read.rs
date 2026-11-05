//! Reader-only selection from one immutable publication. No observation capabilities.

use super::{Published, project::View};
use crate::error::{Error, Result};
use serde::de::DeserializeOwned;
use serde_json::Value;

fn missing(domain: &str) -> Error {
    Error::Invalid(format!(
        "observation unavailable: {domain} was not published"
    ))
}

pub(crate) fn view<T: DeserializeOwned>(published: &Published, domain: &str) -> Result<T> {
    let value: View<T> = serde_json::from_value(
        published
            .views
            .get(domain)
            .ok_or_else(|| missing(domain))?
            .clone(),
    )?;
    for gap in &value.gaps {
        eprintln!(
            "observation gap: {}: {:?}: {}",
            gap.domain, gap.availability, gap.detail
        );
    }
    value.data.ok_or_else(|| {
        if value.gaps.is_empty() {
            missing(domain)
        } else {
            Error::Invalid(format!(
                "observation unavailable: {domain}; gaps: {}",
                value
                    .gaps
                    .iter()
                    .map(|gap| format!("{}: {:?}: {}", gap.domain, gap.availability, gap.detail))
                    .collect::<Vec<_>>()
                    .join("; ")
            ))
        }
    })
}

pub(crate) fn local<T: DeserializeOwned>(published: &Published, domain: &str) -> Result<T> {
    for gap in published.gaps.iter().filter(|gap| gap.domain == domain) {
        eprintln!(
            "observation gap: {}: {:?}: {}",
            gap.domain, gap.availability, gap.detail
        );
    }
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

pub(crate) fn inventory(published: &Published, owner: Option<&str>) -> Result<Value> {
    owned(
        published,
        "inventories",
        owner,
        "inventory",
        "empty_inventory",
    )
}
