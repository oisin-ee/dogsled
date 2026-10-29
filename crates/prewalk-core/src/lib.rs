//! Pure prewalk protocol, config validation, and session boundary types.

mod config;
mod protocol;
pub mod types;

pub use config::{configure, validate_model};
pub use protocol::{decide, project};

#[cfg(test)]
mod tests;
