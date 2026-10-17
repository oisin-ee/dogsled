//! One retry schedule for everything that can fail and try again: peer reads, inbox
//! deliveries, and launches. The delay starts at 10 s, doubles per attempt, stops growing at
//! 5 min, and loses up to a fifth to jitter so retries from many callers spread out.

use serde::{Deserialize, Serialize};

pub const BASE_MS: u64 = 10_000;
pub const CAP_MS: u64 = 5 * 60_000;

/// Consecutive failures and when the next attempt may run. The default allows one now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backoff {
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub next_ms: u64,
}

/// The delay after `attempts` consecutive failures (1 is the first). `seed` picks the jitter,
/// so one record or host gets the same delay each time it is computed.
pub fn delay(attempts: u32, seed: &str) -> u64 {
    let doubled = BASE_MS.saturating_mul(1 << attempts.saturating_sub(1).min(16));
    let full = doubled.min(CAP_MS);
    // FNV-1a over the seed and the attempt: stable, and spread across callers.
    let hash = seed
        .bytes()
        .chain(attempts.to_le_bytes())
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
        });
    full - hash % (full / 5 + 1)
}

impl Backoff {
    pub fn ready(&self, now: u64) -> bool {
        now >= self.next_ms
    }

    /// Record a failure at `now` and schedule the next attempt.
    pub fn fail(&mut self, now: u64, seed: &str) {
        self.attempts = self.attempts.saturating_add(1);
        self.next_ms = now.saturating_add(delay(self.attempts, seed));
    }

    /// A success, or a user's "retry now": the next attempt may run at once.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_doubles_from_ten_seconds_to_a_five_minute_cap_with_bounded_jitter() {
        let expected = [10_000, 20_000, 40_000, 80_000, 160_000, 300_000, 300_000];
        for (attempt, full) in (1..).zip(expected) {
            for seed in ["a", "b", "netcup", "r-1-2-3"] {
                let d = delay(attempt, seed);
                assert!(
                    d <= full && d >= full - full / 5,
                    "{attempt} {seed}: {d} not within jitter of {full}"
                );
                assert_eq!(d, delay(attempt, seed), "jitter is stable per seed");
            }
        }
        assert_eq!(delay(u32::MAX, "x").min(CAP_MS), delay(u32::MAX, "x"));
        let spread: std::collections::BTreeSet<_> =
            (0..20).map(|i| delay(3, &format!("seed-{i}"))).collect();
        assert!(spread.len() > 1, "different seeds spread out");
    }

    #[test]
    fn failures_schedule_the_next_attempt_and_reset_allows_one_now() {
        let mut backoff = Backoff::default();
        assert!(backoff.ready(0));
        backoff.fail(1_000, "h");
        assert_eq!(backoff.attempts, 1);
        assert!(!backoff.ready(1_000));
        assert!(backoff.ready(1_000 + BASE_MS));
        backoff.fail(20_000, "h");
        assert_eq!(backoff.next_ms, 20_000 + delay(2, "h"));
        backoff.reset();
        assert!(backoff.ready(0));
        assert_eq!(backoff.attempts, 0);
    }
}
