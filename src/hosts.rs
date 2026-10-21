//! Fleet discovery, parallel snapshot reads, and the shared SSH transport.

use crate::config::{Policy, Thresholds};
use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::node::{Node, ReserveOutcome, Snapshot};
use crate::plan::{Headroom, evaluate};
use crate::probe::Probe;
use crate::shell::{Shell, quote};
use serde::{Deserialize, Serialize};

pub const LOCAL: &str = "local";
const CACHE_MS: u64 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

impl Host {
    pub fn local() -> Self {
        Self {
            id: LOCAL.into(),
            label: LOCAL.into(),
            target: None,
        }
    }
    pub fn is_local(&self) -> bool {
        self.target.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostReport {
    #[serde(flatten)]
    pub host: Host,
    pub service: String,
    pub reachable: bool,
    pub sampled_ms: Option<u64>,
    pub probe: Option<Probe>,
    pub headroom: Option<Headroom>,
    pub error: Option<String>,
    pub jobs: Vec<crate::jobs::Job>,
    /// When an unreachable peer is read again, after backoff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_ms: Option<u64>,
}

impl HostReport {
    /// Readers that need current measurements must not substitute service liveness for age.
    pub fn fresh(&self, now: u64) -> bool {
        self.reachable
            && self.service == "up"
            && self
                .sampled_ms
                .is_some_and(|sampled| now.saturating_sub(sampled) <= crate::node::STALE_MS)
    }
}

#[derive(Deserialize)]
struct MachineRow {
    id: String,
    label: String,
    target: String,
    #[serde(default = "default_true")]
    enabled: bool,
}
fn default_true() -> bool {
    true
}

fn parse_machine_list(text: &str) -> Result<Vec<Host>> {
    let rows: Vec<MachineRow> = serde_json::from_str(text)?;
    Ok(rows
        .into_iter()
        .filter(|m| m.enabled)
        .map(|m| Host {
            id: m.id,
            label: m.label,
            target: Some(m.target),
        })
        .collect())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Cached {
    fetched_ms: u64,
    source: Vec<String>,
    snapshot: Option<Snapshot>,
    reachable: bool,
    available: bool,
    #[serde(default)]
    running: Option<bool>,
    error: Option<String>,
    /// Consecutive failed reads: an unreachable peer is asked again with backoff, not on
    /// every cache expiry.
    #[serde(default)]
    retry: crate::backoff::Backoff,
}

#[derive(Debug)]
pub struct Fleet<'a> {
    pub shell: &'a dyn Shell,
    pub ledger: &'a Ledger,
    pub policy: &'a Policy,
    pub home: String,
    pub hosts: Vec<Host>,
}

impl<'a> Fleet<'a> {
    pub fn discover(
        shell: &'a dyn Shell,
        ledger: &'a Ledger,
        policy: &'a Policy,
        home: String,
    ) -> Result<Self> {
        let out = shell.run_ok("herdr", &["machine".into(), "list".into(), "--json".into()])?;
        let mut hosts = vec![Host::local()];
        hosts.extend(parse_machine_list(&out.stdout)?);
        Ok(Self {
            shell,
            ledger,
            policy,
            home,
            hosts,
        })
    }

    pub fn thresholds(&self) -> Thresholds {
        Thresholds::from(self.policy)
    }

    pub fn find(&self, host: &str) -> Result<&Host> {
        self.hosts
            .iter()
            .find(|h| h.id == host || h.label == host)
            .ok_or_else(|| Error::UnknownMachine(host.into()))
    }

    pub fn node(&self) -> Node<'_> {
        Node {
            shell: self.shell,
            ledger: self.ledger,
            thresholds: self.thresholds(),
            home: &self.home,
            cleanup: &self.policy.cleanup,
        }
    }

    pub fn remote_argv(&self, host: &Host, args: &[String]) -> Vec<String> {
        let words: Vec<_> = self
            .policy
            .remote_command
            .iter()
            .cloned()
            .chain(["host".into()])
            .chain(args.iter().cloned())
            .chain(["--json".into()])
            .collect();
        vec![
            "-T".into(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ConnectTimeout=5".into(),
            "--".into(),
            host.target.clone().unwrap_or_default(),
            words.iter().map(|w| quote(w)).collect::<Vec<_>>().join(" "),
        ]
    }

    pub fn remote<T: for<'de> Deserialize<'de>>(&self, host: &Host, args: &[String]) -> Result<T> {
        self.remote_bounded(host, args, self.shell.budget().remaining("ssh")?)
    }

    pub fn remote_setup<T: for<'de> Deserialize<'de>>(
        &self,
        host: &Host,
        args: &[String],
    ) -> Result<T> {
        self.remote_bounded(host, args, crate::budget::SETUP_LIMIT)
    }

    pub fn remote_bounded<T: for<'de> Deserialize<'de>>(
        &self,
        host: &Host,
        args: &[String],
        timeout: std::time::Duration,
    ) -> Result<T> {
        let timeout = self.shell.budget().with_limit("ssh", timeout)?;
        let mut bounded_args = args.to_vec();
        if self.shell.budget().deadline.is_some() {
            bounded_args.extend(["--budget-ms".into(), timeout.as_millis().to_string()]);
        }
        let out = self
            .shell
            .run_bounded("ssh", &self.remote_argv(host, &bounded_args), timeout)?;
        Self::parse_remote(host, args, &out)
    }

    fn parse_remote<T: for<'de> Deserialize<'de>>(
        host: &Host,
        args: &[String],
        out: &crate::shell::Output,
    ) -> Result<T> {
        if out.status != 0 && out.status != 1 {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&out.stdout)
                && value["deadline"] == true
            {
                return Err(Error::Deadline(format!(
                    "ssh {}: {}",
                    host.label,
                    value["error"].as_str().unwrap_or("remote deadline")
                )));
            }
            let detail = serde_json::from_str::<serde_json::Value>(&out.stdout)
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_owned))
                .unwrap_or_else(|| out.stderr.trim().to_owned());
            return Err(Error::Ssh {
                target: host.label.clone(),
                detail: format!("exit {}: {detail}", out.status),
            });
        }
        serde_json::from_str(&out.stdout).map_err(|e| Error::Parse {
            origin: format!("{} host {}", host.label, args.join(" ")),
            detail: e.to_string(),
        })
    }

    pub fn reserve(&self, host: &Host, id: &str, now: u64) -> Result<ReserveOutcome> {
        if host.is_local() {
            return self.node().reserve(id, now);
        }
        let t = self.thresholds();
        self.remote(
            host,
            &[
                "reserve".into(),
                "--id".into(),
                id.into(),
                "--free-core-fraction-min".into(),
                t.free_core_fraction_min.to_string(),
                "--memory-reserve-bytes".into(),
                t.memory_reserve_bytes.to_string(),
                "--disk-free-min-percent".into(),
                t.disk_free_min_percent.to_string(),
                "--reservation-load".into(),
                t.reservation_load.to_string(),
                "--reservation-ttl-ms".into(),
                t.reservation_ttl_ms.to_string(),
            ],
        )
    }

    pub fn status(&self, now: u64) -> Result<Vec<HostReport>> {
        let reports = std::thread::scope(|scope| {
            let handles: Vec<_> = self
                .hosts
                .iter()
                .map(|host| (host, scope.spawn(move || self.read_host(host, now))))
                .collect();
            handles
                .into_iter()
                .map(|(host, handle)| {
                    handle
                        .join()
                        .map_err(|_| Error::Invalid(format!("{} status worker failed", host.label)))
                })
                .collect()
        });
        self.shell.budget().remaining("fleet status")?;
        reports
    }

    fn read_host(&self, host: &Host, now: u64) -> HostReport {
        let result = if host.is_local() {
            self.node().snapshot().map(|report| Cached {
                fetched_ms: now,
                source: Vec::new(),
                snapshot: report.snapshot,
                reachable: true,
                available: true,
                running: report.running,
                error: None,
                retry: crate::backoff::Backoff::default(),
            })
        } else {
            self.cached_peer(host, now)
        };
        let cached = result.unwrap_or_else(|e| Cached {
            fetched_ms: now,
            source: Vec::new(),
            snapshot: None,
            reachable: false,
            available: false,
            running: None,
            error: Some(e.to_string()),
            retry: crate::backoff::Backoff::default(),
        });
        let fresh = cached.available && cached.snapshot.as_ref().is_some_and(|s| s.fresh(now));
        // Old peers have no liveness field. Only they retain the age-based fallback.
        let running = cached.reachable && cached.running.unwrap_or(fresh);
        let sampled_ms = cached.snapshot.as_ref().map(|s| s.sampled_ms);
        let jobs = cached
            .snapshot
            .as_ref()
            .map_or_else(Vec::new, |s| s.jobs.clone());
        let probe = cached.snapshot.map(|s| {
            let mut probe = s.probe;
            probe.host.clone_from(&host.id);
            probe.label.clone_from(&host.label);
            probe
                .reservations
                .retain(|r| now.saturating_sub(r.created_ms) < self.policy.reservation_ttl_ms);
            probe
        });
        let headroom = probe.as_ref().map(|probe| {
            let mut headroom = evaluate(probe, &self.thresholds());
            if !running || !fresh {
                headroom.eligible = false;
                let reason = if running {
                    "snapshot: stale"
                } else {
                    "service: down"
                };
                headroom.reason = Some(sampled_ms.map_or_else(
                    || reason.into(),
                    |sampled| {
                        format!(
                            "{reason} (last snapshot {} ago)",
                            crate::watchdog::age(now.saturating_sub(sampled))
                        )
                    },
                ));
            }
            headroom
        });
        HostReport {
            host: host.clone(),
            service: if running { "up" } else { "down" }.into(),
            reachable: cached.reachable,
            sampled_ms,
            probe,
            headroom,
            error: cached.error,
            jobs,
            retry_ms: (!cached.reachable && cached.retry.attempts > 0)
                .then_some(cached.retry.next_ms),
        }
    }

    /// Clear `host`'s backoff and cache so the next status reads it at once ("retry now").
    pub fn retry_now(&self, host: &Host) -> Result<()> {
        if host.is_local() {
            return Ok(());
        }
        let name = Self::peer_file(host);
        self.ledger.with_lock(&name, || {
            let file = format!("{name}.json");
            if let Some(mut cached) = self.ledger.read::<Cached>(&file)? {
                cached.retry.reset();
                cached.fetched_ms = 0;
                self.ledger.write(&file, &cached)?;
            }
            Ok(())
        })
    }

    fn peer_file(host: &Host) -> String {
        // Hex encoding prevents host ids from becoming path components.
        let key = host
            .id
            .as_bytes()
            .iter()
            .flat_map(|b| {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                [
                    char::from(HEX[usize::from(b >> 4)]),
                    char::from(HEX[usize::from(b & 15)]),
                ]
            })
            .collect::<String>();
        format!("peer-{key}")
    }

    fn cached_peer(&self, host: &Host, now: u64) -> Result<Cached> {
        let name = Self::peer_file(host);
        let file = format!("{name}.json");
        let source = self.remote_argv(host, &["snapshot".into()]);
        self.ledger.with_lock(&name, || {
            let previous: Option<Cached> = self
                .ledger
                .read::<Cached>(&file)?
                .filter(|c| c.source == source);
            if let Some(cached) = &previous
                && now.saturating_sub(cached.fetched_ms) < CACHE_MS
            {
                return Ok(cached.clone());
            }
            if let Some(cached) = &previous
                && !cached.retry.ready(now)
            {
                return Ok(cached.clone());
            }
            let mut retry = previous.as_ref().map(|c| c.retry).unwrap_or_default();
            let (snapshot, reachable, available, running, error) = match self.remote::<Option<
                crate::node::SnapshotReport,
            >>(
                host,
                &["snapshot".into()],
            ) {
                Ok(Some(report)) => {
                    let available = report.snapshot.is_some();
                    (
                        report
                            .snapshot
                            .or_else(|| previous.and_then(|c| c.snapshot)),
                        true,
                        available,
                        report.running,
                        None,
                    )
                }
                Ok(None) => (
                    previous.and_then(|c| c.snapshot),
                    true,
                    false,
                    None,
                    Some("host.json is missing; service: down".into()),
                ),
                Err(e) => (
                    previous.and_then(|c| c.snapshot),
                    false,
                    false,
                    None,
                    Some(e.to_string()),
                ),
            };
            if reachable {
                retry.reset();
            } else {
                retry.fail(now, &host.id);
            }
            let cached = Cached {
                fetched_ms: now,
                source: source.clone(),
                snapshot,
                reachable,
                available,
                running,
                error,
                retry,
            };
            self.ledger.write(&file, &cached)?;
            Ok(cached)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::new_id;
    use crate::probe::{AgentCounts, Memory};
    use crate::shell::Fake;

    #[test]
    fn cache_and_service_boundaries_use_injected_time() {
        let shell = Fake::new();
        let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
        let policy = Policy::default();
        let host = Host {
            id: "peer".into(),
            label: "peer".into(),
            target: Some("peer-ssh".into()),
        };
        let fleet = Fleet {
            shell: &shell,
            ledger: &ledger,
            policy: &policy,
            home: "/home".into(),
            hosts: vec![host.clone()],
        };
        let snapshot = Snapshot {
            sampled_ms: 100,
            jobs: Vec::new(),
            probe: Probe {
                host: "local".into(),
                label: "local".into(),
                load1: 0.0,
                cores: 8,
                cpu_ticks: None,
                busy: Some(0.0),
                memory: Memory::default(),
                disk_free_percent: 50.0,
                agents: AgentCounts::default(),
                agent_panes: Vec::new(),
                reservations: Vec::new(),
                panes: Vec::new(),
            },
        };
        let command = format!(
            "ssh {}",
            fleet.remote_argv(&host, &["snapshot".into()]).join(" ")
        );
        shell.on(&command, &serde_json::to_string(&snapshot).unwrap());
        assert_eq!(fleet.status(100).unwrap()[0].service, "up");
        shell.on(&command, "null");
        assert_eq!(fleet.status(10_099).unwrap()[0].service, "up");
        let missing = fleet.status(10_100).unwrap();
        assert_eq!(missing[0].service, "down");
        assert!(missing[0].reachable);
        assert!(missing[0].probe.is_some());
        shell.on(&command, &serde_json::to_string(&snapshot).unwrap());
        assert_eq!(fleet.status(30_100).unwrap()[0].service, "up");
        assert_eq!(fleet.status(30_101).unwrap()[0].service, "down");
        let stale = fleet.status(44_100).unwrap();
        assert_eq!(stale[0].service, "down");
        assert_eq!(
            stale[0].headroom.as_ref().unwrap().reason.as_deref(),
            Some("service: down (last snapshot 44s ago)")
        );
        // New peers report process liveness independently of their sample timestamp.
        let mut wire = serde_json::to_value(&snapshot).unwrap();
        wire["running"] = serde_json::json!(true);
        shell.on(&command, &wire.to_string());
        let late = fleet.status(60_100).unwrap();
        assert_eq!(late[0].service, "up");
        assert_eq!(late[0].sampled_ms, Some(100));
        assert!(!late[0].headroom.as_ref().unwrap().eligible);
        wire["running"] = serde_json::json!(false);
        shell.on(&command, &wire.to_string());
        assert_eq!(fleet.status(70_100).unwrap()[0].service, "down");
        shell.on(&command, r#"{"running":true}"#);
        let starting = fleet.status(80_100).unwrap();
        assert_eq!(starting[0].service, "up");
        assert!(!starting[0].headroom.as_ref().unwrap().eligible);
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn an_unreachable_peer_is_asked_again_with_backoff_and_at_once_after_it_answers() {
        let shell = Fake::new();
        let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
        let policy = Policy::default();
        let host = Host {
            id: "peer".into(),
            label: "peer".into(),
            target: Some("peer-ssh".into()),
        };
        let fleet = Fleet {
            shell: &shell,
            ledger: &ledger,
            policy: &policy,
            home: "/home".into(),
            hosts: vec![host.clone()],
        };
        let reads = || {
            shell
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.starts_with("ssh "))
                .count()
        };
        // No scripted answer: every read fails.
        assert!(!fleet.status(0).unwrap()[0].reachable);
        assert_eq!(reads(), 1);
        // A retry waits for both the cache and the backoff.
        let first = crate::backoff::delay(1, "peer").max(CACHE_MS);
        fleet.status(first - 1).unwrap();
        assert_eq!(reads(), 1);
        fleet.status(first).unwrap();
        assert_eq!(reads(), 2);
        let second = first + crate::backoff::delay(2, "peer");
        fleet.status(second - 1).unwrap();
        assert_eq!(reads(), 2);
        let command = format!(
            "ssh {}",
            fleet.remote_argv(&host, &["snapshot".into()]).join(" ")
        );
        shell.on(&command, "null");
        fleet.status(second).unwrap();
        assert_eq!(reads(), 3);
        // Reachable again: the next read follows the plain cache.
        fleet.status(second + CACHE_MS).unwrap();
        assert_eq!(reads(), 4);
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
}
