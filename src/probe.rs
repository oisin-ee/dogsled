//! Local measurements. The service collects agents and all panes separately.

use crate::error::{Error, Result};
use crate::ledger::Reservation;
use crate::shell::Shell;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCounts {
    pub total: usize,
    pub by_status: BTreeMap<String, usize>,
}

impl AgentCounts {
    pub fn live(&self) -> usize {
        self.total
            .saturating_sub(self.by_status.get("done").copied().unwrap_or(0))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pane {
    pub pane_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_status: Option<String>,
}

/// Stable service contract, independent of Herdr's raw response fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    pub pane: String,
    pub agent: Option<String>,
    pub state: String,
    pub name: Option<String>,
    pub transcript: Option<String>,
    pub transcript_modified: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RawAgent {
    pane_id: String,
    agent: Option<String>,
    agent_status: Option<String>,
    name: Option<String>,
    agent_session: Option<AgentSession>,
}

#[derive(Debug, Deserialize)]
struct AgentSession {
    kind: String,
    value: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Memory {
    pub used_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Probe {
    pub host: String,
    pub label: String,
    pub load1: f64,
    pub cores: u32,
    pub memory: Memory,
    pub disk_free_percent: f64,
    pub agents: AgentCounts,
    pub agent_panes: Vec<Agent>,
    pub reservations: Vec<Reservation>,
    pub panes: Vec<Pane>,
}

pub fn local(shell: &dyn Shell, home: &str) -> Result<Probe> {
    let agent_panes = agent_list(shell)?;
    let mut agents = AgentCounts::default();
    for pane in &agent_panes {
        agents.total += 1;
        *agents.by_status.entry(pane.state.clone()).or_default() += 1;
    }
    let panes = shell.run_ok("herdr", &["pane".into(), "list".into()])?;
    let value: serde_json::Value = serde_json::from_str(&panes.stdout)?;
    let panes = value
        .pointer("/result/panes")
        .ok_or_else(|| parse_error("herdr pane list", &panes.stdout))?;
    Ok(Probe {
        host: "local".into(),
        label: "local".into(),
        load1: load1(shell)?,
        cores: cores(),
        memory: memory(shell)?,
        disk_free_percent: disk_free_percent(shell, home)?,
        agents,
        agent_panes,
        reservations: Vec::new(),
        panes: serde_json::from_value(panes.clone())?,
    })
}

pub fn agent_list(shell: &dyn Shell) -> Result<Vec<Agent>> {
    let out = shell.run_ok("herdr", &["agent".into(), "list".into()])?;
    let value: serde_json::Value = serde_json::from_str(&out.stdout)?;
    let agents = value
        .pointer("/result/agents")
        .ok_or_else(|| parse_error("herdr agent list", &out.stdout))?;
    let agents: Vec<RawAgent> = serde_json::from_value(agents.clone())?;
    Ok(agents
        .into_iter()
        .map(|raw| {
            let transcript = raw
                .agent_session
                .filter(|session| session.kind == "path")
                .map(|session| session.value);
            let transcript_modified = transcript.as_deref().and_then(|path| {
                std::fs::metadata(path)
                    .ok()?
                    .modified()
                    .ok()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .and_then(|duration| u64::try_from(duration.as_millis()).ok())
            });
            Agent {
                pane: raw.pane_id,
                agent: raw.agent,
                state: raw.agent_status.unwrap_or_else(|| "unknown".into()),
                name: raw.name,
                transcript,
                transcript_modified,
            }
        })
        .collect())
}

fn parse_error(origin: &str, detail: &str) -> Error {
    Error::Parse {
        origin: origin.into(),
        detail: detail.trim().into(),
    }
}

pub fn cores() -> u32 {
    std::thread::available_parallelism().map_or(1, |n| n.get() as u32)
}

pub fn load1(shell: &dyn Shell) -> Result<f64> {
    let text = match shell.read_file("/proc/loadavg") {
        Ok(text) => text,
        Err(_) => {
            shell
                .run_ok("sysctl", &["-n".into(), "vm.loadavg".into()])?
                .stdout
        }
    };
    parse_loadavg(&text).ok_or_else(|| parse_error("load average", &text))
}

pub fn parse_loadavg(text: &str) -> Option<f64> {
    text.split(|c: char| c.is_whitespace() || c == '{' || c == '}')
        .find(|s| !s.is_empty())
        .and_then(|s| s.parse().ok())
}

pub fn disk_free_percent(shell: &dyn Shell, path: &str) -> Result<f64> {
    let out = shell.run_ok("df", &["-Pk".into(), path.into()])?;
    parse_df(&out.stdout).ok_or_else(|| parse_error("df -Pk", &out.stdout))
}

pub fn parse_df(text: &str) -> Option<f64> {
    let cols: Vec<_> = text
        .lines()
        .rfind(|l| !l.trim().is_empty())?
        .split_whitespace()
        .collect();
    let total: f64 = cols.get(1)?.parse().ok()?;
    let available: f64 = cols.get(3)?.parse().ok()?;
    (total > 0.0).then_some(available / total * 100.0)
}

pub fn memory(shell: &dyn Shell) -> Result<Memory> {
    if let Ok(meminfo) = shell.read_file("/proc/meminfo") {
        if let Some(memory) = cgroup_memory(shell)? {
            return Ok(memory);
        }
        return parse_meminfo(&meminfo).ok_or_else(|| parse_error("/proc/meminfo", &meminfo));
    }
    let total = shell
        .run_ok("sysctl", &["-n".into(), "hw.memsize".into()])?
        .stdout;
    let vm = shell.run_ok("vm_stat", &[])?.stdout;
    parse_vm_stat(
        &vm,
        total
            .trim()
            .parse()
            .map_err(|_| parse_error("hw.memsize", &total))?,
    )
    .ok_or_else(|| parse_error("vm_stat", &vm))
}

/// The smallest finite ancestor limit governs the process. Measure that
/// scope's working set, excluding inactive file cache.
fn cgroup_memory(shell: &dyn Shell) -> Result<Option<Memory>> {
    let Ok(cgroup) = shell.read_file("/proc/self/cgroup") else {
        return Ok(None);
    };
    let Some(path) = cgroup.lines().find_map(|line| line.strip_prefix("0::")) else {
        return Ok(None);
    };
    let root = std::path::Path::new("/sys/fs/cgroup");
    let process = root.join(path.trim_start_matches('/'));
    let mut governing: Option<(std::path::PathBuf, u64)> = None;
    for scope in process
        .ancestors()
        .take_while(|scope| scope.starts_with(root))
    {
        let Ok(limit) = shell.read_file(&scope.join("memory.max").display().to_string()) else {
            continue;
        };
        if limit.trim() == "max" {
            continue;
        }
        let limit: u64 = limit
            .trim()
            .parse()
            .map_err(|_| parse_error("memory.max", &limit))?;
        if governing
            .as_ref()
            .is_none_or(|(_, current)| limit < *current)
        {
            governing = Some((scope.to_path_buf(), limit));
        }
    }
    let Some((scope, total_bytes)) = governing else {
        return Ok(None);
    };
    let current = shell.read_file(&scope.join("memory.current").display().to_string())?;
    let current: u64 = current
        .trim()
        .parse()
        .map_err(|_| parse_error("memory.current", &current))?;
    let stat = shell.read_file(&scope.join("memory.stat").display().to_string())?;
    let inactive: u64 = stat
        .lines()
        .find_map(|line| line.strip_prefix("inactive_file "))
        .and_then(|value| value.trim().parse().ok())
        .ok_or_else(|| parse_error("memory.stat inactive_file", &stat))?;
    Ok(Some(Memory {
        used_bytes: current.saturating_sub(inactive),
        total_bytes,
    }))
}

fn parse_meminfo(text: &str) -> Option<Memory> {
    let kb = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key))?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()?
            .checked_mul(1024)
    };
    let total_bytes = kb("MemTotal:")?;
    Some(Memory {
        used_bytes: total_bytes.saturating_sub(kb("MemAvailable:")?),
        total_bytes,
    })
}

fn parse_vm_stat(text: &str, total_bytes: u64) -> Option<Memory> {
    let header = text.lines().next()?;
    let page_size: u64 = header
        .split("page size of ")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    let pages = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key))?
            .trim()
            .trim_end_matches('.')
            .parse::<u64>()
            .ok()
    };
    let available = pages("Pages free:")?
        .checked_add(pages("Pages inactive:")?)?
        .checked_add(pages("Pages speculative:")?)?
        .checked_mul(page_size)?;
    Some(Memory {
        used_bytes: total_bytes.saturating_sub(available),
        total_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::Fake;

    #[test]
    fn parses_system_measurements() {
        assert_eq!(parse_loadavg("{ 33.81 24.81 15.46 }"), Some(33.81));
        assert_eq!(parse_loadavg("0.5 0.2 0.1 1/10 42"), Some(0.5));
        assert_eq!(parse_df("FS 1000 500 400 60% /"), Some(40.0));
        assert_eq!(
            parse_meminfo("MemTotal: 100 kB\nMemAvailable: 25 kB\n"),
            Some(Memory {
                used_bytes: 76800,
                total_bytes: 102_400
            })
        );
        assert_eq!(
            parse_vm_stat(
                "Mach Virtual Memory Statistics: (page size of 4096 bytes)\nPages free: 2.\nPages inactive: 3.\nPages speculative: 1.\n",
                40960
            ),
            Some(Memory {
                used_bytes: 16384,
                total_bytes: 40960
            })
        );
        assert!(parse_vm_stat("bad", 10).is_none());
    }

    #[test]
    fn memory_uses_tightest_ancestor_working_set() {
        let shell = Fake::new();
        shell
            .on(
                "read /proc/meminfo",
                "MemTotal: 100 kB\nMemAvailable: 25 kB",
            )
            .on(
                "read /proc/self/cgroup",
                "0::/user.slice/user-1000.slice/session.scope\n",
            )
            .on(
                "read /sys/fs/cgroup/user.slice/user-1000.slice/session.scope/memory.max",
                "16384",
            )
            .on(
                "read /sys/fs/cgroup/user.slice/user-1000.slice/memory.max",
                "8192",
            )
            .on("read /sys/fs/cgroup/user.slice/memory.max", "32768")
            .on("read /sys/fs/cgroup/memory.max", "max")
            .on(
                "read /sys/fs/cgroup/user.slice/user-1000.slice/memory.current",
                "7000",
            )
            .on(
                "read /sys/fs/cgroup/user.slice/user-1000.slice/memory.stat",
                "anon 1000\ninactive_file 3000\n",
            );
        assert_eq!(
            memory(&shell).unwrap(),
            Memory {
                used_bytes: 4000,
                total_bytes: 8192
            }
        );
        shell
            .on("read /sys/fs/cgroup/memory.max", "4096")
            .on("read /sys/fs/cgroup/memory.current", "2000")
            .on("read /sys/fs/cgroup/memory.stat", "inactive_file 500\n");
        assert_eq!(
            memory(&shell).unwrap(),
            Memory {
                used_bytes: 1500,
                total_bytes: 4096
            }
        );
        for path in [
            "",
            "/user.slice",
            "/user.slice/user-1000.slice",
            "/user.slice/user-1000.slice/session.scope",
        ] {
            shell.on(&format!("read /sys/fs/cgroup{path}/memory.max"), "max");
        }
        assert_eq!(
            memory(&shell).unwrap(),
            Memory {
                used_bytes: 76800,
                total_bytes: 102_400
            }
        );
    }

    #[test]
    fn mac_memory_uses_vm_stat_page_size() {
        let shell = Fake::new();
        shell.on("sysctl -n hw.memsize", "65536\n")
            .on("vm_stat", "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free: 1.\nPages inactive: 1.\nPages speculative: 0.\n");
        assert_eq!(
            memory(&shell).unwrap(),
            Memory {
                used_bytes: 32768,
                total_bytes: 65536
            }
        );
    }
}
