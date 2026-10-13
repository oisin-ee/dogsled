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
    error: Option<String>,
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
        let out = self.shell.run("ssh", &self.remote_argv(host, args))?;
        if out.status != 0 && out.status != 1 {
            return Err(Error::Ssh {
                target: host.label.clone(),
                detail: format!("exit {}: {}", out.status, out.stderr.trim()),
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
                "--load-per-core-max".into(),
                t.load_per_core_max.to_string(),
                "--disk-free-min-percent".into(),
                t.disk_free_min_percent.to_string(),
                "--reservation-load".into(),
                t.reservation_load.to_string(),
                "--reservation-ttl-ms".into(),
                t.reservation_ttl_ms.to_string(),
            ],
        )
    }

    pub fn bind(&self, host: &Host, id: &str, pane: &str, now: u64) -> Result<()> {
        if host.is_local() {
            return self.node().bind(id, pane, now);
        }
        let _: serde_json::Value = self.remote(
            host,
            &[
                "bind".into(),
                "--id".into(),
                id.into(),
                "--pane".into(),
                pane.into(),
            ],
        )?;
        Ok(())
    }

    pub fn release(&self, host: &Host, id: &str, now: u64) -> Result<()> {
        if host.is_local() {
            return self.node().release(id, now);
        }
        let _: serde_json::Value =
            self.remote(host, &["release".into(), "--id".into(), id.into()])?;
        Ok(())
    }

    pub fn status(&self, now: u64) -> Result<Vec<HostReport>> {
        std::thread::scope(|scope| {
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
        })
    }

    fn read_host(&self, host: &Host, now: u64) -> HostReport {
        let result = if host.is_local() {
            self.ledger
                .read::<Snapshot>("host.json")
                .map(|snapshot| Cached {
                    fetched_ms: now,
                    source: Vec::new(),
                    snapshot,
                    reachable: true,
                    available: true,
                    error: None,
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
            error: Some(e.to_string()),
        });
        let fresh = cached.available && cached.snapshot.as_ref().is_some_and(|s| s.fresh(now));
        let sampled_ms = cached.snapshot.as_ref().map(|s| s.sampled_ms);
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
            if !fresh {
                headroom.eligible = false;
                headroom.reason = Some("service: down".into());
            }
            headroom
        });
        HostReport {
            host: host.clone(),
            service: if fresh { "up" } else { "down" }.into(),
            reachable: cached.reachable,
            sampled_ms,
            probe,
            headroom,
            error: cached.error,
        }
    }

    fn cached_peer(&self, host: &Host, now: u64) -> Result<Cached> {
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
        let name = format!("peer-{key}");
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
            let (snapshot, reachable, available, error) =
                match self.remote::<Option<Snapshot>>(host, &["snapshot".into()]) {
                    Ok(Some(snapshot)) => (Some(snapshot), true, true, None),
                    Ok(None) => (
                        previous.and_then(|c| c.snapshot),
                        true,
                        false,
                        Some("host.json is missing; service: down".into()),
                    ),
                    Err(e) => (
                        previous.and_then(|c| c.snapshot),
                        false,
                        false,
                        Some(e.to_string()),
                    ),
                };
            let cached = Cached {
                fetched_ms: now,
                source: source.clone(),
                snapshot,
                reachable,
                available,
                error,
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
            probe: Probe {
                host: "local".into(),
                label: "local".into(),
                load1: 0.0,
                cores: 8,
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
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
}
