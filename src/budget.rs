//! One monotonic command deadline, shared by transport and lock acquisition.

use crate::error::{Error, Result};
use std::time::{Duration, Instant};

pub const CALL_LIMIT: Duration = Duration::from_secs(30);
pub const SETUP_LIMIT: Duration = Duration::from_mins(10);

#[derive(Debug, Clone, Copy, Default)]
pub struct Budget {
    pub deadline: Option<Instant>,
}

impl Budget {
    pub fn until(deadline: Instant) -> Self {
        Self {
            deadline: Some(deadline),
        }
    }

    pub fn remaining(self, operation: &str) -> Result<Duration> {
        self.with_limit(operation, CALL_LIMIT)
    }

    pub fn with_limit(self, operation: &str, limit: Duration) -> Result<Duration> {
        let remaining = self.deadline.map_or(limit, |end| {
            end.saturating_duration_since(Instant::now()).min(limit)
        });
        if remaining.is_zero() {
            Err(Error::Deadline(operation.into()))
        } else {
            Ok(remaining)
        }
    }

    pub fn pause(self) -> Result<()> {
        std::thread::sleep(self.remaining("wait")?.min(Duration::from_millis(100)));
        Ok(())
    }
}
