//! Bounded, service-local observation timings. No observation payload enters the log.

use crate::service::wake;
use std::collections::BTreeMap;
use std::fmt::Write;
use std::time::{Duration, Instant};

const WINDOW: Duration = Duration::from_secs(60);
pub(crate) const SCHEDULED: u8 = 32;
const DOMAINS: [(&str, u8); 6] = [
    ("ledger", wake::LEDGER),
    ("local", wake::LOCAL & !wake::LEDGER),
    ("peers", wake::PEERS),
    ("branches", wake::BRANCHES),
    ("details", wake::DETAILS),
    ("scheduled", SCHEDULED),
];

#[derive(Debug, Default)]
struct Phase {
    count: u64,
    total: Duration,
    max: Duration,
}

#[derive(Debug)]
pub(crate) struct PassTimings {
    started: Instant,
    passes: u64,
    wakes: [u64; DOMAINS.len()],
    phases: BTreeMap<&'static str, Phase>,
}

impl Default for PassTimings {
    fn default() -> Self {
        Self::new(Instant::now())
    }
}

impl PassTimings {
    fn new(started: Instant) -> Self {
        Self {
            started,
            passes: 0,
            wakes: [0; DOMAINS.len()],
            phases: BTreeMap::new(),
        }
    }

    pub(crate) fn phase(&mut self, name: &'static str, elapsed: Duration) {
        let phase = self.phases.entry(name).or_default();
        phase.count = phase.count.saturating_add(1);
        phase.total = phase.total.saturating_add(elapsed);
        phase.max = phase.max.max(elapsed);
    }

    pub(crate) fn pass(&mut self, domains: u8) {
        self.passes = self.passes.saturating_add(1);
        for ((_, bit), count) in DOMAINS.iter().zip(&mut self.wakes) {
            if domains & bit != 0 {
                *count = count.saturating_add(1);
            }
        }
    }

    pub(crate) fn flush(&mut self, now: Instant, mut write: impl FnMut(String)) {
        if now.saturating_duration_since(self.started) < WINDOW {
            return;
        }
        let mut line = format!("dogsled serve timing: passes={} wakes", self.passes);
        for ((name, _), count) in DOMAINS.iter().zip(self.wakes) {
            let _ = write!(line, " {name}={count}");
        }
        for (name, phase) in &self.phases {
            let _ = write!(
                line,
                " phase {name}={}/{}/{}",
                phase.count,
                phase.total.as_millis(),
                phase.max.as_millis(),
            );
        }
        write(line);
        self.started = now;
        self.passes = 0;
        self.wakes.fill(0);
        self.phases.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_emits_once_after_sixty_seconds_and_resets() {
        let start = Instant::now();
        let mut timings = PassTimings::new(start);
        let mut lines = Vec::new();
        timings.pass(wake::LEDGER);
        timings.phase("collect", Duration::from_millis(12));
        timings.flush(start + Duration::from_nanos(59_999_999_999), |line| {
            lines.push(line);
        });
        assert_eq!(lines.len(), 0);
        timings.flush(start + WINDOW, |line| lines.push(line));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("passes=1"));
        assert!(lines[0].contains("phase collect=1/12/12"));
        timings.flush(start + WINDOW, |line| lines.push(line));
        timings.flush(start + WINDOW + Duration::from_secs(59), |line| {
            lines.push(line);
        });
        assert_eq!(lines.len(), 1);
        timings.flush(start + WINDOW * 2, |line| lines.push(line));
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[1],
            "dogsled serve timing: passes=0 wakes ledger=0 local=0 peers=0 branches=0 details=0 scheduled=0"
        );
    }

    #[test]
    fn phase_counts_totals_and_max_are_independent() {
        let start = Instant::now();
        let mut timings = PassTimings::new(start);
        for millis in [7, 19, 4] {
            timings.phase("collect", Duration::from_millis(millis));
        }
        timings.phase("publish.replace", Duration::from_millis(3));
        let mut lines = Vec::new();
        timings.flush(start + WINDOW, |line| lines.push(line));
        assert_eq!(
            lines,
            [
                "dogsled serve timing: passes=0 wakes ledger=0 local=0 peers=0 branches=0 details=0 scheduled=0 phase collect=3/30/19 phase publish.replace=1/3/3"
            ]
        );
    }

    #[test]
    fn wake_domains_count_each_bit_not_the_composite_local_mask() {
        let start = Instant::now();
        let mut timings = PassTimings::new(start);
        timings.pass(wake::LEDGER);
        timings.pass(wake::LOCAL | wake::PEERS);
        timings.pass(wake::BRANCHES | wake::DETAILS);
        timings.pass(SCHEDULED);
        timings.pass(0);
        let mut lines = Vec::new();
        timings.flush(start + WINDOW, |line| lines.push(line));
        assert_eq!(
            lines,
            [
                "dogsled serve timing: passes=5 wakes ledger=2 local=1 peers=1 branches=1 details=1 scheduled=1"
            ]
        );
    }

    #[test]
    fn delayed_flush_emits_no_backlog_and_only_static_phase_names() {
        let start = Instant::now();
        let mut timings = PassTimings::new(start);
        timings.pass(u8::MAX);
        timings.phase("activity.views", Duration::from_millis(2));
        let mut lines = Vec::new();
        timings.flush(start + WINDOW * 10, |line| lines.push(line));
        timings.flush(start + WINDOW * 10, |line| lines.push(line));
        assert_eq!(
            lines,
            [
                "dogsled serve timing: passes=1 wakes ledger=1 local=1 peers=1 branches=1 details=1 scheduled=1 phase activity.views=1/2/2"
            ]
        );
    }
}
