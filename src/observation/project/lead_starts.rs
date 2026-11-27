//! Lead starts belong to the service that accepted them, not the execution host.

use super::{Domains, Gap, insert};
use crate::error::Result;
use portal_types::agents::LeadStartView;

pub(in crate::observation) const VIEW: &str = "lead_starts";

pub(super) fn project(
    local: &Domains,
    views: &mut Domains,
    gaps: &[Gap],
) -> Result<Option<Vec<LeadStartView>>> {
    let gaps: Vec<_> = gaps
        .iter()
        .filter(|gap| gap.domain == VIEW)
        .cloned()
        .collect();
    let starts = if gaps.is_empty() {
        local
            .get(VIEW)
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
    } else {
        None
    };
    insert(views, VIEW, starts.as_ref(), &gaps)?;
    Ok(starts)
}
