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

    /// Agents that hold their declared demand: every live agent except `idle`.
    /// An idle agent waits for a prompt and runs no build; any other state, including
    /// one Herdr adds later, counts.
    pub fn active(&self) -> usize {
        self.live()
            .saturating_sub(self.by_status.get("idle").copied().unwrap_or(0))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pane {
    pub pane_id: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub focused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_status: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub foreground_cwd: Option<String>,
    /// A pane without an agent whose shell runs no foreground command.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub idle_shell: bool,
    /// Start of the observed idle-shell run. Herdr reports no activity time, so the service
    /// keeps it across samples.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_since_ms: Option<u64>,
}

/// Herdr's process information for a pane: its shell and foreground processes. `None` when the
/// pane closed after it was listed. Every caller reads a pane from an earlier list, so a closed
/// pane is an ordinary race, not a failure.
pub fn process_info(shell: &dyn Shell, pane: &str) -> Result<Option<serde_json::Value>> {
    let args = [
        "pane".to_string(),
        "process-info".into(),
        "--pane".into(),
        pane.into(),
    ];
    let out = shell.run("herdr", &args)?;
    if !out.ok() {
        let code = |text: &str| {
            serde_json::from_str::<serde_json::Value>(text)
                .ok()
                .and_then(|v| v.pointer("/error/code")?.as_str().map(str::to_string))
        };
        if [&out.stderr, &out.stdout]
            .into_iter()
            .any(|text| code(text).as_deref() == Some("pane_not_found"))
        {
            return Ok(None);
        }
    }
    let out = crate::shell::checked("herdr", &args, out)?;
    let mut value: serde_json::Value = serde_json::from_str(&out.stdout)?;
    value
        .pointer_mut("/result/process_info")
        .map(serde_json::Value::take)
        .map(Some)
        .ok_or_else(|| Error::Invalid(format!("{pane} has no process information")))
}

/// What a pane runs in the foreground, or `None` when the pane is an idle shell at its
/// prompt.
///
/// A pane is idle only when the evidence proves it. Herdr reports the pane's own root
/// process as `shell_pid`, and the root process is in the foreground list only while the
/// pane itself is the foreground. An idle pane is therefore an interactive shell that
/// waits at its prompt. Three cases are not idle:
///
/// - The root process is another program, such as a plugin pane that draws the sidebar.
///   That program runs for the whole life of the pane.
/// - The root process is a shell that runs a command, such as `sh -c "..."`. It runs the
///   command instead of waiting.
/// - The evidence does not name the root process, or does not say what it runs. Then the
///   pane is kept, because nothing proves that closing it loses nothing.
///
/// A pane that closed after it was listed runs nothing.
pub fn foreground(shell: &dyn Shell, pane: &str) -> Result<Option<String>> {
    let Some(info) = process_info(shell, pane)? else {
        return Ok(None);
    };
    let processes = info["foreground_processes"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    let shell_pid = info["shell_pid"].as_u64().filter(|pid| *pid > 0);
    let Some(root) = processes
        .iter()
        .find(|process| shell_pid.is_some() && process["pid"].as_u64() == shell_pid)
    else {
        // The root process is not the foreground: a command runs in front of it, or the
        // evidence does not say what the pane runs. Then the pane is kept.
        return Ok(Some(
            processes
                .first()
                .map_or_else(|| "a command".to_string(), process_name),
        ));
    };
    if !waits_at_a_prompt(root) {
        return Ok(Some(process_name(root)));
    }
    // The shell waits at its prompt. Another process in the same foreground group means it
    // is not waiting.
    Ok(processes
        .iter()
        .find(|process| process["pid"].as_u64() != shell_pid)
        .map(process_name))
}

/// The shell names Herdr can give a pane. A pane whose root process has another name runs
/// that program itself, so it never waits at a prompt.
fn is_shell_name(name: &str) -> bool {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let name = name.trim_start_matches('-');
    let name = name.strip_suffix(".exe").unwrap_or(name);
    matches!(
        name.to_ascii_lowercase().as_str(),
        "sh" | "bash"
            | "dash"
            | "zsh"
            | "fish"
            | "ksh"
            | "mksh"
            | "csh"
            | "tcsh"
            | "elvish"
            | "xonsh"
            | "nu"
            | "pwsh"
            | "powershell"
            | "cmd"
    )
}

/// True when the process is a shell that waits at its prompt.
///
/// The evidence must be positive: a known shell, started with no argument that runs
/// something. A shell that runs a command, such as `sh -c "..."` or `zsh script.zsh`,
/// does not wait. A shell whose arguments are unknown, malformed, or not recognised does
/// not prove that it waits: the pane is kept.
fn waits_at_a_prompt(process: &serde_json::Value) -> bool {
    let name = process_name(process);
    if !is_shell_name(&name) {
        return false;
    }
    let Some(args) = argv(process) else {
        return false;
    };
    let Some((program, flags)) = args.split_first() else {
        return false;
    };
    // The first argument names the program. It must agree with the reported name.
    let program = program.rsplit(['/', '\\']).next().unwrap_or(program);
    if !program.trim_start_matches('-').eq_ignore_ascii_case(&name) {
        return false;
    }
    // Only a known prompt-only invocation proves idle evidence. An unknown option, a
    // script, a command string, or an option operand keeps the pane.
    flags.is_empty()
        || (POSIX_SHELLS.contains(&name.to_ascii_lowercase().as_str())
            && flags.iter().all(|flag| waits_at_a_prompt_flag(flag)))
}

/// The shells whose options are known to leave them at a prompt. Other shells, such as
/// `pwsh` and `cmd`, run a command with their own options, so their arguments keep the pane.
const POSIX_SHELLS: [&str; 9] = [
    "sh", "bash", "dash", "zsh", "fish", "ksh", "mksh", "csh", "tcsh",
];

/// True when the option leaves the shell at its prompt. Only the interactive and login
/// options are known to do so. Every other option, including `-c`, may run a command.
fn waits_at_a_prompt_flag(flag: &str) -> bool {
    if flag.starts_with("--") {
        return matches!(
            flag.to_ascii_lowercase().as_str(),
            "--login" | "--interactive"
        );
    }
    flag.strip_prefix('-').is_some_and(|flags| {
        !flags.is_empty() && flags.chars().all(|flag| matches!(flag, 'i' | 'l'))
    })
}

/// The arguments of a process, from `argv` or from its command line. The first entry is the
/// program itself.
fn argv(process: &serde_json::Value) -> Option<Vec<&str>> {
    if let Some(value) = process.get("argv") {
        return value
            .as_array()?
            .iter()
            .map(serde_json::Value::as_str)
            .collect();
    }
    // A command line is not structured argv. Only a bare executable is unambiguous;
    // do not infer argument boundaries by splitting quoted shell text.
    let line = process["cmdline"].as_str()?;
    (!line.is_empty() && !line.chars().any(char::is_whitespace)).then(|| vec![line])
}

/// The name of a process, from its program name.
fn process_name(process: &serde_json::Value) -> String {
    let name = process["argv0"]
        .as_str()
        .or_else(|| process["name"].as_str())
        .unwrap_or("a command");
    name.rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .trim_start_matches('-')
        .to_string()
}

/// Stable service contract, independent of Herdr's raw response fields.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    pub pane: String,
    pub agent: Option<String>,
    pub state: String,
    pub name: Option<String>,
    pub transcript: Option<String>,
    pub transcript_modified: Option<u64>,
    #[serde(default)]
    pub stalled_since_ms: Option<u64>,
    #[serde(default)]
    pub state_since_ms: Option<u64>,
    /// Herdr's terminal identity: a reused pane id gets a new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_id: Option<String>,
    /// Stable harness session identity, including across a Herdr terminal restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session: Option<String>,
    /// Herdr's count of finished turns. A rise is a completion event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_seq: Option<u64>,
    /// Herdr's counter at the agent's latest state change. While it works, a later completion
    /// has a higher number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_change_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// What the transcript says: the model that answers, and the latest activity.
    #[serde(default, skip_serializing_if = "crate::facts::Session::is_empty")]
    pub session: crate::facts::Session,
    /// The linked worktree the agent runs in, measured on this host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<crate::facts::Worktree>,
    /// Canonical `host/owner/name` of the repository the agent's checkout names as `origin`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// The agent called `done`, and its turn that called it is its latest completion. Only a
    /// done agent is finished: idle alone means it waits for input.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub done: bool,
    /// The agent called `done`, but its terminal handoff is not complete yet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub finishing: bool,
    /// The native `done` handoff this host published: `none`, `accepted` or `recorded`.
    /// A host that predates this field leaves it empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<String>,
    /// How far this host's launch for the pane reached: `allocating`, `allocated`,
    /// `attempted` or `confirmed`. A host that predates this field leaves it empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_progress: Option<String>,
    /// The agent owns open items or pending or admitting child tickets. This is the same
    /// scan the done guard uses.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub owns_open: bool,
    /// The lifecycle state the host derived for this agent. Older hosts publish no state,
    /// and older readers ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<String>,
    /// Why the lifecycle state is not positively established.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle_gap: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawAgent {
    pane_id: String,
    agent: Option<String>,
    agent_status: Option<String>,
    name: Option<String>,
    agent_session: Option<AgentSession>,
    #[serde(default)]
    terminal_id: Option<String>,
    #[serde(default)]
    completion_seq: Option<u64>,
    #[serde(default)]
    state_change_seq: Option<u64>,
    #[serde(default)]
    cwd: Option<String>,
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
    /// macOS memory pressure: 1 normal, 2 warn, 4 critical. None on Linux.
    #[serde(default)]
    pub pressure_level: Option<u8>,
}

/// Cumulative CPU ticks since boot in `USER_HZ`. Guest ticks are already
/// included in user/nice; busy excludes idle and iowait.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuTicks {
    pub busy: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Probe {
    pub host: String,
    pub label: String,
    pub load1: f64,
    pub cores: u32,
    /// Linux cumulative ticks; the service derives busy from two samples.
    #[serde(default)]
    pub cpu_ticks: Option<CpuTicks>,
    /// Busy fraction over the last interval. Filled directly only on macOS.
    #[serde(default)]
    pub busy: Option<f64>,
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
    let mut panes: Vec<Pane> = serde_json::from_value(
        value
            .pointer("/result/panes")
            .ok_or_else(|| parse_error("herdr pane list", &panes.stdout))?
            .clone(),
    )?;
    let mut shells: Vec<_> = panes.iter_mut().filter(|p| p.agent.is_none()).collect();
    let chunk = shells.len().div_ceil(cores() as usize).max(1);
    std::thread::scope(|scope| {
        for group in shells.chunks_mut(chunk) {
            scope.spawn(move || {
                for pane in group {
                    // A pane closed since the list is not idle; it is gone.
                    pane.idle_shell = matches!(foreground(shell, &pane.pane_id), Ok(None));
                }
            });
        }
    });
    Ok(Probe {
        host: "local".into(),
        label: "local".into(),
        load1: load1(shell)?,
        cores: cores(),
        cpu_ticks: cpu_ticks(shell)?,
        busy: mac_busy(shell)?,
        memory: memory(shell)?,
        disk_free_percent: disk_free_percent(shell, home)?,
        agents,
        agent_panes,
        reservations: Vec::new(),
        panes,
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
            let agent_session = raw.agent_session.as_ref().map(|s| s.value.clone());
            let transcript = raw.agent_session.and_then(|session| {
                crate::transcript::resolve(raw.agent.as_deref(), &session.kind, &session.value)
            });
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
                stalled_since_ms: None,
                state_since_ms: None,
                terminal_id: raw.terminal_id,
                agent_session,
                completion_seq: raw.completion_seq,
                state_change_seq: raw.state_change_seq,
                cwd: raw.cwd,
                session: crate::facts::Session::default(),
                worktree: None,
                repository: None,
                done: false,
                finishing: false,
                handoff: None,
                launch_progress: None,
                owns_open: false,
                lifecycle: None,
                lifecycle_gap: None,
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

pub fn cpu_ticks(shell: &dyn Shell) -> Result<Option<CpuTicks>> {
    let Ok(text) = shell.read_file("/proc/stat") else {
        return Ok(None);
    };
    parse_proc_stat(&text)
        .map(Some)
        .ok_or_else(|| parse_error("/proc/stat", &text))
}

pub fn parse_proc_stat(text: &str) -> Option<CpuTicks> {
    let mut fields = text.lines().next()?.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    let ticks: Vec<u64> = fields
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()
        .ok()?;
    // The first eight counters exclude guest and guest_nice.
    let total = ticks
        .get(..8)?
        .iter()
        .try_fold(0_u64, |sum, tick| sum.checked_add(*tick))?;
    let busy = total.checked_sub(ticks[3])?.checked_sub(ticks[4])?;
    Some(CpuTicks { busy, total })
}

pub fn mac_busy(shell: &dyn Shell) -> Result<Option<f64>> {
    if shell.read_file("/proc/stat").is_ok() {
        return Ok(None);
    }
    let out = shell.run_ok(
        "iostat",
        &["-c".into(), "2".into(), "-w".into(), "1".into()],
    )?;
    parse_iostat(&out.stdout)
        .map(Some)
        .ok_or_else(|| parse_error("iostat", &out.stdout))
}

pub fn parse_iostat(text: &str) -> Option<f64> {
    let idle: f64 = text
        .lines()
        .rfind(|line| !line.trim().is_empty())?
        .split_whitespace()
        .rev()
        .nth(3)?
        .parse()
        .ok()?;
    (0.0..=100.0).contains(&idle).then_some(1.0 - idle / 100.0)
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
        let host = parse_meminfo(&meminfo).ok_or_else(|| parse_error("/proc/meminfo", &meminfo))?;
        let cgroup = cgroup_memory(shell)?;
        return Ok(cgroup
            .filter(|memory| {
                memory.total_bytes.saturating_sub(memory.used_bytes)
                    < host.total_bytes.saturating_sub(host.used_bytes)
            })
            .unwrap_or(host));
    }
    let total = shell
        .run_ok("sysctl", &["-n".into(), "hw.memsize".into()])?
        .stdout;
    let vm = shell.run_ok("vm_stat", &[])?.stdout;
    let pressure = shell
        .run_ok(
            "sysctl",
            &["-n".into(), "kern.memorystatus_vm_pressure_level".into()],
        )?
        .stdout;
    let mut memory = parse_vm_stat(
        &vm,
        total
            .trim()
            .parse()
            .map_err(|_| parse_error("hw.memsize", &total))?,
    )
    .ok_or_else(|| parse_error("vm_stat", &vm))?;
    memory.pressure_level = Some(
        pressure
            .trim()
            .parse()
            .map_err(|_| parse_error("kern.memorystatus_vm_pressure_level", &pressure))?,
    );
    Ok(memory)
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
        pressure_level: None,
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
        pressure_level: None,
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
        pressure_level: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::Fake;

    #[test]
    fn a_pane_that_closed_after_it_was_listed_runs_nothing() {
        let shell = Fake::new();
        let failed = |stderr: &str| crate::shell::Output {
            status: 1,
            stdout: String::new(),
            stderr: stderr.to_string(),
        };
        // Herdr 0.9.4 writes this to stderr for a closed pane.
        shell.on_output(
            "herdr pane process-info --pane w1:p1",
            failed(r#"{"error":{"code":"pane_not_found","message":"pane not found"},"id":"cli:pane:process_info"}"#),
        );
        shell.on_output(
            "herdr pane process-info --pane w1:p2",
            failed(r#"{"error":{"code":"server_unavailable"}}"#),
        );
        assert_eq!(foreground(&shell, "w1:p1").unwrap(), None);
        assert!(foreground(&shell, "w1:p2").is_err());
    }

    /// The process information of one pane, as Herdr reports it.
    fn process_info(shell_pid: u64, processes: &serde_json::Value) -> String {
        serde_json::json!({
            "result": {"process_info": {
                "shell_pid": shell_pid,
                "foreground_processes": processes,
            }}
        })
        .to_string()
    }

    #[test]
    fn an_idle_shell_is_its_own_foreground_process() {
        let shell = Fake::new();
        shell.on(
            "herdr pane process-info --pane w1:p1",
            &process_info(
                10,
                &serde_json::json!([{"pid":10,"name":"zsh","argv":["zsh"]}]),
            ),
        );
        assert_eq!(foreground(&shell, "w1:p1").unwrap(), None);
    }

    #[test]
    fn a_plugin_pane_that_draws_the_sidebar_is_not_an_idle_shell() {
        let shell = Fake::new();
        // Herdr runs the plugin command as the pane's own root process, so the root is the
        // only foreground process and its pid is the shell pid.
        shell.on(
            "herdr pane process-info --pane w1:p1",
            &process_info(
                10,
                &serde_json::json!([{"pid":10,"name":"dogsled","argv":["bin/dogsled","sidebar"]}]),
            ),
        );
        assert_eq!(foreground(&shell, "w1:p1").unwrap(), Some("dogsled".into()));
    }

    #[test]
    fn a_shell_that_runs_a_command_is_not_an_idle_shell() {
        let shell = Fake::new();
        shell.on(
            "herdr pane process-info --pane w1:p1",
            &process_info(
                10,
                &serde_json::json!([{"pid":10,"name":"sh","argv":["sh","-c","sleep 60"]}]),
            ),
        );
        // The diagnostic names the program, never its arguments.
        assert_eq!(foreground(&shell, "w1:p1").unwrap(), Some("sh".into()));
    }

    #[test]
    fn a_shell_that_runs_a_command_in_a_flag_cluster_is_not_an_idle_shell() {
        let shell = Fake::new();
        shell.on(
            "herdr pane process-info --pane w1:p1",
            &process_info(
                10,
                &serde_json::json!([{"pid":10,"name":"bash","argv":["bash","-lc","run"]}]),
            ),
        );
        assert_eq!(foreground(&shell, "w1:p1").unwrap(), Some("bash".into()));
    }

    #[test]
    fn a_shell_without_known_arguments_does_not_prove_an_idle_pane() {
        let shell = Fake::new();
        // The name says shell, but the evidence does not say that it waits at a prompt.
        shell.on(
            "herdr pane process-info --pane w1:p1",
            &process_info(10, &serde_json::json!([{"pid":10,"name":"zsh"}])),
        );
        assert!(foreground(&shell, "w1:p1").unwrap().is_some());
    }

    #[test]
    fn a_shell_with_an_unknown_option_does_not_prove_an_idle_pane() {
        let shell = Fake::new();
        // `-x` is not a known prompt-only option, so the shell may be running something.
        shell.on(
            "herdr pane process-info --pane w1:p1",
            &process_info(
                10,
                &serde_json::json!([{"pid":10,"name":"zsh","argv":["zsh","-x"]}]),
            ),
        );
        assert_eq!(foreground(&shell, "w1:p1").unwrap(), Some("zsh".into()));
    }

    #[test]
    fn a_shell_that_runs_a_script_does_not_prove_an_idle_pane() {
        let shell = Fake::new();
        shell.on(
            "herdr pane process-info --pane w1:p1",
            &process_info(
                10,
                &serde_json::json!([{"pid":10,"name":"zsh","argv":["zsh","build.sh"]}]),
            ),
        );
        assert_eq!(foreground(&shell, "w1:p1").unwrap(), Some("zsh".into()));
    }

    #[test]
    fn a_shell_with_an_interactive_option_still_waits_at_a_prompt() {
        let shell = Fake::new();
        shell.on(
            "herdr pane process-info --pane w1:p1",
            &process_info(
                10,
                &serde_json::json!([{"pid":10,"name":"zsh","argv":["zsh","-l"]}]),
            ),
        );
        assert_eq!(foreground(&shell, "w1:p1").unwrap(), None);
    }

    #[test]
    fn a_shell_with_a_full_path_still_waits_at_a_prompt() {
        let shell = Fake::new();
        // Herdr reports the full path of the shell in `argv`.
        shell.on(
            "herdr pane process-info --pane w1:p1",
            &process_info(
                10,
                &serde_json::json!([{"pid":10,"name":"zsh","argv":["/bin/zsh"]}]),
            ),
        );
        assert_eq!(foreground(&shell, "w1:p1").unwrap(), None);
    }

    #[test]
    fn malformed_arguments_do_not_prove_an_idle_pane() {
        let shell = Fake::new();
        // An argument that is not a string is malformed evidence.
        shell.on(
            "herdr pane process-info --pane w1:p1",
            &process_info(
                10,
                &serde_json::json!([{"pid":10,"name":"zsh","argv":["zsh",7]}]),
            ),
        );
        assert_eq!(foreground(&shell, "w1:p1").unwrap(), Some("zsh".into()));
    }

    #[test]
    fn a_command_line_without_arguments_does_not_prove_an_idle_pane() {
        let shell = Fake::new();
        // The command line carries arguments, but their boundaries are unknown.
        shell.on(
            "herdr pane process-info --pane w1:p1",
            &process_info(
                10,
                &serde_json::json!([{"pid":10,"name":"zsh","cmdline":"zsh -c sleep 60"}]),
            ),
        );
        assert_eq!(foreground(&shell, "w1:p1").unwrap(), Some("zsh".into()));
    }

    #[test]
    fn a_bare_command_line_waits_at_a_prompt() {
        let shell = Fake::new();
        shell.on(
            "herdr pane process-info --pane w1:p1",
            &process_info(
                10,
                &serde_json::json!([{"pid":10,"name":"zsh","cmdline":"/bin/zsh"}]),
            ),
        );
        assert_eq!(foreground(&shell, "w1:p1").unwrap(), None);
    }

    #[test]
    fn a_pane_without_a_root_pid_does_not_prove_an_idle_pane() {
        let shell = Fake::new();
        // No shell pid, so the evidence does not name the root process.
        shell.on(
            "herdr pane process-info --pane w1:p1",
            &serde_json::json!({"result": {"process_info": {"foreground_processes": [
                {"pid":10,"name":"zsh","argv":["zsh"]}
            ]}}})
            .to_string(),
        );
        assert_eq!(foreground(&shell, "w1:p1").unwrap(), Some("zsh".into()));
    }

    #[test]
    fn a_diagnostic_never_shows_the_arguments_of_a_command() {
        let shell = Fake::new();
        // The arguments can carry a secret. The diagnostic names the program only.
        shell.on(
            "herdr pane process-info --pane w1:p1",
            &process_info(
                10,
                &serde_json::json!([{"pid":10,"name":"sh","argv":["sh","-c","curl -H 'token: s3cr3t'"]}]),
            ),
        );
        let detail = foreground(&shell, "w1:p1").unwrap().unwrap();
        assert_eq!(detail, "sh");
        assert!(!detail.contains("s3cr3t"), "{detail}");
    }

    #[test]
    fn an_unknown_root_process_does_not_prove_an_idle_pane() {
        let shell = Fake::new();
        // The root process is not the foreground, so a command runs in front of the shell.
        shell.on(
            "herdr pane process-info --pane w1:p1",
            &process_info(
                10,
                &serde_json::json!([{"pid":11,"name":"vim","argv":["vim"]}]),
            ),
        );
        assert_eq!(foreground(&shell, "w1:p1").unwrap(), Some("vim".into()));
    }

    #[test]
    fn a_pane_without_process_information_does_not_prove_an_idle_pane() {
        let shell = Fake::new();
        shell.on(
            "herdr pane process-info --pane w1:p1",
            &process_info(10, &serde_json::json!([])),
        );
        assert!(foreground(&shell, "w1:p1").unwrap().is_some());
    }

    #[test]
    fn a_command_in_front_of_an_idle_shell_is_not_an_idle_pane() {
        let shell = Fake::new();
        shell.on(
            "herdr pane process-info --pane w1:p1",
            &process_info(
                10,
                &serde_json::json!([
                    {"pid":10,"name":"zsh","argv":["zsh"]},
                    {"pid":11,"name":"vim","argv":["vim"]},
                ]),
            ),
        );
        assert_eq!(foreground(&shell, "w1:p1").unwrap(), Some("vim".into()));
    }

    #[test]
    fn cpu_ticks_exclude_idle_iowait_and_duplicate_guest_time() {
        assert_eq!(
            parse_proc_stat("cpu  100 20 30 400 50 6 7 8 9 10\ncpu0 1 2 3\n"),
            Some(CpuTicks {
                busy: 171,
                total: 621
            })
        );
        for text in [
            "",
            "cpu0 1 2 3 4 5 6 7 8",
            "cpu 1 2",
            "cpu 1 2 3 4 bad 6 7 8",
            "cpu 18446744073709551615 1 0 0 0 0 0 0",
        ] {
            assert_eq!(parse_proc_stat(text), None, "{text}");
        }
    }

    #[test]
    fn iostat_uses_last_interval() {
        let text = "              disk0       cpu    load average\n    KB/t tps MB/s  us sy id  1m 5m 15m\n    1.00 1 1.0  1 2 97  3.00 4.00 5.00\n    21.34 77 1.63  38 25 37  8.59 12.37 15.00\n\n";
        assert_eq!(parse_iostat(text), Some(0.63));
        for text in ["", "\n", "bad", "1 2 101 3 4 5", "1 2 NaN 3 4 5"] {
            assert_eq!(parse_iostat(text), None, "{text}");
        }
    }

    fn local_shell() -> Fake {
        let shell = Fake::new();
        shell
            .on("herdr agent list", r#"{"result":{"agents":[]}}"#)
            .on("herdr pane list", r#"{"result":{"panes":[]}}"#)
            .on("df -Pk /home", "FS 1000 500 400 60% /");
        shell
    }

    #[test]
    fn linux_local_collects_ticks_without_mac_measurements() {
        let shell = local_shell();
        shell
            .on("read /proc/stat", "cpu 100 20 30 400 50 6 7 8 9 10")
            .on("read /proc/loadavg", "0.5 0.2 0.1 1/10 42")
            .on(
                "read /proc/meminfo",
                "MemTotal: 100 kB\nMemAvailable: 25 kB",
            );
        let probe = local(&shell, "/home").unwrap();
        assert_eq!(
            probe.cpu_ticks,
            Some(CpuTicks {
                busy: 171,
                total: 621
            })
        );
        assert_eq!(probe.busy, None);
        assert_eq!(probe.memory.pressure_level, None);
    }

    #[test]
    fn mac_local_collects_busy_and_memory_pressure() {
        let shell = local_shell();
        shell.on("sysctl -n vm.loadavg", "{ 1.0 2.0 3.0 }")
            .on("iostat -c 2 -w 1", "us sy id 1m 5m 15m\n1 2 97 1 2 3\n38 25 37 8.59 12.37 15.00\n")
            .on("sysctl -n hw.memsize", "65536\n")
            .on("sysctl -n kern.memorystatus_vm_pressure_level", "2\n")
            .on("vm_stat", "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free: 1.\nPages inactive: 1.\nPages speculative: 0.\n");
        let probe = local(&shell, "/home").unwrap();
        assert_eq!(probe.cpu_ticks, None);
        assert_eq!(probe.busy, Some(0.63));
        assert_eq!(probe.memory.pressure_level, Some(2));
    }

    #[test]
    fn older_probes_default_new_measurements() {
        let memory: Memory = serde_json::from_str(r#"{"used_bytes":1,"total_bytes":2}"#).unwrap();
        assert_eq!(memory.pressure_level, None);
        let probe: Probe = serde_json::from_str(r#"{"host":"old","label":"old","load1":0,"cores":1,"memory":{"used_bytes":1,"total_bytes":2},"disk_free_percent":50,"agents":{"total":0,"by_status":{}},"agent_panes":[],"reservations":[],"panes":[]}"#).unwrap();
        assert_eq!(probe.cpu_ticks, None);
        assert_eq!(probe.busy, None);
        assert_eq!(probe.memory.pressure_level, None);
    }

    #[test]
    fn malformed_measurements_report_parse_errors() {
        let shell = Fake::new();
        shell.on("read /proc/stat", "cpu broken");
        assert!(matches!(cpu_ticks(&shell), Err(Error::Parse { .. })));
        let shell = Fake::new();
        shell.on("iostat -c 2 -w 1", "bad");
        assert!(matches!(mac_busy(&shell), Err(Error::Parse { .. })));
    }

    #[test]
    fn memory_uses_scope_with_less_free_space() {
        let gib = 1024_u64.pow(3);
        let shell = Fake::new();
        shell
            .on("read /proc/self/cgroup", "0::/\n")
            .on("read /sys/fs/cgroup/memory.max", &(4 * gib).to_string())
            .on("read /sys/fs/cgroup/memory.current", &(3 * gib).to_string())
            .on("read /sys/fs/cgroup/memory.stat", "inactive_file 0\n")
            .on(
                "read /proc/meminfo",
                "MemTotal: 67108864 kB\nMemAvailable: 524288 kB\n",
            );
        // Host has 0.5 GiB free, less than the cgroup's 1 GiB.
        assert_eq!(
            memory(&shell).unwrap(),
            Memory {
                used_bytes: 64 * gib - gib / 2,
                total_bytes: 64 * gib,
                pressure_level: None,
            }
        );
        shell.on(
            "read /proc/meminfo",
            "MemTotal: 67108864 kB\nMemAvailable: 2097152 kB\n",
        );
        // Host now has 2 GiB free; the cgroup's 1 GiB governs.
        assert_eq!(
            memory(&shell).unwrap(),
            Memory {
                used_bytes: 3 * gib,
                total_bytes: 4 * gib,
                pressure_level: None,
            }
        );
    }

    #[test]
    fn parses_system_measurements() {
        assert_eq!(parse_loadavg("{ 33.81 24.81 15.46 }"), Some(33.81));
        assert_eq!(parse_loadavg("0.5 0.2 0.1 1/10 42"), Some(0.5));
        assert_eq!(parse_df("FS 1000 500 400 60% /"), Some(40.0));
        assert_eq!(
            parse_meminfo("MemTotal: 100 kB\nMemAvailable: 25 kB\n"),
            Some(Memory {
                used_bytes: 76800,
                total_bytes: 102_400,
                pressure_level: None,
            })
        );
        assert_eq!(
            parse_vm_stat(
                "Mach Virtual Memory Statistics: (page size of 4096 bytes)\nPages free: 2.\nPages inactive: 3.\nPages speculative: 1.\n",
                40960
            ),
            Some(Memory {
                used_bytes: 16384,
                total_bytes: 40960,
                pressure_level: None,
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
                total_bytes: 8192,
                pressure_level: None,
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
                total_bytes: 4096,
                pressure_level: None,
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
                total_bytes: 102_400,
                pressure_level: None,
            }
        );
    }

    #[test]
    fn mac_memory_uses_vm_stat_page_size() {
        let shell = Fake::new();
        shell.on("sysctl -n kern.memorystatus_vm_pressure_level", "1\n");
        shell.on("sysctl -n hw.memsize", "65536\n")
            .on("vm_stat", "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free: 1.\nPages inactive: 1.\nPages speculative: 0.\n");
        assert_eq!(
            memory(&shell).unwrap(),
            Memory {
                used_bytes: 32768,
                total_bytes: 65536,
                pressure_level: Some(1),
            }
        );
    }
}
