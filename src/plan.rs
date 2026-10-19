//! The admission rule, with no I/O. `evaluate` scores one host; `choose`
//! picks the host with the most free cores; `plan` combines both.
//!
//! A host must have room twice. Declared demand: every active agent and every
//! reservation holds `reservation_load` cores and `memory_reserve_bytes` of the
//! host's capacity for as long as it lives, as schedulers that admit on
//! requests do (research note §6). Measured load: what the probe saw, so work
//! that dogsled does not know about still counts. An agent waiting on its model
//! uses almost no CPU, so measured load alone admits onto a host whose agents
//! will all build again within minutes.
//!
//! The measured rule reads the CPU busy fraction over the last probe interval, not the
//! load average: load1 is a two-minute trailing mean of runnable threads
//! (63% of a step after 60 s, 95% after 180 s on Linux and xnu), and on the
//! sampled fleet it refused the Mac while it was free 84% of the time.
//! `docs/research/2026-10-15-admission-metric.md` holds the measurements.

use serde::{Deserialize, Serialize};

use crate::config::Thresholds;
use crate::probe::Probe;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Headroom {
    pub host: String,
    /// Free cores: the smaller of `cores × (1 − busy) − reservation_load × reservations`
    /// and `cores − reservation_load × (active agents + reservations)`.
    /// Hosts are ordered by this value.
    pub headroom: f64,
    pub eligible: bool,
    /// Why the host is not eligible, when it is not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub live_agents: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "plan", rename_all = "snake_case")]
#[cfg(test)]
pub enum Plan {
    Admit { host: String },
    Full,
}

#[must_use]
pub fn evaluate(p: &Probe, t: &Thresholds) -> Headroom {
    let reservations = p.reservations.len();
    let active = p.agents.active();
    let reserved = t.reservation_load * reservations as f64;
    let cores = f64::from(p.cores);
    let declared_free = cores - t.reservation_load * (active + reservations) as f64;
    let free_cores = p
        .busy
        .map(|busy| (cores * (1.0 - busy.clamp(0.0, 1.0)) - reserved).min(declared_free));
    let cores_min = t.free_core_fraction_min * cores;
    let memory_free = p.memory.total_bytes.saturating_sub(p.memory.used_bytes);
    let memory_need = t
        .memory_reserve_bytes
        .saturating_mul(reservations as u64 + 1);
    let memory_declared = t
        .memory_reserve_bytes
        .saturating_mul((active + reservations) as u64 + 1);
    let reason = if p.disk_free_percent < t.disk_free_min_percent {
        Some(format!(
            "disk {:.1}% free < {:.1}%",
            p.disk_free_percent, t.disk_free_min_percent
        ))
    } else if p.memory.pressure_level.is_some_and(|level| level >= 4) {
        Some("memory pressure critical".into())
    } else if memory_free < memory_need {
        Some(format!(
            "memory {:.1} GiB free < {:.1} GiB for {} reserved + 1",
            memory_free as f64 / GIB,
            memory_need as f64 / GIB,
            reservations
        ))
    } else if p.memory.total_bytes < memory_declared {
        Some(format!(
            "memory {:.1} GiB total < {:.1} GiB for {active} active + {reservations} reserved + 1",
            p.memory.total_bytes as f64 / GIB,
            memory_declared as f64 / GIB,
        ))
    } else {
        match free_cores {
            None => Some("no CPU sample yet".into()),
            Some(free) if free < cores_min => Some(format!(
                "{free:.1} free cores (busy {:.0}%, {active} active, {reservations} reserved) < {cores_min:.1}",
                p.busy.unwrap_or_default() * 100.0
            )),
            Some(_) => None,
        }
    };
    Headroom {
        host: p.host.clone(),
        headroom: free_cores.unwrap_or(f64::NEG_INFINITY),
        eligible: reason.is_none(),
        reason,
        live_agents: p.agents.live(),
    }
}

/// Most free cores; ties go to the host with fewer live agents,
/// then to the lexically smaller host id so the choice is deterministic.
#[must_use]
pub fn choose(evals: &[Headroom]) -> Option<&Headroom> {
    evals.iter().filter(|h| h.eligible).min_by(|a, b| {
        b.headroom
            .total_cmp(&a.headroom)
            .then(a.live_agents.cmp(&b.live_agents))
            .then(a.host.cmp(&b.host))
    })
}

#[must_use]
#[cfg(test)]
pub fn plan(probes: &[Probe], t: &Thresholds) -> Plan {
    let evals: Vec<Headroom> = probes.iter().map(|p| evaluate(p, t)).collect();
    match choose(&evals) {
        Some(h) => Plan::Admit {
            host: h.host.clone(),
        },
        None => Plan::Full,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::Reservation;
    use crate::probe::AgentCounts;

    fn thresholds() -> Thresholds {
        Thresholds {
            free_core_fraction_min: 0.25,
            memory_reserve_bytes: 3 * GIB as u64,
            disk_free_min_percent: 15.0,
            reservation_load: 1.0,
            reservation_ttl_ms: 180_000,
        }
    }

    fn probe(host: &str, busy: f64, cores: u32, disk: f64, reservations: usize) -> Probe {
        Probe {
            host: host.to_string(),
            label: host.to_string(),
            load1: 0.0,
            cores,
            cpu_ticks: None,
            busy: Some(busy),
            disk_free_percent: disk,
            memory: crate::probe::Memory {
                used_bytes: 8 * GIB as u64,
                total_bytes: 32 * GIB as u64,
                pressure_level: None,
            },
            agent_panes: Vec::new(),
            agents: AgentCounts::default(),
            reservations: (0..reservations)
                .map(|i| Reservation {
                    id: format!("r{i}"),
                    created_ms: 0,
                })
                .collect(),
            panes: Vec::new(),
        }
    }

    #[test]
    fn free_cores_formula() {
        let h = evaluate(&probe("a", 0.5, 8, 50.0, 2), &thresholds());
        // 8 × (1 − 0.5) − 2 = 2 ≥ 0.25 × 8
        assert!((h.headroom - 2.0).abs() < 1e-9);
        assert!(h.eligible);
    }

    #[test]
    fn disk_rule_blocks_before_load() {
        let h = evaluate(&probe("a", 0.0, 8, 10.0, 0), &thresholds());
        assert!(!h.eligible);
        assert!(h.reason.unwrap().starts_with("disk"));
    }

    #[test]
    fn below_quarter_of_cores_is_full() {
        // 8 × (1 − 0.8) = 1.6 < 2
        let h = evaluate(&probe("a", 0.8, 8, 50.0, 0), &thresholds());
        assert!(!h.eligible);
        assert_eq!(
            h.reason.unwrap(),
            "1.6 free cores (busy 80%, 0 active, 0 reserved) < 2.0"
        );
        // Exactly a quarter admits.
        assert!(evaluate(&probe("a", 0.75, 8, 50.0, 0), &thresholds()).eligible);
    }

    #[test]
    fn no_cpu_sample_is_full_and_sorts_last() {
        let mut p = probe("a", 0.0, 8, 50.0, 0);
        p.busy = None;
        let h = evaluate(&p, &thresholds());
        assert!(!h.eligible);
        assert_eq!(h.reason.as_deref(), Some("no CPU sample yet"));
        assert_eq!(h.headroom, f64::NEG_INFINITY);
    }

    #[test]
    fn memory_reserve_scales_with_reservations() {
        let t = thresholds();
        // 24 GiB free covers 1 + 7 reservations at 3 GiB each, not 8.
        assert!(evaluate(&probe("a", 0.0, 32, 50.0, 7), &t).eligible);
        let h = evaluate(&probe("a", 0.0, 32, 50.0, 8), &t);
        assert!(!h.eligible);
        assert_eq!(
            h.reason.unwrap(),
            "memory 24.0 GiB free < 27.0 GiB for 8 reserved + 1"
        );
    }

    #[test]
    fn critical_memory_pressure_is_full_warn_is_not() {
        let mut p = probe("a", 0.0, 8, 50.0, 0);
        p.memory.pressure_level = Some(2);
        assert!(evaluate(&p, &thresholds()).eligible);
        p.memory.pressure_level = Some(4);
        let h = evaluate(&p, &thresholds());
        assert!(!h.eligible);
        assert_eq!(h.reason.as_deref(), Some("memory pressure critical"));
    }

    #[test]
    fn reservations_count_as_load() {
        let t = thresholds();
        // 8 × (1 − 0.5) = 4 free: one reservation leaves 3, two leave 2 (limit), three leave 1.
        assert!(evaluate(&probe("a", 0.5, 8, 50.0, 2), &t).eligible);
        assert!(!evaluate(&probe("a", 0.5, 8, 50.0, 3), &t).eligible);
    }

    fn with_agents(mut p: Probe, states: &[(&str, usize)]) -> Probe {
        p.agents = AgentCounts {
            total: states.iter().map(|(_, n)| n).sum(),
            by_status: states.iter().map(|(s, n)| ((*s).to_string(), *n)).collect(),
        };
        p
    }

    #[test]
    fn active_agents_hold_cores_while_they_wait_on_the_model() {
        // Memory for 14 agents, so the CPU rule decides.
        let t = Thresholds {
            memory_reserve_bytes: GIB as u64,
            ..thresholds()
        };
        // Idle CPU, but 16 − 12 working = 4 declared free cores: the limit admits.
        let h = evaluate(
            &with_agents(probe("a", 0.05, 16, 50.0, 0), &[("working", 12)]),
            &t,
        );
        assert!((h.headroom - 4.0).abs() < 1e-9);
        assert!(h.eligible);
        // One more active agent, or one reservation, leaves 3 < 4.
        let busy = with_agents(
            probe("a", 0.05, 16, 50.0, 0),
            &[("working", 12), ("blocked", 1)],
        );
        assert_eq!(
            evaluate(&busy, &t).reason.unwrap(),
            "3.0 free cores (busy 5%, 13 active, 0 reserved) < 4.0"
        );
        let reserved = with_agents(probe("a", 0.05, 16, 50.0, 1), &[("working", 12)]);
        assert!(!evaluate(&reserved, &t).eligible);
    }

    #[test]
    fn idle_and_done_agents_hold_no_declared_demand() {
        let t = thresholds();
        let p = with_agents(
            probe("a", 0.05, 16, 50.0, 0),
            &[("idle", 10), ("done", 10), ("working", 2)],
        );
        assert_eq!(p.agents.active(), 2);
        let h = evaluate(&p, &t);
        assert!((h.headroom - 14.0).abs() < 1e-9);
        assert_eq!(h.live_agents, 12);
    }

    #[test]
    fn measured_load_still_refuses_when_declared_demand_fits() {
        // Work dogsled does not know about: no agents, CPU 90% busy.
        let h = evaluate(&probe("a", 0.9, 16, 50.0, 0), &thresholds());
        assert!(!h.eligible);
        assert!((h.headroom - 1.6).abs() < 1e-9);
    }

    #[test]
    fn active_agents_hold_memory_against_the_host_total() {
        let t = thresholds();
        // 32 GiB total holds 3 GiB for 9 active + 1 new, not for 10 + 1,
        // although 24 GiB is free right now.
        assert!(
            evaluate(
                &with_agents(probe("a", 0.0, 64, 50.0, 0), &[("working", 9)]),
                &t
            )
            .eligible
        );
        let h = evaluate(
            &with_agents(probe("a", 0.0, 64, 50.0, 0), &[("working", 10)]),
            &t,
        );
        assert_eq!(
            h.reason.unwrap(),
            "memory 32.0 GiB total < 33.0 GiB for 10 active + 0 reserved + 1"
        );
    }

    #[test]
    fn picks_most_headroom_then_fewer_agents_then_id() {
        let t = thresholds();
        let mut probes = vec![
            probe("b", 0.25, 8, 50.0, 0), // 6
            probe("a", 0.25, 8, 50.0, 0), // 6
            probe("c", 0.5, 8, 50.0, 0),  // 4
        ];
        assert_eq!(plan(&probes, &t), Plan::Admit { host: "a".into() });
        // Idle agents hold no declared demand, so headroom ties and live agents decide.
        probes[1] = with_agents(probes[1].clone(), &[("idle", 3)]);
        assert_eq!(plan(&probes, &t), Plan::Admit { host: "b".into() });
    }

    #[test]
    fn full_when_no_host_is_eligible() {
        let t = thresholds();
        let probes = vec![probe("a", 1.0, 8, 50.0, 0), probe("b", 0.0, 8, 5.0, 0)];
        assert_eq!(plan(&probes, &t), Plan::Full);
        assert_eq!(plan(&[], &t), Plan::Full);
    }

    #[test]
    fn balances_as_reservations_accumulate() {
        // Two equal hosts: successive admissions alternate because each
        // reservation lowers that host's headroom by one.
        let t = thresholds();
        let mut a = probe("a", 0.25, 8, 50.0, 0);
        let mut b = probe("b", 0.25, 8, 50.0, 0);
        let mut picks = Vec::new();
        for i in 0..4 {
            let Plan::Admit { host } = plan(&[a.clone(), b.clone()], &t) else {
                unreachable!("two idle hosts cannot be full");
            };
            let r = Reservation {
                id: format!("r{i}"),
                created_ms: 0,
            };
            if host == "a" {
                a.reservations.push(r);
            } else {
                b.reservations.push(r);
            }
            picks.push(host);
        }
        assert_eq!(picks, ["a", "b", "a", "b"]);
    }
}
