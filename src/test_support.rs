//! Shared bounds for test liveness, not product timing or negative checks.

/// Allow process startup, I/O, and scheduling under full-suite contention.
pub(crate) const WAIT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);
