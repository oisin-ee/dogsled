//! Lead validation and shared main checkout resolution.

use super::CreateContext;
use crate::checkout::{Repository, slug_origin};
use crate::error::{Error, Result};

pub(super) fn validate(slug: &str, prompt: &str) -> Result<()> {
    slug_origin(slug)?;
    if prompt.trim().is_empty() {
        return Err(Error::Invalid("the first prompt is empty".into()));
    }
    Ok(())
}

pub(super) fn ensure(context: &CreateContext<'_>, slug: &str) -> Result<Repository> {
    let origin = slug_origin(slug)?;
    crate::checkout::resolve(context.shell, context.ledger, context.home, &origin)
        .map(|(repository, _)| repository)
}
