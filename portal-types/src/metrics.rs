//! The metrics report: tool calls, owner wait, and agent runs over one window.
//!
//! Each host builds its own report from its transcripts and `state.db`. The portal merges
//! the reports of every host. Durations travel as fixed-bucket histograms, so a merge adds
//! counts exactly and every host computes the same percentile.

use serde::{Deserialize, Serialize};

/// Metrics payload version; independent of [`crate::PORTAL_VERSION`].
pub const METRICS_VERSION: u32 = 1;

/// The upper bound of each histogram bucket, in milliseconds. One more bucket holds the
/// durations above the last bound.
pub const BUCKET_BOUNDS_MS: [u64; 16] = [
    100, 250, 500, 1_000, 2_500, 5_000, 10_000, 30_000, 60_000, 120_000, 300_000, 600_000,
    1_800_000, 3_600_000, 14_400_000, 86_400_000,
];

/// The number of buckets: one per bound, and one for the durations above the last bound.
pub const BUCKETS: usize = BUCKET_BOUNDS_MS.len() + 1;

/// Durations counted in [`BUCKET_BOUNDS_MS`] buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Histogram {
    pub counts: [u64; BUCKETS],
    /// The sum of all durations. Parallel calls overlap, so this is not wall time.
    pub total_ms: u64,
    pub max_ms: u64,
}

impl Default for Histogram {
    fn default() -> Self {
        Self {
            counts: [0; BUCKETS],
            total_ms: 0,
            max_ms: 0,
        }
    }
}

impl Histogram {
    /// Count one duration.
    pub fn record(&mut self, duration_ms: u64) {
        let bucket = BUCKET_BOUNDS_MS
            .iter()
            .position(|bound| duration_ms <= *bound)
            .unwrap_or(BUCKET_BOUNDS_MS.len());
        if let Some(count) = self.counts.get_mut(bucket) {
            *count = count.saturating_add(1);
        }
        self.total_ms = self.total_ms.saturating_add(duration_ms);
        self.max_ms = self.max_ms.max(duration_ms);
    }

    /// Add the durations of `other`.
    pub fn merge(&mut self, other: &Self) {
        for (count, add) in self.counts.iter_mut().zip(other.counts) {
            *count = count.saturating_add(add);
        }
        self.total_ms = self.total_ms.saturating_add(other.total_ms);
        self.max_ms = self.max_ms.max(other.max_ms);
    }

    /// The number of durations counted.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.counts
            .iter()
            .fold(0, |sum, count| sum.saturating_add(*count))
    }

    /// The duration at quantile `q` (0.0 to 1.0): the upper bound of the bucket that holds
    /// that rank, capped at the largest duration. `None` when nothing was counted.
    #[must_use]
    pub fn quantile(&self, q: f64) -> Option<u64> {
        let count = self.count();
        if count == 0 {
            return None;
        }
        // The rank is 1-based: q = 0.5 of 4 durations is the 2nd, q = 1.0 the 4th.
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let rank = ((q.clamp(0.0, 1.0) * count as f64).ceil() as u64).max(1);
        let mut seen = 0_u64;
        for (bucket, bucket_count) in self.counts.iter().enumerate() {
            seen = seen.saturating_add(*bucket_count);
            if seen >= rank {
                let bound = BUCKET_BOUNDS_MS.get(bucket).copied().unwrap_or(u64::MAX);
                return Some(bound.min(self.max_ms));
            }
        }
        Some(self.max_ms)
    }
}

/// The period a report covers, ending at [`MetricsReport::until_ms`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Window {
    Day,
    #[default]
    Week,
    Month,
}

impl Window {
    #[must_use]
    pub const fn as_ms(self) -> u64 {
        const DAY: u64 = 24 * 60 * 60 * 1000;
        match self {
            Self::Day => DAY,
            Self::Week => 7 * DAY,
            Self::Month => 30 * DAY,
        }
    }
}

/// How tool rows are grouped. [`ToolGroup::Tool`] gives one row per tool; each other group
/// gives one row per tool and group value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolGroup {
    #[default]
    Tool,
    Repository,
    Model,
    Host,
    Lead,
    Harness,
}

/// How run rows are grouped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunGroup {
    #[default]
    Profile,
    Model,
    Harness,
}

/// The group value of a row whose transcript or item has no value for the group.
pub const UNATTRIBUTED: &str = "unattributed";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricsReport {
    pub version: u32,
    pub window: Window,
    pub tool_group: ToolGroup,
    pub run_group: RunGroup,
    pub until_ms: u64,
    /// One entry per host asked. A host that failed has its error and no rows.
    pub hosts: Vec<HostCoverage>,
    /// Sorted by calls, most first.
    pub tools: Vec<ToolRow>,
    /// One row per kind.
    pub owner_waits: Vec<WaitRow>,
    pub runs: Vec<RunRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostCoverage {
    pub host: String,
    /// Transcripts indexed in the window.
    pub transcripts: u64,
    /// The end of the host's last complete index pass.
    pub indexed_ms: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRow {
    /// The raw tool name from the transcript.
    pub tool: String,
    /// `None` for [`ToolGroup::Tool`]; otherwise the group value or [`UNATTRIBUTED`].
    pub group: Option<String>,
    /// Calls started in the window.
    pub calls: u64,
    /// Calls the harness marked as failed.
    pub errors: u64,
    /// Calls whose harness does not mark failures; the error rate excludes them.
    pub unknown_status: u64,
    /// Calls with no result yet; they have no duration.
    pub running: u64,
    pub duration: Histogram,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitKind {
    Question,
    PlanReview,
    Ask,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitRow {
    pub kind: WaitKind,
    pub answered: u64,
    /// Waits still open at `until_ms`.
    pub open: u64,
    pub wait: Histogram,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Outcomes {
    pub running: u64,
    pub done: u64,
    pub ask: u64,
    pub blocked: u64,
    pub closed: u64,
    pub dropped: u64,
}

impl Outcomes {
    pub fn merge(&mut self, other: &Self) {
        self.running = self.running.saturating_add(other.running);
        self.done = self.done.saturating_add(other.done);
        self.ask = self.ask.saturating_add(other.ask);
        self.blocked = self.blocked.saturating_add(other.blocked);
        self.closed = self.closed.saturating_add(other.closed);
        self.dropped = self.dropped.saturating_add(other.dropped);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRow {
    /// The group value or [`UNATTRIBUTED`].
    pub group: String,
    pub outcomes: Outcomes,
    /// Ticket creation to item creation. Direct launches have no queue wait.
    pub queue_wait: Histogram,
    /// Item creation to launch confirmation.
    pub spawn: Histogram,
    /// Launch confirmation to the captured done reply.
    pub to_done: Histogram,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merged_histograms_give_the_quantiles_of_all_their_durations() {
        let left = [40, 90, 300, 2_000, 2_400];
        let right = [700, 45_000, 50_000, 4_000_000];
        let mut all = Histogram::default();
        let mut merged = Histogram::default();
        let mut other = Histogram::default();
        for duration in left {
            all.record(duration);
            merged.record(duration);
        }
        for duration in right {
            all.record(duration);
            other.record(duration);
        }
        merged.merge(&other);
        assert_eq!(merged, all);
        assert_eq!(merged.count(), 9);
        // The 5th of 9 sorted durations (2,000 ms) is in the 2,500 ms bucket.
        assert_eq!(merged.quantile(0.5), Some(2_500));
        // The 9th is in the 14,400,000 ms bucket, capped at the largest duration.
        assert_eq!(merged.quantile(0.95), Some(4_000_000));
        assert_eq!(merged.quantile(0.0), Some(100));
        assert_eq!(Histogram::default().quantile(0.5), None);
    }
}
