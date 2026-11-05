//! Service-local domain failure transitions. Diagnostic text is never a published view.

use super::{Availability, Gap};
use crate::backoff::Backoff;
use std::collections::{BTreeMap, BTreeSet};

type Key = (String, &'static str);

#[derive(Default)]
pub(crate) struct Diagnostics {
    failures: BTreeMap<Key, Backoff>,
}

impl Diagnostics {
    /// Keep gap diagnostics ordered with the publication that exposes them.
    pub(crate) fn publish(
        &mut self,
        publisher: &mut super::Publisher,
        input: super::Publication,
        now: u64,
        write: impl FnMut(String),
    ) -> crate::error::Result<()> {
        self.record(&input.gaps, now, write);
        publisher.replace(input)?;
        Ok(())
    }

    pub(crate) fn record(&mut self, gaps: &[Gap], now: u64, mut write: impl FnMut(String)) {
        let mut current: BTreeMap<Key, BTreeSet<&str>> = BTreeMap::new();
        for gap in gaps {
            if gap.availability != Availability::Available {
                current
                    .entry((gap.domain.clone(), kind(&gap.availability)))
                    .or_default()
                    .insert(&gap.detail);
            }
        }
        let old_domains: BTreeSet<_> = self.failures.keys().map(|key| key.0.clone()).collect();
        let domains: BTreeSet<_> = current.keys().map(|key| key.0.clone()).collect();
        let mut remaining = std::mem::take(&mut self.failures);
        for (key, details) in current {
            let previous = remaining.remove(&key);
            let mut retry = previous.unwrap_or_default();
            if retry.ready(now) {
                let phase = if previous.is_some() {
                    "still failing".to_owned()
                } else if old_domains.contains(&key.0) {
                    format!("failure changed to {}", key.1)
                } else {
                    "failed".to_owned()
                };
                write(format!(
                    "dogsled service: {} {phase}: {}",
                    subject(&key.0),
                    details.into_iter().collect::<Vec<_>>().join("; ")
                ));
                retry.fail(now, &key.0);
            }
            self.failures.insert(key, retry);
        }
        for domain in old_domains.difference(&domains) {
            write(format!("dogsled service: {} recovered", subject(domain)));
        }
    }
}

fn kind(value: &Availability) -> &'static str {
    match value {
        Availability::Available => "available",
        Availability::Unavailable => "unavailable",
        Availability::Failed => "failed",
        Availability::Unsupported => "unsupported",
        Availability::Stale => "stale",
    }
}

fn subject(domain: &str) -> String {
    if domain == "todos" {
        "todo intake".into()
    } else {
        format!("{domain} collection")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::Availability;

    fn failure(domain: &str, detail: &str) -> Gap {
        Gap {
            domain: domain.into(),
            availability: Availability::Failed,
            detail: detail.into(),
        }
    }

    #[test]
    fn gap_diagnostics_are_written_before_the_new_revision_is_visible() {
        let mut publisher = super::super::Publisher::new().unwrap();
        let input = super::super::Publication {
            published_ms: 1,
            sources: super::super::Sources::default(),
            local: super::super::Domains::new(),
            views: super::super::Domains::new(),
            gaps: Vec::new(),
        };
        let previous = publisher.replace(input.clone()).unwrap();
        let reader = publisher.subscribe();
        let gaps = vec![failure("peer:8103:branches", "branch-check-failed")];
        let mut lines = Vec::new();
        Diagnostics::default()
            .publish(
                &mut publisher,
                super::super::Publication {
                    published_ms: 2,
                    gaps: gaps.clone(),
                    ..input
                },
                2,
                |line| {
                    assert_eq!(
                        reader.borrow().as_ref().unwrap().published.revision,
                        previous.revision
                    );
                    lines.push(line);
                },
            )
            .unwrap();
        assert_eq!(
            lines,
            ["dogsled service: peer:8103:branches collection failed: branch-check-failed"]
        );
        let frame = reader.borrow();
        let observation = &frame.as_ref().unwrap().published;
        assert_eq!(observation.revision, previous.revision + 1);
        assert_eq!(observation.gaps, gaps);
    }

    #[test]
    fn persistent_failure_over_many_ticks_logs_once_then_recovers() {
        let mut diagnostics = Diagnostics::default();
        let mut lines = Vec::new();
        for tick in 0..100 {
            diagnostics.record(&[failure("host", "probe failed")], tick, |line| {
                lines.push(line);
            });
        }
        diagnostics.record(&[], 100, |line| lines.push(line));
        diagnostics.record(&[], 101, |line| lines.push(line));
        assert_eq!(
            lines,
            [
                "dogsled service: host collection failed: probe failed",
                "dogsled service: host collection recovered",
            ]
        );
    }

    #[test]
    fn continuing_failure_uses_backoff_even_when_details_change() {
        let mut diagnostics = Diagnostics::default();
        let mut lines = Vec::new();
        diagnostics.record(&[failure("todos", "first")], 0, |line| lines.push(line));
        let first = crate::backoff::delay(1, "todos");
        let second = first + crate::backoff::delay(2, "todos");
        for tick in [first - 1, first, second - 1, second] {
            diagnostics.record(&[failure("todos", "latest")], tick, |line| lines.push(line));
        }
        assert_eq!(
            lines,
            [
                "dogsled service: todo intake failed: first",
                "dogsled service: todo intake still failing: latest",
                "dogsled service: todo intake still failing: latest",
            ]
        );
        diagnostics.record(&[], second + 1, |line| lines.push(line));
        diagnostics.record(&[failure("todos", "new failure")], second + 2, |line| {
            lines.push(line);
        });
        assert_eq!(
            &lines[3..],
            [
                "dogsled service: todo intake recovered",
                "dogsled service: todo intake failed: new failure",
            ]
        );
    }

    #[test]
    fn changed_gap_class_logs_immediately_without_using_message_identity() {
        let mut diagnostics = Diagnostics::default();
        let mut lines = Vec::new();
        diagnostics.record(&[failure("host", "first")], 0, |line| lines.push(line));
        let stale = Gap {
            domain: "host".into(),
            availability: Availability::Stale,
            detail: "sample age 31s".into(),
        };
        diagnostics.record(std::slice::from_ref(&stale), 1, |line| lines.push(line));
        diagnostics.record(
            &[Gap {
                detail: "sample age 32s".into(),
                ..stale
            }],
            2,
            |line| lines.push(line),
        );
        diagnostics.record(&[], 3, |line| lines.push(line));
        assert_eq!(
            lines,
            [
                "dogsled service: host collection failed: first",
                "dogsled service: host collection failure changed to stale: sample age 31s",
                "dogsled service: host collection recovered",
            ]
        );
    }

    #[test]
    fn domain_recovery_and_multiple_gap_details_are_independent() {
        let mut diagnostics = Diagnostics::default();
        let mut lines = Vec::new();
        let gaps = [
            failure("host", "second"),
            failure("todos", "intake"),
            failure("host", "first"),
        ];
        diagnostics.record(&gaps, 0, |line| lines.push(line));
        diagnostics.record(&[failure("todos", "changed")], 1, |line| lines.push(line));
        assert_eq!(
            lines,
            [
                "dogsled service: host collection failed: first; second",
                "dogsled service: todo intake failed: intake",
                "dogsled service: host collection recovered",
            ]
        );
    }
}
