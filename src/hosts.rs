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
    /// Safe words for why the host is in this state; for an unreachable host, its `failure`.
    pub error: Option<String>,
    /// Why an unreachable host could not be read. Never raw ssh or remote text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
    pub jobs: Vec<crate::jobs::Job>,
    #[serde(default)]
    pub handoffs: Vec<crate::done::Mark>,
    /// When an unreachable peer is read again, after backoff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_ms: Option<u64>,
}

/// Why a host could not be read, as a fixed category.
///
/// SSH and remote error text can carry secrets, such as a token in a failing command or a
/// banner. The transport that sees that text classifies it here, and status shows only the
/// category. Raw text never reaches a status line, a headroom reason, or the peer cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    Timeout,
    ConnectionRefused,
    HostNotResolved,
    AuthenticationFailed,
    /// SSH connected, and the remote dogsled command failed.
    RemoteCommandFailed,
    /// The remote command answered with output dogsled cannot read.
    UnexpectedOutput,
    /// Dogsled's own time budget ran out before the host answered.
    Deadline,
    Other,
}

/// The exit status ssh uses for its own failures; any other status is the remote command's.
const SSH_FAILED: i32 = 255;

impl Failure {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::ConnectionRefused => "connection refused",
            Self::HostNotResolved => "host not resolved",
            Self::AuthenticationFailed => "authentication failed",
            Self::RemoteCommandFailed => "remote command failed",
            Self::UnexpectedOutput => "unexpected output",
            Self::Deadline => "deadline reached",
            Self::Other => "other",
        }
    }

    /// Classify a failed ssh run by its exit status and, for ssh's own failures, its stderr.
    fn of_ssh(status: i32, stderr: &str) -> Self {
        if status != SSH_FAILED {
            return Self::RemoteCommandFailed;
        }
        let stderr = stderr.to_lowercase();
        let says = |needles: &[&str]| needles.iter().any(|n| stderr.contains(n));
        if says(&["timed out", "timeout"]) {
            Self::Timeout
        } else if says(&["connection refused"]) {
            Self::ConnectionRefused
        } else if says(&[
            "could not resolve hostname",
            "name or service not known",
            "nodename nor servname",
            "temporary failure in name resolution",
        ]) {
            Self::HostNotResolved
        } else if says(&[
            "permission denied",
            "host key verification failed",
            "too many authentication failures",
            "no supported authentication methods",
        ]) {
            Self::AuthenticationFailed
        } else {
            Self::Other
        }
    }

    /// The category of an error from reading a host.
    #[must_use]
    pub const fn of(error: &Error) -> Self {
        match error {
            Error::Ssh { failure, .. } => *failure,
            Error::Deadline(_) => Self::Deadline,
            Error::Parse { .. } => Self::UnexpectedOutput,
            _ => Self::Other,
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Environment variable naming a local file that receives the raw ssh and remote error text.
/// Unset by default, so raw text goes nowhere. This is the only sink for it: it is never put
/// in an `Error`, a ticket, a handoff, a unit record, or a log line.
pub const SSH_DEBUG_FILE: &str = "DOGSLED_SSH_DEBUG_FILE";

fn record_ssh_detail(target: &str, detail: &str) {
    let Some(path) = std::env::var_os(SSH_DEBUG_FILE) else {
        return;
    };
    // A debug aid: a failed or refused write must not change the failure being reported.
    let _ = write_ssh_detail(
        std::path::Path::new(&path),
        &format!("ssh {target}: {detail}"),
    );
}

fn write_ssh_detail(path: &std::path::Path, line: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
    let mut options = std::fs::OpenOptions::new();
    // O_NOFOLLOW: a symlink is refused. O_NONBLOCK: a FIFO with no reader fails instead of
    // hanging. The mode applies only on creation, so the opened file is checked below.
    options
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    let mut file = options.open(path)?;
    let meta = file.metadata()?;
    let owned = meta.uid() == nix::unistd::geteuid().as_raw();
    if !meta.is_file() || !owned || meta.mode() & 0o077 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "ssh debug file must be a regular file owned by this user with mode 0600",
        ));
    }
    writeln!(file, "{line}")
}

/// `unreachable: <category>` for a host that could not be read.
/// A host that answered but runs no service is `service: down`, never this.
#[must_use]
pub fn unreachable(failure: Failure) -> String {
    format!("unreachable: {failure}")
}

impl HostReport {
    /// Why the host is in this state, for one status line: unreachable, or its service.
    #[must_use]
    pub fn condition(&self) -> String {
        if self.reachable {
            format!("service: {}", self.service)
        } else {
            unreachable(self.failure.unwrap_or(Failure::Other))
        }
    }

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
    #[serde(default)]
    handoffs: Vec<crate::done::Mark>,
    /// Safe words for a reachable host only; an unreachable one carries its `failure`.
    error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failure: Option<Failure>,
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
                record_ssh_detail(
                    &host.label,
                    value["error"].as_str().unwrap_or("remote deadline"),
                );
                return Err(Error::Deadline(format!(
                    "ssh {}: remote deadline",
                    host.label
                )));
            }
            let refusal = serde_json::from_str::<serde_json::Value>(&out.stdout)
                .ok()
                .and_then(|v| {
                    v["code"]
                        .as_str()
                        .and_then(crate::error::Refusal::from_code)
                });
            let detail = serde_json::from_str::<serde_json::Value>(&out.stdout)
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_owned))
                .unwrap_or_else(|| out.stderr.trim().to_owned());
            record_ssh_detail(&host.label, &format!("exit {}: {detail}", out.status));
            return Err(Error::Ssh {
                target: host.label.clone(),
                failure: Failure::of_ssh(out.status, &out.stderr),
                refusal,
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
                handoffs: report.handoffs,
                error: None,
                failure: None,
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
            handoffs: Vec::new(),
            error: None,
            failure: Some(Failure::of(&e)),
            retry: crate::backoff::Backoff::default(),
        });
        // Only the category describes an unreachable host. This also covers a cache written
        // before categories existed, which may still hold raw ssh text.
        let failure = (!cached.reachable).then(|| cached.failure.unwrap_or(Failure::Other));
        let error = failure.map_or_else(|| cached.error.clone(), |f| Some(f.to_string()));
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
                let reason = if let Some(failure) = failure {
                    unreachable(failure)
                } else if running {
                    "snapshot: stale".into()
                } else {
                    "service: down".into()
                };
                headroom.reason = Some(sampled_ms.map_or_else(
                    || reason.clone(),
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
            error,
            failure,
            jobs,
            handoffs: cached.handoffs,
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
            let (snapshot, reachable, available, running, handoffs, error, failure) = match self
                .remote::<Option<
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
                        report.handoffs,
                        None,
                        None,
                    )
                }
                Ok(None) => (
                    previous.and_then(|c| c.snapshot),
                    true,
                    false,
                    None,
                    Vec::new(),
                    Some("host.json is missing; service: down".into()),
                    None,
                ),
                Err(e) => (
                    previous.and_then(|c| c.snapshot),
                    false,
                    false,
                    None,
                    Vec::new(),
                    None,
                    Some(Failure::of(&e)),
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
                handoffs,
                error,
                failure,
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
    use crate::error::Refusal;
    use crate::ledger::new_id;
    use crate::probe::{AgentCounts, Memory};
    use crate::shell::Fake;

    fn debug_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(new_id());
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn debug_file_is_created_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let path = debug_dir().join("new.log");
        write_ssh_detail(&path, "ssh peer: boom").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "ssh peer: boom\n");
    }

    #[test]
    fn debug_file_refuses_an_existing_group_or_world_readable_file() {
        use std::os::unix::fs::PermissionsExt as _;
        let path = debug_dir().join("open.log");
        std::fs::write(&path, "").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(write_ssh_detail(&path, "secret").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o644, "refusal must not chmod and continue");
    }

    #[test]
    fn debug_file_refuses_a_symlink_and_leaves_its_target_alone() {
        let dir = debug_dir();
        let target = dir.join("target.log");
        std::fs::write(&target, "").unwrap();
        let link = dir.join("link.log");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(write_ssh_detail(&link, "secret").is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "");
    }

    #[test]
    fn debug_file_refuses_a_non_regular_path() {
        assert!(write_ssh_detail(std::path::Path::new("/dev/null"), "secret").is_err());
        assert!(write_ssh_detail(&debug_dir(), "secret").is_err());
    }

    #[test]
    fn debug_sink_writes_nothing_when_the_variable_is_unset() {
        // Reads the real environment, so only assert when it is unset for this process.
        if std::env::var_os(SSH_DEBUG_FILE).is_none() {
            record_ssh_detail("peer", "secret");
        }
    }

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
    fn status_words_an_unreachable_peer_apart_from_a_stopped_service() {
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
        let command = format!(
            "ssh {}",
            fleet.remote_argv(&host, &["snapshot".into()]).join(" ")
        );
        let snapshot = serde_json::to_string(&Snapshot {
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
        })
        .unwrap();
        let snapshot = snapshot.as_str();
        // A reachable host whose service is not running keeps `service: down`.
        shell.on(&command, snapshot);
        fleet.status(100).unwrap();
        shell.on(&command, "null");
        let stopped = fleet.status(20_100).unwrap().remove(0);
        assert!(stopped.reachable);
        assert_eq!(stopped.condition(), "service: down");
        assert_eq!(
            stopped.headroom.unwrap().reason.as_deref(),
            Some("service: down (last snapshot 20s ago)")
        );
        // A peer that SSH cannot read is unreachable, with the age of its last snapshot.
        let ledger = Ledger::new(std::env::temp_dir().join(new_id()));
        let fleet = Fleet {
            ledger: &ledger,
            ..fleet
        };
        shell.on(&command, snapshot);
        fleet.status(100).unwrap();
        shell.on_output(
            &command,
            crate::shell::Output {
                status: 255,
                stdout: String::new(),
                stderr: "ssh: connect to host peer port 22: Operation timed out\n".into(),
            },
        );
        let asleep = fleet.status(20_100).unwrap().remove(0);
        assert!(!asleep.reachable);
        assert_eq!(asleep.failure, Some(Failure::Timeout));
        assert_eq!(asleep.condition(), "unreachable: timeout");
        assert_eq!(asleep.error.as_deref(), Some("timeout"));
        assert_eq!(
            asleep.headroom.unwrap().reason.as_deref(),
            Some("unreachable: timeout (last snapshot 20s ago)")
        );
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    const SECRET: &str = "token=EXAMPLE_SECRET";

    fn ssh_failure(status: i32, stderr: &str) -> Failure {
        Failure::of_ssh(status, stderr)
    }

    #[test]
    fn ssh_failures_map_to_safe_categories() {
        for (status, stderr, want) in [
            (
                255,
                "ssh: connect to host h port 22: Operation timed out",
                Failure::Timeout,
            ),
            (
                255,
                "ssh: connect to host h port 22: Connection timed out",
                Failure::Timeout,
            ),
            (
                255,
                "ssh: connect to host h port 22: Connection refused",
                Failure::ConnectionRefused,
            ),
            (
                255,
                "ssh: Could not resolve hostname h: Name or service not known",
                Failure::HostNotResolved,
            ),
            (
                255,
                "ssh: Could not resolve hostname h: nodename nor servname provided",
                Failure::HostNotResolved,
            ),
            (
                255,
                "user@h: Permission denied (publickey).",
                Failure::AuthenticationFailed,
            ),
            (
                255,
                "Host key verification failed.",
                Failure::AuthenticationFailed,
            ),
            (255, "something unrecognised", Failure::Other),
            (255, "", Failure::Other),
            (2, "remote said no", Failure::RemoteCommandFailed),
            // A remote command's own output never picks a connection category.
            (2, "Connection refused", Failure::RemoteCommandFailed),
        ] {
            assert_eq!(ssh_failure(status, stderr), want, "{status} {stderr}");
        }
        assert_eq!(Failure::of(&Error::Deadline("x".into())), Failure::Deadline);
        assert_eq!(
            Failure::of(&Error::Parse {
                origin: "o".into(),
                detail: "d".into()
            }),
            Failure::UnexpectedOutput
        );
        assert_eq!(Failure::of(&Error::Full), Failure::Other);
        assert_eq!(
            unreachable(Failure::ConnectionRefused),
            "unreachable: connection refused"
        );
    }

    #[test]
    fn status_never_shows_raw_ssh_or_remote_text() {
        for (status, stdout, stderr, want) in [
            (
                255,
                String::new(),
                format!("ssh: connect to host peer: Connection refused {SECRET}\n"),
                "connection refused",
            ),
            (
                255,
                String::new(),
                format!("{SECRET}\nunrecognised"),
                "other",
            ),
            (
                2,
                String::new(),
                format!("remote failed {SECRET}"),
                "remote command failed",
            ),
            (
                2,
                format!("{{\"error\":\"{SECRET}\"}}"),
                String::new(),
                "remote command failed",
            ),
        ] {
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
            let command = format!(
                "ssh {}",
                fleet.remote_argv(&host, &["snapshot".into()]).join(" ")
            );
            shell.on_output(
                &command,
                crate::shell::Output {
                    status,
                    stdout,
                    stderr,
                },
            );
            let report = fleet.status(100).unwrap().remove(0);
            assert_eq!(report.condition(), format!("unreachable: {want}"));
            let printed = serde_json::to_string(&report).unwrap();
            assert!(!printed.contains("EXAMPLE_SECRET"), "{printed}");
            assert!(!format!("{report:?}").contains("EXAMPLE_SECRET"));
            let cache = std::fs::read_to_string(
                ledger.dir.join(format!("{}.json", Fleet::peer_file(&host))),
            )
            .unwrap();
            assert!(!cache.contains("EXAMPLE_SECRET"), "{cache}");
            std::fs::remove_dir_all(ledger.dir).unwrap();
        }
    }

    #[test]
    fn an_ssh_error_never_carries_raw_text_to_any_sink() {
        let host = Host {
            id: "peer".into(),
            label: "peer".into(),
            target: Some("peer-ssh".into()),
        };
        for (status, stdout, stderr, category) in [
            (
                255,
                String::new(),
                format!("Connection refused {SECRET}"),
                "connection refused",
            ),
            (
                255,
                String::new(),
                format!("{SECRET} unrecognised"),
                "other",
            ),
            (
                2,
                String::new(),
                format!("remote failed {SECRET}"),
                "remote command failed",
            ),
            (
                2,
                format!("{{\"error\":\"{SECRET}\"}}"),
                String::new(),
                "remote command failed",
            ),
            (
                3,
                format!("{{\"deadline\":true,\"error\":\"{SECRET}\"}}"),
                String::new(),
                "deadline reached",
            ),
        ] {
            let out = crate::shell::Output {
                status,
                stdout,
                stderr,
            };
            let error =
                Fleet::parse_remote::<serde_json::Value>(&host, &["x".into()], &out).unwrap_err();
            // The sinks: ticket and attempt errors, handoff text, unit evidence log, unit JSON.
            let sinks = [
                error.to_string(),
                format!("{error:?}"),
                format!("attempt 1: {error}"),
                format!("{error} (after 3 attempts)"),
                format!("dogsled unit evidence r: {error}"),
                serde_json::json!({"error":{"code":"LAUNCH_UNRESOLVED","message":error.to_string()}})
                    .to_string(),
            ];
            for sink in sinks {
                assert!(!sink.contains("EXAMPLE_SECRET"), "{sink}");
            }
            assert!(error.to_string().contains("ssh peer"), "{error}");
            assert!(
                error.to_string().contains(category) || matches!(error, Error::Deadline(_)),
                "{error}"
            );
        }
    }

    #[test]
    fn a_peer_refusal_code_maps_to_a_fixed_message_and_unknown_text_stays_hidden() {
        let host = Host {
            id: "peer".into(),
            label: "peer".into(),
            target: Some("peer-ssh".into()),
        };
        let read = |stdout: String| {
            let out = crate::shell::Output {
                status: 2,
                stdout,
                stderr: String::new(),
            };
            Fleet::parse_remote::<serde_json::Value>(&host, &["x".into()], &out)
                .unwrap_err()
                .to_string()
        };
        for refusal in [Refusal::NoInboxReader, Refusal::CloneFailed] {
            let shown = read(format!(
                "{{\"error\":\"{SECRET}\",\"code\":\"{}\"}}",
                refusal.code()
            ));
            assert_eq!(shown, format!("ssh peer: {}", refusal.message()));
            assert!(!shown.contains("EXAMPLE_SECRET"), "{shown}");
        }
        for stdout in [
            format!("{{\"error\":\"{SECRET}\",\"code\":\"SOMETHING_NEW\"}}"),
            format!("{{\"error\":\"{SECRET}\",\"code\":7}}"),
            format!("{{\"error\":\"{SECRET}\"}}"),
        ] {
            let shown = read(stdout);
            assert_eq!(shown, "ssh peer: remote command failed");
        }
    }

    #[test]
    fn a_cache_with_raw_text_from_before_categories_is_not_shown() {
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
        let old = serde_json::json!({
            "fetched_ms": 100,
            "source": fleet.remote_argv(&host, &["snapshot".into()]),
            "snapshot": null, "reachable": false, "available": false,
            "error": format!("ssh peer: exit 255: {SECRET}")
        });
        ledger
            .write(&format!("{}.json", Fleet::peer_file(&host)), &old)
            .unwrap();
        let report = fleet.status(100).unwrap().remove(0);
        assert_eq!(report.condition(), "unreachable: other");
        assert!(
            !serde_json::to_string(&report)
                .unwrap()
                .contains("EXAMPLE_SECRET")
        );
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
